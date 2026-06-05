//! Issue #994 (parent #988, Issue F): `ContractConflictJob` — arbitrate an
//! ambiguous contract among implementation / test / setup / docs / data /
//! research evidence when no-progress recovery is about to give up.
//!
//! When the repair lifecycle exhausts every repairable cluster
//! (`PromotionResult.all_clusters_exhausted`), the failure is frequently NOT a
//! single-artifact bug but a **disagreement between artifacts** (impl returns
//! 200 while the test expects 404; a `Cargo.toml` `[lib] name` that no test
//! imports; a data schema that contradicts an assertion). Before the loop falls
//! back to free LLM regeneration, this module classifies the failure as a
//! contract conflict and emits a **typed [`ContractArbitrationDecision`]**
//! recording which role is authoritative, which is weaker, what change kind is
//! allowed, the (masked) target, a confidence, and a (masked) reason.
//!
//! # Design boundaries
//!
//! - **Authority model integrity (non-functional req).** Arbitration delegates
//!   to the existing [`super::spec_authority`] SSOT
//!   (`resolve` + `detect_consensus_from_contract_conflict`). This module is the
//!   first production consumer of that (previously `#[allow(dead_code)]`)
//!   authority model.
//! - **LLM is selection-only (functional req).** The sidecar adapter
//!   ([`run_contract_arbitration_with_strategy`]) presents a bounded candidate
//!   allowlist and the model may only pick an index. Role / change-kind /
//!   target are taken from the chosen candidate; untrusted LLM free-text never
//!   becomes an instruction (it is masked, length-capped, and log-only).
//! - **No new runtime-specific prompts (non-functional req).** A single generic
//!   prompt covers every runtime; runtime differences live in the
//!   `SemanticFailureReport` shape, not here.
//! - **Legacy terminal projection preserved.** No new `StopReason` variant is
//!   introduced — the existing `repair_exhausted` terminal label is unchanged.
//!   The decomposition is surfaced via the typed report payload only.
//! - **Visibility.** Everything is `pub(super)` and is **not** re-exported from
//!   `src/agent/loop_run.rs` (CLAUDE.md DR3-001). Consumers: this module's
//!   production hook (`maybe_record_contract_arbitration_on_repair_exhausted`),
//!   `job_report.rs` (the typed report), and the in-crate e2e test module.

use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;

use super::repair_brief::AllowedChangeKind;
use super::semantic_failure::SemanticFailureReport;
use super::spec_authority::{ArtifactConsensus, SpecAuthority, SpecAuthorityInput};
use super::task_contract::{ArtifactRole, mask_and_cap_recovery_field};
use crate::session::feedback::mask_secrets;

/// Byte cap for the selection-only arbitration prompt (mirrors the
/// work_mode / task_kind confirm adapters' 4 KiB input bound).
pub(super) const CONTRACT_ARBITRATION_PROMPT_MAX_BYTES: usize = 4 * 1024;

/// Hard cap on how many times the same `ContractConflictJob` may be
/// re-arbitrated. After this the job is no longer actionable, so an ambiguous
/// conflict cannot keep re-directing implementation edits (acceptance: "do not
/// repeat implementation edits without bound").
pub(super) const MAX_CONTRACT_ARBITRATION_ROUNDS: u32 = 2;

/// Role-symmetric allowed change kind for a contract arbitration decision.
///
/// The existing [`AllowedChangeKind`] is repair-centric (impl / test / setup /
/// verifier) and cannot express docs / data deliverable changes. This enum is
/// **one variant per [`ArtifactRole`]** plus an abstain marker so docs / data /
/// research conflicts route through the same lifecycle (acceptance criterion).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ContractChangeKind {
    FixImplementation,
    FixTest,
    FixSetup,
    FixUsageDocs,
    FixDataOutput,
    /// The conflict is genuinely ambiguous: no authoritative source of truth
    /// could be established. The arbiter abstains rather than directing more
    /// (especially implementation) edits.
    InsufficientEvidence,
}

impl ContractChangeKind {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            ContractChangeKind::FixImplementation => "fix_implementation",
            ContractChangeKind::FixTest => "fix_test",
            ContractChangeKind::FixSetup => "fix_setup",
            ContractChangeKind::FixUsageDocs => "fix_usage_docs",
            ContractChangeKind::FixDataOutput => "fix_data_output",
            ContractChangeKind::InsufficientEvidence => "insufficient_evidence",
        }
    }

    /// Canonical change kind for the artifact that should be changed (the
    /// weaker role). Exhaustive over [`ArtifactRole`] — a 6th role would be a
    /// compile error here (CLAUDE.md append-only enum convention).
    pub(super) fn for_role(role: ArtifactRole) -> Self {
        match role {
            ArtifactRole::Implementation => ContractChangeKind::FixImplementation,
            ArtifactRole::Test => ContractChangeKind::FixTest,
            ArtifactRole::Setup => ContractChangeKind::FixSetup,
            ArtifactRole::UsageDocs => ContractChangeKind::FixUsageDocs,
            ArtifactRole::DataOutput => ContractChangeKind::FixDataOutput,
        }
    }

    /// Bridge into the existing repair model. Only the three roles the verifier
    /// repair path can act on map to a concrete [`AllowedChangeKind`]; docs /
    /// data have no repair-brief equivalent (they go to deliverable recovery)
    /// so they return `None`.
    ///
    /// Seam for the controller dispatch (#990): the NoProgressRecoveryPolicy
    /// follow-up routes an actionable decision into the verifier-repair path
    /// via this bridge. Exercised by unit tests today.
    #[allow(dead_code)]
    pub(super) fn to_allowed_change_kind(self) -> Option<AllowedChangeKind> {
        match self {
            ContractChangeKind::FixImplementation => {
                Some(AllowedChangeKind::FixImplementationBehavior)
            }
            ContractChangeKind::FixTest => Some(AllowedChangeKind::FixGeneratedTestExpectation),
            ContractChangeKind::FixSetup => Some(AllowedChangeKind::FixDependencyOrConfig),
            ContractChangeKind::InsufficientEvidence => {
                Some(AllowedChangeKind::InsufficientEvidence)
            }
            ContractChangeKind::FixUsageDocs | ContractChangeKind::FixDataOutput => None,
        }
    }
}

