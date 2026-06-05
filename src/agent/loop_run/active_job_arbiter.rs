//! Issue #660: active-job arbitration SSOT.
//!
//! Pure function that selects at most one **write-owner** job among the
//! selectable kinds (`VerifierRepair` / `ForcedSmallEditRecovery` /
//! `ArtifactRecovery` / `FocusedEditRecovery` / `LocalLlmSmallEditAfterRead`).
//! The legacy if-elif chain in `turn.rs::effective_tool_policy()` is rewritten
//! to delegate to `select_active_job` + `project_policy`.
//!
//! ### Layer rules (CLAUDE.md DR3-001 / DR3-002)
//! - **private mod**: `loop_run.rs` MUST NOT `pub use active_job_arbiter::*;`.
//!   `turn.rs` is the only in-crate consumer.
//! - **no session / photon import**: arbiter reads `EffectiveToolPolicy` /
//!   `AllowedWriteActions` / `AllowedReadScope` / `MissingVerifierJob` /
//!   `StopReason` from sibling `loop_run` mods only. It MUST NOT import
//!   `crate::session::*` / `crate::photon::*`.
//! - **no `unsafe`**: pure value projection only.
//!
//! ### Security invariants (CLAUDE.md)
//! - **no log emit**: `select_active_job` and `project_policy` are pure
//!   functions; structured log emission (`agent.active_job.selected`) lives
//!   in `turn.rs` event payload builders that route through
//!   `logging::mask_payload_inplace` and `session::feedback::mask_secrets` /
//!   `redact_verifier_command_for_storage`.
//! - **no raw path / command**: arbiter does not format / `Debug` payload
//!   strings. `DesiredAction::VerifierRepair.command` is held only for the
//!   internal contract; the payload builder must redact via
//!   `redact_verifier_command_for_storage` before logging (never `Debug`).

use std::num::NonZeroU32;
use std::path::{Path, PathBuf};

use super::artifact_completion_job::{AllowedReadScope, AllowedWriteActions};
use super::repair_job::{RepairNextAction, StopReason, VerifierBootstrapNextAction};
use super::required_behavior::{
    BehaviorContractProjection, LOW_CONFIDENCE_THRESHOLD, behavior_projection_has_setup_label,
};
use super::task_contract::{
    ArtifactRecoveryAction, ArtifactRole, RecoveryTargetHint, TaskContract,
    VerifierPrerequisiteSignal, has_required_setup_artifact,
};
use super::tool_policy::EffectiveToolPolicy;
use super::worker_contract::{DiagnosticRepairWorkerRequest, TestAuthorWorkerRequest};
use crate::logging::stable_path_hash;
use crate::modes::plan_act::ExecutionMode;
use crate::session::feedback::mask_secrets;

/// Controller-facing next action for the pre-model part of the actor loop.
///
/// This is deliberately small: it does not execute tools, mutate state, or
/// infer semantic progress from model prose. It only projects already-typed
/// state into one dispatch source. Keeping it in this arbiter module prevents
/// `turn.rs` from growing another local dispatch vocabulary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum LoopControlAction {
    ContinueRepairJob {
        next_action: RepairNextAction,
    },
    ContinueMissingVerifierJob {
        next_action: VerifierBootstrapNextAction,
    },
    RunVerifier,
    RequestModelTurn,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct LoopControlInputs {
    pub(super) mode: ExecutionMode,
    pub(super) task_contract_verifier_repair_pending: bool,
    pub(super) repair_next_action: Option<RepairNextAction>,
    pub(super) missing_verifier_next_action: Option<VerifierBootstrapNextAction>,
    pub(super) task_contract_action: Option<ArtifactRecoveryAction>,
}

pub(super) fn determine_loop_control_action(inputs: LoopControlInputs) -> LoopControlAction {
    if inputs.mode == ExecutionMode::Plan {
        return LoopControlAction::RequestModelTurn;
    }

    if inputs.task_contract_verifier_repair_pending {
        if let Some(next_action) = inputs.repair_next_action {
            return LoopControlAction::ContinueRepairJob { next_action };
        }
        if let Some(next_action) = inputs.missing_verifier_next_action {
            return LoopControlAction::ContinueMissingVerifierJob { next_action };
        }
    }

    if matches!(
        inputs.task_contract_action,
        Some(ArtifactRecoveryAction::RunVerifier)
    ) {
        return LoopControlAction::RunVerifier;
    }

    LoopControlAction::RequestModelTurn
}

#[cfg(test)]
pub(super) fn loop_control_action_owns_recovery(action: &LoopControlAction) -> bool {
    matches!(
        action,
        LoopControlAction::ContinueRepairJob { .. }
            | LoopControlAction::ContinueMissingVerifierJob { .. }
    )
}

pub(super) fn loop_control_action_requires_missing_verifier_setup(
    action: &LoopControlAction,
) -> bool {
    matches!(
        action,
        LoopControlAction::ContinueMissingVerifierJob {
            next_action: VerifierBootstrapNextAction::RequestSetupEdit
        }
    )
}

/// Generic recovery job categories introduced by Issue #948.
///
/// These are a projection over the existing controller-specific jobs. They let
/// coding and non-coding objective gaps share a lifecycle vocabulary while the
/// legacy job/terminal labels remain available for compatibility consumers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[allow(clippy::enum_variant_names)] // Issue #948 names are the compatibility vocabulary.
pub(super) enum RecoveryJobKind {
    MissingDeliverableJob,
    MissingEvidenceJob,
    EvidenceFailedJob,
    ToolFailureJob,
}

impl RecoveryJobKind {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            RecoveryJobKind::MissingDeliverableJob => "MissingDeliverableJob",
            RecoveryJobKind::MissingEvidenceJob => "MissingEvidenceJob",
            RecoveryJobKind::EvidenceFailedJob => "EvidenceFailedJob",
            RecoveryJobKind::ToolFailureJob => "ToolFailureJob",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RecoveryOwner {
    None,
    ArtifactCompletion,
    RepairJob,
    MissingVerifierJob,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct RecoveryDispatchGate {
    owner: RecoveryOwner,
}

impl RecoveryOwner {
    pub(super) fn from_control_action(
        action: &LoopControlAction,
        task_contract_action: Option<&ArtifactRecoveryAction>,
    ) -> Self {
        match action {
            LoopControlAction::ContinueRepairJob { .. } => Self::RepairJob,
            LoopControlAction::ContinueMissingVerifierJob { .. } => Self::MissingVerifierJob,
            LoopControlAction::RunVerifier | LoopControlAction::RequestModelTurn => {
                match task_contract_action {
                    Some(
                        ArtifactRecoveryAction::Continue { .. }
                        | ArtifactRecoveryAction::RepairArtifact { .. },
                    ) => Self::ArtifactCompletion,
                    Some(
                        ArtifactRecoveryAction::RunVerifier
                        | ArtifactRecoveryAction::Done
                        | ArtifactRecoveryAction::SafeStop { .. },
                    )
                    | None => Self::None,
                }
            }
        }
    }

    pub(super) fn recovery_job_kind(self) -> Option<RecoveryJobKind> {
        match self {
            Self::None => None,
            Self::ArtifactCompletion => Some(RecoveryJobKind::MissingDeliverableJob),
            Self::RepairJob => Some(RecoveryJobKind::EvidenceFailedJob),
            Self::MissingVerifierJob => Some(RecoveryJobKind::MissingEvidenceJob),
        }
    }

    pub(super) fn is_verifier_owned(self) -> bool {
        matches!(self, Self::RepairJob | Self::MissingVerifierJob)
    }

    pub(super) fn allows_generic_repo_change_recovery(self) -> bool {
        matches!(self, Self::None)
    }

    pub(super) fn allows_focused_edit_recovery(self) -> bool {
        matches!(self, Self::None | Self::ArtifactCompletion)
    }

    pub(super) fn allows_deterministic_fallback(self) -> bool {
        matches!(self, Self::None)
    }
}

impl RecoveryDispatchGate {
    pub(super) fn from_owner(owner: RecoveryOwner) -> Self {
        Self { owner }
    }

    #[cfg(test)]
    pub(super) fn owner(self) -> RecoveryOwner {
        self.owner
    }

    pub(super) fn allows_generic_repo_change_recovery(self) -> bool {
        self.owner.allows_generic_repo_change_recovery()
    }

    pub(super) fn allows_focused_edit_recovery(self) -> bool {
        self.owner.allows_focused_edit_recovery()
    }

    pub(super) fn allows_deterministic_fallback(self) -> bool {
        self.owner.allows_deterministic_fallback()
    }

    #[allow(dead_code)] // Issue #948: generic job projection for telemetry/report callers.
    pub(super) fn recovery_job_kind(self) -> Option<RecoveryJobKind> {
        self.owner.recovery_job_kind()
    }
}

/// Arbitration-selectable job kinds. `AnswerOnlyMode` / `PlanModeGate`
/// are pre-arbitration gates and do NOT appear here.
///
/// Issue #664 (AD1 / AD6 / 判断 5): `SetupBootstrap` is added as the
/// 6th selectable variant with priority rank = 4 (between ArtifactRecovery
/// and FocusedEditRecovery). The legacy variants below are renumbered
/// (FocusedEditRecovery 4→5, LocalLlmSmallEditAfterRead 5→6); the
/// **relative order** of the 5 legacy variants is preserved so the #660
/// pairwise priority tests are unchanged.
///
/// `#[non_exhaustive]`: additive variants can be added without breaking
/// in-crate matches (DR1-008 / DR1-003).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub(super) enum ActiveJobKind {
    /// Priority 1: verifier-repair (highest selectable priority).
    VerifierRepair,
    /// Priority 2: forced small-edit recovery (diagnostic target known).
    ForcedSmallEditRecovery,
    /// Priority 3: artifact-recovery (missing required artifact role).
    ArtifactRecovery,
    /// Issue #664: Priority 4: setup bootstrap (environment provisioning).
    SetupBootstrap,
    /// Priority 5: focused-edit recovery (heuristic last-read target).
    FocusedEditRecovery,
    /// Priority 6: local-LLM small-edit fallback (generic retry path).
    LocalLlmSmallEditAfterRead,
}

impl ActiveJobKind {
    /// Lower number = higher priority. Issue #664 (判断 5): SetupBootstrap
    /// is rank 4 (between ArtifactRecovery and FocusedEditRecovery); the
    /// legacy 5 variants retain their relative order.
    fn priority_rank(self) -> u8 {
        match self {
            ActiveJobKind::VerifierRepair => 1,
            ActiveJobKind::ForcedSmallEditRecovery => 2,
            ActiveJobKind::ArtifactRecovery => 3,
            ActiveJobKind::SetupBootstrap => 4,
            ActiveJobKind::FocusedEditRecovery => 5,
            ActiveJobKind::LocalLlmSmallEditAfterRead => 6,
        }
    }

