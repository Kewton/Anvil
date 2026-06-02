//! Issue #667: PAM (photon-action-memory) advisory adapter SSOT.
//!
//! Constrains memory context to a **downstream filter** that respects the
//! current task's active job policy (`#660` `ActiveJobSelection`) and the
//! current task's required artifact roles (`#663` `ArtifactCompletionJob::role()`
//! / `#665` `BehaviorContractProjection`). Memory candidates are NEVER
//! promoted into write owners — they cannot override `select_active_job`'s
//! decision (受入 1: type-level non-promotion).
//!
//! ### Layer rules (CLAUDE.md)
//! - **DR3-001**: private mod, `pub(super)` items only. `loop_run.rs` MUST
//!   NOT `pub use pam_advisory::*;`. `turn.rs` is the sole in-crate consumer
//!   via the `record_pam_advisory_decision` thin shell on `impl Agent`.
//! - **DR3-002**: this module does NOT import `crate::session::*`. It reads
//!   `crate::photon::prompt::*` and `crate::photon::schema::ContextPackResponse`
//!   only (photon → session → agent single-direction dependency preserved
//!   because the agent layer already consumes photon types via turn.rs).
//! - **DR3-003**: pure functions only — no log emit / no IO / no `Agent` borrow.
//!   `agent.memory.report` emit is still owned by `record_job_report` SSOT.
//! - **DR1-004 (OCP)**: `PamAdvisoryMode` / `SuppressionReason` carry
//!   `#[non_exhaustive]` so future variants can be added additively.
//!
//! ### Security invariants (CLAUDE.md)
//! - The adapter does NOT call `sanitize_summary_id` directly. It reads
//!   sanitized `provenance.summary_id` from `enumerate_admitted_items_with_provenance`
//!   (DR1-007 SSOT preservation).
//! - Final-defence masking is performed by `mask_payload_inplace` at
//!   `record_job_report` → `log_llm_event`; the adapter returns raw
//!   serde values only.
//!
//! ### Field set / cap (Issue 本文 S3-007 / DR1-010)
//! - `MAX_PAM_DECISION_LIST_LEN = 16` applies to `injected_summary_ids` /
//!   `suppressed_summary_ids` / `shadow_vs_live_diff.would_inject_in_live`.
//!   Each list pairs with a `*_truncated: bool` flag so future readers can
//!   distinguish "no items" from "cap hit".

use std::collections::HashSet;

use super::active_job_arbiter::{ActiveJobKind, ActiveJobSelection};
use super::required_behavior::BehaviorContractProjection;
use super::task_contract::ArtifactRole;
use crate::photon::prompt::{AdmittedItemView, enumerate_admitted_items_with_provenance};
use crate::photon::schema::ContextPackResponse;

// ---------------------------------------------------------------------------
// Constants (DR1-010 SSOT)
// ---------------------------------------------------------------------------

/// Per-list cap (Issue 本文 S3-007 / DR1-010). Reserved for this module —
/// must not be shared with other 16-caps elsewhere in the crate (e.g.
/// `MAX_REPAIR_ATTEMPT_OUTCOMES`, `VERIFIER_INVOKED_BOUND_ARTIFACTS_CAP`),
/// which carry different semantic ranges.
pub(super) const MAX_PAM_DECISION_LIST_LEN: usize = 16;

/// Per-candidate context excerpt cap. This keeps the decision log useful for
/// A/B attribution while leaving final envelope bounds to `job_report`.
const MAX_PAM_CONTEXT_EXCERPT_CHARS: usize = 160;

// ---------------------------------------------------------------------------
// Enum types
// ---------------------------------------------------------------------------

/// PAM advisory pipeline operating mode (Issue 本文 S5-003).
///
/// `#[non_exhaustive]` (DR1-004): future variants are additive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub(super) enum PamAdvisoryMode {
    /// `config.photon_shadow_mode == true`. Observation + report only.
    Shadow,
    /// Non-shadow turn, advisory admitted at least 1 item for live injection.
    /// Mixed cases with simultaneous suppression are included.
    Live,
    /// Non-shadow turn, advisory suppressed ALL items (zero live injection).
    Suppressed,
}

impl PamAdvisoryMode {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            PamAdvisoryMode::Shadow => "shadow",
            PamAdvisoryMode::Live => "live",
            PamAdvisoryMode::Suppressed => "suppressed",
        }
    }
}

/// Reason label attached to a suppressed summary (Issue 本文 機能要件).
///
/// `#[non_exhaustive]` (DR1-004): future variants are additive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub(super) enum SuppressionReason {
    /// Current task's missing role and the memory-suggested role do not match
    /// (判定軸 (1)+(2): path classify + role mismatch).
    RoleMismatch,
    /// `ActiveJobKind::VerifierRepair` / `ArtifactRecovery` was active and the
    /// memory item suggested implementation expansion (判定軸 (3)).
    ImplExpansionBlocked,
    /// `#665` `BehaviorContractProjection.non_goals` contained the role
    /// suggested by the memory item (判定軸 (4)).
    BehaviorNonGoalMismatch,
    /// The memory item did not expose an artifact-role signal strong enough
    /// to become TaskContract / repair candidate input.
    LowRelevance,
}

impl SuppressionReason {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            SuppressionReason::RoleMismatch => "role_mismatch",
            SuppressionReason::ImplExpansionBlocked => "impl_expansion_blocked",
            SuppressionReason::BehaviorNonGoalMismatch => "behavior_non_goal_mismatch",
            SuppressionReason::LowRelevance => "low_relevance",
        }
    }
}

/// Per-memory action recorded in the decision log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub(super) enum PamCandidateAction {
    Inject,
    Suppress,
    WouldInjectInLive,
}

impl PamCandidateAction {
    fn as_str(self) -> &'static str {
        match self {
            PamCandidateAction::Inject => "inject",
            PamCandidateAction::Suppress => "suppress",
            PamCandidateAction::WouldInjectInLive => "would_inject_in_live",
        }
    }
}

impl serde::Serialize for PamCandidateAction {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

/// Issue #878: the only downstream surfaces PAM may advise.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub(super) enum PamAdvisoryTarget {
    TaskContractCandidateGeneration,
    RepairPacketCandidateGeneration,
    PromptContextOnly,
}

impl PamAdvisoryTarget {
    fn as_str(self) -> &'static str {
        match self {
            Self::TaskContractCandidateGeneration => "task_contract_candidate_generation",
            Self::RepairPacketCandidateGeneration => "repair_packet_candidate_generation",
            Self::PromptContextOnly => "prompt_context_only",
        }
    }
}

impl serde::Serialize for PamAdvisoryTarget {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

// ---------------------------------------------------------------------------
// Struct types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) struct SuppressedSummary {
    pub(super) summary_id: String, // sanitize_summary_id-clean (from renderer SSOT)
    pub(super) reason: SuppressionReason,
}

// Manual serde impl: the wire vocabulary is the `as_str()` SSOT (NOT a
// `rename_all` snake_case derivation, which would otherwise drop the
// established `behavior_non_goal_mismatch` wording).
impl serde::Serialize for SuppressionReason {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

/// Shadow-turn-only delta: "what would have been injected if live?"
///
/// DR1-006: Held as `Option<ShadowVsLiveDiff>`. `Some` only on shadow turns;
/// presence of `Some` is synonymous with `decision.mode == Shadow`, so a
/// separate `differs: bool` flag is intentionally omitted (YAGNI).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) struct ShadowVsLiveDiff {
    pub(super) would_inject_in_live: Vec<String>,
    /// DR2-005: `#[serde(default)]` keeps the additive-only invariant so
    /// future readers tolerate missing fields without bumping schema version.
    #[serde(default)]
    pub(super) would_inject_in_live_truncated: bool,
}