/// Typed arbitration output (Issue #994 functional requirement). All free-text
/// fields are masked at construction; the report envelope additionally passes
/// `mask_payload_inplace` as the final defence line.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct ContractArbitrationDecision {
    /// The artifact treated as the source of truth (must be preserved).
    pub(super) authoritative_role: ArtifactRole,
    /// The artifact the change should target.
    pub(super) weaker_role: ArtifactRole,
    pub(super) allowed_change_kind: ContractChangeKind,
    /// Masked (mask_secrets) path-identity of the target, when a hint exists.
    pub(super) target: Option<String>,
    /// In `[0.0, 1.0]`.
    pub(super) confidence: f32,
    /// Masked + length-capped human-readable rationale (never an instruction
    /// source — purely descriptive / log-only).
    pub(super) reason: String,
}

impl ContractArbitrationDecision {
    /// Additive JSON projection consumed by the
    /// `agent.contract_arbitration.report` payload. Fields are already masked;
    /// `target` is a masked path-identity (not hashed) so the controller can
    /// act on the exact path, mirroring the recovery-prompt masking convention
    /// (Issue #931 Choke C).
    pub(super) fn to_json_value(&self) -> serde_json::Value {
        serde_json::json!({
            "authoritative_role": self.authoritative_role.label(),
            "weaker_role": self.weaker_role.label(),
            "allowed_change_kind": self.allowed_change_kind.as_str(),
            "target": self.target,
            "confidence": self.confidence,
            "reason": self.reason,
        })
    }

    /// True when the decision directs a concrete (non-abstain) repair.
    pub(super) fn is_actionable(&self) -> bool {
        self.allowed_change_kind != ContractChangeKind::InsufficientEvidence
    }
}

/// Classifier output: the set of artifacts in conflict plus the masked views /
/// target hints used to build arbitration candidates.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct ContractConflictAssessment {
    /// Distinct artifact roles implicated by the failure (always `>= 2`).
    pub(super) involved_roles: BTreeSet<ArtifactRole>,
    /// 2-vs-1 agreement among impl / test / usage_docs, when present.
    pub(super) consensus: Option<ArtifactConsensus>,
    /// Masked per-role conflict view (for the report + prompt).
    pub(super) role_views: BTreeMap<ArtifactRole, String>,
    /// Masked per-role target path hint, when the cluster carried one.
    pub(super) role_targets: BTreeMap<ArtifactRole, String>,
}

/// A single typed arbitration candidate (the allowlist the LLM picks from).
#[derive(Debug, Clone, PartialEq)]
pub(super) struct ArbitrationCandidate {
    pub(super) authoritative_role: ArtifactRole,
    pub(super) weaker_role: ArtifactRole,
    pub(super) allowed_change_kind: ContractChangeKind,
    /// Masked target path-identity (mask_secrets), when known.
    pub(super) target: Option<String>,
}

/// Source-of-truth preference (most authoritative first).
const AUTHORITATIVE_PREF: [ArtifactRole; 5] = [
    ArtifactRole::Implementation,
    ArtifactRole::Setup,
    ArtifactRole::DataOutput,
    ArtifactRole::UsageDocs,
    ArtifactRole::Test,
];

/// Weaker-artifact preference (least authoritative first — the role to change).
const WEAKER_PREF: [ArtifactRole; 5] = [
    ArtifactRole::Test,
    ArtifactRole::UsageDocs,
    ArtifactRole::DataOutput,
    ArtifactRole::Setup,
    ArtifactRole::Implementation,
];

fn first_involved(pref: &[ArtifactRole], roles: &BTreeSet<ArtifactRole>) -> Option<ArtifactRole> {
    pref.iter().copied().find(|r| roles.contains(r))
}

fn first_involved_excluding(
    pref: &[ArtifactRole],
    roles: &BTreeSet<ArtifactRole>,
    exclude: ArtifactRole,
) -> Option<ArtifactRole> {
    pref.iter()
        .copied()
        .find(|r| *r != exclude && roles.contains(r))
}