    /// Static label for structured log payloads. Never `Debug`-dump the
    /// enum into a log — use this method so the wire vocabulary stays
    /// pinned and a future variant rename does not silently change
    /// downstream dataset columns.
    pub(super) fn as_str(self) -> &'static str {
        match self {
            ActiveJobKind::VerifierRepair => "VerifierRepair",
            ActiveJobKind::ForcedSmallEditRecovery => "ForcedSmallEditRecovery",
            ActiveJobKind::ArtifactRecovery => "ArtifactRecovery",
            ActiveJobKind::SetupBootstrap => "SetupBootstrap",
            ActiveJobKind::FocusedEditRecovery => "FocusedEditRecovery",
            ActiveJobKind::LocalLlmSmallEditAfterRead => "LocalLlmSmallEditAfterRead",
        }
    }

    fn default_recovery_job_kind(self) -> RecoveryJobKind {
        match self {
            ActiveJobKind::VerifierRepair => RecoveryJobKind::EvidenceFailedJob,
            ActiveJobKind::ForcedSmallEditRecovery
            | ActiveJobKind::ArtifactRecovery
            | ActiveJobKind::FocusedEditRecovery
            | ActiveJobKind::LocalLlmSmallEditAfterRead => RecoveryJobKind::MissingDeliverableJob,
            ActiveJobKind::SetupBootstrap => RecoveryJobKind::ToolFailureJob,
        }
    }
}

/// Semantic description of "what the job wants to do next". Distinct from
/// `policy: EffectiveToolPolicy` (which is the projection used by
/// `enforce_*`). Used today for priority/progress reasoning; Phase C will
/// project a sanitized label into the structured log payload.
///
/// `#[non_exhaustive]` per DR1-008.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub(super) enum DesiredAction {
    /// Run / re-run the project verifier with the given command. The raw
    /// command MUST NOT be `Debug`-dumped into a log payload — the
    /// `turn.rs` event builder is required to route through
    /// `redact_verifier_command_for_storage` (DR4-001 / §7 SSOT).
    VerifierRepair {
        /// Internal only. Never log raw. Used by the legacy path to drive
        /// the verifier rerun and by the future #666 reporter to compute a
        /// redacted summary.
        #[allow(dead_code)]
        command: String,
        #[allow(dead_code)]
        target_hint: Option<RecoveryTargetHint>,
        #[allow(dead_code)]
        worker_request: Option<DiagnosticRepairWorkerRequest>,
    },
    /// Create missing evidence for the current task. For coding tasks this may
    /// carry a bounded TestAuthorWorker request; legacy missing-verifier setup
    /// keeps `None` for back-compat projections.
    MissingVerifierCreate {
        worker_request: Option<TestAuthorWorkerRequest>,
    },
    /// Focused-edit recovery: model should Read (if not yet) and Edit/Write
    /// the named target.
    FocusedEdit {
        #[allow(dead_code)]
        target: PathBuf,
        #[allow(dead_code)]
        already_read: bool,
    },
    /// Artifact-directed recovery (least-privilege Write/Edit on a single
    /// required-artifact target).
    ArtifactDirected {
        #[allow(dead_code)]
        target: PathBuf,
        #[allow(dead_code)]
        already_read: bool,
        #[allow(dead_code)]
        write_actions: AllowedWriteActions,
        #[allow(dead_code)]
        read_scope: AllowedReadScope,
    },
    /// Issue #664 (AD14 / S5-003): SetupBootstrap marker variant. The
    /// command-level allow set is decided at tool enforcement time by
    /// `crate::tools::bash::is_setup_command(arguments["command"])`;
    /// the arbiter NEVER observes the raw bash command and therefore
    /// stores no `command` / `target` payload on this variant.
    SetupBash,
}

impl DesiredAction {
    /// Short static label for structured log payloads. Never include the
    /// raw verifier command / raw path in the label — those go through the
    /// `turn.rs` payload builder's redaction pipeline.
    pub(super) fn label(&self) -> &'static str {
        match self {
            DesiredAction::VerifierRepair { .. } => "verifier_repair",
            DesiredAction::MissingVerifierCreate { .. } => "missing_verifier_create",
            DesiredAction::FocusedEdit { .. } => "focused_edit",
            DesiredAction::ArtifactDirected { .. } => "artifact_directed",
            DesiredAction::SetupBash => "setup_bash",
        }
    }

    /// Optional workspace-relative target path. `None` for action variants
    /// with no path semantic (`VerifierRepair` / `SetupBash`). Consumed by the
    /// Phase C `agent.active_job.selected`
    /// payload builder to derive `target_path_hash` via
    /// `stable_path_hash(mask_secrets(...))`. The raw path MUST NOT be
    /// logged — callers route through the redaction pipeline.
    pub(super) fn target_path(&self) -> Option<&Path> {
        match self {
            DesiredAction::VerifierRepair { .. } | DesiredAction::SetupBash => None,
            DesiredAction::MissingVerifierCreate { worker_request } => worker_request
                .as_ref()
                .map(|request| request.target_test_path()),
            DesiredAction::FocusedEdit { target, .. } => Some(target.as_path()),
            DesiredAction::ArtifactDirected { target, .. } => Some(target.as_path()),
        }
    }
}

/// Budget envelope for a `JobCandidate`. DR1-003: bounded variant carries
/// the `StopReason` it would settle to on exhaustion (`#[non_exhaustive]`
/// is intentionally NOT applied here — the 2-variant set is closed-fixed).
///
/// Phase A+B note: production candidate builders (`turn.rs::
/// build_arbiter_candidates`) use `Budget::Unbounded` for every kind so
/// behaviour parity with the legacy chain is exact (the legacy chain
/// never short-circuited on budget exhaustion at the
/// `effective_tool_policy()` layer; budget exhaustion was enforced by
/// `task_contract` / `repair_job` higher up). `Budget::Bounded` is wired
/// by tests today and will be wired by production candidate builders in
/// a follow-up Issue when budget exhaustion is hoisted into the arbiter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // `Bounded` is exercised by `#[cfg(test)]` tests today; production wiring lands in a follow-up Issue.
pub(super) enum Budget {
    /// Unbounded jobs (`FocusedEditRecovery` / `LocalLlmSmallEditAfterRead`).
    Unbounded,
    /// Bounded jobs (`VerifierRepair` / `ForcedSmallEditRecovery` /
    /// `ArtifactRecovery`). Once `attempts_used >= attempts_limit.get()`,
    /// `select_active_job` rejects with `RejectionReason::BudgetExhausted`.
    Bounded {
        attempts_used: u32,
        attempts_limit: NonZeroU32,
        /// Stop reason emitted when the budget is exhausted. Must be one
        /// of the existing 6 `StopReason` variants (Issue #660 introduced
        /// the closed enum, Issue #662 added `RepairExhausted`). #662
        /// intentionally does NOT switch `VerifierRepair` to a
        /// `Bounded::RepairExhausted` budget — that path is reserved for a
        /// follow-up Issue (design judgment #1 (b) deferred).
        exhausted_stop_reason: StopReason,
    },
}