/// Per-memory attribution entry. This is intentionally additive to the
/// existing `pam_decision` payload: older consumers can continue reading the
/// summary-id lists, while A/B analysis can explain why each memory was or was
/// not allowed to affect prompt context.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) struct PamCandidateDecision {
    pub(super) summary_id: String,
    pub(super) action: PamCandidateAction,
    pub(super) advisory_target: PamAdvisoryTarget,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) inferred_role: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) suppression_reason: Option<SuppressionReason>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) unused_reason: Option<&'static str>,
    pub(super) decision_impact: &'static str,
    pub(super) context_excerpt: String,
    #[serde(default)]
    pub(super) context_excerpt_truncated: bool,
}

/// Roll-up for turn-level PAM effect attribution. It describes what kind of
/// downstream decision PAM was allowed to influence in this turn, not whether
/// the final task outcome was good or bad.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) struct PamDecisionEffect {
    pub(super) actual_injected_count: u32,
    pub(super) suppressed_count: u32,
    pub(super) would_inject_in_live_count: u32,
    pub(super) influenced_decision: &'static str,
}

/// Turn-local decision value. Adapter writes exactly one of these per turn
/// (DR1-005 single-set contract) into `Agent.last_pam_decision_this_turn`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PamAdvisoryDecision {
    pub(super) mode: PamAdvisoryMode,
    pub(super) injected_summary_ids: Vec<String>, // sanitize_summary_id-clean
    pub(super) injected_summary_ids_truncated: bool,
    pub(super) suppressed_summary_ids: Vec<SuppressedSummary>,
    pub(super) suppressed_summary_ids_truncated: bool,
    /// DR1-006: `Option`. `Some` only on shadow turns.
    pub(super) shadow_vs_live_diff: Option<ShadowVsLiveDiff>,
    /// `"<ActiveJobKind>:<ArtifactRole?>"` string. Built ONCE by
    /// `format_active_job_role` (DR1-003 SSOT).
    pub(super) active_job_role: String,
    pub(super) candidate_decisions: Vec<PamCandidateDecision>,
    pub(super) candidate_decisions_truncated: bool,
    pub(super) decision_effect: PamDecisionEffect,
}

/// Adapter evaluation result. `decision` flows into `MemoryReport.pam_decision`
/// (via `record_job_report`). `live_admitted_views` flows back into prompt
/// injection cache on non-shadow live paths (DR3-001 — report-only is not
/// sufficient).
#[derive(Debug, Clone)]
pub(super) struct PamAdvisoryOutcome {
    pub(super) decision: PamAdvisoryDecision,
    pub(super) live_admitted_views: Vec<AdmittedItemView>,
}

/// Serde projection for `MemoryReport.pam_decision: Option<serde_json::Value>`
/// (DR1-011). All `serde_json::json!{...}` macro usage for advisory payloads
/// is forbidden — this struct is the SSOT for the wire schema, type-checked
/// against the envelope contract by the compiler.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) struct PamAdvisoryDecisionPayload {
    pub(super) mode: &'static str, // PamAdvisoryMode::as_str()
    pub(super) injected_summary_ids: Vec<String>,
    /// DR2-005: same default-flag invariant as `MemoryReport` siblings.
    #[serde(default)]
    pub(super) injected_summary_ids_truncated: bool,
    pub(super) suppressed_summary_ids: Vec<SuppressedSummary>,
    /// DR2-005: same as above.
    #[serde(default)]
    pub(super) suppressed_summary_ids_truncated: bool,
    /// DR1-006: `Option`; key omitted on non-shadow turns via
    /// `skip_serializing_if`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) shadow_vs_live_diff: Option<ShadowVsLiveDiff>,
    pub(super) active_job_role: String,
    #[serde(default)]
    pub(super) candidate_decisions: Vec<PamCandidateDecision>,
    #[serde(default)]
    pub(super) candidate_decisions_truncated: bool,
    pub(super) decision_effect: PamDecisionEffect,
}

impl From<&PamAdvisoryDecision> for PamAdvisoryDecisionPayload {
    fn from(d: &PamAdvisoryDecision) -> Self {
        Self {
            mode: d.mode.as_str(),
            injected_summary_ids: d.injected_summary_ids.clone(),
            injected_summary_ids_truncated: d.injected_summary_ids_truncated,
            suppressed_summary_ids: d.suppressed_summary_ids.clone(),
            suppressed_summary_ids_truncated: d.suppressed_summary_ids_truncated,
            shadow_vs_live_diff: d.shadow_vs_live_diff.clone(),
            active_job_role: d.active_job_role.clone(),
            candidate_decisions: d.candidate_decisions.clone(),
            candidate_decisions_truncated: d.candidate_decisions_truncated,
            decision_effect: d.decision_effect.clone(),
        }
    }
}

impl PamAdvisoryDecision {
    /// Projection used by `job_report.rs` to populate
    /// `MemoryReport.pam_decision`. **Security boundary**: the caller routes
    /// the resulting envelope through `record_job_report` →
    /// `log_llm_event` → `mask_payload_inplace`; this method does NOT
    /// re-sanitize (SSOT preservation).
    pub(super) fn to_json_value(&self) -> serde_json::Value {
        serde_json::to_value(PamAdvisoryDecisionPayload::from(self))
            .unwrap_or(serde_json::Value::Null)
    }

    /// Issue #857: bounded projection for `logs/eval.jsonl`.
    ///
    /// The eval log records only categorical impact and counts. Per-context
    /// adoption/suppression reasons stay in the structured memory report.
    pub(super) fn to_eval_summary(&self) -> crate::session::eval_log::PamEvalSummary {
        let mut affected_targets = Vec::new();
        let mut decision_types = Vec::new();
        push_unique_decision_type(
            &mut decision_types,
            self.decision_effect.influenced_decision,
        );
        for decision in &self.candidate_decisions {
            push_unique_decision_type(&mut decision_types, decision.decision_impact);
            push_eval_target(
                &mut affected_targets,
                decision,
                self.active_job_role.as_str(),
            );
        }
        let unused_reason = self
            .candidate_decisions
            .iter()
            .find_map(|decision| decision.unused_reason.map(str::to_string));
        crate::session::eval_log::PamEvalSummary {
            mode: self.mode.as_str().to_string(),
            decision_type: self.decision_effect.influenced_decision.to_string(),
            decision_types,
            affected_targets,
            actual_injected_count: self.decision_effect.actual_injected_count,
            suppressed_count: self.decision_effect.suppressed_count,
            would_inject_in_live_count: self.decision_effect.would_inject_in_live_count,
            advisory_only: true,
            completion_judgement_override: false,
            unused_reason,
        }
    }
}

/// Adapter input view for shadow / live state (DR1-002).
///
/// `caller` (the Agent shell) reads `config.photon_shadow_mode` and builds
/// this struct by value — adapter never imports `Config` itself
/// (DR3-002 purity).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct PamAdvisoryModeInput {
    pub(super) shadow: bool,
}