/// Derive a per-role conflict view, preferring the structured
/// `contract_conflict` slot and falling back to a synthesized
/// observed/expected view from the first cluster that mentions the role.
fn role_view(report: &SemanticFailureReport, role: ArtifactRole) -> Option<String> {
    let direct = match role {
        ArtifactRole::Implementation => Some(report.contract_conflict.implementation.as_str()),
        ArtifactRole::Test => Some(report.contract_conflict.test.as_str()),
        ArtifactRole::UsageDocs => Some(report.contract_conflict.usage_docs.as_str()),
        ArtifactRole::Setup | ArtifactRole::DataOutput => None,
    };
    if let Some(s) = direct
        && !s.trim().is_empty()
    {
        return Some(s.trim().to_string());
    }
    for cluster in &report.failure_clusters {
        if cluster.involved_artifacts.contains(&role) {
            let observed = cluster.observed.trim();
            let expected = cluster.expected.trim();
            let view = if !expected.is_empty() {
                format!("{observed} (expected {expected})")
            } else {
                observed.to_string()
            };
            if !view.trim().is_empty() {
                return Some(view);
            }
        }
    }
    None
}

/// Derive a masked target path hint for `role` from the cluster's admitted /
/// proposed targets.
fn role_target(report: &SemanticFailureReport, role: ArtifactRole) -> Option<String> {
    for cluster in &report.failure_clusters {
        if let Some(hint) = cluster
            .admitted_cluster_targets
            .iter()
            .find(|h| h.role == role)
        {
            return Some(mask_secrets(&hint.path));
        }
        if let Some(cand) = cluster
            .proposed_target_candidates
            .iter()
            .find(|c| c.role_hint == Some(role))
        {
            return Some(mask_secrets(&cand.raw_path));
        }
    }
    None
}

/// Classify a `SemanticFailureReport` as a contract conflict.
///
/// A contract conflict exists when **at least two distinct artifact roles** are
/// implicated with their own views — i.e. the failure is an inter-artifact
/// disagreement, not a single-artifact bug. Returns `None` when fewer than two
/// roles are involved.
pub(super) fn classify_contract_conflict(
    report: &SemanticFailureReport,
) -> Option<ContractConflictAssessment> {
    let mut involved_roles: BTreeSet<ArtifactRole> = BTreeSet::new();
    for cluster in &report.failure_clusters {
        for role in &cluster.involved_artifacts {
            involved_roles.insert(*role);
        }
    }
    let cc = &report.contract_conflict;
    if !cc.implementation.trim().is_empty() {
        involved_roles.insert(ArtifactRole::Implementation);
    }
    if !cc.test.trim().is_empty() {
        involved_roles.insert(ArtifactRole::Test);
    }
    if !cc.usage_docs.trim().is_empty() {
        involved_roles.insert(ArtifactRole::UsageDocs);
    }

    if involved_roles.len() < 2 {
        return None;
    }

    let consensus = super::spec_authority::detect_consensus_from_contract_conflict(
        &cc.implementation,
        &cc.test,
        &cc.usage_docs,
    );

    let mut role_views: BTreeMap<ArtifactRole, String> = BTreeMap::new();
    let mut role_targets: BTreeMap<ArtifactRole, String> = BTreeMap::new();
    for role in &involved_roles {
        if let Some(view) = role_view(report, *role) {
            role_views.insert(*role, mask_and_cap_recovery_field(&view));
        }
        if let Some(target) = role_target(report, *role) {
            role_targets.insert(*role, target);
        }
    }

    Some(ContractConflictAssessment {
        involved_roles,
        consensus,
        role_views,
        role_targets,
    })
}

/// Build the bounded, ranked candidate allowlist for arbitration.
///
/// Candidate 0 is the deterministic preferred decision (driven by
/// [`super::spec_authority::resolve`] + consensus). When the conflict is
/// ambiguous (newly-generated task, no consensus, no contract / user signal)
/// the **only** candidate is `InsufficientEvidence` — so neither the
/// deterministic path nor an LLM selection can direct an implementation edit.
pub(super) fn build_arbitration_candidates(
    assessment: &ContractConflictAssessment,
    authority_input: &SpecAuthorityInput,
) -> Vec<ArbitrationCandidate> {
    let roles = &assessment.involved_roles;
    let authority = super::spec_authority::resolve(authority_input);

    // Representative pair used both for the ambiguous record and as the
    // structural basis for actionable candidates.
    let authoritative =
        first_involved(&AUTHORITATIVE_PREF, roles).unwrap_or(ArtifactRole::Implementation);
    let weaker = first_involved_excluding(&WEAKER_PREF, roles, authoritative)
        .or_else(|| first_involved(&WEAKER_PREF, roles))
        .unwrap_or(ArtifactRole::Test);

    // Ambiguity guard: a brand-new task with no agreement and no authoritative
    // contract cannot be arbitrated safely. Abstain.
    let ambiguous =
        matches!(authority, SpecAuthority::LlmGeneratedTest) && assessment.consensus.is_none();
    if ambiguous {
        return vec![abstain_candidate(authoritative, weaker)];
    }

    // Determine the deterministic authoritative/weaker pair.
    let (det_auth, det_weak) = if let Some(consensus) = assessment.consensus.as_ref() {
        match (consensus.agreeing.first(), consensus.dissenting.first()) {
            (Some(&auth), Some(&weak)) => (auth, weak),
            _ => (authoritative, weaker),
        }
    } else {
        (authoritative, weaker)
    };

    let mut candidates: Vec<ArbitrationCandidate> = Vec::new();
    candidates.push(actionable_candidate(assessment, det_auth, det_weak));

    // Inverse candidate: "the other artifact is actually the one to change."
    // Only meaningful when both roles are distinct and the inverse is not a
    // duplicate of candidate 0.
    if det_auth != det_weak {
        let inverse = actionable_candidate(assessment, det_weak, det_auth);
        if inverse != candidates[0] {
            candidates.push(inverse);
        }
    }

    // Always offer an explicit abstain option so a selecting arbiter can
    // decline rather than be forced into a directive.
    candidates.push(abstain_candidate(det_auth, det_weak));
    candidates
}