impl Budget {
    /// Has the budget been exhausted? `Unbounded` is never exhausted;
    /// `Bounded` exhausts when `attempts_used >= attempts_limit.get()`.
    fn is_exhausted(&self) -> bool {
        match self {
            Budget::Unbounded => false,
            Budget::Bounded {
                attempts_used,
                attempts_limit,
                ..
            } => *attempts_used >= attempts_limit.get(),
        }
    }

    /// `StopReason` for an exhausted bounded budget, or `None` for
    /// `Unbounded`. Used by `select_active_job` to populate
    /// `RejectionReason::BudgetExhausted { stop_reason }`.
    fn exhausted_stop_reason(&self) -> Option<StopReason> {
        match self {
            Budget::Unbounded => None,
            Budget::Bounded {
                exhausted_stop_reason,
                ..
            } => Some(*exhausted_stop_reason),
        }
    }
}

/// One candidate fed into `select_active_job`. Per DR1-004 the candidate
/// list contains only entries whose `desired_action` is **already
/// determined** by the caller; the arbiter never observes "no source"
/// (turn.rs / each job mod is responsible for skipping the branch).
///
/// DR1-001: `policy: EffectiveToolPolicy` is held **directly** — no
/// `AllowedPolicy` wrapper struct.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct JobCandidate {
    pub(super) kind: ActiveJobKind,
    pub(super) desired_action: DesiredAction,
    /// The final policy that `enforce_*` will consume if this candidate
    /// wins. Constructed by the caller (turn.rs side) so the arbiter does
    /// not need to know which constructor (`restricted` /
    /// `artifact_directed` / `focused_edit` / ...) was used.
    pub(super) policy: EffectiveToolPolicy,
    pub(super) budget: Budget,
}

impl JobCandidate {
    pub(super) fn recovery_job_kind(&self) -> RecoveryJobKind {
        match &self.desired_action {
            DesiredAction::MissingVerifierCreate { .. } => RecoveryJobKind::MissingEvidenceJob,
            DesiredAction::SetupBash => RecoveryJobKind::ToolFailureJob,
            _ => self.kind.default_recovery_job_kind(),
        }
    }
}

/// Arbitration result. `selected.is_none()` projects to
/// `EffectiveToolPolicy::unrestricted()`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ActiveJobSelection {
    pub(super) selected: Option<JobCandidate>,
    pub(super) rejected: Vec<RejectedJob>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RejectedJob {
    pub(super) kind: ActiveJobKind,
    pub(super) reason: RejectionReason,
}

/// Reasons a `JobCandidate` was rejected. DR1-004: `NoDesiredAction` is
/// NOT a variant — the candidate must arrive with a determined desired
/// action.
///
/// `#[non_exhaustive]` per DR1-008.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub(super) enum RejectionReason {
    /// Lost to a higher-priority candidate (`winner_kind` is the selected
    /// job's kind).
    LowerPriority { winner_kind: ActiveJobKind },
    /// Bounded budget already exhausted. The `stop_reason` is the one
    /// `Budget::Bounded.exhausted_stop_reason` carried, so the structured
    /// report / `safe_stop` payload builder can attach the right
    /// terminator without re-deriving it.
    BudgetExhausted { stop_reason: StopReason },
}

/// Pure function: pick at most one selectable active job.
///
/// Algorithm (matches the legacy `turn.rs::effective_tool_policy()`
/// if-elif chain in §4 of the design policy):
///
/// 1. Drop every candidate whose `budget.is_exhausted()` is true, marking
///    them `RejectionReason::BudgetExhausted { stop_reason }` (stop_reason
///    comes from the budget itself — bounded jobs without a stop reason
///    are a type error).
/// 2. Among the surviving candidates, pick the one with the **lowest**
///    `ActiveJobKind::priority_rank()` (1 = highest priority).
/// 3. Tie-break (same priority, which should not happen in production)
///    keeps the **first** candidate in declaration order — matches
///    `slice::iter().min_by_key()` documented behaviour.
/// 4. Every other surviving candidate is rejected with
///    `RejectionReason::LowerPriority { winner_kind }`.
///
/// Pure: no `Agent` self, no log emit, no filesystem, no `Clone` of
/// `Agent` state. Caller is responsible for masking / sanitization
/// downstream of this pure function.
pub(super) fn select_active_job(candidates: &[JobCandidate]) -> ActiveJobSelection {
    // Step 1: partition into (eligible, exhausted).
    let mut eligible: Vec<&JobCandidate> = Vec::with_capacity(candidates.len());
    let mut rejected: Vec<RejectedJob> = Vec::new();
    for candidate in candidates {
        if candidate.budget.is_exhausted() {
            // `exhausted_stop_reason` is always `Some` for `Bounded` and
            // `None` for `Unbounded`; `Unbounded.is_exhausted()` is false
            // so we only reach here with `Bounded`. Defensively use a
            // fallback that mirrors the highest-bound stop reason if the
            // future adds a bounded variant without a stop reason (which
            // the type forbids today).
            let stop_reason = candidate
                .budget
                .exhausted_stop_reason()
                .unwrap_or(StopReason::VerifierFailedSafeStop);
            rejected.push(RejectedJob {
                kind: candidate.kind,
                reason: RejectionReason::BudgetExhausted { stop_reason },
            });
            continue;
        }
        eligible.push(candidate);
    }

    // Step 2: pick the highest-priority survivor.
    let winner_idx = eligible
        .iter()
        .enumerate()
        .min_by_key(|(_, c)| c.kind.priority_rank())
        .map(|(idx, _)| idx);

    let selected = match winner_idx {
        Some(idx) => {
            let winner = eligible[idx].clone();
            let winner_kind = winner.kind;
            for (other_idx, other) in eligible.iter().enumerate() {
                if other_idx == idx {
                    continue;
                }
                rejected.push(RejectedJob {
                    kind: other.kind,
                    reason: RejectionReason::LowerPriority { winner_kind },
                });
            }
            Some(winner)
        }
        None => None,
    };

    ActiveJobSelection { selected, rejected }
}

/// Issue #664 (AD22 / §3 / DR1-003 SSOT): SetupBootstrap candidate install
/// decision tree. Caller (`turn.rs::build_arbiter_candidates`) is the
/// **sole** consumer; source iter() inlining at the caller is forbidden
/// (DR4-002 SSOT bypass防止).
///
/// Evaluation order (short-circuit OR):
/// 0. `artifact_ledger_overflowed == true` → false (Stage 4 fail-closed /
///    DR4-001). Ledger overflow means artifact state is ambiguous; we MUST
///    NOT freshly install Bash permission on top of unknown state.
/// 1. `has_required_setup_artifact(contract)` → true. Primary (AD13), no
///    confidence gate — `required_artifacts::Setup` is the strong pure-Install
///    intent signal.
/// 2. Confidence gate: if `BehaviorContractProjection` is absent OR
///    `confidence < LOW_CONFIDENCE_THRESHOLD` → false (DR4-001 fail-closed).
///    The remaining secondary / fallback signals require deterministic
///    behavior coverage to keep false-positives bounded.
/// 3. **Secondary (CB-001 refined / AD18)**: `optional_artifacts::Setup`
///    is present OR Stage A live (`OwnedTestVerifierPlan::Missing`)
///    fired. Stage B (label-only) is NOT sufficient here — a derived
///    `verification_expectations = ["test"]` label from a plain "add tests"
///    request would otherwise overfire. Stage B may still fire step (4)
///    via the Setup-label fallback when corroborated by Setup keywords.
/// 4. `behavior_projection_has_setup_label(p)` → true. Tertiary fallback
///    (旧 AD9 / AD13 で格下げ, label substring on
///    `required_capabilities` / `verification_expectations`).
/// 5. Otherwise → false.
pub(super) fn should_install_setup_bootstrap(
    contract: &TaskContract,
    projection: Option<&BehaviorContractProjection>,
    verifier_signal: &VerifierPrerequisiteSignal,
    artifact_ledger_overflowed: bool,
) -> bool {
    // (0) Ledger overflow fail-closed.
    if artifact_ledger_overflowed {
        return false;
    }

    // (1) Primary: required artifacts (no confidence gate).
    if has_required_setup_artifact(contract) {
        return true;
    }

    // Confidence gate guards (2)-(4).
    let Some(p) = projection else {
        return false;
    };
    if !p.confidence.is_finite() || p.confidence < LOW_CONFIDENCE_THRESHOLD {
        return false;
    }

    // (2) Secondary (CB-001 refined): `optional_artifacts::Setup` or Stage
    // A live observation. We intentionally do NOT call
    // `has_optional_setup_or_verifier_prerequisite` here (which OR-composes
    // Stage A + Stage B); instead we ask `verifier_signal.stage_a_live()`
    // for the strong live signal only. Stage B (label-derived) flows into
    // step (4) where it is corroborated by a Setup label.
    //
    // The AD18 accessor `has_optional_setup_or_verifier_prerequisite`
    // remains the SSOT for other consumers (e.g. system prompt rendering)
    // — `should_install_setup_bootstrap` is the only call site that
    // refines the signal for false-positive suppression on plain "add
    // tests" requests.
    let optional_setup_present = contract
        .optional_artifacts
        .iter()
        .any(|role| matches!(role, ArtifactRole::Setup));
    if optional_setup_present || verifier_signal.stage_a_live() {
        return true;
    }

    // (3) Tertiary fallback: label substring on capabilities /
    // verification expectations. This is where Stage B (label-only) can
    // legitimately fire — when the projection ALSO carries a Setup label
    // the SetupBootstrap install is corroborated by the user's setup
    // wording (旧 AD9). Pure verifier-capability labels without a Setup
    // anchor (e.g. derived "test" from `Add tests`) cannot reach step
    // (4) so the false-positive surface is bounded.
    if behavior_projection_has_setup_label(p) {
        return true;
    }

    false
}

