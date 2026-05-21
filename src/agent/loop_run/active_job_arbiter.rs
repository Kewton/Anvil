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
//!   functions; structured log emission (`agent.active_job.selected` /
//!   `agent.active_job.divergence_detected`) lives in `turn.rs` event payload
//!   builders that route through `logging::mask_payload_inplace` and
//!   `session::feedback::mask_secrets` / `redact_verifier_command_for_storage`.
//! - **no raw path / command**: arbiter does not format / `Debug` payload
//!   strings. `DesiredAction::VerifierRepair.command` is held only for the
//!   internal contract; the payload builder must redact via
//!   `redact_verifier_command_for_storage` before logging (never `Debug`).

use std::num::NonZeroU32;
use std::path::PathBuf;

use super::artifact_completion_job::{AllowedReadScope, AllowedWriteActions};
use super::repair_job::StopReason;
use super::task_contract::RecoveryTargetHint;
use super::turn::EffectiveToolPolicy;

/// Arbitration-selectable job kinds (priority 1-5 in §4 of the design
/// policy). `AnswerOnlyMode` / `PlanModeGate` / `SetupBootstrap` are
/// pre-arbitration gates and do NOT appear here.
///
/// `#[non_exhaustive]`: future jobs (e.g. SetupBootstrap promotion in #664)
/// can be added without a breaking match (DR1-008).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub(super) enum ActiveJobKind {
    /// Priority 1: verifier-repair (highest selectable priority).
    VerifierRepair,
    /// Priority 2: forced small-edit recovery (diagnostic target known).
    ForcedSmallEditRecovery,
    /// Priority 3: artifact-recovery (missing required artifact role).
    ArtifactRecovery,
    /// Priority 4: focused-edit recovery (heuristic last-read target).
    FocusedEditRecovery,
    /// Priority 5: local-LLM small-edit fallback (generic retry path).
    LocalLlmSmallEditAfterRead,
}

impl ActiveJobKind {
    /// Lower number = higher priority. §4 of the design policy fixes
    /// the order (VerifierRepair=1, ..., LocalLlmSmallEditAfterRead=5).
    fn priority_rank(self) -> u8 {
        match self {
            ActiveJobKind::VerifierRepair => 1,
            ActiveJobKind::ForcedSmallEditRecovery => 2,
            ActiveJobKind::ArtifactRecovery => 3,
            ActiveJobKind::FocusedEditRecovery => 4,
            ActiveJobKind::LocalLlmSmallEditAfterRead => 5,
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
            ActiveJobKind::FocusedEditRecovery => "FocusedEditRecovery",
            ActiveJobKind::LocalLlmSmallEditAfterRead => "LocalLlmSmallEditAfterRead",
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
    },
    /// Create the missing verifier file (no diagnostic command yet).
    MissingVerifierCreate,
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
}

impl DesiredAction {
    /// Short static label for structured log payloads. Never include the
    /// raw verifier command / raw path in the label — those go through the
    /// `turn.rs` payload builder's redaction pipeline.
    pub(super) fn label(&self) -> &'static str {
        match self {
            DesiredAction::VerifierRepair { .. } => "verifier_repair",
            DesiredAction::MissingVerifierCreate => "missing_verifier_create",
            DesiredAction::FocusedEdit { .. } => "focused_edit",
            DesiredAction::ArtifactDirected { .. } => "artifact_directed",
        }
    }

    /// Optional workspace-relative target path. `None` for action variants
    /// with no path semantic (`VerifierRepair` / `MissingVerifierCreate`).
    /// Consumed by the Phase C `agent.active_job.selected` payload builder
    /// to derive `target_path_hash` via
    /// `stable_path_hash(mask_secrets(...))`. The raw path MUST NOT be
    /// logged — callers route through the redaction pipeline.
    pub(super) fn target_path(&self) -> Option<&PathBuf> {
        match self {
            DesiredAction::VerifierRepair { .. } | DesiredAction::MissingVerifierCreate => None,
            DesiredAction::FocusedEdit { target, .. } => Some(target),
            DesiredAction::ArtifactDirected { target, .. } => Some(target),
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
        /// of the existing 5 `StopReason` variants (§4 — no new variants
        /// introduced by this Issue).
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

/// Stable, non-cryptographic correlator for masked workspace-relative
/// paths. **NOT** a secret-hiding hash: the legitimate path-secrecy
/// defence is `mask_secrets` + `mask_payload_inplace` (CLAUDE.md Security
/// Invariants); this helper merely lets dataset consumers correlate
/// observations about the same path across `agent.active_job.*` events
/// without leaking the literal path.
///
/// Algorithm (must match `artifact_ledger.rs::stable_path_hash` —
/// `DefaultHasher` → `{:016x}`). The two SSOTs are duplicated by design
/// (DR2-003 / DR4-004): `artifact_ledger.rs::stable_path_hash` is module-
/// private and intentionally not re-exported, so importing it here would
/// require widening that mod's visibility surface and break the
/// "ledger has no consumers outside turn.rs" rule. Future change must
/// update **both** sites; the doc-comment alignment is the SSOT contract.
#[cfg(test)]
fn stable_path_hash(masked_path: &str) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    masked_path.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::super::turn::EffectiveToolPolicyReason;
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
        let policy = EffectiveToolPolicy::restricted(policy_reason, vec!["Read", "Edit"]);
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
    fn desired_action_labels_are_stable() {
        let a = DesiredAction::VerifierRepair {
            command: "cargo test".to_string(),
            target_hint: None,
        };
        assert_eq!(a.label(), "verifier_repair");
        let b = DesiredAction::MissingVerifierCreate;
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

    // -------- stable_path_hash --------

    #[test]
    fn stable_path_hash_is_deterministic_and_16_hex() {
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
            desired_action: DesiredAction::MissingVerifierCreate,
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
}