/// Bundled inputs returned by `Agent::pam_advisory_inputs` (DR1-008).
/// Resolves `S3-008` ordering priorities (1)(2)(3) and the `#665`
/// behavior projection in one helper call.
pub(super) struct PamAdvisoryInputs {
    pub(super) role_hint: Option<ArtifactRole>,
    pub(super) behavior: Option<BehaviorContractProjection>,
}

// ---------------------------------------------------------------------------
// Pure-fn API
// ---------------------------------------------------------------------------

/// `ActiveJobKind` → `Option<ArtifactRole>` mapping (DR2-002 / DR3-003).
///
/// All variants return `None` because no kind alone determines an artifact
/// role: `ArtifactRecovery` is role-specific (#663) so the authoritative
/// role lives on `ArtifactCompletionJob::role()`; the other kinds operate
/// outside the artifact-role taxonomy. The caller falls back to
/// `S3-008 (2)(3)` priorities when `None` is returned.
pub(super) fn job_kind_to_artifact_role(kind: ActiveJobKind) -> Option<ArtifactRole> {
    // `#[non_exhaustive]` on `ActiveJobKind` means future variants can be
    // added without breaking compilation here. We intentionally enumerate
    // every known variant (rather than using `_ =>`) so a new kind is
    // surfaced as a compile-time warning, prompting the author to decide
    // whether S3-008 (1) should map it to a role or fall through to (2)(3).
    match kind {
        ActiveJobKind::ArtifactRecovery => None,
        ActiveJobKind::VerifierRepair => None,
        ActiveJobKind::ForcedSmallEditRecovery => None,
        ActiveJobKind::FocusedEditRecovery => None,
        ActiveJobKind::LocalLlmSmallEditAfterRead => None,
        ActiveJobKind::SetupBootstrap => None,
    }
}

/// Build the `"<ActiveJobKind>:<ArtifactRole?>"` SSOT string (DR1-003).
///
/// Module-private — callers always observe the value via
/// `PamAdvisoryDecision.active_job_role`, never construct it externally.
fn format_active_job_role(
    active_kind: Option<ActiveJobKind>,
    current_role_hint: Option<ArtifactRole>,
) -> String {
    let kind_part = active_kind.map(|k| k.as_str()).unwrap_or("");
    let role_part = current_role_hint.map(|r| r.label()).unwrap_or("");
    format!("{kind_part}:{role_part}")
}

/// Per-summary classification (純関数, exported for unit test).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum SummaryClassification {
    Inject,
    Suppress(SuppressionReason),
}

/// Determine whether a summary item is `Inject` or `Suppress(reason)`.
///
/// 4-signal decision tree:
///   (1) renderer admitted view path/text → infer suggested role
///   (2) compare against `current_role_hint` (missing role mismatch)
///   (3) active job kind = `VerifierRepair` / `ArtifactRecovery` blocks
///       implementation expansion
///   (4) `behavior.non_goals` contains the suggested role → suppress
pub(super) fn classify_summary(
    view: &AdmittedItemView,
    current_role_hint: Option<ArtifactRole>,
    active_job_kind: Option<ActiveJobKind>,
    behavior: Option<&BehaviorContractProjection>,
) -> SummaryClassification {
    let suggested_role = infer_role_from_view(view);

    // Issue #878: PAM is no longer fail-open into generic prompt context.
    // Without a role signal, the item cannot be bounded to TaskContract or
    // repair-packet candidate generation, so it is logged but not injected.
    let Some(suggested_role) = suggested_role else {
        return SummaryClassification::Suppress(SuppressionReason::LowRelevance);
    };

    // Axis (3): VerifierRepair / ArtifactRecovery suppress implementation
    // expansion. This takes precedence over (1)(2)(4) when active.
    if let Some(kind) = active_job_kind
        && matches!(
            kind,
            ActiveJobKind::VerifierRepair | ActiveJobKind::ArtifactRecovery
        )
        && suggested_role == ArtifactRole::Implementation
    {
        return SummaryClassification::Suppress(SuppressionReason::ImplExpansionBlocked);
    }

    // Axis (1)+(2): role mismatch (only fires when both sides are known
    // and differ). If current_role_hint is None, known-role advice is
    // permitted as candidate-generation context only.
    if let Some(hint) = current_role_hint
        && suggested_role != hint
    {
        return SummaryClassification::Suppress(SuppressionReason::RoleMismatch);
    }

    // Axis (4): behavior.non_goals contains suggested role label.
    if let Some(b) = behavior
        && b.non_goals
            .iter()
            .any(|ng| label_matches_role(&ng.label, suggested_role))
    {
        return SummaryClassification::Suppress(SuppressionReason::BehaviorNonGoalMismatch);
    }

    SummaryClassification::Inject
}

/// Mode precedence (S5-003).
pub(super) fn derive_mode(
    shadow_mode_enabled: bool,
    classifications: &[SummaryClassification],
) -> PamAdvisoryMode {
    if shadow_mode_enabled {
        return PamAdvisoryMode::Shadow;
    }
    let has_inject = classifications
        .iter()
        .any(|c| matches!(c, SummaryClassification::Inject));
    if has_inject {
        PamAdvisoryMode::Live
    } else {
        PamAdvisoryMode::Suppressed
    }
}