#[allow(dead_code)] // Issue #948: projection seam; first production consumers are telemetry/reporting follow-ups.
pub(super) fn recovery_job_kind_for_artifact_recovery_action(
    action: &ArtifactRecoveryAction,
) -> Option<RecoveryJobKind> {
    match action {
        ArtifactRecoveryAction::Continue { .. } => Some(RecoveryJobKind::MissingDeliverableJob),
        ArtifactRecoveryAction::RunVerifier => Some(RecoveryJobKind::MissingEvidenceJob),
        ArtifactRecoveryAction::RepairArtifact { .. } => Some(RecoveryJobKind::EvidenceFailedJob),
        ArtifactRecoveryAction::Done => None,
        ArtifactRecoveryAction::SafeStop { .. } => Some(RecoveryJobKind::ToolFailureJob),
    }
}

/// Pure projection: `ActiveJobSelection` → `EffectiveToolPolicy`.
///
/// DR1-001: `JobCandidate.policy` already IS the `EffectiveToolPolicy`
/// the caller will consume; this function unwraps it (clone) when a
/// candidate was selected, else returns `EffectiveToolPolicy::unrestricted()`.
pub(super) fn project_policy(selection: &ActiveJobSelection) -> EffectiveToolPolicy {
    selection
        .selected
        .as_ref()
        .map(|c| c.policy.clone())
        .unwrap_or_else(EffectiveToolPolicy::unrestricted)
}

/// Issue #660 (Phase C / DD-5 / Stage 4 DR4-001/002) — pure builder that
/// renders an `ActiveJobSelection` to the `agent.active_job.selected`
/// payload. The payload schema (proposed to #666) is:
///
/// ```json
/// {
///   "iteration_seq": <u32>,
///   "selected": {
///     "job_kind": "<kind>|None",
///     "desired_action": "<short type label>",
///     "policy_reason": "<EffectiveToolPolicyReason::as_str()>",
///     "allowed_tools_count": <u32>,
///     "target_path_hash": "<hex16>|null"
///   },
///   "rejected": [{"job_kind": "<kind>", "rejection_reason": "LowerPriority|BudgetExhausted"}],
///   "policy_projected": {"reason_label": "<...>", "allowed_tool_kinds": <u32>},
///   "budget_state": {"repair_attempts": <u32>, "artifact_attempts": <u32>}
/// }
/// ```
///
/// Security invariants:
/// - Raw verifier commands never appear; `DesiredAction::VerifierRepair`
///   collapses to the static label `"verifier_repair"` only.
/// - Raw `PathBuf` targets never appear; `target_path_hash` is the
///   non-cryptographic correlator `stable_path_hash(mask_secrets(...))`.
/// - Log emission still re-applies `mask_payload_inplace` in `turn.rs`.
pub(super) fn build_active_job_selected_payload(
    selection: &ActiveJobSelection,
    iteration_seq: u32,
    repair_attempts: u32,
    artifact_attempts: u32,
) -> serde_json::Value {
    let projected_policy = project_policy(selection);
    let policy_reason_label = projected_policy.reason().as_str();
    let allowed_tool_kinds = projected_policy
        .allowed_tool_names_for_prompt()
        .map(|t| t.len() as u32)
        .unwrap_or(0);

    let selected_block = match selection.selected.as_ref() {
        Some(candidate) => {
            let target_path_hash = candidate
                .desired_action
                .target_path()
                .map(|path| {
                    // Never emit the raw path. The mask pass catches inline
                    // credentials; the hash gives dataset consumers a stable
                    // correlator without leaking the literal path.
                    let masked = mask_secrets(&path.display().to_string());
                    serde_json::Value::String(stable_path_hash(&masked))
                })
                .unwrap_or(serde_json::Value::Null);
            serde_json::json!({
                "job_kind": candidate.kind.as_str(),
                "legacy_job_kind": candidate.kind.as_str(),
                "recovery_job_kind": candidate.recovery_job_kind().as_str(),
                "desired_action": candidate.desired_action.label(),
                "policy_reason": candidate.policy.reason().as_str(),
                "allowed_tools_count": candidate
                    .policy
                    .allowed_tool_names_for_prompt()
                    .map(|t| t.len() as u32)
                    .unwrap_or(0),
                "target_path_hash": target_path_hash,
            })
        }
        None => serde_json::json!({
            "job_kind": "None",
            "legacy_job_kind": "None",
            "recovery_job_kind": serde_json::Value::Null,
            "desired_action": serde_json::Value::Null,
            "policy_reason": projected_policy.reason().as_str(),
            "allowed_tools_count": allowed_tool_kinds,
            "target_path_hash": serde_json::Value::Null,
        }),
    };

    let rejected_block: Vec<serde_json::Value> = selection
        .rejected
        .iter()
        .map(|rj| {
            // `RejectionReason` only has `LowerPriority` and
            // `BudgetExhausted` — both reduce to a single static label
            // without leaking external strings.
            let reason_label = match rj.reason {
                RejectionReason::LowerPriority { .. } => "LowerPriority",
                RejectionReason::BudgetExhausted { .. } => "BudgetExhausted",
            };
            serde_json::json!({
                "job_kind": rj.kind.as_str(),
                "rejection_reason": reason_label,
            })
        })
        .collect();

    serde_json::json!({
        "iteration_seq": iteration_seq,
        "selected": selected_block,
        "rejected": rejected_block,
        "policy_projected": {
            "reason_label": policy_reason_label,
            "allowed_tool_kinds": allowed_tool_kinds,
        },
        "budget_state": {
            "repair_attempts": repair_attempts,
            "artifact_attempts": artifact_attempts,
        },
    })
}