fn actionable_candidate(
    assessment: &ContractConflictAssessment,
    authoritative: ArtifactRole,
    weaker: ArtifactRole,
) -> ArbitrationCandidate {
    ArbitrationCandidate {
        authoritative_role: authoritative,
        weaker_role: weaker,
        allowed_change_kind: ContractChangeKind::for_role(weaker),
        target: assessment.role_targets.get(&weaker).cloned(),
    }
}

fn abstain_candidate(authoritative: ArtifactRole, weaker: ArtifactRole) -> ArbitrationCandidate {
    ArbitrationCandidate {
        authoritative_role: authoritative,
        weaker_role: weaker,
        allowed_change_kind: ContractChangeKind::InsufficientEvidence,
        target: None,
    }
}

/// Deterministic arbitration: the production default. It routes through the
/// selection-only orchestrator with a "no sidecar" strategy (the closure
/// always errors), so the orchestrator deterministically falls back to the
/// preferred (index 0) candidate. Keeping a single arbitration path means the
/// controller dispatch (#990) only has to swap the closure for a real sidecar
/// call to enable selection-only LLM arbitration — no second code path.
pub(super) fn arbitrate_contract_conflict(
    assessment: &ContractConflictAssessment,
    authority_input: &SpecAuthorityInput,
    report_confidence: f32,
) -> ContractArbitrationDecision {
    let candidates = build_arbitration_candidates(assessment, authority_input);
    let authority = super::spec_authority::resolve(authority_input);
    run_contract_arbitration_with_strategy(&candidates, authority, report_confidence, |_prompt| {
        Err("deterministic_no_sidecar".to_string())
    })
}

fn decision_from_candidate(
    candidate: &ArbitrationCandidate,
    confidence: f32,
    authority: SpecAuthority,
    selected_by_arbiter: bool,
) -> ContractArbitrationDecision {
    let selection = if selected_by_arbiter {
        "selected_by_arbiter"
    } else {
        "deterministic"
    };
    let reason = format!(
        "authority={}; authoritative={}; weaker={}; change={}; {}",
        spec_authority_label(authority),
        candidate.authoritative_role.label(),
        candidate.weaker_role.label(),
        candidate.allowed_change_kind.as_str(),
        selection,
    );
    ContractArbitrationDecision {
        authoritative_role: candidate.authoritative_role,
        weaker_role: candidate.weaker_role,
        allowed_change_kind: candidate.allowed_change_kind,
        target: candidate.target.clone(),
        confidence: confidence.clamp(0.0, 1.0),
        reason: mask_and_cap_recovery_field(&reason),
    }
}

fn spec_authority_label(authority: SpecAuthority) -> &'static str {
    match authority {
        SpecAuthority::UserRequest => "user_request",
        SpecAuthority::BehaviorContract => "behavior_contract",
        SpecAuthority::VerifiedPublicInterface => "verified_public_interface",
        SpecAuthority::ImplementationContract => "implementation_contract",
        SpecAuthority::LlmGeneratedTest => "llm_generated_test",
    }
}

// ---------------------------------------------------------------------------
// Selection-only LLM adapter (closure-DI; mirrors work_mode / task_kind confirm)
// ---------------------------------------------------------------------------

/// Parsed selection response. The model may ONLY choose an index into the
/// candidate allowlist; `reason` is masked, length-capped, and log-only.
#[derive(Debug, Deserialize)]
struct ArbitrationSelectionResponse {
    selected_index: usize,
    #[serde(default)]
    confidence: f64,
}

/// Build the selection-only prompt. The model is constrained to return
/// `{"selected_index": <n>, "confidence": <0..1>}`. No free regeneration is
/// possible: the role / change-kind / target come from the chosen candidate.
pub(super) fn build_contract_arbitration_prompt(candidates: &[ArbitrationCandidate]) -> String {
    let mut body = String::new();
    body.push_str(
        "A repair loop stalled on a contract conflict between artifacts. Choose which \
         arbitration candidate best resolves it. Reply with ONLY a JSON object: \
         {\"selected_index\": <integer>, \"confidence\": <0.0-1.0>}. \
         Do not invent new options; pick exactly one listed index.\n\nCandidates:\n",
    );
    for (idx, cand) in candidates.iter().enumerate() {
        let target = cand.target.as_deref().unwrap_or("(none)");
        // Each candidate field is already masked / enum-derived; mask the
        // assembled line again as defence in depth before it reaches the model.
        let line = format!(
            "[{idx}] authoritative={}, weaker={}, change={}, target={}\n",
            cand.authoritative_role.label(),
            cand.weaker_role.label(),
            cand.allowed_change_kind.as_str(),
            target,
        );
        body.push_str(&mask_secrets(&line));
    }
    truncate_utf8(&body, CONTRACT_ARBITRATION_PROMPT_MAX_BYTES).to_string()
}