/// Adapter SSOT — pure function (DR3-003).
///
/// Inputs:
/// - `context_pack`: photon sidecar response (`crate::photon::schema::ContextPackResponse`).
///   Adapter never parses raw JSON; it calls `enumerate_admitted_items_with_provenance`
///   to consume sanitized `AdmittedItemView` only.
/// - `blocked_ids`: caller-provided same `HashSet` used by the renderer
///   (`extract_blocked_summary_ids`) — DR4-001 parity. Adapter does not
///   recompute. Empty set when `photon_respect_warnings = false`.
/// - `active_job`: `Option<&ActiveJobSelection>`. `None` is a fail-open
///   contract (axis (3) is skipped) — path-a is expected to see `None`
///   because `select_active_job` runs later in the turn.
/// - `current_role_hint`: caller-resolved S3-008 (1)(2)(3) result.
/// - `behavior`: `Option<&BehaviorContractProjection>`. `None` skips axis (4).
/// - `mode_input`: shadow / live flag.
pub(super) fn evaluate_pam_advisory(
    context_pack: &ContextPackResponse,
    blocked_ids: &HashSet<String>,
    active_job: Option<&ActiveJobSelection>,
    current_role_hint: Option<ArtifactRole>,
    behavior: Option<&BehaviorContractProjection>,
    mode_input: PamAdvisoryModeInput,
) -> PamAdvisoryOutcome {
    let (admitted_views, _stats) =
        enumerate_admitted_items_with_provenance(context_pack, blocked_ids);

    let active_kind = active_job.and_then(|s| s.selected.as_ref()).map(|c| c.kind);

    let classifications: Vec<SummaryClassification> = admitted_views
        .iter()
        .map(|v| classify_summary(v, current_role_hint, active_kind, behavior))
        .collect();

    let mode = derive_mode(mode_input.shadow, &classifications);

    // Build live_admitted_views: keep only `Inject` items (shadow turn
    // discards these for prompt injection but they still drive
    // `would_inject_in_live`).
    let mut live_admitted_views: Vec<AdmittedItemView> = Vec::new();
    let mut injected_summary_ids: Vec<String> = Vec::new();
    let mut suppressed_summary_ids: Vec<SuppressedSummary> = Vec::new();
    let mut candidate_decisions: Vec<PamCandidateDecision> = Vec::new();
    let all_admitted_ids: Vec<String> = admitted_views
        .iter()
        .filter_map(|v| v.provenance.summary_id.clone())
        .collect();

    for (view, classification) in admitted_views.iter().zip(classifications.iter()) {
        let suggested_role = infer_role_from_view(view);
        match classification {
            SummaryClassification::Inject => {
                live_admitted_views.push(view.clone());
                if let Some(id) = view.provenance.summary_id.clone() {
                    injected_summary_ids.push(id);
                }
                push_candidate_decision(
                    &mut candidate_decisions,
                    view,
                    suggested_role,
                    None,
                    if mode_input.shadow {
                        PamCandidateAction::WouldInjectInLive
                    } else {
                        PamCandidateAction::Inject
                    },
                    active_kind,
                    if mode_input.shadow {
                        shadow_candidate_impact(suggested_role, active_kind)
                    } else {
                        admitted_candidate_impact(suggested_role, active_kind)
                    },
                );
            }
            SummaryClassification::Suppress(reason) => {
                if let Some(id) = view.provenance.summary_id.clone() {
                    suppressed_summary_ids.push(SuppressedSummary {
                        summary_id: id,
                        reason: *reason,
                    });
                }
                push_candidate_decision(
                    &mut candidate_decisions,
                    view,
                    suggested_role,
                    Some(*reason),
                    PamCandidateAction::Suppress,
                    active_kind,
                    suppression_impact(*reason, active_kind),
                );
            }
        }
    }

    let mut injected_summary_ids_truncated = apply_cap_strings(&mut injected_summary_ids);
    let suppressed_summary_ids_truncated = apply_cap_suppressed(&mut suppressed_summary_ids);
    let candidate_decisions_truncated = apply_cap_candidate_decisions(&mut candidate_decisions);

    // shadow_vs_live_diff: only populated on shadow turns.
    let shadow_vs_live_diff = if mode_input.shadow {
        // "would_inject_in_live" is the set of Inject ids (NOT all admitted),
        // because suppressed items would not have been injected even in live.
        let mut would: Vec<String> = injected_summary_ids.clone();
        let _ = &all_admitted_ids; // retained for future diff modes
        let would_truncated = apply_cap_strings(&mut would);
        Some(ShadowVsLiveDiff {
            would_inject_in_live: would,
            would_inject_in_live_truncated: would_truncated,
        })
    } else {
        None
    };

    // Codex CB-001 fix: on shadow turns, `injected_summary_ids` reports the
    // IDs that were *actually* injected into the prompt. Since shadow turns
    // never inject anything (caller contract DR1-001), this list MUST be
    // empty — the live-candidate IDs belong in
    // `shadow_vs_live_diff.would_inject_in_live` (built above using the
    // pre-clear snapshot). Reset the truncated flag because an empty list
    // can never be truncated.
    //
    // On shadow turns, live_admitted_views must NOT flow into prompt
    // injection cache (caller contract DR1-001) — clear them so a
    // forgetful caller cannot accidentally inject.
    if mode_input.shadow {
        live_admitted_views.clear();
        injected_summary_ids.clear();
        injected_summary_ids_truncated = false;
    }

    let active_job_role = format_active_job_role(active_kind, current_role_hint);
    let decision_effect = build_decision_effect(
        mode,
        injected_summary_ids.len(),
        suppressed_summary_ids.len(),
        shadow_vs_live_diff
            .as_ref()
            .map(|diff| diff.would_inject_in_live.len())
            .unwrap_or(0),
        &candidate_decisions,
    );

    let decision = PamAdvisoryDecision {
        mode,
        injected_summary_ids,
        injected_summary_ids_truncated,
        suppressed_summary_ids,
        suppressed_summary_ids_truncated,
        shadow_vs_live_diff,
        active_job_role,
        candidate_decisions,
        candidate_decisions_truncated,
        decision_effect,
    };

    PamAdvisoryOutcome {
        decision,
        live_admitted_views,
    }
}

// ---------------------------------------------------------------------------
// Internal helpers (module-private)
// ---------------------------------------------------------------------------

/// Apply `MAX_PAM_DECISION_LIST_LEN` cap to a `Vec<String>` in-place,
/// returning `true` when truncation occurred.
fn apply_cap_strings(v: &mut Vec<String>) -> bool {
    if v.len() > MAX_PAM_DECISION_LIST_LEN {
        v.truncate(MAX_PAM_DECISION_LIST_LEN);
        true
    } else {
        false
    }
}

/// Apply `MAX_PAM_DECISION_LIST_LEN` cap to a `Vec<SuppressedSummary>` in-place,
/// returning `true` when truncation occurred.
fn apply_cap_suppressed(v: &mut Vec<SuppressedSummary>) -> bool {
    if v.len() > MAX_PAM_DECISION_LIST_LEN {
        v.truncate(MAX_PAM_DECISION_LIST_LEN);
        true
    } else {
        false
    }
}

/// Apply `MAX_PAM_DECISION_LIST_LEN` cap to candidate attribution entries.
fn apply_cap_candidate_decisions(v: &mut Vec<PamCandidateDecision>) -> bool {
    if v.len() > MAX_PAM_DECISION_LIST_LEN {
        v.truncate(MAX_PAM_DECISION_LIST_LEN);
        true
    } else {
        false
    }
}

fn push_unique_decision_type(out: &mut Vec<String>, value: &'static str) {
    if out.len() >= MAX_PAM_DECISION_LIST_LEN || out.iter().any(|existing| existing == value) {
        return;
    }
    out.push(value.to_string());
}

fn push_eval_target(
    out: &mut Vec<crate::session::eval_log::PamEvalTarget>,
    decision: &PamCandidateDecision,
    active_job_role: &str,
) {
    if out.len() >= crate::session::eval_log::MAX_PAM_EVAL_TARGETS {
        return;
    }
    let (target_type, target) = pam_eval_target(decision, active_job_role);
    out.push(crate::session::eval_log::PamEvalTarget {
        target_type: target_type.to_string(),
        target,
        decision_type: decision.decision_impact.to_string(),
        summary_id: Some(decision.summary_id.clone()),
    });
}

fn pam_eval_target(
    decision: &PamCandidateDecision,
    active_job_role: &str,
) -> (&'static str, String) {
    match decision.advisory_target {
        PamAdvisoryTarget::TaskContractCandidateGeneration => match decision.suppression_reason {
            Some(SuppressionReason::BehaviorNonGoalMismatch) => {
                ("task_contract_candidate", "non_goal".to_string())
            }
            _ => {
                let role = decision.inferred_role.unwrap_or("unknown");
                ("task_contract_candidate", format!("deliverable:{role}"))
            }
        },
        PamAdvisoryTarget::RepairPacketCandidateGeneration => match decision.suppression_reason {
            Some(SuppressionReason::ImplExpansionBlocked) => (
                "repair_packet_candidate",
                "failure_domain:implementation_expansion".to_string(),
            ),
            _ => (
                "repair_packet_candidate",
                format!("repair_target:{active_job_role}"),
            ),
        },
        PamAdvisoryTarget::PromptContextOnly => match decision.suppression_reason {
            Some(SuppressionReason::BehaviorNonGoalMismatch) => {
                ("task_contract", "behavior_contract:non_goals".to_string())
            }
            Some(SuppressionReason::ImplExpansionBlocked) => {
                ("repair_hint", active_job_role.to_string())
            }
            Some(SuppressionReason::RoleMismatch) | None => {
                let role = decision.inferred_role.unwrap_or("unknown");
                ("task_contract", format!("required_artifact_role:{role}"))
            }
            Some(SuppressionReason::LowRelevance) => {
                ("prompt_context_only", "low_relevance".to_string())
            }
        },
    }
}