// Issue #661 DR1-002 / DR2-005: the previous `#[cfg(test)]` private
// `stable_path_hash` helper has been removed. The shared 16-hex
// `DefaultHasher` correlator now lives at `crate::logging::stable_path_hash`
// as the single SSOT for `agent.artifact_ledger.*` / `agent.active_job.*` /
// `agent.verifier.invoked` payload `path_hash` values. Issue #666
// `job_report.rs` correlator emits also route through the same SSOT.
// The unit tests below reach into the SSOT directly via the
// `crate::logging` import.

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::super::tool_policy::EffectiveToolPolicyReason;
    use super::*;
    use std::path::PathBuf;

    /// Build a minimal `JobCandidate` with a synthetic `EffectiveToolPolicy`
    /// so tests stay independent of the real `turn.rs` constructors. The
    /// `reason` field of the policy is mapped to the kind for uniqueness
    /// in `project_policy` assertions.
    fn candidate(kind: ActiveJobKind, budget: Budget) -> JobCandidate {
        let (policy_reason, desired_action) = match kind {
            ActiveJobKind::VerifierRepair => (
                EffectiveToolPolicyReason::VerifierRepair,
                DesiredAction::VerifierRepair {
                    command: "cargo test".to_string(),
                    target_hint: None,
                    worker_request: None,
                },
            ),
            ActiveJobKind::ForcedSmallEditRecovery => (
                EffectiveToolPolicyReason::FocusedEditRecovery,
                DesiredAction::FocusedEdit {
                    target: PathBuf::from("src/lib.rs"),
                    already_read: false,
                },
            ),
            ActiveJobKind::ArtifactRecovery => (
                EffectiveToolPolicyReason::ArtifactDirectedRecovery,
                DesiredAction::ArtifactDirected {
                    target: PathBuf::from("tests/foo.rs"),
                    already_read: false,
                    write_actions: AllowedWriteActions::target_create_only(),
                    read_scope: AllowedReadScope::TargetOnly,
                },
            ),
            // Issue #664: SetupBootstrap synthesises Bash-only policy.
            ActiveJobKind::SetupBootstrap => (
                EffectiveToolPolicyReason::SetupBootstrap,
                DesiredAction::SetupBash,
            ),
            ActiveJobKind::FocusedEditRecovery => (
                EffectiveToolPolicyReason::FocusedEditRecovery,
                DesiredAction::FocusedEdit {
                    target: PathBuf::from("src/lib.rs"),
                    already_read: true,
                },
            ),
            ActiveJobKind::LocalLlmSmallEditAfterRead => (
                EffectiveToolPolicyReason::LocalLlmSmallEditAfterRead,
                DesiredAction::FocusedEdit {
                    target: PathBuf::from("src/lib.rs"),
                    already_read: true,
                },
            ),
        };
        let policy = match kind {
            ActiveJobKind::SetupBootstrap => EffectiveToolPolicy::setup_bootstrap(),
            _ => EffectiveToolPolicy::restricted(policy_reason, vec!["Read", "Edit"]),
        };
        JobCandidate {
            kind,
            desired_action,
            policy,
            budget,
        }
    }

    fn bounded(attempts_used: u32, stop_reason: StopReason) -> Budget {
        Budget::Bounded {
            attempts_used,
            attempts_limit: NonZeroU32::new(3).unwrap(),
            exhausted_stop_reason: stop_reason,
        }
    }

    // -------- priority order (pairwise, 10 pairs) ----------

    fn pairwise_priority_check(higher: ActiveJobKind, lower: ActiveJobKind) {
        let candidates = vec![
            candidate(lower, Budget::Unbounded),
            candidate(higher, Budget::Unbounded),
        ];
        let selection = select_active_job(&candidates);
        let selected = selection.selected.expect("a winner exists");
        assert_eq!(
            selected.kind, higher,
            "higher priority {:?} should beat {:?}",
            higher, lower
        );
        // The losing candidate must appear in `rejected` with the
        // winner attached.
        assert!(selection.rejected.iter().any(|r| r.kind == lower
            && matches!(
                r.reason,
                RejectionReason::LowerPriority { winner_kind } if winner_kind == higher
            )));
    }

    #[test]
    fn priority_verifier_repair_beats_forced_small_edit() {
        pairwise_priority_check(
            ActiveJobKind::VerifierRepair,
            ActiveJobKind::ForcedSmallEditRecovery,
        );
    }

    #[test]
    fn priority_verifier_repair_beats_artifact_recovery() {
        pairwise_priority_check(
            ActiveJobKind::VerifierRepair,
            ActiveJobKind::ArtifactRecovery,
        );
    }

    #[test]
    fn priority_verifier_repair_beats_focused_edit_recovery() {
        pairwise_priority_check(
            ActiveJobKind::VerifierRepair,
            ActiveJobKind::FocusedEditRecovery,
        );
    }

    #[test]
    fn priority_verifier_repair_beats_local_llm_small_edit() {
        pairwise_priority_check(
            ActiveJobKind::VerifierRepair,
            ActiveJobKind::LocalLlmSmallEditAfterRead,
        );
    }

    #[test]
    fn priority_forced_small_edit_beats_artifact_recovery() {
        pairwise_priority_check(
            ActiveJobKind::ForcedSmallEditRecovery,
            ActiveJobKind::ArtifactRecovery,
        );
    }

    #[test]
    fn priority_forced_small_edit_beats_focused_edit_recovery() {
        pairwise_priority_check(
            ActiveJobKind::ForcedSmallEditRecovery,
            ActiveJobKind::FocusedEditRecovery,
        );
    }

    #[test]
    fn priority_forced_small_edit_beats_local_llm_small_edit() {
        pairwise_priority_check(
            ActiveJobKind::ForcedSmallEditRecovery,
            ActiveJobKind::LocalLlmSmallEditAfterRead,
        );
    }

    #[test]
    fn priority_artifact_recovery_beats_focused_edit_recovery() {
        pairwise_priority_check(
            ActiveJobKind::ArtifactRecovery,
            ActiveJobKind::FocusedEditRecovery,
        );
    }

    #[test]
    fn priority_artifact_recovery_beats_local_llm_small_edit() {
        pairwise_priority_check(
            ActiveJobKind::ArtifactRecovery,
            ActiveJobKind::LocalLlmSmallEditAfterRead,
        );
    }

    #[test]
    fn priority_focused_edit_beats_local_llm_small_edit() {
        pairwise_priority_check(
            ActiveJobKind::FocusedEditRecovery,
            ActiveJobKind::LocalLlmSmallEditAfterRead,
        );
    }

    // -------- bounded budget exhaustion -> RejectionReason --------

    #[test]
    fn bounded_verifier_repair_exhausted_rejects_with_safe_stop() {
        let exhausted = candidate(
            ActiveJobKind::VerifierRepair,
            bounded(3, StopReason::VerifierFailedSafeStop),
        );
        let selection = select_active_job(&[exhausted]);
        assert!(
            selection.selected.is_none(),
            "exhausted bounded candidate must not be selected"
        );
        assert_eq!(selection.rejected.len(), 1);
        let r = &selection.rejected[0];
        assert_eq!(r.kind, ActiveJobKind::VerifierRepair);
        assert_eq!(
            r.reason,
            RejectionReason::BudgetExhausted {
                stop_reason: StopReason::VerifierFailedSafeStop
            }
        );
    }

    #[test]
    fn bounded_forced_small_edit_exhausted_rejects_with_diagnostic_target_missing() {
        let exhausted = candidate(
            ActiveJobKind::ForcedSmallEditRecovery,
            bounded(3, StopReason::DiagnosticTargetMissing),
        );
        let selection = select_active_job(&[exhausted]);
        assert!(selection.selected.is_none());
        assert_eq!(
            selection.rejected[0].reason,
            RejectionReason::BudgetExhausted {
                stop_reason: StopReason::DiagnosticTargetMissing
            }
        );
    }

    #[test]
    fn bounded_artifact_recovery_exhausted_rejects_with_artifact_completion_failed() {
        let exhausted = candidate(
            ActiveJobKind::ArtifactRecovery,
            bounded(3, StopReason::ArtifactCompletionFailed),
        );
        let selection = select_active_job(&[exhausted]);
        assert!(selection.selected.is_none());
        assert_eq!(
            selection.rejected[0].reason,
            RejectionReason::BudgetExhausted {
                stop_reason: StopReason::ArtifactCompletionFailed
            }
        );
    }

    /// Exhausted bounded job + healthy lower-priority job → lower wins
    /// (legacy chain would also reach the lower branch because the
    /// exhausted higher branch returns nothing).
    #[test]
    fn exhausted_higher_priority_lets_lower_priority_win() {
        let candidates = vec![
            candidate(
                ActiveJobKind::VerifierRepair,
                bounded(3, StopReason::VerifierFailedSafeStop),
            ),
            candidate(ActiveJobKind::ArtifactRecovery, Budget::Unbounded),
        ];
        let selection = select_active_job(&candidates);
        let selected = selection.selected.expect("artifact recovery wins");
        assert_eq!(selected.kind, ActiveJobKind::ArtifactRecovery);
        // Two rejection entries: one BudgetExhausted, one... wait, only
        // one rejection (the exhausted higher-priority job). The winner
        // is not in `rejected`.
        assert_eq!(selection.rejected.len(), 1);
        assert_eq!(selection.rejected[0].kind, ActiveJobKind::VerifierRepair);
        assert!(matches!(
            selection.rejected[0].reason,
            RejectionReason::BudgetExhausted { .. }
        ));
    }

    // -------- empty input --------

    #[test]
    fn empty_candidate_list_projects_to_unrestricted() {
        let selection = select_active_job(&[]);
        assert!(selection.selected.is_none());
        assert!(selection.rejected.is_empty());
        let policy = project_policy(&selection);
        assert_eq!(policy, EffectiveToolPolicy::unrestricted());
    }

    #[test]
    fn active_job_payload_preserves_legacy_job_label_and_adds_recovery_job_kind() {
        let selection = ActiveJobSelection {
            selected: Some(candidate(ActiveJobKind::VerifierRepair, Budget::Unbounded)),
            rejected: vec![],
        };
        let payload = build_active_job_selected_payload(&selection, 9, 0, 0);
        let selected = payload.get("selected").expect("selected block");

        assert_eq!(
            selected.get("job_kind").and_then(|v| v.as_str()),
            Some("VerifierRepair")
        );
        assert_eq!(
            selected.get("legacy_job_kind").and_then(|v| v.as_str()),
            Some("VerifierRepair")
        );
        assert_eq!(
            selected.get("recovery_job_kind").and_then(|v| v.as_str()),
            Some("EvidenceFailedJob")
        );
    }

    // -------- project_policy: 5 kinds --------

    fn project_kind_policy(kind: ActiveJobKind) {
        let c = candidate(kind, Budget::Unbounded);
        let expected = c.policy.clone();
        let selection = select_active_job(std::slice::from_ref(&c));
        let projected = project_policy(&selection);
        assert_eq!(
            projected, expected,
            "project_policy must unwrap the selected job's policy for {:?}",
            kind
        );
    }

    #[test]
    fn project_policy_verifier_repair() {
        project_kind_policy(ActiveJobKind::VerifierRepair);
    }

    #[test]
    fn project_policy_forced_small_edit_recovery() {
        project_kind_policy(ActiveJobKind::ForcedSmallEditRecovery);
    }

    #[test]
    fn project_policy_artifact_recovery() {
        project_kind_policy(ActiveJobKind::ArtifactRecovery);
    }

    #[test]
    fn project_policy_focused_edit_recovery() {
        project_kind_policy(ActiveJobKind::FocusedEditRecovery);
    }

    #[test]
    fn project_policy_local_llm_small_edit() {
        project_kind_policy(ActiveJobKind::LocalLlmSmallEditAfterRead);
    }

    // -------- DesiredAction labels / ActiveJobKind labels --------

    #[test]
    fn active_job_kind_labels_are_stable() {
        // Wire vocabulary: future renames MUST update this test.
        assert_eq!(ActiveJobKind::VerifierRepair.as_str(), "VerifierRepair");
        assert_eq!(
            ActiveJobKind::ForcedSmallEditRecovery.as_str(),
            "ForcedSmallEditRecovery"
        );
        assert_eq!(ActiveJobKind::ArtifactRecovery.as_str(), "ArtifactRecovery");
        assert_eq!(
            ActiveJobKind::FocusedEditRecovery.as_str(),
            "FocusedEditRecovery"
        );
        assert_eq!(
            ActiveJobKind::LocalLlmSmallEditAfterRead.as_str(),
            "LocalLlmSmallEditAfterRead"
        );
    }

    #[test]
    fn recovery_job_kind_labels_are_stable() {
        assert_eq!(
            RecoveryJobKind::MissingDeliverableJob.as_str(),
            "MissingDeliverableJob"
        );
        assert_eq!(
            RecoveryJobKind::MissingEvidenceJob.as_str(),
            "MissingEvidenceJob"
        );
        assert_eq!(
            RecoveryJobKind::EvidenceFailedJob.as_str(),
            "EvidenceFailedJob"
        );
        assert_eq!(RecoveryJobKind::ToolFailureJob.as_str(), "ToolFailureJob");
    }

    #[test]
    fn active_job_candidates_project_generic_recovery_jobs() {
        let verifier = candidate(ActiveJobKind::VerifierRepair, Budget::Unbounded);
        assert_eq!(
            verifier.recovery_job_kind(),
            RecoveryJobKind::EvidenceFailedJob
        );

        let missing_verifier = JobCandidate {
            kind: ActiveJobKind::VerifierRepair,
            desired_action: DesiredAction::MissingVerifierCreate {
                worker_request: None,
            },
            policy: EffectiveToolPolicy::restricted(
                EffectiveToolPolicyReason::VerifierRepair,
                vec!["Read", "Edit"],
            ),
            budget: Budget::Unbounded,
        };
        assert_eq!(
            missing_verifier.recovery_job_kind(),
            RecoveryJobKind::MissingEvidenceJob
        );

        let artifact = candidate(ActiveJobKind::ArtifactRecovery, Budget::Unbounded);
        assert_eq!(
            artifact.recovery_job_kind(),
            RecoveryJobKind::MissingDeliverableJob
        );

        let setup = candidate(ActiveJobKind::SetupBootstrap, Budget::Unbounded);
        assert_eq!(setup.recovery_job_kind(), RecoveryJobKind::ToolFailureJob);
    }

    #[test]
    fn desired_action_labels_are_stable() {
        let a = DesiredAction::VerifierRepair {
            command: "cargo test".to_string(),
            target_hint: None,
            worker_request: None,
        };
        assert_eq!(a.label(), "verifier_repair");
        let b = DesiredAction::MissingVerifierCreate {
            worker_request: None,
        };
        assert_eq!(b.label(), "missing_verifier_create");
        let c = DesiredAction::FocusedEdit {
            target: PathBuf::from("x"),
            already_read: false,
        };
        assert_eq!(c.label(), "focused_edit");
        let d = DesiredAction::ArtifactDirected {
            target: PathBuf::from("x"),
            already_read: false,
            write_actions: AllowedWriteActions::target_create_only(),
            read_scope: AllowedReadScope::TargetOnly,
        };
        assert_eq!(d.label(), "artifact_directed");
    }

    // -------- stable_path_hash (Issue #661 DR1-002: SSOT in logging.rs) --------

    #[test]
    fn stable_path_hash_is_deterministic_and_16_hex() {
        use crate::logging::stable_path_hash;
        let h1 = stable_path_hash("src/lib.rs");
        let h2 = stable_path_hash("src/lib.rs");
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 16);
        assert!(h1.chars().all(|c| c.is_ascii_hexdigit()));
        // different input → (almost certainly) different hash
        let h3 = stable_path_hash("src/other.rs");
        assert_ne!(h1, h3);
    }

    #[test]
    fn missing_verifier_create_variant_constructs_and_labels() {
        let c = JobCandidate {
            kind: ActiveJobKind::VerifierRepair,
            desired_action: DesiredAction::MissingVerifierCreate {
                worker_request: None,
            },
            policy: EffectiveToolPolicy::restricted(
                EffectiveToolPolicyReason::VerifierRepair,
                vec!["Write"],
            ),
            budget: Budget::Bounded {
                attempts_used: 0,
                attempts_limit: NonZeroU32::new(1).unwrap(),
                exhausted_stop_reason: StopReason::VerifierMissing,
            },
        };
        assert_eq!(c.desired_action.label(), "missing_verifier_create");
        let kind = c.kind;
        let sel = select_active_job(std::slice::from_ref(&c));
        assert_eq!(sel.selected.as_ref().map(|s| s.kind), Some(kind));
    }

    // -----------------------------------------------------------------
    // Issue #664: SetupBootstrap variant + priority_rank tests.
    // -----------------------------------------------------------------

    /// Acceptance (g): `ActiveJobKind::SetupBootstrap` is present and the
    /// enum retains `#[non_exhaustive]`. The presence assertion uses the
    /// `as_str()` SSOT (label is "SetupBootstrap").
    #[test]
    fn active_job_kind_includes_setup_bootstrap_non_exhaustive() {
        assert_eq!(ActiveJobKind::SetupBootstrap.as_str(), "SetupBootstrap");
    }

    /// `DesiredAction::SetupBash` is a marker variant — no `command` /
    /// `target` fields.
    #[test]
    fn desired_action_setup_bash_is_marker_variant_no_command_field() {
        let a = DesiredAction::SetupBash;
        // No payload — clone is value-equal.
        assert_eq!(a.clone(), DesiredAction::SetupBash);
    }

    /// `DesiredAction::SetupBash.label()` returns "setup_bash".
    #[test]
    fn desired_action_label_returns_setup_bash() {
        assert_eq!(DesiredAction::SetupBash.label(), "setup_bash");
    }

    /// `DesiredAction::SetupBash.target_path()` returns `None`.
    #[test]
    fn desired_action_target_path_returns_none_for_setup_bash() {
        assert!(DesiredAction::SetupBash.target_path().is_none());
    }

    /// 判断 5 (a): the 5 legacy variants' **relative order** is preserved
    /// after SetupBootstrap is inserted at rank 4. Rank numbers shift
    /// (FocusedEditRecovery 4→5, LocalLlmSmallEditAfterRead 5→6) but the
    /// comparison ordering is unchanged.
    #[test]
    fn other_active_job_kinds_priority_rank_preserved() {
        let ordered = [
            ActiveJobKind::VerifierRepair,
            ActiveJobKind::ForcedSmallEditRecovery,
            ActiveJobKind::ArtifactRecovery,
            ActiveJobKind::FocusedEditRecovery,
            ActiveJobKind::LocalLlmSmallEditAfterRead,
        ];
        for window in ordered.windows(2) {
            assert!(
                window[0].priority_rank() < window[1].priority_rank(),
                "legacy order {:?} -> {:?} broken",
                window[0],
                window[1]
            );
        }
    }

    /// 判断 5 (b): SetupBootstrap sits strictly between ArtifactRecovery
    /// and FocusedEditRecovery.
    #[test]
    fn setup_bootstrap_priority_rank_between_artifact_recovery_and_focused_edit() {
        let setup = ActiveJobKind::SetupBootstrap.priority_rank();
        assert!(ActiveJobKind::ArtifactRecovery.priority_rank() < setup);
        assert!(setup < ActiveJobKind::FocusedEditRecovery.priority_rank());
    }

    // -----------------------------------------------------------------
    // Issue #664: EffectiveToolPolicy::setup_bootstrap() builder.
    // -----------------------------------------------------------------

    #[test]
    fn effective_tool_policy_setup_bootstrap_allows_only_bash() {
        let policy = EffectiveToolPolicy::setup_bootstrap();
        assert_eq!(policy.reason(), EffectiveToolPolicyReason::SetupBootstrap);
        assert_eq!(policy.allowed_tool_names_for_prompt(), Some(&["Bash"][..]));
        assert!(policy.focused_edit_policy().is_none());
        assert!(policy.artifact_directed_policy().is_none());
    }

    #[test]
    fn effective_tool_policy_reason_setup_bootstrap_as_str_returns_setup_bootstrap() {
        assert_eq!(
            EffectiveToolPolicyReason::SetupBootstrap.as_str(),
            "setup_bootstrap"
        );
    }

    // -----------------------------------------------------------------
    // Issue #664: `should_install_setup_bootstrap` decision tree.
    // -----------------------------------------------------------------

    use super::super::required_behavior::{BoundedLabelWithExcerpt, project_behavior_contract};
    use super::super::task_contract::TaskContract;

    fn install_contract() -> TaskContract {
        // Pure install intent — `ArtifactRole::Setup` in `required_artifacts`.
        TaskContract::from_request("Install the dependencies listed in requirements.txt.")
    }

    fn build_contract_no_setup() -> TaskContract {
        TaskContract::from_request("FastAPIでcrudのAPIを開発してください。")
    }

    fn projection_with_setup_label() -> BehaviorContractProjection {
        BehaviorContractProjection {
            confidence: 0.9,
            fields_used: vec!["required_capabilities"],
            behavior_goal: None,
            required_capabilities: vec![BoundedLabelWithExcerpt {
                label: "install dependencies".to_string(),
                excerpt: None,
            }],
            verification_expectations: vec![],
            non_goals: vec![],
        }
    }

    fn projection_low_confidence() -> BehaviorContractProjection {
        BehaviorContractProjection {
            confidence: 0.3,
            fields_used: vec!["required_capabilities"],
            behavior_goal: None,
            required_capabilities: vec![BoundedLabelWithExcerpt {
                label: "install dependencies".to_string(),
                excerpt: None,
            }],
            verification_expectations: vec![],
            non_goals: vec![],
        }
    }

    fn projection_no_setup_label() -> BehaviorContractProjection {
        BehaviorContractProjection {
            confidence: 0.9,
            fields_used: vec!["required_capabilities"],
            behavior_goal: None,
            required_capabilities: vec![BoundedLabelWithExcerpt {
                label: "draw chart".to_string(),
                excerpt: None,
            }],
            verification_expectations: vec![],
            non_goals: vec![],
        }
    }

    #[test]
    fn should_install_setup_bootstrap_true_when_required_artifact_setup_present() {
        let contract = install_contract();
        let no_signal = VerifierPrerequisiteSignal::from_sources(false, None);
        // Primary path: no projection needed
        assert!(should_install_setup_bootstrap(
            &contract, None, &no_signal, false
        ));
        // Even with low confidence projection — primary short-circuits
        let p = projection_low_confidence();
        assert!(should_install_setup_bootstrap(
            &contract,
            Some(&p),
            &no_signal,
            false
        ));
    }

    #[test]
    fn should_install_setup_bootstrap_fails_closed_when_artifact_ledger_overflowed() {
        let contract = install_contract();
        let no_signal = VerifierPrerequisiteSignal::from_sources(false, None);
        // ledger overflow beats primary required_artifact_setup
        assert!(!should_install_setup_bootstrap(
            &contract, None, &no_signal, true
        ));
    }

    #[test]
    fn should_install_setup_bootstrap_false_when_behavior_projection_none_and_no_required_setup() {
        let contract = build_contract_no_setup();
        let no_signal = VerifierPrerequisiteSignal::from_sources(false, None);
        // No required Setup + no projection → fail-closed false (DR4-001).
        assert!(!should_install_setup_bootstrap(
            &contract, None, &no_signal, false
        ));
    }

    #[test]
    fn should_install_setup_bootstrap_false_when_confidence_below_threshold() {
        let contract = build_contract_no_setup();
        let no_signal = VerifierPrerequisiteSignal::from_sources(false, None);
        let p = projection_low_confidence();
        // confidence < 0.5 → fail-closed even with a matching setup label.
        assert!(!should_install_setup_bootstrap(
            &contract,
            Some(&p),
            &no_signal,
            false
        ));
    }

    #[test]
    fn should_install_setup_bootstrap_true_when_verifier_prerequisite_required_and_confidence_above_threshold()
     {
        let contract = build_contract_no_setup();
        let verifier_signal = VerifierPrerequisiteSignal::from_sources(true, None);
        let p = projection_no_setup_label();
        // Verifier prerequisite Stage A active + sufficient confidence → install.
        assert!(should_install_setup_bootstrap(
            &contract,
            Some(&p),
            &verifier_signal,
            false
        ));
    }

    #[test]
    fn should_install_setup_bootstrap_falls_back_to_label_substring_only_with_confidence_gate() {
        let contract = build_contract_no_setup();
        let no_signal = VerifierPrerequisiteSignal::from_sources(false, None);
        // High confidence + setup label fallback → install.
        let p = projection_with_setup_label();
        assert!(should_install_setup_bootstrap(
            &contract,
            Some(&p),
            &no_signal,
            false
        ));
        // Same projection but no setup label → false (no fallback signal).
        let p_no = projection_no_setup_label();
        assert!(!should_install_setup_bootstrap(
            &contract,
            Some(&p_no),
            &no_signal,
            false
        ));
    }

    #[test]
    fn should_install_setup_bootstrap_true_for_optional_setup_and_confidence_above_threshold() {
        // Force `optional_artifacts::Setup` and a high-confidence projection
        // without a setup label so optional Setup is the only fallback driver.
        let mut contract = build_contract_no_setup();
        contract
            .optional_artifacts
            .push(super::super::task_contract::ArtifactRole::Setup);
        let no_signal = VerifierPrerequisiteSignal::from_sources(false, None);
        let p = projection_no_setup_label();
        assert!(should_install_setup_bootstrap(
            &contract,
            Some(&p),
            &no_signal,
            false
        ));
    }

    /// Regression: AD20 — `BehaviorContractProjection == None` (e.g. low
    /// confidence at extraction time) must still allow `required_artifacts::Setup`
    /// primary to install SetupBootstrap. `project_behavior_contract` may
    /// return `None` for a contract whose `required_behavior.confidence` is
    /// below threshold; SetupBootstrap primary still fires.
    #[test]
    fn setup_bootstrap_install_succeeds_when_behavior_projection_none() {
        let contract = install_contract();
        let proj = project_behavior_contract(&contract);
        let no_signal = VerifierPrerequisiteSignal::from_sources(false, None);
        assert!(should_install_setup_bootstrap(
            &contract,
            proj.as_ref(),
            &no_signal,
            false
        ));
    }

    fn loop_inputs() -> LoopControlInputs {
        LoopControlInputs {
            mode: ExecutionMode::Act,
            task_contract_verifier_repair_pending: false,
            repair_next_action: None,
            missing_verifier_next_action: None,
            task_contract_action: None,
        }
    }

    #[test]
    fn loop_control_repair_job_wins_over_verifier_run_inside_arbiter_module() {
        let next_action = RepairNextAction::RequestDiagnostic;
        let action = determine_loop_control_action(LoopControlInputs {
            task_contract_verifier_repair_pending: true,
            repair_next_action: Some(next_action.clone()),
            task_contract_action: Some(ArtifactRecoveryAction::RunVerifier),
            ..loop_inputs()
        });

        assert_eq!(action, LoopControlAction::ContinueRepairJob { next_action });
        let owner =
            RecoveryOwner::from_control_action(&action, Some(&ArtifactRecoveryAction::RunVerifier));
        assert_eq!(owner, RecoveryOwner::RepairJob);
        assert!(!owner.allows_focused_edit_recovery());
        assert!(!owner.allows_deterministic_fallback());
    }

    #[test]
    fn loop_control_missing_verifier_wins_over_model_turn_inside_arbiter_module() {
        let next_action = VerifierBootstrapNextAction::RequestSetupEdit;
        let action = determine_loop_control_action(LoopControlInputs {
            task_contract_verifier_repair_pending: true,
            missing_verifier_next_action: Some(next_action.clone()),
            ..loop_inputs()
        });

        assert_eq!(
            action,
            LoopControlAction::ContinueMissingVerifierJob { next_action }
        );
        assert!(loop_control_action_requires_missing_verifier_setup(&action));
        assert!(loop_control_action_owns_recovery(&action));
    }

    #[test]
    fn recoverable_missing_evidence_setup_beats_safe_stop_action() {
        let next_action = VerifierBootstrapNextAction::RequestSetupEdit;
        let action = determine_loop_control_action(LoopControlInputs {
            task_contract_verifier_repair_pending: true,
            missing_verifier_next_action: Some(next_action.clone()),
            task_contract_action: Some(ArtifactRecoveryAction::SafeStop {
                reason: super::super::task_contract::SafeStopReason::VerifierMissing,
            }),
            ..loop_inputs()
        });

        assert_eq!(
            action,
            LoopControlAction::ContinueMissingVerifierJob { next_action }
        );
        let owner = RecoveryOwner::from_control_action(
            &action,
            Some(&ArtifactRecoveryAction::SafeStop {
                reason: super::super::task_contract::SafeStopReason::VerifierMissing,
            }),
        );
        assert_eq!(
            owner.recovery_job_kind(),
            Some(RecoveryJobKind::MissingEvidenceJob)
        );
        assert!(loop_control_action_requires_missing_verifier_setup(&action));
    }

    #[test]
    fn loop_control_plan_mode_never_dispatches_controller_jobs() {
        let action = determine_loop_control_action(LoopControlInputs {
            mode: ExecutionMode::Plan,
            task_contract_verifier_repair_pending: true,
            repair_next_action: Some(RepairNextAction::RequestDiagnostic),
            missing_verifier_next_action: Some(VerifierBootstrapNextAction::RequestSetupEdit),
            task_contract_action: Some(ArtifactRecoveryAction::RunVerifier),
        });

        assert_eq!(action, LoopControlAction::RequestModelTurn);
    }

    #[test]
    fn loop_control_stale_pending_flag_without_owner_requests_model_turn() {
        let action = determine_loop_control_action(LoopControlInputs {
            task_contract_verifier_repair_pending: true,
            repair_next_action: None,
            missing_verifier_next_action: None,
            task_contract_action: None,
            ..loop_inputs()
        });

        assert_eq!(action, LoopControlAction::RequestModelTurn);
    }

    #[test]
    fn loop_control_transition_table_covers_controller_owned_states() {
        let target_hint = RecoveryTargetHint {
            role: ArtifactRole::Implementation,
            path: "app/main.py".to_string(),
            reason: "synthetic transition target".to_string(),
        };
        let cases = vec![
            (
                "repair diagnostic",
                LoopControlInputs {
                    task_contract_verifier_repair_pending: true,
                    repair_next_action: Some(RepairNextAction::RequestDiagnostic),
                    ..loop_inputs()
                },
                RecoveryOwner::RepairJob,
            ),
            (
                "repair patch",
                LoopControlInputs {
                    task_contract_verifier_repair_pending: true,
                    repair_next_action: Some(RepairNextAction::RequestPatch {
                        target_hint: target_hint.clone(),
                    }),
                    ..loop_inputs()
                },
                RecoveryOwner::RepairJob,
            ),
            (
                "repair rerun",
                LoopControlInputs {
                    task_contract_verifier_repair_pending: true,
                    repair_next_action: Some(RepairNextAction::RerunVerifier),
                    ..loop_inputs()
                },
                RecoveryOwner::RepairJob,
            ),
            (
                "repair safe stop",
                LoopControlInputs {
                    task_contract_verifier_repair_pending: true,
                    repair_next_action: Some(RepairNextAction::SafeStop {
                        reason:
                            super::super::repair_job::RepairTerminalReason::RepairBudgetExhausted,
                    }),
                    ..loop_inputs()
                },
                RecoveryOwner::RepairJob,
            ),
            (
                "missing verifier setup",
                LoopControlInputs {
                    task_contract_verifier_repair_pending: true,
                    missing_verifier_next_action: Some(
                        VerifierBootstrapNextAction::RequestSetupEdit,
                    ),
                    ..loop_inputs()
                },
                RecoveryOwner::MissingVerifierJob,
            ),
            (
                "missing verifier rerun",
                LoopControlInputs {
                    task_contract_verifier_repair_pending: true,
                    missing_verifier_next_action: Some(VerifierBootstrapNextAction::RerunVerifier),
                    ..loop_inputs()
                },
                RecoveryOwner::MissingVerifierJob,
            ),
            (
                "missing verifier safe stop",
                LoopControlInputs {
                    task_contract_verifier_repair_pending: true,
                    missing_verifier_next_action: Some(VerifierBootstrapNextAction::SafeStop {
                        reason: "synthetic missing-verifier exhaustion",
                    }),
                    ..loop_inputs()
                },
                RecoveryOwner::MissingVerifierJob,
            ),
            (
                "verifier run",
                LoopControlInputs {
                    task_contract_action: Some(ArtifactRecoveryAction::RunVerifier),
                    ..loop_inputs()
                },
                RecoveryOwner::None,
            ),
            (
                "artifact completion",
                LoopControlInputs {
                    task_contract_action: Some(ArtifactRecoveryAction::Continue {
                        missing: vec![ArtifactRole::Implementation],
                        target_hint: Some(target_hint),
                    }),
                    ..loop_inputs()
                },
                RecoveryOwner::ArtifactCompletion,
            ),
        ];

        for (label, input, expected_owner) in cases {
            let task_contract_action = input.task_contract_action.clone();
            let action = determine_loop_control_action(input);
            let owner = RecoveryOwner::from_control_action(&action, task_contract_action.as_ref());
            let gate = RecoveryDispatchGate::from_owner(owner);

            assert_eq!(owner, expected_owner, "{label}");
            match owner {
                RecoveryOwner::RepairJob | RecoveryOwner::MissingVerifierJob => {
                    assert!(!gate.allows_generic_repo_change_recovery(), "{label}");
                    assert!(!gate.allows_focused_edit_recovery(), "{label}");
                    assert!(!gate.allows_deterministic_fallback(), "{label}");
                }
                RecoveryOwner::ArtifactCompletion => {
                    assert!(!gate.allows_generic_repo_change_recovery(), "{label}");
                    assert!(gate.allows_focused_edit_recovery(), "{label}");
                    assert!(!gate.allows_deterministic_fallback(), "{label}");
                }
                RecoveryOwner::None => {
                    assert!(gate.allows_generic_repo_change_recovery(), "{label}");
                    assert!(gate.allows_focused_edit_recovery(), "{label}");
                    assert!(gate.allows_deterministic_fallback(), "{label}");
                }
            }
        }
    }

    #[test]
    fn recovery_owners_project_to_generic_job_kinds() {
        let cases = [
            (
                RecoveryOwner::ArtifactCompletion,
                Some(RecoveryJobKind::MissingDeliverableJob),
            ),
            (
                RecoveryOwner::RepairJob,
                Some(RecoveryJobKind::EvidenceFailedJob),
            ),
            (
                RecoveryOwner::MissingVerifierJob,
                Some(RecoveryJobKind::MissingEvidenceJob),
            ),
            (RecoveryOwner::None, None),
        ];

        for (owner, expected) in cases {
            assert_eq!(owner.recovery_job_kind(), expected, "owner={owner:?}");
            assert_eq!(
                RecoveryDispatchGate::from_owner(owner).recovery_job_kind(),
                expected,
                "gate owner={owner:?}"
            );
        }
    }

    #[test]
    fn recovery_owner_gates_lower_level_fallbacks() {
        let repair = RecoveryDispatchGate::from_owner(RecoveryOwner::RepairJob);
        assert_eq!(repair.owner(), RecoveryOwner::RepairJob);
        assert!(!repair.allows_generic_repo_change_recovery());
        assert!(!repair.allows_focused_edit_recovery());
        assert!(!repair.allows_deterministic_fallback());

        let missing_verifier = RecoveryDispatchGate::from_owner(RecoveryOwner::MissingVerifierJob);
        assert!(!missing_verifier.allows_generic_repo_change_recovery());
        assert!(!missing_verifier.allows_focused_edit_recovery());
        assert!(!missing_verifier.allows_deterministic_fallback());

        let artifact = RecoveryDispatchGate::from_owner(RecoveryOwner::ArtifactCompletion);
        assert!(!artifact.allows_generic_repo_change_recovery());
        assert!(artifact.allows_focused_edit_recovery());
        assert!(!artifact.allows_deterministic_fallback());

        let none = RecoveryDispatchGate::from_owner(RecoveryOwner::None);
        assert!(none.allows_generic_repo_change_recovery());
        assert!(none.allows_focused_edit_recovery());
        assert!(none.allows_deterministic_fallback());
    }
}