/// UTF-8-safe byte truncation (shared shape with the confirm adapters).
fn truncate_utf8(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Resolved selection: which candidate index, and the model's confidence.
struct ArbitrationSelection {
    index: usize,
    confidence: Option<f32>,
    selected_by_arbiter: bool,
}

impl ArbitrationSelection {
    fn fallback() -> Self {
        ArbitrationSelection {
            index: 0,
            confidence: None,
            selected_by_arbiter: false,
        }
    }
}

/// Parse a selection response, enforcing the index allowlist. Returns the
/// chosen index only when it is in range; out-of-range / malformed / empty all
/// map to `Err` so the caller falls back to the deterministic candidate.
fn parse_contract_arbitration_selection(
    raw: &str,
    candidate_len: usize,
) -> Result<ArbitrationSelection, ()> {
    let stripped = crate::ollama::xml_fallback::strip_think_tags(raw);
    let json_slice = super::lifecycle::extract_first_json_object(&stripped).ok_or(())?;
    let parsed: ArbitrationSelectionResponse = serde_json::from_str(json_slice).map_err(|_| ())?;
    if parsed.selected_index >= candidate_len {
        return Err(());
    }
    let confidence = if parsed.confidence.is_finite() {
        Some(parsed.confidence.clamp(0.0, 1.0) as f32)
    } else {
        None
    };
    Ok(ArbitrationSelection {
        index: parsed.selected_index,
        confidence,
        selected_by_arbiter: true,
    })
}

/// Selection-only arbitration via an injected LLM closure (closure-DI). The
/// closure receives the prompt and returns the raw model reply (or an error to
/// signal "no sidecar"). Any failure deterministically falls back to candidate
/// 0. The decision's role / change-kind / target ALWAYS come from a listed
/// candidate — never from LLM free-text.
pub(super) fn run_contract_arbitration_with_strategy<F>(
    candidates: &[ArbitrationCandidate],
    authority: SpecAuthority,
    report_confidence: f32,
    llm_call: F,
) -> ContractArbitrationDecision
where
    F: FnOnce(&str) -> Result<String, String>,
{
    debug_assert!(
        !candidates.is_empty(),
        "candidates must be non-empty (build_arbitration_candidates guarantees >= 1)"
    );
    if candidates.is_empty() {
        return decision_from_candidate(
            &abstain_candidate(ArtifactRole::Implementation, ArtifactRole::Test),
            report_confidence,
            authority,
            false,
        );
    }
    let prompt = build_contract_arbitration_prompt(candidates);
    let selection = match llm_call(&prompt) {
        Ok(raw) => parse_contract_arbitration_selection(&raw, candidates.len())
            .unwrap_or_else(|()| ArbitrationSelection::fallback()),
        Err(_) => ArbitrationSelection::fallback(),
    };
    let confidence = selection.confidence.unwrap_or(report_confidence);
    decision_from_candidate(
        &candidates[selection.index],
        confidence,
        authority,
        selection.selected_by_arbiter,
    )
}

// ---------------------------------------------------------------------------
// ContractConflictJob — bounded lifecycle container
// ---------------------------------------------------------------------------

/// Bounded contract-conflict arbitration job. The round cap is the structural
/// guarantee that an ambiguous conflict cannot keep re-directing edits
/// (acceptance: "do not repeat implementation edits without bound").
#[derive(Debug, Clone, PartialEq)]
pub(super) struct ContractConflictJob {
    pub(super) assessment: ContractConflictAssessment,
    pub(super) decision: ContractArbitrationDecision,
    arbitration_round: u32,
}

impl ContractConflictJob {
    pub(super) fn new(
        assessment: ContractConflictAssessment,
        decision: ContractArbitrationDecision,
    ) -> Self {
        ContractConflictJob {
            assessment,
            decision,
            arbitration_round: 1,
        }
    }

    pub(super) fn arbitration_round(&self) -> u32 {
        self.arbitration_round
    }

    /// Re-arbitrate the same conflict, advancing the bounded round counter.
    ///
    /// Seam for the controller dispatch (#990): the NoProgressRecoveryPolicy
    /// loop re-arbitrates on subsequent no-progress turns until
    /// [`Self::is_exhausted`]. Exercised by unit tests today.
    #[allow(dead_code)]
    pub(super) fn record_rearbitration(&mut self, decision: ContractArbitrationDecision) {
        self.arbitration_round = self.arbitration_round.saturating_add(1);
        self.decision = decision;
    }

    pub(super) fn is_exhausted(&self) -> bool {
        self.arbitration_round >= MAX_CONTRACT_ARBITRATION_ROUNDS
    }

    /// The job may still drive a concrete edit only while it has budget AND the
    /// current decision is not an abstain.
    pub(super) fn is_actionable(&self) -> bool {
        !self.is_exhausted() && self.decision.is_actionable()
    }
}

// ---------------------------------------------------------------------------
// Production hook
// ---------------------------------------------------------------------------

/// Production entry point invoked at the `repair_exhausted` chokepoint
/// (`verifier_orchestration::maybe_emit_repair_exhausted_from_promotion`) BEFORE
/// the safe-stop report is emitted.
///
/// Reads the active `SemanticRepairPlan.semantic_report`, classifies it, and —
/// when it is a contract conflict — records a deterministic
/// [`ContractArbitrationDecision`] on the agent so the
/// `agent.contract_arbitration.report` is emitted (with `repair_exhausted`
/// linkage) by the existing job-report chokepoint. No LLM call is made on this
/// terminal path; the selection-only adapter is reserved for the controller
/// dispatch wired by the NoProgressRecoveryPolicy follow-up (#990).
pub(super) fn maybe_record_contract_arbitration_on_repair_exhausted(agent: &mut super::Agent) {
    if agent.last_contract_conflict_job_this_turn.is_some() {
        return;
    }
    let Some(report) = agent
        .repair_job
        .as_ref()
        .and_then(|job| job.semantic_plan.as_ref())
        .map(|plan| plan.semantic_report.clone())
    else {
        return;
    };
    let Some(assessment) = classify_contract_conflict(&report) else {
        return;
    };
    let authority_input = SpecAuthorityInput {
        has_user_request_match: false,
        has_behavior_contract: false,
        has_verified_public_interface: false,
        // Conservative at the terminal point: artifacts already exist, so this
        // is not a brand-new task. The ambiguity guard (which depends on this
        // flag) is exercised by the controller dispatch (#990) and tests.
        is_newly_generated_task: false,
        consensus: assessment.consensus.clone(),
    };
    let decision = arbitrate_contract_conflict(&assessment, &authority_input, report.confidence);
    agent.last_contract_conflict_job_this_turn =
        Some(ContractConflictJob::new(assessment, decision));
}

#[cfg(test)]
mod tests {
    use super::super::VerifierDiagnosticFailureKind;
    use super::super::semantic_failure::{
        ContractConflict, FailureCluster, SemanticFailureReport, cluster_key_for_test,
    };
    use super::*;

    fn report_with(
        contract: ContractConflict,
        clusters: Vec<FailureCluster>,
        confidence: f32,
    ) -> SemanticFailureReport {
        SemanticFailureReport {
            failure_kind: VerifierDiagnosticFailureKind::AssertionMismatch,
            failure_clusters: clusters,
            contract_conflict: contract,
            preferred_repair_role: ArtifactRole::Implementation,
            repair_hypothesis: String::new(),
            confidence,
        }
    }

    fn cluster(
        label: &str,
        observed: &str,
        expected: &str,
        roles: Vec<ArtifactRole>,
    ) -> FailureCluster {
        FailureCluster {
            cluster_key: cluster_key_for_test(label),
            observed: observed.to_string(),
            expected: expected.to_string(),
            affected_cases: Vec::new(),
            involved_artifacts: roles,
            proposed_target_candidates: Vec::new(),
            admitted_cluster_targets: Vec::new(),
        }
    }

    fn contract(implementation: &str, test: &str, usage_docs: &str) -> ContractConflict {
        ContractConflict {
            implementation: implementation.to_string(),
            test: test.to_string(),
            usage_docs: usage_docs.to_string(),
        }
    }

    fn decisive_input(consensus: Option<ArtifactConsensus>) -> SpecAuthorityInput {
        SpecAuthorityInput {
            has_user_request_match: false,
            has_behavior_contract: true,
            has_verified_public_interface: false,
            is_newly_generated_task: false,
            consensus,
        }
    }

    fn ambiguous_input() -> SpecAuthorityInput {
        SpecAuthorityInput {
            has_user_request_match: false,
            has_behavior_contract: false,
            has_verified_public_interface: false,
            is_newly_generated_task: true,
            consensus: None,
        }
    }

    #[test]
    fn change_kind_is_role_symmetric() {
        assert_eq!(
            ContractChangeKind::for_role(ArtifactRole::Implementation),
            ContractChangeKind::FixImplementation
        );
        assert_eq!(
            ContractChangeKind::for_role(ArtifactRole::Test),
            ContractChangeKind::FixTest
        );
        assert_eq!(
            ContractChangeKind::for_role(ArtifactRole::Setup),
            ContractChangeKind::FixSetup
        );
        assert_eq!(
            ContractChangeKind::for_role(ArtifactRole::UsageDocs),
            ContractChangeKind::FixUsageDocs
        );
        assert_eq!(
            ContractChangeKind::for_role(ArtifactRole::DataOutput),
            ContractChangeKind::FixDataOutput
        );
        // docs / data have no repair-brief bridge.
        assert!(
            ContractChangeKind::FixUsageDocs
                .to_allowed_change_kind()
                .is_none()
        );
        assert!(
            ContractChangeKind::FixDataOutput
                .to_allowed_change_kind()
                .is_none()
        );
        assert_eq!(
            ContractChangeKind::FixImplementation.to_allowed_change_kind(),
            Some(AllowedChangeKind::FixImplementationBehavior)
        );
    }

    #[test]
    fn single_role_is_not_a_contract_conflict() {
        let report = report_with(
            contract("returns 200", "", ""),
            vec![cluster("c", "boom", "", vec![ArtifactRole::Implementation])],
            0.8,
        );
        assert!(classify_contract_conflict(&report).is_none());
    }

    #[test]
    fn fastapi_impl_vs_test_classifies_and_arbitrates() {
        // impl + docs say "returns 200", test says "expects 404" -> consensus:
        // impl & docs agree (identical normalized text), test dissents.
        let report = report_with(
            contract("returns 200", "expects 404", "returns 200"),
            vec![cluster(
                "status",
                "200",
                "404",
                vec![ArtifactRole::Implementation, ArtifactRole::Test],
            )],
            0.7,
        );
        let assessment =
            classify_contract_conflict(&report).expect("impl/test divergence is a conflict");
        assert!(
            assessment
                .involved_roles
                .contains(&ArtifactRole::Implementation)
        );
        assert!(assessment.involved_roles.contains(&ArtifactRole::Test));
        assert!(assessment.consensus.is_some());
        let decision = arbitrate_contract_conflict(
            &assessment,
            &decisive_input(assessment.consensus.clone()),
            report.confidence,
        );
        // Consensus says impl is authoritative; the test is the weaker role.
        assert_eq!(decision.weaker_role, ArtifactRole::Test);
        assert_eq!(decision.allowed_change_kind, ContractChangeKind::FixTest);
        assert!(decision.is_actionable());
    }

    #[test]
    fn toml_setup_vs_test_classifies() {
        // Cargo.toml [lib] name vs test import — Setup vs Test conflict, no
        // impl/test/docs three-way text (so no consensus).
        let report = report_with(
            contract("", "", ""),
            vec![cluster(
                "lib-name",
                "lib name is `mycrate`",
                "test imports `othercrate`",
                vec![ArtifactRole::Setup, ArtifactRole::Test],
            )],
            0.6,
        );
        let assessment = classify_contract_conflict(&report).expect("setup/test is a conflict");
        assert!(assessment.involved_roles.contains(&ArtifactRole::Setup));
        assert!(assessment.involved_roles.contains(&ArtifactRole::Test));
        assert!(assessment.consensus.is_none());
        let decision =
            arbitrate_contract_conflict(&assessment, &decisive_input(None), report.confidence);
        // Setup is more authoritative than Test, so Test is changed.
        assert_eq!(decision.authoritative_role, ArtifactRole::Setup);
        assert_eq!(decision.weaker_role, ArtifactRole::Test);
        assert_eq!(decision.allowed_change_kind, ContractChangeKind::FixTest);
    }

    #[test]
    fn docs_data_research_share_the_lifecycle() {
        // docs conflict: implementation vs usage_docs.
        let docs = report_with(
            contract("computes mean", "", "documents median"),
            vec![cluster(
                "docs",
                "mean",
                "median",
                vec![ArtifactRole::Implementation, ArtifactRole::UsageDocs],
            )],
            0.6,
        );
        let a = classify_contract_conflict(&docs).expect("docs conflict");
        let d = arbitrate_contract_conflict(&a, &decisive_input(a.consensus.clone()), 0.6);
        assert_eq!(d.weaker_role, ArtifactRole::UsageDocs);
        assert_eq!(d.allowed_change_kind, ContractChangeKind::FixUsageDocs);

        // data conflict: implementation vs data_output.
        let data = report_with(
            contract("", "", ""),
            vec![cluster(
                "schema",
                "csv has 3 columns",
                "schema declares 4",
                vec![ArtifactRole::Implementation, ArtifactRole::DataOutput],
            )],
            0.6,
        );
        let a = classify_contract_conflict(&data).expect("data conflict");
        let d = arbitrate_contract_conflict(&a, &decisive_input(None), 0.6);
        assert_eq!(d.weaker_role, ArtifactRole::DataOutput);
        assert_eq!(d.allowed_change_kind, ContractChangeKind::FixDataOutput);

        // research conflict uses the UsageDocs role.
        let research = report_with(
            contract("returns cached value", "", "report claims live fetch"),
            vec![cluster(
                "research",
                "cached",
                "live",
                vec![ArtifactRole::Implementation, ArtifactRole::UsageDocs],
            )],
            0.6,
        );
        let a = classify_contract_conflict(&research).expect("research conflict");
        let d = arbitrate_contract_conflict(&a, &decisive_input(a.consensus.clone()), 0.6);
        assert_eq!(d.allowed_change_kind, ContractChangeKind::FixUsageDocs);
    }

    #[test]
    fn ambiguous_conflict_abstains_and_does_not_direct_impl_edits() {
        let report = report_with(
            contract("returns 200", "expects 404", ""),
            vec![cluster(
                "ambiguous",
                "200",
                "404",
                vec![ArtifactRole::Implementation, ArtifactRole::Test],
            )],
            0.4,
        );
        let assessment = classify_contract_conflict(&report).expect("two roles -> conflict");
        let candidates = build_arbitration_candidates(&assessment, &ambiguous_input());
        // Ambiguous => the ONLY candidate is abstain.
        assert_eq!(candidates.len(), 1);
        assert_eq!(
            candidates[0].allowed_change_kind,
            ContractChangeKind::InsufficientEvidence
        );
        let decision = arbitrate_contract_conflict(&assessment, &ambiguous_input(), 0.4);
        assert_eq!(
            decision.allowed_change_kind,
            ContractChangeKind::InsufficientEvidence
        );
        // Critically: it does NOT direct an implementation edit.
        assert_ne!(
            decision.allowed_change_kind,
            ContractChangeKind::FixImplementation
        );
        assert!(!decision.is_actionable());

        // Bounded job: after the round cap it is no longer actionable even if a
        // later decision were actionable.
        let mut job = ContractConflictJob::new(assessment.clone(), decision);
        assert_eq!(job.arbitration_round(), 1);
        let actionable = ContractArbitrationDecision {
            authoritative_role: ArtifactRole::Implementation,
            weaker_role: ArtifactRole::Test,
            allowed_change_kind: ContractChangeKind::FixTest,
            target: None,
            confidence: 0.9,
            reason: String::new(),
        };
        job.record_rearbitration(actionable);
        assert!(job.is_exhausted());
        assert!(!job.is_actionable());
    }

    #[test]
    fn selection_adapter_honors_index_allowlist() {
        let report = report_with(
            contract("returns 200", "expects 404", "documents 200"),
            vec![cluster(
                "status",
                "200",
                "404",
                vec![ArtifactRole::Implementation, ArtifactRole::Test],
            )],
            0.7,
        );
        let assessment = classify_contract_conflict(&report).unwrap();
        let input = decisive_input(assessment.consensus.clone());
        let candidates = build_arbitration_candidates(&assessment, &input);
        assert!(candidates.len() >= 2, "need alternatives to select among");
        let authority = super::super::spec_authority::resolve(&input);

        // In-range index is honored.
        let last = candidates.len() - 1;
        let picked = run_contract_arbitration_with_strategy(
            &candidates,
            authority,
            report.confidence,
            |_prompt| {
                Ok(format!(
                    "{{\"selected_index\": {last}, \"confidence\": 0.9}}"
                ))
            },
        );
        assert_eq!(picked.weaker_role, candidates[last].weaker_role);
        assert_eq!(
            picked.allowed_change_kind,
            candidates[last].allowed_change_kind
        );

        // Out-of-range index falls back to candidate 0 (deterministic).
        let oob = run_contract_arbitration_with_strategy(
            &candidates,
            authority,
            report.confidence,
            |_prompt| Ok("{\"selected_index\": 99}".to_string()),
        );
        assert_eq!(oob.weaker_role, candidates[0].weaker_role);

        // Malformed reply falls back to candidate 0.
        let malformed = run_contract_arbitration_with_strategy(
            &candidates,
            authority,
            report.confidence,
            |_prompt| Ok("not json".to_string()),
        );
        assert_eq!(malformed.weaker_role, candidates[0].weaker_role);

        // No sidecar (Err) falls back to candidate 0.
        let no_sidecar = run_contract_arbitration_with_strategy(
            &candidates,
            authority,
            report.confidence,
            |_prompt| Err("sidecar_unavailable".to_string()),
        );
        assert_eq!(no_sidecar.weaker_role, candidates[0].weaker_role);
    }

    #[test]
    fn decision_masks_secret_target_and_reason() {
        let report = report_with(
            contract("returns 200", "expects 404", "documents 200"),
            vec![{
                let mut c = cluster(
                    "status",
                    "200",
                    "404",
                    vec![ArtifactRole::Implementation, ArtifactRole::Test],
                );
                c.proposed_target_candidates =
                    vec![super::super::semantic_failure::RawClusterTargetCandidate {
                        raw_path: "tests/api.rs?token=sk-secret-value-123".to_string(),
                        role_hint: Some(ArtifactRole::Test),
                        reason: String::new(),
                    }];
                c
            }],
            0.7,
        );
        let assessment = classify_contract_conflict(&report).unwrap();
        let decision = arbitrate_contract_conflict(
            &assessment,
            &decisive_input(assessment.consensus.clone()),
            report.confidence,
        );
        let json = decision.to_json_value();
        let serialized = serde_json::to_string(&json).unwrap();
        assert!(
            !serialized.contains("sk-secret-value-123"),
            "raw secret leaked into decision payload: {serialized}"
        );
    }

    #[test]
    fn to_json_value_carries_all_six_typed_fields() {
        let decision = ContractArbitrationDecision {
            authoritative_role: ArtifactRole::Implementation,
            weaker_role: ArtifactRole::Test,
            allowed_change_kind: ContractChangeKind::FixTest,
            target: Some("tests/api.rs".to_string()),
            confidence: 0.75,
            reason: "deterministic".to_string(),
        };
        let json = decision.to_json_value();
        assert_eq!(json["authoritative_role"], "implementation");
        assert_eq!(json["weaker_role"], "test");
        assert_eq!(json["allowed_change_kind"], "fix_test");
        assert_eq!(json["target"], "tests/api.rs");
        assert_eq!(json["confidence"], 0.75);
        assert_eq!(json["reason"], "deterministic");
    }
}