fn push_candidate_decision(
    out: &mut Vec<PamCandidateDecision>,
    view: &AdmittedItemView,
    inferred_role: Option<ArtifactRole>,
    suppression_reason: Option<SuppressionReason>,
    action: PamCandidateAction,
    active_kind: Option<ActiveJobKind>,
    decision_impact: &'static str,
) {
    let Some(summary_id) = view.provenance.summary_id.clone() else {
        return;
    };
    let (context_excerpt, context_excerpt_truncated) =
        truncate_context_excerpt(&view.render_text, MAX_PAM_CONTEXT_EXCERPT_CHARS);
    let advisory_target =
        advisory_target_for_candidate(action, inferred_role, suppression_reason, active_kind);
    out.push(PamCandidateDecision {
        summary_id,
        action,
        advisory_target,
        inferred_role: inferred_role.map(ArtifactRole::label),
        suppression_reason,
        unused_reason: suppression_reason.map(suppression_unused_reason),
        decision_impact,
        context_excerpt,
        context_excerpt_truncated,
    });
}

fn truncate_context_excerpt(text: &str, max_chars: usize) -> (String, bool) {
    let mut out = String::new();
    for (count, ch) in text.chars().enumerate() {
        if count == max_chars {
            return (out, true);
        }
        out.push(ch);
    }
    (out, false)
}

fn suppression_impact(
    reason: SuppressionReason,
    active_kind: Option<ActiveJobKind>,
) -> &'static str {
    match (reason, active_kind) {
        (SuppressionReason::ImplExpansionBlocked, Some(ActiveJobKind::VerifierRepair)) => {
            "repair_context_suppressed_impl_expansion"
        }
        (SuppressionReason::ImplExpansionBlocked, Some(ActiveJobKind::ArtifactRecovery)) => {
            "artifact_recovery_suppressed_impl_expansion"
        }
        (SuppressionReason::BehaviorNonGoalMismatch, _) => "behavior_contract_suppressed_non_goal",
        (SuppressionReason::RoleMismatch, _) => "artifact_role_mismatch_suppressed",
        (SuppressionReason::ImplExpansionBlocked, _) => "impl_expansion_suppressed",
        (SuppressionReason::LowRelevance, _) => "low_relevance_suppressed",
    }
}

fn suppression_unused_reason(reason: SuppressionReason) -> &'static str {
    match reason {
        SuppressionReason::RoleMismatch => "artifact_role_mismatch",
        SuppressionReason::ImplExpansionBlocked => "impl_expansion_blocked",
        SuppressionReason::BehaviorNonGoalMismatch => "behavior_non_goal",
        SuppressionReason::LowRelevance => "low_relevance",
    }
}

fn advisory_target_for_candidate(
    action: PamCandidateAction,
    inferred_role: Option<ArtifactRole>,
    suppression_reason: Option<SuppressionReason>,
    active_kind: Option<ActiveJobKind>,
) -> PamAdvisoryTarget {
    match suppression_reason {
        Some(SuppressionReason::ImplExpansionBlocked) => {
            PamAdvisoryTarget::RepairPacketCandidateGeneration
        }
        Some(SuppressionReason::RoleMismatch | SuppressionReason::BehaviorNonGoalMismatch) => {
            PamAdvisoryTarget::TaskContractCandidateGeneration
        }
        Some(SuppressionReason::LowRelevance) => PamAdvisoryTarget::PromptContextOnly,
        None if matches!(
            active_kind,
            Some(ActiveJobKind::VerifierRepair | ActiveJobKind::ArtifactRecovery)
        ) =>
        {
            PamAdvisoryTarget::RepairPacketCandidateGeneration
        }
        None => match (action, inferred_role) {
            (
                PamCandidateAction::Inject | PamCandidateAction::WouldInjectInLive,
                Some(ArtifactRole::Implementation | ArtifactRole::Test),
            ) => PamAdvisoryTarget::TaskContractCandidateGeneration,
            (PamCandidateAction::Inject | PamCandidateAction::WouldInjectInLive, Some(_)) => {
                PamAdvisoryTarget::TaskContractCandidateGeneration
            }
            _ => PamAdvisoryTarget::PromptContextOnly,
        },
    }
}

fn admitted_candidate_impact(
    inferred_role: Option<ArtifactRole>,
    active_kind: Option<ActiveJobKind>,
) -> &'static str {
    if matches!(
        active_kind,
        Some(ActiveJobKind::VerifierRepair | ActiveJobKind::ArtifactRecovery)
    ) {
        return "repair_packet_candidate_advised";
    }
    if inferred_role.is_some() {
        "task_contract_candidate_advised"
    } else {
        "prompt_context_only"
    }
}

fn shadow_candidate_impact(
    inferred_role: Option<ArtifactRole>,
    active_kind: Option<ActiveJobKind>,
) -> &'static str {
    if matches!(
        active_kind,
        Some(ActiveJobKind::VerifierRepair | ActiveJobKind::ArtifactRecovery)
    ) {
        return "shadow_repair_packet_candidate_advised";
    }
    if inferred_role.is_some() {
        "shadow_task_contract_candidate_advised"
    } else {
        "shadow_prompt_context_only"
    }
}

fn build_decision_effect(
    mode: PamAdvisoryMode,
    _injected_count: usize,
    suppressed_count: usize,
    would_inject_count: usize,
    candidate_decisions: &[PamCandidateDecision],
) -> PamDecisionEffect {
    let influenced_decision = match mode {
        PamAdvisoryMode::Shadow => "shadow_counterfactual",
        PamAdvisoryMode::Live
            if candidate_decisions.iter().any(|decision| {
                matches!(
                    decision.advisory_target,
                    PamAdvisoryTarget::RepairPacketCandidateGeneration
                ) && matches!(decision.action, PamCandidateAction::Inject)
            }) =>
        {
            "repair_packet_candidate_generation"
        }
        PamAdvisoryMode::Live
            if candidate_decisions.iter().any(|decision| {
                matches!(
                    decision.advisory_target,
                    PamAdvisoryTarget::TaskContractCandidateGeneration
                ) && matches!(decision.action, PamCandidateAction::Inject)
            }) =>
        {
            "task_contract_candidate_generation"
        }
        PamAdvisoryMode::Suppressed | PamAdvisoryMode::Live if suppressed_count > 0 => {
            "advisory_suppression"
        }
        PamAdvisoryMode::Live | PamAdvisoryMode::Suppressed => "none",
    };
    PamDecisionEffect {
        actual_injected_count: _injected_count as u32,
        suppressed_count: suppressed_count as u32,
        would_inject_in_live_count: would_inject_count as u32,
        influenced_decision,
    }
}

/// Heuristic: infer the suggested `ArtifactRole` from an admitted view's
/// render text. Adapter does NOT parse raw paths — it scans the
/// already-normalized renderer output for canonical hints (e.g. file
/// suffixes / directory tokens). Returns `None` when no hint is found
/// (fail-open at axis (1)+(2)).
fn infer_role_from_view(view: &AdmittedItemView) -> Option<ArtifactRole> {
    let text = view.render_text.to_ascii_lowercase();

    // Test role: any `tests/` or `_test` / `test_` token.
    if text.contains("tests/")
        || text.contains("_test.")
        || text.contains("test_")
        || text.contains("/test_")
        || text.contains(".test.")
        || text.contains("conftest.py")
    {
        return Some(ArtifactRole::Test);
    }

    // Setup role: pyproject / requirements / Cargo.toml etc.
    if text.contains("pyproject.toml")
        || text.contains("requirements.txt")
        || text.contains("cargo.toml")
        || text.contains("setup.py")
        || text.contains("package.json")
    {
        return Some(ArtifactRole::Setup);
    }

    // UsageDocs role: readme / docs / .md.
    if text.contains("readme")
        || text.contains("docs/")
        || text.contains(".md")
        || text.contains("documentation")
    {
        return Some(ArtifactRole::UsageDocs);
    }

    // Implementation role: src/ / .rs / .py / .ts / .js suffix-style hints.
    if text.contains("src/")
        || text.contains(".rs")
        || text.contains(".py")
        || text.contains(".ts")
        || text.contains(".js")
        || text.contains(".tsx")
        || text.contains(".jsx")
        || text.contains("database.py")
        || text.contains("models.py")
        || text.contains("crud.py")
    {
        return Some(ArtifactRole::Implementation);
    }

    None
}

/// Case-insensitive substring match: does the given non-goal label refer to
/// the suggested role? Uses the role's canonical `label()` token.
fn label_matches_role(label: &str, role: ArtifactRole) -> bool {
    let lower = label.to_ascii_lowercase();
    let token = role.label(); // already lowercase
    lower.contains(token)
}

// ---------------------------------------------------------------------------
// Test seam (Tier-1, DR2-004 — pure-fn direct drive)
// ---------------------------------------------------------------------------

/// Test seam #1 (CB-001 / DR2-004 Tier-1): drive `evaluate_pam_advisory`
/// without an `Agent`. `#[cfg(test)]` keeps this out of release binaries.
#[cfg(test)]
#[allow(dead_code)] // Phase-1 callers land in extended T1-T6 / T11 / T12 unit tests; retained as the documented direct-drive seam.
pub(super) fn pam_advisory_decide_for_test(
    context_pack: &ContextPackResponse,
    blocked_ids: &HashSet<String>,
    active_job: Option<&ActiveJobSelection>,
    current_role_hint: Option<ArtifactRole>,
    behavior: Option<&BehaviorContractProjection>,
    mode_input: PamAdvisoryModeInput,
) -> PamAdvisoryOutcome {
    evaluate_pam_advisory(
        context_pack,
        blocked_ids,
        active_job,
        current_role_hint,
        behavior,
        mode_input,
    )
}

// Issue #667 iteration-2 (CB2-003 fix): the `apply_cap_strings_for_test`
// crate-visible test seam previously here has been removed. Per-list cap
// boundary coverage (15 / 16 / 17 entries) now lives in
// `unit_tests::apply_cap_strings_boundary_15_16_17` below — no extra
// `#[cfg(test)] pub(crate)` adapter is needed to pin the SSOT.

/// Issue #667 iteration-2 (CB-001 fix corner case): build a synthetic
/// `ActiveJobSelection` whose selected kind is `ArtifactRecovery`. Tests use
/// this to drive `evaluate_pam_advisory` with a realistic active-job state
/// (Implementation expansion must be suppressed via axis (3)). `#[cfg(test)]`
/// keeps this out of release binaries.
///
/// Mirrors the synthetic fixture used by `active_job_arbiter::tests`
/// (private `candidate` helper there is module-local), keeping the
/// `EffectiveToolPolicy` opaque to consumers — `PamAdvisoryDecision` only
/// reads `selected.kind` so a minimal restricted policy suffices.
#[cfg(test)]
pub(super) fn build_active_job_selection_artifact_recovery_for_test() -> ActiveJobSelection {
    use super::active_job_arbiter::{Budget, DesiredAction, JobCandidate};
    use super::artifact_completion_job::{AllowedReadScope, AllowedWriteActions};
    use super::tool_policy::{EffectiveToolPolicy, EffectiveToolPolicyReason};
    let candidate = JobCandidate {
        kind: ActiveJobKind::ArtifactRecovery,
        desired_action: DesiredAction::ArtifactDirected {
            target: std::path::PathBuf::from("tests/foo.rs"),
            already_read: false,
            write_actions: AllowedWriteActions::target_create_only(),
            read_scope: AllowedReadScope::TargetOnly,
        },
        policy: EffectiveToolPolicy::restricted(
            EffectiveToolPolicyReason::ArtifactDirectedRecovery,
            vec!["Read", "Edit"],
        ),
        budget: Budget::Unbounded,
    };
    ActiveJobSelection {
        selected: Some(candidate),
        rejected: Vec::new(),
    }
}

/// Issue #667 iteration-2: build a synthetic `BehaviorContractProjection`
/// whose `non_goals` carry the given labels so axis (4)
/// `BehaviorNonGoalMismatch` can be exercised end-to-end. `confidence` is
/// pinned above `LOW_CONFIDENCE_THRESHOLD` so the projection would not be
/// dropped by `project_behavior_contract`. `#[cfg(test)]` keeps this out of
/// release binaries.
#[cfg(test)]
pub(super) fn build_behavior_projection_with_non_goals_for_test(
    non_goal_labels: &[&str],
) -> BehaviorContractProjection {
    use super::required_behavior::BoundedLabelWithExcerpt;
    let non_goals: Vec<BoundedLabelWithExcerpt> = non_goal_labels
        .iter()
        .map(|label| BoundedLabelWithExcerpt {
            label: (*label).to_string(),
            excerpt: None,
        })
        .collect();
    BehaviorContractProjection {
        confidence: 0.9,
        fields_used: vec!["non_goals"],
        behavior_goal: None,
        required_capabilities: Vec::new(),
        verification_expectations: Vec::new(),
        non_goals,
    }
}

#[cfg(test)]
mod unit_tests {
    use super::super::completion_evidence::{CompletionEvidence, EvidenceSet, RepoEditCategory};
    use super::super::task_contract::{CompletionDecision, TaskContract};
    use super::*;
    use crate::tools::bash::BashCommandClass;

    fn context_pack_with_summary(id: &str, summary: &str) -> ContextPackResponse {
        ContextPackResponse(serde_json::json!({
            "items": [
                {
                    "kind": "summary",
                    "id": id,
                    "summary": summary,
                }
            ]
        }))
    }

    fn repo_edit(category: RepoEditCategory) -> CompletionEvidence {
        CompletionEvidence::RepoEdit {
            category,
            count: 1,
            path: None,
        }
    }

    fn build_test_bound(bound_count: usize) -> CompletionEvidence {
        CompletionEvidence::VerifierExitZero {
            class: BashCommandClass::BuildTest,
            command: "pytest tests/test_feature.py".to_string(),
            bound_test_artifacts_count: Some(bound_count),
        }
    }

    #[test]
    fn mode_as_str_matches_documented_vocabulary() {
        assert_eq!(PamAdvisoryMode::Shadow.as_str(), "shadow");
        assert_eq!(PamAdvisoryMode::Live.as_str(), "live");
        assert_eq!(PamAdvisoryMode::Suppressed.as_str(), "suppressed");
    }

    #[test]
    fn suppression_reason_as_str_matches_documented_vocabulary() {
        assert_eq!(SuppressionReason::RoleMismatch.as_str(), "role_mismatch");
        assert_eq!(
            SuppressionReason::ImplExpansionBlocked.as_str(),
            "impl_expansion_blocked"
        );
        assert_eq!(
            SuppressionReason::BehaviorNonGoalMismatch.as_str(),
            "behavior_non_goal_mismatch"
        );
        assert_eq!(SuppressionReason::LowRelevance.as_str(), "low_relevance");
    }

    #[test]
    fn job_kind_to_artifact_role_returns_none_for_all_kinds() {
        assert_eq!(
            job_kind_to_artifact_role(ActiveJobKind::VerifierRepair),
            None
        );
        assert_eq!(
            job_kind_to_artifact_role(ActiveJobKind::ArtifactRecovery),
            None
        );
        assert_eq!(
            job_kind_to_artifact_role(ActiveJobKind::ForcedSmallEditRecovery),
            None
        );
        assert_eq!(
            job_kind_to_artifact_role(ActiveJobKind::FocusedEditRecovery),
            None
        );
        assert_eq!(
            job_kind_to_artifact_role(ActiveJobKind::LocalLlmSmallEditAfterRead),
            None
        );
        assert_eq!(
            job_kind_to_artifact_role(ActiveJobKind::SetupBootstrap),
            None
        );
    }

    #[test]
    fn format_active_job_role_handles_all_combinations() {
        assert_eq!(format_active_job_role(None, None), ":");
        assert_eq!(
            format_active_job_role(None, Some(ArtifactRole::Test)),
            ":test"
        );
        assert_eq!(
            format_active_job_role(Some(ActiveJobKind::VerifierRepair), None),
            "VerifierRepair:"
        );
        assert_eq!(
            format_active_job_role(
                Some(ActiveJobKind::ArtifactRecovery),
                Some(ArtifactRole::Test)
            ),
            "ArtifactRecovery:test"
        );
    }

    #[test]
    fn derive_mode_shadow_takes_precedence() {
        assert_eq!(derive_mode(true, &[]), PamAdvisoryMode::Shadow);
        assert_eq!(
            derive_mode(true, &[SummaryClassification::Inject]),
            PamAdvisoryMode::Shadow
        );
        assert_eq!(
            derive_mode(
                true,
                &[SummaryClassification::Suppress(
                    SuppressionReason::RoleMismatch
                )]
            ),
            PamAdvisoryMode::Shadow
        );
    }

    #[test]
    fn derive_mode_live_when_any_inject() {
        assert_eq!(
            derive_mode(false, &[SummaryClassification::Inject]),
            PamAdvisoryMode::Live
        );
        assert_eq!(
            derive_mode(
                false,
                &[
                    SummaryClassification::Inject,
                    SummaryClassification::Suppress(SuppressionReason::RoleMismatch)
                ]
            ),
            PamAdvisoryMode::Live
        );
    }

    #[test]
    fn derive_mode_suppressed_when_no_inject() {
        assert_eq!(derive_mode(false, &[]), PamAdvisoryMode::Suppressed);
        assert_eq!(
            derive_mode(
                false,
                &[SummaryClassification::Suppress(
                    SuppressionReason::RoleMismatch
                )]
            ),
            PamAdvisoryMode::Suppressed
        );
    }

    #[test]
    fn apply_cap_strings_truncates_above_limit() {
        let mut v: Vec<String> = (0..20).map(|i| format!("id-{i}")).collect();
        assert!(apply_cap_strings(&mut v));
        assert_eq!(v.len(), MAX_PAM_DECISION_LIST_LEN);
    }

    #[test]
    fn apply_cap_strings_noop_at_or_below_limit() {
        let mut v: Vec<String> = (0..16).map(|i| format!("id-{i}")).collect();
        assert!(!apply_cap_strings(&mut v));
        assert_eq!(v.len(), 16);
        let mut v2: Vec<String> = (0..3).map(|i| format!("id-{i}")).collect();
        assert!(!apply_cap_strings(&mut v2));
        assert_eq!(v2.len(), 3);
    }

    /// Issue #667 iteration-2 (CB2-003 fix): 15 / 16 / 17 boundary
    /// coverage moved from the now-removed `apply_cap_strings_for_test`
    /// crate-visible seam into this in-module unit test. The cap value is
    /// pinned via the module-private `MAX_PAM_DECISION_LIST_LEN` constant.
    /// Keeping this coverage in `unit_tests` removes a crate-visible
    /// `#[cfg(test)] pub(crate)` test seam from the iteration-2 inventory.
    #[test]
    fn apply_cap_strings_boundary_15_16_17() {
        // Documented cap is 16 (S3-007). Pin the value so a silent change
        // to MAX_PAM_DECISION_LIST_LEN reaches this regression test.
        assert_eq!(
            MAX_PAM_DECISION_LIST_LEN, 16,
            "documented per-list cap (S3-007)"
        );
        // 15 — strictly below the cap: no truncation, all entries retained.
        let mut v15: Vec<String> = (0..15).map(|i| format!("s{i:02}")).collect();
        assert!(!apply_cap_strings(&mut v15), "15 entries must not truncate");
        assert_eq!(v15.len(), 15);
        // 16 — exactly at the cap: no truncation, all entries retained.
        let mut v16: Vec<String> = (0..16).map(|i| format!("s{i:02}")).collect();
        assert!(
            !apply_cap_strings(&mut v16),
            "16 entries (== cap) must not truncate"
        );
        assert_eq!(v16.len(), 16);
        // 17 — just above the cap: truncation flag set, list cut to 16.
        let mut v17: Vec<String> = (0..17).map(|i| format!("s{i:02}")).collect();
        assert!(
            apply_cap_strings(&mut v17),
            "17 entries (> cap) must trigger truncation"
        );
        assert_eq!(v17.len(), 16);
    }

    #[test]
    fn pam_eval_summary_records_contract_and_repair_hint_targets() {
        let blocked = HashSet::new();
        let contract_resp = context_pack_with_summary(
            "contract_seed",
            "Prior task edited src/lib.rs implementation details.",
        );
        let contract_outcome = evaluate_pam_advisory(
            &contract_resp,
            &blocked,
            None,
            Some(ArtifactRole::Test),
            None,
            PamAdvisoryModeInput { shadow: false },
        );
        let contract_summary = contract_outcome.decision.to_eval_summary();
        assert!(
            contract_summary.affected_targets.iter().any(|target| {
                target.target_type == "task_contract_candidate"
                    && target.target == "deliverable:implementation"
                    && target.summary_id.as_deref() == Some("contract_seed")
            }),
            "role mismatch must be attributable to the task contract"
        );

        let repair_selection = build_active_job_selection_artifact_recovery_for_test();
        let repair_resp = context_pack_with_summary(
            "repair_seed",
            "Earlier implementation expansion touched src/app.rs.",
        );
        let repair_outcome = evaluate_pam_advisory(
            &repair_resp,
            &blocked,
            Some(&repair_selection),
            Some(ArtifactRole::Test),
            None,
            PamAdvisoryModeInput { shadow: false },
        );
        let repair_summary = repair_outcome.decision.to_eval_summary();
        assert!(
            repair_summary.affected_targets.iter().any(|target| {
                target.target_type == "repair_packet_candidate"
                    && target.target == "failure_domain:implementation_expansion"
                    && target.summary_id.as_deref() == Some("repair_seed")
            }),
            "active artifact-recovery suppression must be attributable to a repair hint"
        );
        assert!(repair_summary.advisory_only);
        assert!(!repair_summary.completion_judgement_override);
    }

    #[test]
    fn low_relevance_advice_does_not_pollute_task_contract_candidates() {
        let blocked = HashSet::new();
        let resp = context_pack_with_summary(
            "generic_seed",
            "A previous session had a similar vague issue without artifact details.",
        );
        let outcome = evaluate_pam_advisory(
            &resp,
            &blocked,
            None,
            Some(ArtifactRole::Implementation),
            None,
            PamAdvisoryModeInput { shadow: false },
        );

        assert!(outcome.live_admitted_views.is_empty());
        assert_eq!(outcome.decision.injected_summary_ids, Vec::<String>::new());
        assert_eq!(
            outcome.decision.suppressed_summary_ids[0].reason,
            SuppressionReason::LowRelevance
        );
        let summary = outcome.decision.to_eval_summary();
        assert_eq!(summary.unused_reason.as_deref(), Some("low_relevance"));
        assert!(summary.affected_targets.iter().any(|target| {
            target.target_type == "prompt_context_only"
                && target.target == "low_relevance"
                && target.summary_id.as_deref() == Some("generic_seed")
        }));
    }

    #[test]
    fn pam_advisory_modes_do_not_change_task_contract_completion_decision() {
        let contract = TaskContract::from_request("Implement feature X and add tests");
        let mut missing_test_evidence = EvidenceSet::new();
        missing_test_evidence.push(repo_edit(RepoEditCategory::Impl));
        let before_missing = contract.evaluate(&missing_test_evidence);

        let blocked = HashSet::new();
        let resp = context_pack_with_summary(
            "test_seed",
            "Prior task added tests/test_feature.py for similar behavior.",
        );
        let outcome = evaluate_pam_advisory(
            &resp,
            &blocked,
            None,
            Some(ArtifactRole::Test),
            None,
            PamAdvisoryModeInput { shadow: false },
        );
        assert_eq!(outcome.decision.decision_effect.actual_injected_count, 1);
        let after_missing = contract.evaluate(&missing_test_evidence);
        assert_eq!(
            before_missing, after_missing,
            "PAM injection must not satisfy missing required deliverables"
        );
        assert!(matches!(
            after_missing,
            CompletionDecision::Continue { missing } if missing.contains(&ArtifactRole::Test)
        ));
        let _shadow_outcome = evaluate_pam_advisory(
            &resp,
            &blocked,
            None,
            Some(ArtifactRole::Test),
            None,
            PamAdvisoryModeInput { shadow: true },
        );
        assert_eq!(
            before_missing,
            contract.evaluate(&missing_test_evidence),
            "shadow PAM must not satisfy missing required deliverables"
        );
        assert_eq!(
            before_missing,
            contract.evaluate(&missing_test_evidence),
            "disabled/skipped PAM path has no completion input"
        );

        let mut unverified_evidence = EvidenceSet::new();
        unverified_evidence.push(repo_edit(RepoEditCategory::Impl));
        unverified_evidence.push(repo_edit(RepoEditCategory::Test));
        let before_verify = contract.evaluate(&unverified_evidence);
        let _ = evaluate_pam_advisory(
            &resp,
            &blocked,
            None,
            Some(ArtifactRole::Test),
            None,
            PamAdvisoryModeInput { shadow: false },
        );
        assert_eq!(
            before_verify,
            contract.evaluate(&unverified_evidence),
            "PAM injection must not bypass verifier-required completion"
        );
        assert_eq!(before_verify, CompletionDecision::Verify);

        unverified_evidence.push(CompletionEvidence::VerifierExitZero {
            class: BashCommandClass::BuildTest,
            command: "pytest".to_string(),
            bound_test_artifacts_count: None,
        });
        assert_eq!(
            contract.evaluate(&unverified_evidence),
            CompletionDecision::Done
        );
    }

    #[test]
    fn pam_advisory_has_no_owned_test_completion_authority() {
        let contract = TaskContract::from_request("Implement feature X and add tests");
        assert!(contract.required_behavior.test_execution_required);

        let mut missing_test_evidence = EvidenceSet::new();
        missing_test_evidence.push(repo_edit(RepoEditCategory::Impl));
        let no_pam_missing =
            contract.evaluate_with_owned_test_artifacts(&missing_test_evidence, &[]);

        let blocked = HashSet::new();
        let resp = context_pack_with_summary(
            "pam_test_seed",
            "Prior task added tests/test_feature.py and the verifier passed.",
        );
        let live_outcome = evaluate_pam_advisory(
            &resp,
            &blocked,
            None,
            Some(ArtifactRole::Test),
            None,
            PamAdvisoryModeInput { shadow: false },
        );
        assert_eq!(
            live_outcome.decision.decision_effect.actual_injected_count,
            1
        );
        assert_eq!(
            live_outcome
                .decision
                .candidate_decisions
                .first()
                .map(|decision| decision.advisory_target),
            Some(PamAdvisoryTarget::TaskContractCandidateGeneration),
            "PAM may advise candidate generation, not completion judgement"
        );
        assert_eq!(
            contract.evaluate_with_owned_test_artifacts(&missing_test_evidence, &[]),
            no_pam_missing,
            "live PAM advice alone must not produce Done"
        );
        assert!(matches!(
            no_pam_missing,
            CompletionDecision::Continue { ref missing } if missing.contains(&ArtifactRole::Test)
        ));

        let shadow_outcome = evaluate_pam_advisory(
            &resp,
            &blocked,
            None,
            Some(ArtifactRole::Test),
            None,
            PamAdvisoryModeInput { shadow: true },
        );
        assert!(shadow_outcome.live_admitted_views.is_empty());
        assert_eq!(
            contract.evaluate_with_owned_test_artifacts(&missing_test_evidence, &[]),
            no_pam_missing,
            "shadow PAM advice must have the same completion authority as no PAM"
        );

        let mut deterministic_evidence = EvidenceSet::new();
        deterministic_evidence.push(repo_edit(RepoEditCategory::Impl));
        deterministic_evidence.push(repo_edit(RepoEditCategory::Test));
        deterministic_evidence.push(build_test_bound(1));
        let owned = vec!["tests/test_feature.py".to_string()];
        let no_pam_done =
            contract.evaluate_with_owned_test_artifacts(&deterministic_evidence, &owned);
        assert_eq!(no_pam_done, CompletionDecision::Done);

        let _repair_advice = evaluate_pam_advisory(
            &resp,
            &blocked,
            Some(&build_active_job_selection_artifact_recovery_for_test()),
            Some(ArtifactRole::Test),
            None,
            PamAdvisoryModeInput { shadow: false },
        );
        assert_eq!(
            contract.evaluate_with_owned_test_artifacts(&deterministic_evidence, &owned),
            no_pam_done,
            "deterministic evidence must keep the same Done authority with or without PAM"
        );
    }
}
