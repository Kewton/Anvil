//! Issue #637: `RepairJob` state machine for verifier repair.
//!
//! This module collects the verifier-repair state and decision pure functions
//! that previously lived in `loop_run.rs` (`VerifierRepairContext`) and
//! `turn.rs` (`VerifierRepairDecision` / `verifier_repair_decision()` /
//! `task_contract_repair_state()`).
//!
//! Visibility: every export here is `pub(super)` and **must not** be
//! re-exported from `src/agent/loop_run/mod.rs` (CLAUDE.md DR3-001).
//!
//! Security: long-lived text fields and the `failure_snapshot()` output go
//! through `sanitize_repair_job_text()` (which composes `mask_secrets` /
//! `mask_header_family` / control-char neutralization / a 4096-byte cap).
//! `command` flows through `redact_verifier_command_for_storage` (SSOT in
//! `src/session/feedback.rs`).

use std::path::{Path, PathBuf};

use super::semantic_failure::{FailureClusterKey, SemanticFailureReport};
use super::spec_authority::{RepairRole, SpecAuthority};
use super::task_contract::RecoveryTargetHint;
use super::{
    VerifierDiagnosticFailureKind, VerifierFailureType, VerifierRepairAssessment,
    VerifierRepairRerunOutcome,
};
use crate::session::store::ConversationMessage;

/// Maximum byte length retained for sanitized snapshot text fields. Consumed
/// by `truncate_for_snapshot` and the `failure_snapshot` production path (Issue #638).
pub(super) const SNAPSHOT_FIELD_BYTE_CAP: usize = 4096;

/// Issue #647 (Phase B / DR1-003): 1 cluster 攻略 plan を束ねる sub-struct。
/// `slot reuse` (設計判断 #5) 時はこの struct 単位で `RepairJob.semantic_plan`
/// に書き戻される — 失敗 cluster は `RepairJob.exhausted_attempts` ledger に
/// 記録され、新しい `SemanticRepairPlan` が semantic_plan slot に書き込まれる。
///
/// `Eq` derive は drop している (S7-001): `SemanticFailureReport` が `f32`
/// confidence を保持するため、`TaskContract` と同方針で `PartialEq` のみ。
#[derive(Debug, Clone, PartialEq)]
pub(super) struct SemanticRepairPlan {
    /// 観測 — diagnostic LLM が出力した sanitized + bounded report
    /// (`parse_semantic_failure_report` を通過済)。
    pub(super) semantic_report: SemanticFailureReport,
    /// 1 plan = 1 cluster。`semantic_report.failure_clusters` のうち
    /// 今 attack 中の cluster の `cluster_key` を保持する。
    pub(super) failure_cluster_id: FailureClusterKey,
    /// `semantic_report.failure_kind` のコピー (cheap-access)。
    pub(super) semantic_cause: VerifierDiagnosticFailureKind,
    /// この cluster に対して採用された spec authority (Phase D で
    /// `select_authority` の結果がここに書かれる)。
    pub(super) spec_authority: SpecAuthority,
    /// この cluster の修復で attack するアーティファクト役割。
    pub(super) preferred_repair_role: RepairRole,
    /// 修復仮説。`MAX_REPAIR_HYPOTHESIS_CHARS` (240 chars) に
    /// sanitized + truncated 済 (`semantic_report.repair_hypothesis` 由来)。
    pub(super) repair_hypothesis: String,
    /// rerun 後に書き込まれる予測対比の結果。未 rerun 時は `None`。
    pub(super) expected_improvement: Option<VerifierRepairRerunOutcome>,
}

/// Issue #625 / #627 / #637: turn-local diagnostic context for a failed
/// task-contract verifier. Rename of the previous `VerifierRepairContext`
/// type. Fields are 1:1 with the legacy definition (see design policy §4-1).
///
/// Issue #647 (Phase B): `semantic_plan` / `exhausted_attempts` の 2 field を
/// 追加し、`Eq` derive を drop (S7-001) — `SemanticRepairPlan` 経由で
/// `f32` confidence を含むため `PartialEq` のみ。
#[derive(Debug, Clone, PartialEq)]
pub(super) struct RepairJob {
    pub(super) command: String,
    pub(super) output_excerpt: String,
    pub(super) failure_type: VerifierFailureType,
    pub(super) target_hint: Option<RecoveryTargetHint>,
    pub(super) repair_target_hint: Option<RecoveryTargetHint>,
    pub(super) changed_file_hints: Vec<RecoveryTargetHint>,
    pub(super) assessment: Option<VerifierRepairAssessment>,
    pub(super) assessment_attempts: usize,
    pub(super) diagnostic_attempted: bool,
    pub(super) diagnostic_unavailable: bool,
    pub(super) diagnostic_error: Option<String>,
    pub(super) repair_error: Option<String>,
    pub(super) applied_repair_intents: Vec<String>,
    pub(super) target_line: Option<usize>,
    pub(super) error_kind: Option<String>,
    pub(super) failure_signature: String,
    pub(super) failure_count: Option<usize>,
    pub(super) previous_failure_signature: Option<String>,
    pub(super) previous_failure_count: Option<usize>,
    pub(super) rerun_outcome: Option<VerifierRepairRerunOutcome>,
    pub(super) repair_attempt: usize,
    /// Issue #647 (Phase B / DR1-003): 現在 attack 中の cluster と plan。
    /// `slot reuse` 時はこの Option を書き換える (前 cluster の attempt は
    /// `exhausted_attempts` に push されてから replace される)。
    pub(super) semantic_plan: Option<SemanticRepairPlan>,
    /// Issue #647 (Phase B / DR2-007): per-job 累積の重複検出 ledger。
    /// `slot reuse` でも保持される (per-cluster ではない) — 同じ
    /// `(FailureClusterKey, RepairRole)` 組み合わせを 2 度 attack しないため。
    pub(super) exhausted_attempts: Vec<(FailureClusterKey, RepairRole)>,
}

/// Controller-internal decision used by `run_turn` to pick the next action
/// for a verifier-repair cycle. Variants and order match the legacy
/// `VerifierRepairDecision` enum in `turn.rs`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum VerifierRepairDecision {
    NoRepair,
    NeedDiagnostic,
    DiagnosticUnavailable,
    NeedTargetDiscovery,
    NeedFreshRead(PathBuf),
    NeedWrite(PathBuf),
    NeedEdit(PathBuf),
    ReadyToVerify,
}

/// Projection of the repair state surfaced to `task_contract::plan_artifact_recovery()`.
/// Two variants by design (KISS): we only tell `task_contract` whether a
/// verifier-repair edit is pending and on which hint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum VerifierRepairState {
    None,
    WaitingForEdit {
        target_hint: Option<RecoveryTargetHint>,
    },
}

/// Issue #646 (A1): first-class state for "verifier is missing from the
/// repository". Distinct from the failure-driven `RepairJob` so the planner
/// can:
///   1. Suppress verifier retry until an in-scope edit lands.
///   2. Hold the active scope's allowed-tool whitelist (Write / Edit / Bash)
///      independently of the failure-diagnostic state machine.
///   3. Enforce its own retry budget separate from
///      `TASK_CONTRACT_VERIFIER_ATTEMPT_LIMIT`.
///
/// Lives on `Agent` next to `repair_job` and is cleared at the
/// `handle_user_message` head (per-turn cap pattern).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct MissingVerifierJob {
    /// Inclusive ceiling on how many times we'll re-emit the "create a
    /// verifier" recovery loop in a single task.
    pub(super) retry_budget: u8,
    /// How many retries have already been consumed.
    pub(super) retries_used: u8,
    /// Becomes `true` once a successful in-scope Write/Edit lands after
    /// the job entered the `pending` state. Until then, the planner
    /// MUST suppress `RunVerifier` to avoid an infinite retry loop.
    pub(super) in_scope_edit_observed: bool,
    /// Snapshot of `repo_edit_calls_made_this_turn` when the job entered
    /// `pending`. Retained for diagnostics / future log-event emission.
    pub(super) repo_edit_count_at_pending: usize,
}

impl MissingVerifierJob {
    pub(super) fn new(retry_budget: u8, repo_edit_count_at_pending: usize) -> Self {
        Self {
            retry_budget,
            retries_used: 0,
            in_scope_edit_observed: false,
            repo_edit_count_at_pending,
        }
    }

    /// Whether the planner should suppress `RunVerifier` for this turn.
    /// True when the model has not yet produced an in-scope edit since the
    /// `MissingVerifierJob` was raised.
    pub(super) fn should_suppress_verifier_retry(&self) -> bool {
        !self.in_scope_edit_observed
    }

    /// Mark that an in-scope edit has been observed. After this fires,
    /// verifier retries become allowed again.
    pub(super) fn record_in_scope_edit(&mut self) {
        self.in_scope_edit_observed = true;
    }

    /// Consume one retry slot. Returns `true` when the call falls inside
    /// the configured budget.
    pub(super) fn record_retry(&mut self) -> bool {
        if self.retries_used >= self.retry_budget {
            return false;
        }
        self.retries_used = self.retries_used.saturating_add(1);
        true
    }

    /// Allowed-tool whitelist surfaced to the effective tool policy when
    /// this state is active. Intentionally narrow: the model is expected
    /// to either create a verifier file or run one (Bash) — Read alone
    /// cannot make progress out of this state.
    pub(super) fn allowed_tool_names(&self) -> &'static [&'static str] {
        &["Write", "Edit", "Bash"]
    }
}

/// Read-only snapshot consumed by #638 and by event-log persistence. Each
/// text field is re-sanitized at snapshot time so the SSOT for redaction is
/// preserved at every transfer boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct VerifierFailureSnapshot {
    pub(super) failure_signature: String,
    pub(super) command: String,
    pub(super) output_excerpt: String,
    pub(super) failure_type: VerifierFailureType,
    pub(super) target_path: Option<PathBuf>,
    pub(super) diagnostic_error: Option<String>,
    pub(super) repair_error: Option<String>,
    pub(super) rerun_outcome: Option<VerifierRepairRerunOutcome>,
    pub(super) applied_repair_intent_count: u32,
}

impl RepairJob {
    /// Build a `VerifierFailureSnapshot` for #638 / event-log transfer.
    /// Re-runs `sanitize_repair_job_text` / `redact_verifier_command_for_storage`
    /// so the SSOT defence-in-depth posture holds even if upstream code
    /// stored slightly stale text.
    ///
    /// `target_path` is projected from `target_hint` only when the raw path
    /// passes syntactic safety (no absolute path, no `..` traversal, no NUL /
    /// control chars). Symlink-escape validation is the responsibility of
    /// the upstream admission functions (`recovery_target_hint_for_existing_path`
    /// / `recovery_target_hint_for_diagnostic_path`).
    pub(super) fn failure_snapshot(&self) -> VerifierFailureSnapshot {
        VerifierFailureSnapshot {
            failure_signature: sanitize_repair_job_text(&self.failure_signature),
            command: crate::session::feedback::redact_verifier_command_for_storage(&self.command),
            output_excerpt: sanitize_repair_job_text(&self.output_excerpt),
            failure_type: self.failure_type,
            // Issue #638 (Task 1.6 / §5 Security boundary): only project path
            // when it passes syntactic safety. Symlink-escape is upstream's
            // responsibility (recovery_target_hint_for_existing_path).
            target_path: self.target_hint.as_ref().and_then(|hint| {
                let raw = hint.path.as_str();
                // Reject absolute paths, `..` components, and ANY control char
                // (C0 < 0x20 + DEL 0x7f) for parity with sanitize_repair_job_text
                // (Codex CB-003 reflected).
                if raw.is_empty()
                    || raw.chars().any(|c| c.is_control())
                    || Path::new(raw).is_absolute()
                    || Path::new(raw)
                        .components()
                        .any(|c| matches!(c, std::path::Component::ParentDir))
                {
                    None
                } else {
                    Some(PathBuf::from(raw))
                }
            }),
            diagnostic_error: self
                .diagnostic_error
                .as_deref()
                .map(sanitize_repair_job_text),
            repair_error: self.repair_error.as_deref().map(sanitize_repair_job_text),
            rerun_outcome: self.rerun_outcome,
            applied_repair_intent_count: self.applied_repair_intents.len() as u32,
        }
    }

    /// Issue #647 (Phase B / S3-001): test fixture helper.
    ///
    /// Build a `RepairJob` with all fields at sensible defaults so test sites
    /// only need to override the fields that matter to their scenario. Combine
    /// with struct update syntax for partial overrides:
    ///
    /// ```ignore
    /// let job = RepairJob {
    ///     target_hint: Some(my_hint),
    ///     ..RepairJob::new_for_test()
    /// };
    /// ```
    ///
    /// All Issue #647 semantic-repair fields (`semantic_plan` /
    /// `exhausted_attempts`) are initialized to their empty defaults (`None` /
    /// empty `Vec`), so existing fixtures that pre-date Phase B continue to
    /// describe a pre-semantic-planning state.
    #[cfg(test)]
    pub(super) fn new_for_test() -> Self {
        Self {
            command: String::new(),
            output_excerpt: String::new(),
            failure_type: VerifierFailureType::Unknown,
            target_hint: None,
            repair_target_hint: None,
            changed_file_hints: Vec::new(),
            assessment: None,
            assessment_attempts: 0,
            diagnostic_attempted: false,
            diagnostic_unavailable: false,
            diagnostic_error: None,
            repair_error: None,
            applied_repair_intents: Vec::new(),
            target_line: None,
            error_kind: None,
            failure_signature: String::new(),
            failure_count: None,
            previous_failure_signature: None,
            previous_failure_count: None,
            rerun_outcome: None,
            repair_attempt: 0,
            semantic_plan: None,
            exhausted_attempts: Vec::new(),
        }
    }
}

/// SSOT text sanitizer for `RepairJob` long-lived fields and snapshot output.
///
/// - token / kv / URL-userinfo: `session::feedback::mask_secrets`
/// - Authorization / Cookie / X-API-Key / X-Auth-Token: `session::feedback::mask_header_family`
/// - log/report injection: ASCII C0 + DEL → space
/// - bounded retention: UTF-8 safe `SNAPSHOT_FIELD_BYTE_CAP`-byte cap
pub(super) fn sanitize_repair_job_text(input: &str) -> String {
    truncate_for_snapshot(&mask_and_neutralize(input))
}

/// Sanitize a `RepairJob` text field for a caller-supplied char bound. Used
/// at the `RepairJob` store boundary by callers (e.g.
/// `verifier_repair_context_from_failure` / diagnostic_error / repair_error
/// setters) that historically truncated to a smaller bound than the
/// snapshot cap. Composes the same SSOT prefix as
/// [`sanitize_repair_job_text`] (mask_secrets → mask_header_family →
/// control-char neutralization) and then truncates to `max_chars` Unicode
/// chars with a `"..."` suffix when the input exceeds the bound.
pub(super) fn sanitize_repair_job_text_with_char_cap(input: &str, max_chars: usize) -> String {
    truncate_chars_with_ellipsis(&mask_and_neutralize(input), max_chars)
}

/// Shared SSOT prefix: mask_secrets → mask_header_family → control-char neutralize.
///
/// Exposed to `super::turn` so prompt file excerpts (`safe_verifier_*_file_excerpt`)
/// can apply the same defence layer the snapshot pipeline uses (Issue #638
/// design judgment #4 + Codex CB-002 reflected).
pub(super) fn mask_secrets_headers_and_neutralize(input: &str) -> String {
    mask_and_neutralize(input)
}

fn mask_and_neutralize(input: &str) -> String {
    let masked = crate::session::feedback::mask_header_family(
        &crate::session::feedback::mask_secrets(input),
    );
    masked
        .chars()
        .map(|c| {
            if (c as u32) < 0x20 || c == '\x7f' {
                ' '
            } else {
                c
            }
        })
        .collect()
}

/// UTF-8 safe truncate to `SNAPSHOT_FIELD_BYTE_CAP` bytes.
pub(super) fn truncate_for_snapshot(s: &str) -> String {
    if s.len() <= SNAPSHOT_FIELD_BYTE_CAP {
        return s.to_string();
    }
    let mut end = SNAPSHOT_FIELD_BYTE_CAP;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_string()
}

fn truncate_chars_with_ellipsis(s: &str, max_chars: usize) -> String {
    match s.char_indices().nth(max_chars) {
        Some((byte_idx, _)) => {
            let mut out = String::with_capacity(byte_idx + 3);
            out.push_str(&s[..byte_idx]);
            out.push_str("...");
            out
        }
        None => s.to_string(),
    }
}

/// Pure function moved from `turn.rs`. Drives the verifier-repair state
/// machine using `messages` (for fresh-read detection) and `work_root`
/// (for target-path resolution). Behaviour and ordering are identical to
/// the legacy implementation; we keep the two-argument shape `(pending,
/// job)` because the controller can sit in a transitional state where a
/// repair is pending but no `RepairJob` has been built yet (e.g. before
/// `verifier_repair_context_from_failure`). Treating `job=None` as
/// `NoRepair` would break the existing
/// `verifier_repair_unknown_target_uses_discovery_then_latest_read_target`
/// regression test.
pub(super) fn verifier_repair_decision(
    pending: bool,
    job: Option<&RepairJob>,
    messages: &[ConversationMessage],
    work_root: &Path,
    repair_edit_count: Option<usize>,
    repo_edit_calls_made_this_turn: usize,
) -> VerifierRepairDecision {
    if !pending {
        return VerifierRepairDecision::NoRepair;
    }
    if job.is_some_and(|job| job.diagnostic_unavailable) {
        return VerifierRepairDecision::DiagnosticUnavailable;
    }
    if repair_edit_count.is_some_and(|edit_count| repo_edit_calls_made_this_turn > edit_count) {
        return VerifierRepairDecision::ReadyToVerify;
    }
    if job.is_some_and(|job| {
        job.assessment.is_none()
            && job.assessment_attempts
                < crate::agent::loop_run::turn::VERIFIER_DIAGNOSTIC_ATTEMPT_LIMIT
    }) {
        return VerifierRepairDecision::NeedDiagnostic;
    }
    if job.is_some_and(|job| job.assessment.is_none()) {
        return VerifierRepairDecision::DiagnosticUnavailable;
    }
    // Issue #647 (CB-012): When semantic_plan is active but the
    // assessment was constructed before exhausted_attempts grew (= the
    // hint guard in `verifier_repair_context_target_path` returns
    // `None` because exhausted_attempts is non-empty), do NOT fall
    // through to `latest_successful_read_existing_path`. That fallback
    // would route the repair pass to an unrelated turn-local read
    // target. Force a fresh diagnostic instead so the next assessment
    // reflects the advanced cluster.
    if job.is_some_and(|job| {
        job.semantic_plan.is_some()
            && !job.exhausted_attempts.is_empty()
            && super::turn::verifier_repair_context_target_path(work_root, job).is_none()
            && job.assessment_attempts
                < crate::agent::loop_run::turn::VERIFIER_DIAGNOSTIC_ATTEMPT_LIMIT
    }) {
        return VerifierRepairDecision::NeedDiagnostic;
    }
    let target = job
        .and_then(|job| super::turn::verifier_repair_context_target_path(work_root, job))
        .or_else(|| {
            super::turn::latest_successful_read_existing_path(
                messages,
                work_root,
                super::turn::latest_verifier_repair_note_index(messages),
            )
        });
    let Some(target) = target else {
        return VerifierRepairDecision::NeedTargetDiscovery;
    };
    if !target.is_file() {
        return VerifierRepairDecision::NeedWrite(target);
    }
    if super::turn::focused_edit_target_already_read(messages, &target, work_root) {
        VerifierRepairDecision::NeedEdit(target)
    } else {
        VerifierRepairDecision::NeedFreshRead(target)
    }
}

/// Issue #647 (CB-013): detect the "advanced semantic_plan + stale
/// assessment" state that CB-012 catches in the decision layer.
///
/// The diagnostic runner (`run_verifier_diagnostic_pass` in `turn.rs`)
/// consults this helper to know whether to clear the stale assessment
/// before running a fresh diagnostic. Without this clear, the runner's
/// own `assessment.is_some()` Skipped short-circuit would fire and
/// neutralize the CB-012 fix at the production layer — the decision
/// layer returns `NeedDiagnostic`, but the diagnostic runner refuses
/// to actually run because the stale assessment from the previous
/// cluster is still in the slot.
///
/// The predicate mirrors the CB-007 / CB-012 invariant:
/// * `assessment.is_some()` — a stale assessment exists
/// * `semantic_plan.is_some()` — we are inside a semantic plan
/// * `!exhausted_attempts.is_empty()` — at least one cluster has been
///   exhausted, i.e. the plan has advanced
/// * `verifier_repair_context_target_path(...) == None` — the CB-007 hint
///   guard is already returning None for this state, confirming the
///   assessment is stale
pub(super) fn has_stale_assessment_after_cluster_advance(
    job: &RepairJob,
    work_root: &Path,
) -> bool {
    job.assessment.is_some()
        && job.semantic_plan.is_some()
        && !job.exhausted_attempts.is_empty()
        && super::turn::verifier_repair_context_target_path(work_root, job).is_none()
}

/// Adapter moved from `turn.rs::Agent::task_contract_repair_state()`. Pure
/// projection from `(Option<&RepairJob>, &VerifierRepairDecision)` to the
/// two-variant projection consumed by `task_contract::plan_artifact_recovery`.
/// The active hint preference order (assessment plan slot →
/// repair_target_hint → target_hint) mirrors the legacy behaviour.
#[allow(dead_code)] // forward-facing pure adapter; turn.rs still holds the live method during the migration.
pub(super) fn task_contract_repair_state(
    job: Option<&RepairJob>,
    decision: &VerifierRepairDecision,
) -> VerifierRepairState {
    let Some(job) = job else {
        return VerifierRepairState::None;
    };
    match decision {
        VerifierRepairDecision::NeedDiagnostic
        | VerifierRepairDecision::NeedTargetDiscovery
        | VerifierRepairDecision::NeedFreshRead(_)
        | VerifierRepairDecision::NeedWrite(_)
        | VerifierRepairDecision::NeedEdit(_) => VerifierRepairState::WaitingForEdit {
            target_hint: super::turn::verifier_repair_effective_target_hint(job)
                .cloned()
                .or_else(|| job.repair_target_hint.clone())
                .or_else(|| job.target_hint.clone()),
        },
        VerifierRepairDecision::NoRepair
        | VerifierRepairDecision::DiagnosticUnavailable
        | VerifierRepairDecision::ReadyToVerify => VerifierRepairState::None,
    }
}

// ---- Issue #647 (Phase E): cluster-based sequential repair helpers ---- //
//
// Phase E keeps the existing `repair_job: Option<RepairJob>` slot shape
// (設計判断 #5, S3-010): instead of growing a `Vec<RepairJob>` queue, the
// slot is reused for each cluster. Failed `(cluster_id, role)` pairs are
// recorded on `RepairJob.exhausted_attempts` so the same cluster is never
// re-attacked under the same role and so the bookkeeping survives the
// slot reuse.
//
// The three helpers below are intentionally pure functions over
// `&mut RepairJob` / `&RepairJob` / immutable cluster ids:
//
//   - `advance_to_next_cluster` mutates the slot.
//   - `should_re_diagnostic` inspects the slot.
//   - `rerun_outcome_with_cluster` wraps the legacy
//     `verifier_repair_rerun_outcome` result without modifying it
//     (S3-014: failure_count is still the SSOT for outcome).
//
// `Vec<(FailureClusterKey, RepairRole)>` is a tiny ledger (~64 bytes per
// entry) so linear `contains` is fine — total entries are bounded by
// `failure_clusters.len() * artifact roles in scope`.

/// Issue #647 (Phase E.1): advance the `repair_job.semantic_plan` slot to
/// the next unattempted cluster from `report.failure_clusters`.
///
/// Phase E behaviour (設計判断 #5 / S3-010):
///
/// 1. Push the current plan's `(failure_cluster_id, preferred_repair_role)`
///    onto `exhausted_attempts` (idempotent — duplicates are skipped so
///    repeated calls from `turn.rs` cannot grow the ledger unboundedly).
/// 2. Walk `report.failure_clusters` in document order and pick the first
///    cluster whose `(cluster_id, preferred_repair_role)` is **not** in
///    `exhausted_attempts`.
/// 3. If found, replace `semantic_plan` with a new `SemanticRepairPlan`
///    targeting that cluster and return `true`.
/// 4. If no cluster is available, leave `semantic_plan = None` and return
///    `false` (caller falls back to re-diagnostic or task failure).
///
/// `preferred_repair_role` and `repair_hypothesis` are inherited from the
/// report — Phase E does not re-pick a per-cluster role (the diagnostic
/// LLM emits one role per report).
///
/// `spec_authority` is **carried forward** verbatim from the current plan
/// (Codex CB-009): the authority was already elected for the active report
/// — typically by `resolve()` at the diagnostic boundary — and slot reuse
/// only changes *which cluster* we are attacking, not *which spec source*
/// governs the repair. The previous implementation re-ran
/// `select_authority(&[ImplementationContract, LlmGeneratedTest], None)`
/// inside the helper, which silently demoted higher-authority elections
/// (`UserRequest` / `BehaviorContract`) to `ImplementationContract` every
/// time we walked to a new cluster. When no current plan is present
/// (initial fixture / defensive path), we still fall back to the
/// `ImplementationContract` default so the helper remains total.
///
/// Returns `false` (and clears `semantic_plan`) when the report has no
/// remaining clusters; callers MUST treat that as "no more clusters to
/// attack in this report" and switch back to `re_diagnostic`.
#[allow(dead_code)] // wired into turn.rs by a subsequent task; exercised here via unit tests.
pub(super) fn advance_to_next_cluster(
    repair_job: &mut RepairJob,
    report: &SemanticFailureReport,
) -> bool {
    // 1. Record the current plan in the exhausted_attempts ledger before
    //    we drop it, so slot reuse preserves history (S3-010). At the same
    //    time, capture the current plan's `spec_authority` so we can carry
    //    it forward into the next cluster's plan (CB-009).
    let carried_authority = repair_job.semantic_plan.as_ref().map(|plan| {
        let entry = (plan.failure_cluster_id.clone(), plan.preferred_repair_role);
        if !repair_job.exhausted_attempts.contains(&entry) {
            repair_job.exhausted_attempts.push(entry);
        }
        plan.spec_authority
    });

    // 2. Find the next cluster in document order that is NOT exhausted
    //    under the report's preferred_repair_role.
    let role = report.preferred_repair_role;
    let next_cluster = report.failure_clusters.iter().find(|cluster| {
        !repair_job
            .exhausted_attempts
            .contains(&(cluster.cluster_key.clone(), role))
    });

    let Some(next_cluster) = next_cluster else {
        // 3. No more clusters — clear the slot and signal no progress.
        repair_job.semantic_plan = None;
        return false;
    };

    // 4. CB-009: carry the elected authority forward across slot reuse.
    //    Only `failure_cluster_id` (and the per-cluster fields derived
    //    from `next_cluster` / `report`) change; the spec source that
    //    elected this report's repair plan is preserved verbatim. When
    //    no current plan exists (defensive path), fall back to
    //    `ImplementationContract` — the same neutral default `resolve()`
    //    uses when no higher-authority signal fires.
    let spec_authority = carried_authority.unwrap_or(SpecAuthority::ImplementationContract);

    repair_job.semantic_plan = Some(SemanticRepairPlan {
        semantic_cause: report.failure_kind,
        spec_authority,
        preferred_repair_role: role,
        repair_hypothesis: report.repair_hypothesis.clone(),
        failure_cluster_id: next_cluster.cluster_key.clone(),
        expected_improvement: None,
        semantic_report: report.clone(),
    });
    true
}

/// Issue #647 (Phase E.2): predicate that becomes `true` when the active
/// `SemanticRepairPlan` targets a `(cluster_id, role)` pair that is already
/// in `exhausted_attempts` — meaning the same repair has been tried and the
/// caller MUST switch to the re-diagnostic path (`assessment = None,
/// assessment_attempts += 1`, bounded by `VERIFIER_DIAGNOSTIC_ATTEMPT_LIMIT`
/// in `turn.rs`) rather than redoing the same attack.
///
/// Returns `false` when:
/// - `semantic_plan` is `None` (no active plan, nothing to compare).
/// - The active plan's `(cluster_id, role)` pair is not yet in the ledger.
#[allow(dead_code)] // wired into turn.rs by a subsequent task; exercised here via unit tests.
pub(super) fn should_re_diagnostic(repair_job: &RepairJob) -> bool {
    let Some(plan) = repair_job.semantic_plan.as_ref() else {
        return false;
    };
    repair_job
        .exhausted_attempts
        .contains(&(plan.failure_cluster_id.clone(), plan.preferred_repair_role))
}

/// Issue #647 (MF2.3): production dispatch helper that wires the Phase-E
/// cluster-aware helpers into the rerun-handling path.
///
/// Called from `turn.rs::drive_task_contract_verifier` after a fresh
/// `RepairJob` has been built by `verifier_repair_context_from_failure`
/// (which carries `semantic_plan` / `exhausted_attempts` over from the
/// previous turn via MF2.1). Drives the cluster-aware sequential repair
/// state machine:
///
/// 1. If the active `semantic_plan`'s `(cluster_id, role)` is already in
///    `exhausted_attempts` (`should_re_diagnostic` = true — the diagnostic
///    LLM has re-proposed an already-attacked cluster), reset to the
///    re-diagnostic path: `assessment = None`, `assessment_attempts += 1`.
/// 2. Else, if `rerun_outcome` indicates the repair did not help
///    (`SameFailureRemaining` / `Worsened`) AND a `semantic_plan` is
///    active, attempt to walk to the next cluster via
///    `advance_to_next_cluster`.
///    - If a next cluster was found, wrap the rerun outcome via
///      `rerun_outcome_with_cluster` so a cluster-id transition surfaces
///      as `NewFailure` to downstream consumers.
///    - If no next cluster is available, the report is exhausted: reset
///      to the re-diagnostic path so a fresh diagnostic LLM call can
///      identify a new cluster set.
///
/// `previous_cluster_id` is the `failure_cluster_id` carried over from the
/// previous turn's `RepairJob.semantic_plan` (the source of truth for the
/// "what cluster did we just attack" question). The helper does NOT touch
/// `repair_attempt` or `applied_repair_intents` — those remain governed by
/// the legacy diagnostic / repair-pass machinery (S7-005).
pub(super) fn apply_semantic_repair_dispatch_after_rerun(
    repair_job: &mut RepairJob,
    previous_cluster_id: Option<&FailureClusterKey>,
) {
    // 1. Active plan is already exhausted → switch to re-diagnostic.
    if should_re_diagnostic(repair_job) {
        repair_job.assessment = None;
        repair_job.assessment_attempts = repair_job.assessment_attempts.saturating_add(1);
        return;
    }

    // 2. Only walk to the next cluster when the rerun outcome indicates the
    //    repair did not progress (legacy failure_count-based outcome is the
    //    SSOT here, S3-014).
    let needs_advance = matches!(
        repair_job.rerun_outcome,
        Some(
            VerifierRepairRerunOutcome::SameFailureRemaining | VerifierRepairRerunOutcome::Worsened
        )
    ) && repair_job.semantic_plan.is_some();
    if !needs_advance {
        return;
    }

    let Some(base_outcome) = repair_job.rerun_outcome else {
        return;
    };
    let report = repair_job
        .semantic_plan
        .as_ref()
        .map(|plan| plan.semantic_report.clone());
    let Some(report) = report else {
        return;
    };

    let advanced = advance_to_next_cluster(repair_job, &report);

    if advanced {
        // Cluster transition → upgrade the outcome via the SSOT wrapper.
        let new_cluster_id = repair_job
            .semantic_plan
            .as_ref()
            .map(|plan| &plan.failure_cluster_id);
        repair_job.rerun_outcome = Some(rerun_outcome_with_cluster(
            previous_cluster_id,
            new_cluster_id,
            base_outcome,
        ));
    } else {
        // No remaining clusters in the report → fall back to re-diagnostic.
        repair_job.assessment = None;
        repair_job.assessment_attempts = repair_job.assessment_attempts.saturating_add(1);
    }
}

/// Issue #647 (MF2 V3.1): assign a freshly built `SemanticRepairPlan` into
/// the `repair_job.semantic_plan` slot **while preserving the
/// `exhausted_attempts` ledger**.
///
/// Background: when the diagnostic LLM is re-run after `apply_semantic_repair_dispatch_after_rerun`
/// has already pushed a cluster onto `exhausted_attempts`, a naive
/// `current.semantic_plan = new_plan` overwrite throws away the progress the
/// dispatch made — even though the dispatch deliberately recorded the
/// already-attacked cluster so it would be skipped. This helper bridges that
/// gap by routing the assignment through the same Phase-E
/// `advance_to_next_cluster` machinery the rerun dispatch uses.
///
/// Behaviour:
///
/// 1. `new_plan == None` → clear the slot (`semantic_plan = None`). This is
///    the legacy / setup-repair fallback path; no ledger mutation occurs.
/// 2. `new_plan == Some(plan)` whose `(failure_cluster_id,
///    preferred_repair_role)` is **not** already in `exhausted_attempts` →
///    write the new plan verbatim. Ledger untouched.
/// 3. `new_plan == Some(plan)` whose `(failure_cluster_id,
///    preferred_repair_role)` **is** already in `exhausted_attempts` →
///    assign the new plan into the slot, then immediately call
///    `advance_to_next_cluster` with `new_report` (the report carried by the
///    new plan) so the helper walks past the already-exhausted entry to the
///    first unexhausted cluster of the new report. If no cluster remains,
///    the slot ends up as `None` (caller falls back to re-diagnostic /
///    legacy mode — same posture as `advance_to_next_cluster` returning
///    `false`).
///
/// `new_report` is accepted as a separate argument so callers that have a
/// fresh report (e.g. after a re-diagnostic produced a new
/// `SemanticFailureReport`) can pass it explicitly. When `new_plan` is
/// `Some`, the report embedded in `new_plan.semantic_report` is used as the
/// authoritative source for cluster walking; `new_report` is consulted only
/// when the embedded report disagrees with the caller's intent — current
/// callers should pass the same report instance as a defensive precondition.
///
/// The helper deliberately does **not** touch `assessment` /
/// `assessment_attempts` / `repair_attempt` — those remain governed by the
/// legacy diagnostic / repair-pass machinery (mirrors the S7-005 boundary
/// in `apply_semantic_repair_dispatch_after_rerun`).
pub(super) fn assign_semantic_plan_preserving_exhausted(
    repair_job: &mut RepairJob,
    new_plan: Option<SemanticRepairPlan>,
    new_report: Option<&SemanticFailureReport>,
) {
    let Some(plan) = new_plan else {
        // Legacy fallback / setup-repair path: clear the slot, leave the
        // ledger and other fields untouched (matches the pre-MF2-V3
        // overwrite semantics for the `None` case).
        repair_job.semantic_plan = None;
        return;
    };

    let role = plan.preferred_repair_role;
    let cluster_id = plan.failure_cluster_id.clone();
    let already_exhausted = repair_job.exhausted_attempts.contains(&(cluster_id, role));

    // Assign the plan into the slot first. The embedded `semantic_report`
    // becomes the authoritative source for cluster walking when we need to
    // skip an already-exhausted entry.
    let embedded_report = plan.semantic_report.clone();
    repair_job.semantic_plan = Some(plan);

    if !already_exhausted {
        return;
    }

    // The newly proposed plan targets an already-exhausted (cluster, role)
    // pair → ask the Phase-E walker to skip ahead. Prefer the embedded
    // report (it travels with the plan); fall back to the caller-supplied
    // `new_report` only if the caller explicitly passes a different one.
    let report = new_report.cloned().unwrap_or(embedded_report);
    advance_to_next_cluster(repair_job, &report);
    // `advance_to_next_cluster` already mutates `semantic_plan`
    // (either to the next unexhausted cluster or to `None`) and updates the
    // ledger — no further bookkeeping required here.
}

/// Issue #647 (Phase E.3 / S3-014): wrap the existing
/// `verifier_repair_rerun_outcome` (which is failure_count-based and
/// cluster-agnostic) with a cluster-id transition rule.
///
/// Rationale (S3-014): the legacy outcome compares `failure_count`
/// monotonically — if the same number of failures remain after a repair,
/// the outcome is `SameFailureRemaining` even when a completely different
/// cluster is now failing. For Phase-E sequential repair this is
/// misleading: an honest "new cluster surfaced" run looks identical to
/// "same cluster still failing". This wrapper preserves the legacy outcome
/// when the cluster id is unchanged (or unknown on either side) and
/// upgrades it to `NewFailure` when the cluster id transitions.
///
/// Inputs:
/// - `previous_cluster_id`: the `failure_cluster_id` from the prior
///   `SemanticRepairPlan`. `None` means "no prior cluster was tracked"
///   (e.g. legacy path that built no semantic plan).
/// - `current_cluster_id`: the `failure_cluster_id` from the newly built
///   `SemanticRepairPlan`. `None` means "no current cluster is tracked".
/// - `base_outcome`: the verdict returned by
///   `verifier_repair_rerun_outcome` for the same rerun (the legacy
///   failure_count comparison stays the SSOT).
///
/// Returns `base_outcome` unchanged unless both sides are `Some(...)` and
/// the cluster ids differ — then returns `NewFailure`.
#[allow(dead_code)] // wired into turn.rs by a subsequent task; exercised here via unit tests.
pub(super) fn rerun_outcome_with_cluster(
    previous_cluster_id: Option<&FailureClusterKey>,
    current_cluster_id: Option<&FailureClusterKey>,
    base_outcome: VerifierRepairRerunOutcome,
) -> VerifierRepairRerunOutcome {
    match (previous_cluster_id, current_cluster_id) {
        (Some(prev), Some(curr)) if prev != curr => VerifierRepairRerunOutcome::NewFailure,
        _ => base_outcome,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_repair_job_text_masks_secrets_and_neutralizes_controls() {
        let raw = "Authorization: Bearer SECRETVALUE\nGET /\rEND";
        let out = sanitize_repair_job_text(raw);
        assert!(!out.contains("SECRETVALUE"));
        assert!(!out.contains('\n'));
        assert!(!out.contains('\r'));
    }

    #[test]
    fn missing_verifier_job_suppresses_verifier_retry_until_in_scope_edit() {
        // Issue #646 (A1/B2): a fresh job suppresses verifier retry;
        // recording an in-scope edit flips the gate.
        let mut job = MissingVerifierJob::new(3, 0);
        assert!(job.should_suppress_verifier_retry());
        job.record_in_scope_edit();
        assert!(!job.should_suppress_verifier_retry());
    }

    #[test]
    fn missing_verifier_job_retry_budget_is_bounded() {
        let mut job = MissingVerifierJob::new(2, 0);
        assert!(job.record_retry());
        assert!(job.record_retry());
        // Third call should report budget exhausted.
        assert!(!job.record_retry());
        assert_eq!(job.retries_used, 2);
    }

    #[test]
    fn missing_verifier_job_allowed_tool_names_match_first_class_state() {
        let job = MissingVerifierJob::new(3, 0);
        assert_eq!(job.allowed_tool_names(), &["Write", "Edit", "Bash"]);
    }

    #[test]
    fn truncate_for_snapshot_is_utf8_safe_and_bounded() {
        let s = "a".repeat(SNAPSHOT_FIELD_BYTE_CAP + 100);
        assert_eq!(truncate_for_snapshot(&s).len(), SNAPSHOT_FIELD_BYTE_CAP);
        let multi = "あ".repeat(SNAPSHOT_FIELD_BYTE_CAP); // 3 bytes per char
        let truncated = truncate_for_snapshot(&multi);
        assert!(truncated.len() <= SNAPSHOT_FIELD_BYTE_CAP);
        assert!(truncated.chars().all(|c| c == 'あ'));
    }

    // ---- Issue #647 (Phase B): SemanticRepairPlan + RepairJob::new_for_test ---- //

    /// Build a minimal `SemanticFailureReport` for SemanticRepairPlan tests.
    /// Uses Phase A.1 entry points so the fixture stays SSOT-aligned.
    #[cfg(test)]
    fn semantic_report_fixture(
        kind: VerifierDiagnosticFailureKind,
        confidence: f32,
    ) -> super::super::semantic_failure::SemanticFailureReport {
        let json = serde_json::json!({
            "failure_kind": kind_label(kind),
            "confidence": confidence,
            "preferred_repair_role": "implementation",
            "repair_hypothesis": "hypothesis text",
            "failure_clusters": [
                {
                    "observed": "obs",
                    "expected": "exp",
                    "input_shape": "shape",
                    "assertion_shape": "AssertEq",
                    "involved_artifacts": ["test"],
                    "affected_cases": ["case1"],
                }
            ],
        });
        super::super::semantic_failure::parse_semantic_failure_report(&json)
            .expect("fixture parses")
    }

    #[cfg(test)]
    fn kind_label(kind: VerifierDiagnosticFailureKind) -> &'static str {
        match kind {
            VerifierDiagnosticFailureKind::DependencyMissing => "dependency_missing",
            VerifierDiagnosticFailureKind::LocalImportContractMismatch => {
                "local_import_contract_mismatch"
            }
            VerifierDiagnosticFailureKind::CompileOrSyntaxError => "compile_or_syntax_error",
            VerifierDiagnosticFailureKind::AssertionMismatch => "assertion_mismatch",
            VerifierDiagnosticFailureKind::RuntimeError => "runtime_error",
            VerifierDiagnosticFailureKind::TestBug => "test_bug",
            VerifierDiagnosticFailureKind::ConfigOrVerifierError => "config_or_verifier_error",
            VerifierDiagnosticFailureKind::Unknown => "unknown",
        }
    }

    #[test]
    fn semantic_repair_plan_constructs_and_compares_equal_for_same_inputs() {
        let report = semantic_report_fixture(VerifierDiagnosticFailureKind::AssertionMismatch, 0.7);
        let cluster_id = report.failure_clusters[0].cluster_key.clone();
        let plan_a = SemanticRepairPlan {
            semantic_report: report.clone(),
            failure_cluster_id: cluster_id.clone(),
            semantic_cause: VerifierDiagnosticFailureKind::AssertionMismatch,
            spec_authority: SpecAuthority::BehaviorContract,
            preferred_repair_role: super::super::task_contract::ArtifactRole::Implementation,
            repair_hypothesis: "h".to_string(),
            expected_improvement: None,
        };
        let plan_b = SemanticRepairPlan {
            semantic_report: report,
            failure_cluster_id: cluster_id,
            semantic_cause: VerifierDiagnosticFailureKind::AssertionMismatch,
            spec_authority: SpecAuthority::BehaviorContract,
            preferred_repair_role: super::super::task_contract::ArtifactRole::Implementation,
            repair_hypothesis: "h".to_string(),
            expected_improvement: None,
        };
        assert_eq!(plan_a, plan_b);
    }

    #[test]
    fn semantic_repair_plan_partial_eq_distinguishes_authority_and_role() {
        let report = semantic_report_fixture(VerifierDiagnosticFailureKind::AssertionMismatch, 0.7);
        let cluster_id = report.failure_clusters[0].cluster_key.clone();
        let base = SemanticRepairPlan {
            semantic_report: report.clone(),
            failure_cluster_id: cluster_id.clone(),
            semantic_cause: VerifierDiagnosticFailureKind::AssertionMismatch,
            spec_authority: SpecAuthority::BehaviorContract,
            preferred_repair_role: super::super::task_contract::ArtifactRole::Implementation,
            repair_hypothesis: "h".to_string(),
            expected_improvement: None,
        };
        let mut different_authority = base.clone();
        different_authority.spec_authority = SpecAuthority::LlmGeneratedTest;
        assert_ne!(base, different_authority);

        let mut different_role = base.clone();
        different_role.preferred_repair_role = super::super::task_contract::ArtifactRole::Test;
        assert_ne!(base, different_role);
    }

    #[test]
    fn repair_job_new_for_test_initializes_semantic_fields_empty() {
        let job = RepairJob::new_for_test();
        // Issue #647 (Phase B): semantic planning slots start empty so that
        // pre-Phase-D code paths see a "no semantic plan yet" state.
        assert!(job.semantic_plan.is_none());
        assert!(job.exhausted_attempts.is_empty());
    }

    #[test]
    fn repair_job_new_for_test_initializes_legacy_fields_to_neutral_defaults() {
        let job = RepairJob::new_for_test();
        assert_eq!(job.failure_type, VerifierFailureType::Unknown);
        assert!(job.target_hint.is_none());
        assert!(job.repair_target_hint.is_none());
        assert!(job.changed_file_hints.is_empty());
        assert!(job.assessment.is_none());
        assert_eq!(job.assessment_attempts, 0);
        assert!(!job.diagnostic_attempted);
        assert!(!job.diagnostic_unavailable);
        assert!(job.diagnostic_error.is_none());
        assert!(job.repair_error.is_none());
        assert!(job.applied_repair_intents.is_empty());
        assert!(job.target_line.is_none());
        assert!(job.error_kind.is_none());
        assert_eq!(job.failure_signature, "");
        assert!(job.failure_count.is_none());
        assert!(job.previous_failure_signature.is_none());
        assert!(job.previous_failure_count.is_none());
        assert!(job.rerun_outcome.is_none());
        assert_eq!(job.repair_attempt, 0);
    }

    #[test]
    fn repair_job_supports_struct_update_syntax_for_partial_override() {
        // S3-001: existing test fixtures override only the fields they care
        // about and let `new_for_test()` fill the rest. Mirror that pattern
        // here to lock the API shape (no `..Default::default()` indirection).
        let job = RepairJob {
            command: "pytest".to_string(),
            failure_signature: "sig".to_string(),
            ..RepairJob::new_for_test()
        };
        assert_eq!(job.command, "pytest");
        assert_eq!(job.failure_signature, "sig");
        // Untouched fields still take new_for_test defaults.
        assert_eq!(job.failure_type, VerifierFailureType::Unknown);
        assert!(job.semantic_plan.is_none());
        assert!(job.exhausted_attempts.is_empty());
    }

    #[test]
    fn repair_job_partial_eq_holds_with_semantic_plan_some() {
        // S7-001: `Eq` is dropped because `SemanticFailureReport.confidence`
        // is `f32`, but `PartialEq` must still work for assertion / diff
        // workflows in tests.
        let report = semantic_report_fixture(VerifierDiagnosticFailureKind::AssertionMismatch, 0.5);
        let cluster_id = report.failure_clusters[0].cluster_key.clone();
        let plan = SemanticRepairPlan {
            semantic_report: report,
            failure_cluster_id: cluster_id,
            semantic_cause: VerifierDiagnosticFailureKind::AssertionMismatch,
            spec_authority: SpecAuthority::ImplementationContract,
            preferred_repair_role: super::super::task_contract::ArtifactRole::Implementation,
            repair_hypothesis: "h".to_string(),
            expected_improvement: None,
        };
        let a = RepairJob {
            semantic_plan: Some(plan.clone()),
            ..RepairJob::new_for_test()
        };
        let b = RepairJob {
            semantic_plan: Some(plan),
            ..RepairJob::new_for_test()
        };
        assert_eq!(a, b);

        // Mutating exhausted_attempts breaks equality.
        let c = RepairJob {
            exhausted_attempts: vec![(
                a.semantic_plan.as_ref().unwrap().failure_cluster_id.clone(),
                super::super::task_contract::ArtifactRole::Test,
            )],
            ..a.clone()
        };
        assert_ne!(a, c);
    }

    // ---- Issue #647 (Phase E): cluster-based sequential repair ---- //

    /// Helper: build a `SemanticFailureReport` with N distinct clusters by
    /// varying the `observed` text with words that survive the
    /// shape-normalization pass (no numeric / quoted / path / long-token
    /// runs). Up to four clusters supported by the canned labels —
    /// callers MUST keep `cluster_count <= 4`.
    #[cfg(test)]
    fn multi_cluster_report_fixture(
        kind: VerifierDiagnosticFailureKind,
        cluster_count: usize,
    ) -> super::super::semantic_failure::SemanticFailureReport {
        // Distinct word labels — bare alphabetics survive `normalize_to_shape`
        // (no `<num>` / `<token>` / `<str>` collapse) so each cluster gets a
        // unique `cluster_key`.
        const LABELS: &[(&str, &str, &str, &str)] = &[
            ("alpha", "ALPHA", "alphashape", "alphacase"),
            ("beta", "BETA", "betashape", "betacase"),
            ("gamma", "GAMMA", "gammashape", "gammacase"),
            ("delta", "DELTA", "deltashape", "deltacase"),
        ];
        assert!(
            cluster_count <= LABELS.len(),
            "multi_cluster_report_fixture supports up to {} clusters",
            LABELS.len()
        );
        let clusters: Vec<serde_json::Value> = LABELS
            .iter()
            .take(cluster_count)
            .map(|(obs, exp, shape, case)| {
                serde_json::json!({
                    "observed": obs,
                    "expected": exp,
                    "input_shape": shape,
                    "assertion_shape": "AssertEq",
                    "involved_artifacts": ["implementation", "test"],
                    "affected_cases": [case],
                })
            })
            .collect();
        let json = serde_json::json!({
            "failure_kind": kind_label(kind),
            "confidence": 0.8,
            "preferred_repair_role": "implementation",
            "repair_hypothesis": "multi-cluster hypothesis",
            "failure_clusters": clusters,
        });
        super::super::semantic_failure::parse_semantic_failure_report(&json)
            .expect("multi-cluster fixture parses")
    }

    /// Build a fresh `RepairJob` whose `semantic_plan` slot targets the
    /// first cluster of `report`. Mirrors what Phase D writes during
    /// `run_verifier_diagnostic_pass`.
    #[cfg(test)]
    fn job_with_first_cluster_plan(
        report: &super::super::semantic_failure::SemanticFailureReport,
    ) -> RepairJob {
        let first = &report.failure_clusters[0];
        let plan = SemanticRepairPlan {
            semantic_report: report.clone(),
            failure_cluster_id: first.cluster_key.clone(),
            semantic_cause: report.failure_kind,
            spec_authority: SpecAuthority::ImplementationContract,
            preferred_repair_role: report.preferred_repair_role,
            repair_hypothesis: report.repair_hypothesis.clone(),
            expected_improvement: None,
        };
        RepairJob {
            semantic_plan: Some(plan),
            ..RepairJob::new_for_test()
        }
    }

    /// Phase E.1 (S1-015): three-cluster sequential repair.
    ///
    /// Calling `advance_to_next_cluster` twice walks the slot from
    /// cluster 1 → cluster 2 → cluster 3 deterministically, in the
    /// document order returned by the diagnostic LLM.
    #[test]
    fn phase_e_advance_to_next_cluster_walks_three_clusters_in_order() {
        let report =
            multi_cluster_report_fixture(VerifierDiagnosticFailureKind::AssertionMismatch, 3);
        let cluster_ids: Vec<_> = report
            .failure_clusters
            .iter()
            .map(|c| c.cluster_key.clone())
            .collect();
        // Sanity check: three *distinct* cluster keys (shape varies per cluster).
        assert_eq!(cluster_ids.len(), 3);
        assert_ne!(cluster_ids[0], cluster_ids[1]);
        assert_ne!(cluster_ids[1], cluster_ids[2]);
        assert_ne!(cluster_ids[0], cluster_ids[2]);

        let mut job = job_with_first_cluster_plan(&report);
        // Initial slot: cluster 1.
        assert_eq!(
            job.semantic_plan.as_ref().unwrap().failure_cluster_id,
            cluster_ids[0]
        );

        // First advance: cluster 1 → cluster 2.
        assert!(advance_to_next_cluster(&mut job, &report));
        assert_eq!(
            job.semantic_plan.as_ref().unwrap().failure_cluster_id,
            cluster_ids[1]
        );

        // Second advance: cluster 2 → cluster 3.
        assert!(advance_to_next_cluster(&mut job, &report));
        assert_eq!(
            job.semantic_plan.as_ref().unwrap().failure_cluster_id,
            cluster_ids[2]
        );

        // Third advance: no more clusters — slot cleared, return false.
        assert!(!advance_to_next_cluster(&mut job, &report));
        assert!(job.semantic_plan.is_none());
    }

    /// Phase E.1 / S3-010: `exhausted_attempts` accumulates across slot
    /// reuse — each advance adds the prior `(cluster_id, role)` pair and
    /// the ledger survives slot replacement.
    #[test]
    fn phase_e_exhausted_attempts_ledger_preserved_across_slot_reuse() {
        let report =
            multi_cluster_report_fixture(VerifierDiagnosticFailureKind::AssertionMismatch, 3);
        let cluster_ids: Vec<_> = report
            .failure_clusters
            .iter()
            .map(|c| c.cluster_key.clone())
            .collect();
        let role = report.preferred_repair_role;

        let mut job = job_with_first_cluster_plan(&report);
        assert!(job.exhausted_attempts.is_empty());

        // Advance 1: cluster 1 → cluster 2; ledger now holds cluster 1.
        assert!(advance_to_next_cluster(&mut job, &report));
        assert_eq!(job.exhausted_attempts.len(), 1);
        assert_eq!(job.exhausted_attempts[0], (cluster_ids[0].clone(), role));
        // Slot now targets cluster 2.
        assert_eq!(
            job.semantic_plan.as_ref().unwrap().failure_cluster_id,
            cluster_ids[1]
        );

        // Advance 2: cluster 2 → cluster 3; ledger preserves cluster 1.
        assert!(advance_to_next_cluster(&mut job, &report));
        assert_eq!(job.exhausted_attempts.len(), 2);
        assert_eq!(job.exhausted_attempts[0], (cluster_ids[0].clone(), role));
        assert_eq!(job.exhausted_attempts[1], (cluster_ids[1].clone(), role));
        // Slot now targets cluster 3.
        assert_eq!(
            job.semantic_plan.as_ref().unwrap().failure_cluster_id,
            cluster_ids[2]
        );

        // Advance 3: no more clusters; slot cleared, ledger still holds
        // both 1 and 2 (the cluster 3 attempt is recorded too because we
        // pushed it before searching).
        assert!(!advance_to_next_cluster(&mut job, &report));
        assert!(job.semantic_plan.is_none());
        assert_eq!(job.exhausted_attempts.len(), 3);
        assert_eq!(job.exhausted_attempts[2], (cluster_ids[2].clone(), role));
    }

    /// Phase E.1: `advance_to_next_cluster` is idempotent on the ledger —
    /// calling it again after the slot is cleared does not append a
    /// duplicate `(cluster_id, role)` entry.
    #[test]
    fn phase_e_advance_to_next_cluster_is_idempotent_when_no_plan_present() {
        let report =
            multi_cluster_report_fixture(VerifierDiagnosticFailureKind::AssertionMismatch, 1);
        let mut job = job_with_first_cluster_plan(&report);

        // First call: no other clusters → slot cleared, false returned.
        assert!(!advance_to_next_cluster(&mut job, &report));
        let after_first_len = job.exhausted_attempts.len();
        assert_eq!(after_first_len, 1);

        // Second call: semantic_plan is None, so no new ledger entry.
        assert!(!advance_to_next_cluster(&mut job, &report));
        assert_eq!(job.exhausted_attempts.len(), after_first_len);
    }

    /// Phase E.2 (S1-008): same `(cluster_id, role)` re-attempted →
    /// `should_re_diagnostic` flips to `true` so the caller switches to
    /// the re-diagnostic path instead of repeating the same attack.
    #[test]
    fn phase_e_should_re_diagnostic_true_when_current_plan_already_exhausted() {
        let report =
            multi_cluster_report_fixture(VerifierDiagnosticFailureKind::AssertionMismatch, 2);
        let cluster_ids: Vec<_> = report
            .failure_clusters
            .iter()
            .map(|c| c.cluster_key.clone())
            .collect();
        let role = report.preferred_repair_role;

        // Build a job where exhausted_attempts already contains the first
        // cluster, and the active plan still targets the first cluster
        // (simulates "we tried cluster 1, the diagnostic LLM re-proposed
        // the same cluster on retry").
        let plan = SemanticRepairPlan {
            semantic_report: report.clone(),
            failure_cluster_id: cluster_ids[0].clone(),
            semantic_cause: report.failure_kind,
            spec_authority: SpecAuthority::ImplementationContract,
            preferred_repair_role: role,
            repair_hypothesis: report.repair_hypothesis.clone(),
            expected_improvement: None,
        };
        let job = RepairJob {
            semantic_plan: Some(plan),
            exhausted_attempts: vec![(cluster_ids[0].clone(), role)],
            ..RepairJob::new_for_test()
        };
        assert!(should_re_diagnostic(&job));

        // Different cluster targeted → not yet exhausted → false.
        let plan2 = SemanticRepairPlan {
            semantic_report: report.clone(),
            failure_cluster_id: cluster_ids[1].clone(),
            semantic_cause: report.failure_kind,
            spec_authority: SpecAuthority::ImplementationContract,
            preferred_repair_role: role,
            repair_hypothesis: report.repair_hypothesis.clone(),
            expected_improvement: None,
        };
        let job2 = RepairJob {
            semantic_plan: Some(plan2),
            exhausted_attempts: vec![(cluster_ids[0].clone(), role)],
            ..RepairJob::new_for_test()
        };
        assert!(!should_re_diagnostic(&job2));
    }

    /// Phase E.2: `should_re_diagnostic` returns `false` when no plan is
    /// active (legacy fallback / SetupRepair dispatch path).
    #[test]
    fn phase_e_should_re_diagnostic_false_when_no_plan_present() {
        let job = RepairJob::new_for_test();
        assert!(!should_re_diagnostic(&job));
    }

    /// Phase E.3 (S3-014): cluster id unchanged → `rerun_outcome_with_cluster`
    /// returns the base outcome verbatim. The legacy failure_count-based
    /// outcome remains the SSOT.
    #[test]
    fn phase_e_rerun_outcome_unchanged_when_cluster_id_stable() {
        let report =
            multi_cluster_report_fixture(VerifierDiagnosticFailureKind::AssertionMismatch, 2);
        let cluster_a = report.failure_clusters[0].cluster_key.clone();

        // Both sides Some, same id → base outcome wins.
        for base in [
            VerifierRepairRerunOutcome::Improved,
            VerifierRepairRerunOutcome::SameFailureRemaining,
            VerifierRepairRerunOutcome::NewFailure,
            VerifierRepairRerunOutcome::Worsened,
        ] {
            assert_eq!(
                rerun_outcome_with_cluster(Some(&cluster_a), Some(&cluster_a), base),
                base,
                "stable cluster id must keep base outcome verbatim",
            );
        }

        // None on either side → base outcome wins (legacy fallback).
        assert_eq!(
            rerun_outcome_with_cluster(
                None,
                Some(&cluster_a),
                VerifierRepairRerunOutcome::Improved
            ),
            VerifierRepairRerunOutcome::Improved,
        );
        assert_eq!(
            rerun_outcome_with_cluster(
                Some(&cluster_a),
                None,
                VerifierRepairRerunOutcome::SameFailureRemaining
            ),
            VerifierRepairRerunOutcome::SameFailureRemaining,
        );
        assert_eq!(
            rerun_outcome_with_cluster(None, None, VerifierRepairRerunOutcome::NewFailure),
            VerifierRepairRerunOutcome::NewFailure,
        );
    }

    /// Phase E.3 (S3-014): cluster id transitioned → outcome upgraded to
    /// `NewFailure` regardless of the base outcome. This catches the case
    /// where failure_count stayed the same but a different cluster is now
    /// failing — failure_count alone would mis-report `SameFailureRemaining`.
    #[test]
    fn phase_e_rerun_outcome_promoted_to_new_failure_on_cluster_id_change() {
        let report =
            multi_cluster_report_fixture(VerifierDiagnosticFailureKind::AssertionMismatch, 2);
        let cluster_a = report.failure_clusters[0].cluster_key.clone();
        let cluster_b = report.failure_clusters[1].cluster_key.clone();
        assert_ne!(cluster_a, cluster_b);

        for base in [
            VerifierRepairRerunOutcome::Improved,
            VerifierRepairRerunOutcome::SameFailureRemaining,
            VerifierRepairRerunOutcome::NewFailure,
            VerifierRepairRerunOutcome::Worsened,
        ] {
            assert_eq!(
                rerun_outcome_with_cluster(Some(&cluster_a), Some(&cluster_b), base),
                VerifierRepairRerunOutcome::NewFailure,
                "cluster id transition must promote to NewFailure (base={base:?})",
            );
        }
    }

    /// Phase E.4 (S7-005): three-cluster sequential repair stays inside
    /// the existing retry budgets.
    ///
    /// `advance_to_next_cluster` does not touch `assessment_attempts` /
    /// `repair_attempt` (those are bumped by the legacy diagnostic /
    /// repair-pass machinery in `turn.rs`), so walking three clusters
    /// in a single turn does not consume `TASK_CONTRACT_VERIFIER_ATTEMPT_LIMIT`
    /// (= 3) or `TASK_CONTRACT_VERIFIER_REPAIR_ATTEMPT_LIMIT` (= 6) early.
    #[test]
    fn phase_e_three_cluster_sequential_repair_does_not_consume_retry_budget() {
        // The constants the test references — keep this assertion in sync
        // with `turn.rs` so a future limit change is caught here.
        const TASK_CONTRACT_VERIFIER_ATTEMPT_LIMIT: usize = 3;
        const TASK_CONTRACT_VERIFIER_REPAIR_ATTEMPT_LIMIT: usize = 6;

        let report =
            multi_cluster_report_fixture(VerifierDiagnosticFailureKind::AssertionMismatch, 3);
        let mut job = job_with_first_cluster_plan(&report);

        // Walk all three clusters via the Phase-E helper.
        assert!(advance_to_next_cluster(&mut job, &report)); // 1 → 2
        assert!(advance_to_next_cluster(&mut job, &report)); // 2 → 3
        // Third advance: no more clusters.
        assert!(!advance_to_next_cluster(&mut job, &report));

        // The Phase-E slot reuse path must not touch the legacy retry
        // counters — they remain at their fresh-job defaults so the
        // existing `TASK_CONTRACT_VERIFIER_*_ATTEMPT_LIMIT` budgets are
        // fully available for downstream `turn.rs` dispatch.
        assert_eq!(job.assessment_attempts, 0);
        assert_eq!(job.repair_attempt, 0);
        assert!(
            job.assessment_attempts < TASK_CONTRACT_VERIFIER_ATTEMPT_LIMIT,
            "assessment_attempts must stay under TASK_CONTRACT_VERIFIER_ATTEMPT_LIMIT",
        );
        assert!(
            job.repair_attempt < TASK_CONTRACT_VERIFIER_REPAIR_ATTEMPT_LIMIT,
            "repair_attempt must stay under TASK_CONTRACT_VERIFIER_REPAIR_ATTEMPT_LIMIT",
        );
        // The ledger does grow per advance — but it's an O(N_clusters)
        // bounded list, not a retry counter.
        assert_eq!(job.exhausted_attempts.len(), 3);
    }

    // ---- Issue #647 (MF2): production dispatch wiring ---- //

    /// MF2.3: after carryover, when rerun outcome is bad (SameFailureRemaining)
    /// and there is a next cluster, dispatch advances to the next cluster and
    /// promotes rerun_outcome to NewFailure via `rerun_outcome_with_cluster`.
    /// `assessment` is preserved (we have a plan; do not re-diagnostic yet).
    #[test]
    fn mf2_dispatch_advances_to_next_cluster_on_same_failure_remaining() {
        let report =
            multi_cluster_report_fixture(VerifierDiagnosticFailureKind::AssertionMismatch, 2);
        let cluster_ids: Vec<_> = report
            .failure_clusters
            .iter()
            .map(|c| c.cluster_key.clone())
            .collect();
        let mut job = job_with_first_cluster_plan(&report);
        // Simulate a non-empty prior assessment that survived carryover.
        let preserved_assessment = super::super::VerifierRepairAssessment {
            failure_kind: VerifierDiagnosticFailureKind::AssertionMismatch,
            failure_type: VerifierFailureType::Unknown,
            probable_cause_role: Some(super::super::task_contract::ArtifactRole::Implementation),
            needed_reads: Vec::new(),
            repair_target_hint: None,
            repair_plan: Vec::new(),
            summary: Some("preserved".to_string()),
            source: super::super::VerifierRepairAssessmentSource::DiagnosticPass,
        };
        job.assessment = Some(preserved_assessment);
        job.rerun_outcome = Some(VerifierRepairRerunOutcome::SameFailureRemaining);

        let previous_cluster = Some(cluster_ids[0].clone());
        apply_semantic_repair_dispatch_after_rerun(&mut job, previous_cluster.as_ref());

        // Slot walked to the next cluster.
        assert_eq!(
            job.semantic_plan.as_ref().unwrap().failure_cluster_id,
            cluster_ids[1]
        );
        // Outcome promoted to NewFailure via cluster transition wrap.
        assert_eq!(
            job.rerun_outcome,
            Some(VerifierRepairRerunOutcome::NewFailure)
        );
        // assessment preserved — caller proceeds to repair the new cluster
        // without burning the diagnostic budget.
        assert!(job.assessment.is_some());
        assert_eq!(job.assessment_attempts, 0);
    }

    /// MF2.3: when the active plan's (cluster_id, role) is already in the
    /// exhausted ledger (LLM re-proposed the same cluster), dispatch must
    /// switch to the re-diagnostic path (assessment cleared, attempts bumped).
    #[test]
    fn mf2_dispatch_resets_for_re_diagnostic_when_plan_already_exhausted() {
        let report =
            multi_cluster_report_fixture(VerifierDiagnosticFailureKind::AssertionMismatch, 2);
        let cluster_ids: Vec<_> = report
            .failure_clusters
            .iter()
            .map(|c| c.cluster_key.clone())
            .collect();
        let role = report.preferred_repair_role;

        // Job state: plan still targets cluster A, but cluster A is already
        // in the ledger (= we've burned cluster A; diagnostic re-proposed it).
        let plan = SemanticRepairPlan {
            semantic_report: report.clone(),
            failure_cluster_id: cluster_ids[0].clone(),
            semantic_cause: report.failure_kind,
            spec_authority: SpecAuthority::ImplementationContract,
            preferred_repair_role: role,
            repair_hypothesis: report.repair_hypothesis.clone(),
            expected_improvement: None,
        };
        let mut job = RepairJob {
            semantic_plan: Some(plan),
            exhausted_attempts: vec![(cluster_ids[0].clone(), role)],
            assessment: Some(super::super::VerifierRepairAssessment {
                failure_kind: VerifierDiagnosticFailureKind::AssertionMismatch,
                failure_type: VerifierFailureType::Unknown,
                probable_cause_role: Some(role),
                needed_reads: Vec::new(),
                repair_target_hint: None,
                repair_plan: Vec::new(),
                summary: None,
                source: super::super::VerifierRepairAssessmentSource::DiagnosticPass,
            }),
            rerun_outcome: Some(VerifierRepairRerunOutcome::SameFailureRemaining),
            ..RepairJob::new_for_test()
        };

        apply_semantic_repair_dispatch_after_rerun(&mut job, Some(&cluster_ids[0]));

        // Re-diagnostic switch: assessment cleared, attempts bumped.
        assert!(job.assessment.is_none());
        assert_eq!(job.assessment_attempts, 1);
    }

    /// MF2.3: when the rerun-outcome is bad but the report has no remaining
    /// clusters, advance returns false and we fall back to re-diagnostic.
    #[test]
    fn mf2_dispatch_falls_back_to_re_diagnostic_when_no_more_clusters() {
        let report =
            multi_cluster_report_fixture(VerifierDiagnosticFailureKind::AssertionMismatch, 1);
        let cluster_id = report.failure_clusters[0].cluster_key.clone();
        let mut job = job_with_first_cluster_plan(&report);
        job.assessment = Some(super::super::VerifierRepairAssessment {
            failure_kind: VerifierDiagnosticFailureKind::AssertionMismatch,
            failure_type: VerifierFailureType::Unknown,
            probable_cause_role: Some(report.preferred_repair_role),
            needed_reads: Vec::new(),
            repair_target_hint: None,
            repair_plan: Vec::new(),
            summary: None,
            source: super::super::VerifierRepairAssessmentSource::DiagnosticPass,
        });
        job.rerun_outcome = Some(VerifierRepairRerunOutcome::Worsened);

        apply_semantic_repair_dispatch_after_rerun(&mut job, Some(&cluster_id));

        // No more clusters → plan cleared, re-diagnostic path armed.
        assert!(job.semantic_plan.is_none());
        assert!(job.assessment.is_none());
        assert_eq!(job.assessment_attempts, 1);
    }

    /// MF2.3: when the rerun outcome shows progress (Improved / NewFailure),
    /// dispatch does NOT advance — the next cycle handles the new state.
    #[test]
    fn mf2_dispatch_is_noop_when_rerun_outcome_indicates_progress() {
        let report =
            multi_cluster_report_fixture(VerifierDiagnosticFailureKind::AssertionMismatch, 2);
        let cluster_ids: Vec<_> = report
            .failure_clusters
            .iter()
            .map(|c| c.cluster_key.clone())
            .collect();
        let mut job = job_with_first_cluster_plan(&report);
        let preserved_assessment = super::super::VerifierRepairAssessment {
            failure_kind: VerifierDiagnosticFailureKind::AssertionMismatch,
            failure_type: VerifierFailureType::Unknown,
            probable_cause_role: Some(report.preferred_repair_role),
            needed_reads: Vec::new(),
            repair_target_hint: None,
            repair_plan: Vec::new(),
            summary: None,
            source: super::super::VerifierRepairAssessmentSource::DiagnosticPass,
        };
        job.assessment = Some(preserved_assessment.clone());

        for base in [
            VerifierRepairRerunOutcome::Improved,
            VerifierRepairRerunOutcome::NewFailure,
        ] {
            job.rerun_outcome = Some(base);
            apply_semantic_repair_dispatch_after_rerun(&mut job, Some(&cluster_ids[0]));
            // Plan, ledger, assessment all unchanged.
            assert_eq!(
                job.semantic_plan.as_ref().unwrap().failure_cluster_id,
                cluster_ids[0]
            );
            assert!(job.exhausted_attempts.is_empty());
            assert!(job.assessment.is_some());
            assert_eq!(job.rerun_outcome, Some(base));
        }
    }

    /// MF2.3 + MF2.budget: a 3-cluster sequential repair walks all clusters
    /// via dispatch + advance without ever incrementing the legacy retry
    /// counters (`assessment_attempts` / `repair_attempt`). Re-diagnostic
    /// fires only after the LAST cluster is exhausted.
    #[test]
    fn mf2_three_cluster_sequential_repair_walks_without_burning_budget() {
        let report =
            multi_cluster_report_fixture(VerifierDiagnosticFailureKind::AssertionMismatch, 3);
        let cluster_ids: Vec<_> = report
            .failure_clusters
            .iter()
            .map(|c| c.cluster_key.clone())
            .collect();
        let mut job = job_with_first_cluster_plan(&report);
        let preserved_assessment = super::super::VerifierRepairAssessment {
            failure_kind: VerifierDiagnosticFailureKind::AssertionMismatch,
            failure_type: VerifierFailureType::Unknown,
            probable_cause_role: Some(report.preferred_repair_role),
            needed_reads: Vec::new(),
            repair_target_hint: None,
            repair_plan: Vec::new(),
            summary: None,
            source: super::super::VerifierRepairAssessmentSource::DiagnosticPass,
        };
        job.assessment = Some(preserved_assessment);

        // Turn 1: cluster A fails again → advance to B.
        job.rerun_outcome = Some(VerifierRepairRerunOutcome::SameFailureRemaining);
        apply_semantic_repair_dispatch_after_rerun(&mut job, Some(&cluster_ids[0]));
        assert_eq!(
            job.semantic_plan.as_ref().unwrap().failure_cluster_id,
            cluster_ids[1]
        );
        assert_eq!(job.assessment_attempts, 0);
        assert!(job.assessment.is_some());

        // Turn 2: cluster B fails → advance to C.
        job.rerun_outcome = Some(VerifierRepairRerunOutcome::SameFailureRemaining);
        apply_semantic_repair_dispatch_after_rerun(&mut job, Some(&cluster_ids[1]));
        assert_eq!(
            job.semantic_plan.as_ref().unwrap().failure_cluster_id,
            cluster_ids[2]
        );
        assert_eq!(job.assessment_attempts, 0);
        assert!(job.assessment.is_some());

        // Turn 3: cluster C fails → no more clusters → re-diagnostic.
        job.rerun_outcome = Some(VerifierRepairRerunOutcome::SameFailureRemaining);
        apply_semantic_repair_dispatch_after_rerun(&mut job, Some(&cluster_ids[2]));
        assert!(job.semantic_plan.is_none());
        assert!(job.assessment.is_none());
        // Only ONE bump for the entire 3-cluster walk (only on final exhaust).
        assert_eq!(job.assessment_attempts, 1);
        // repair_attempt is owned by the diagnostic / repair machinery and
        // is not touched by this dispatch helper.
        assert_eq!(job.repair_attempt, 0);
    }

    // ---- Issue #647 (MF2 V3): assign_semantic_plan_preserving_exhausted ---- //

    /// MF2 V3.1: legacy / setup-repair fallback — `new_plan = None` clears the
    /// slot without disturbing the `exhausted_attempts` ledger. This matches
    /// the pre-V3 overwrite behaviour for the `None` case (turn.rs:9416).
    #[test]
    fn assign_semantic_plan_preserving_exhausted_none_clears_slot_only() {
        let report =
            multi_cluster_report_fixture(VerifierDiagnosticFailureKind::AssertionMismatch, 2);
        let cluster_ids: Vec<_> = report
            .failure_clusters
            .iter()
            .map(|c| c.cluster_key.clone())
            .collect();
        let role = report.preferred_repair_role;

        let mut job = job_with_first_cluster_plan(&report);
        job.exhausted_attempts = vec![(cluster_ids[0].clone(), role)];

        assign_semantic_plan_preserving_exhausted(&mut job, None, None);

        assert!(job.semantic_plan.is_none(), "None must clear the slot");
        // Ledger preserved verbatim — legacy fallback never mutates it.
        assert_eq!(job.exhausted_attempts, vec![(cluster_ids[0].clone(), role)]);
    }

    /// MF2 V3.1: when the new plan's `(cluster_id, role)` pair is NOT in the
    /// ledger, the helper writes the plan verbatim. No ledger mutation.
    #[test]
    fn assign_semantic_plan_preserving_exhausted_writes_unexhausted_plan_verbatim() {
        let report =
            multi_cluster_report_fixture(VerifierDiagnosticFailureKind::AssertionMismatch, 2);
        let cluster_ids: Vec<_> = report
            .failure_clusters
            .iter()
            .map(|c| c.cluster_key.clone())
            .collect();
        let role = report.preferred_repair_role;

        // Job state: no plan yet, empty ledger.
        let mut job = RepairJob::new_for_test();
        let new_plan = SemanticRepairPlan {
            semantic_report: report.clone(),
            failure_cluster_id: cluster_ids[0].clone(),
            semantic_cause: report.failure_kind,
            spec_authority: SpecAuthority::ImplementationContract,
            preferred_repair_role: role,
            repair_hypothesis: report.repair_hypothesis.clone(),
            expected_improvement: None,
        };

        assign_semantic_plan_preserving_exhausted(&mut job, Some(new_plan), Some(&report));

        assert_eq!(
            job.semantic_plan.as_ref().unwrap().failure_cluster_id,
            cluster_ids[0],
            "unexhausted plan must be written verbatim",
        );
        assert!(job.exhausted_attempts.is_empty(), "ledger untouched");
    }

    /// MF2 V3.1 (core): when the new plan targets an already-exhausted
    /// `(cluster_id, role)` pair, the helper walks to the next unexhausted
    /// cluster instead of overwriting the slot with the doomed plan.
    /// This is the SSOT defence against the turn.rs:9416 regression where
    /// re-diagnostic could revive an already-burned cluster.
    #[test]
    fn assign_semantic_plan_preserving_exhausted_skips_to_next_when_proposed_is_exhausted() {
        let report =
            multi_cluster_report_fixture(VerifierDiagnosticFailureKind::AssertionMismatch, 2);
        let cluster_ids: Vec<_> = report
            .failure_clusters
            .iter()
            .map(|c| c.cluster_key.clone())
            .collect();
        let role = report.preferred_repair_role;

        // Ledger already contains cluster A (we burned it on a prior turn).
        let mut job = RepairJob {
            exhausted_attempts: vec![(cluster_ids[0].clone(), role)],
            ..RepairJob::new_for_test()
        };
        // Re-diagnostic proposes cluster A again (LLM duplicated the cluster
        // it can no longer fix).
        let doomed_plan = SemanticRepairPlan {
            semantic_report: report.clone(),
            failure_cluster_id: cluster_ids[0].clone(),
            semantic_cause: report.failure_kind,
            spec_authority: SpecAuthority::ImplementationContract,
            preferred_repair_role: role,
            repair_hypothesis: report.repair_hypothesis.clone(),
            expected_improvement: None,
        };

        assign_semantic_plan_preserving_exhausted(&mut job, Some(doomed_plan), Some(&report));

        // Walked past cluster A → slot targets cluster B.
        assert_eq!(
            job.semantic_plan.as_ref().unwrap().failure_cluster_id,
            cluster_ids[1],
            "slot must walk past the already-exhausted cluster to the next cluster",
        );
    }

    /// MF2 V3.1: when ALL clusters in the new report are already exhausted,
    /// the helper clears the slot (`semantic_plan = None`) so the caller can
    /// fall back to the re-diagnostic / legacy path.
    #[test]
    fn assign_semantic_plan_preserving_exhausted_clears_slot_when_all_clusters_exhausted() {
        let report =
            multi_cluster_report_fixture(VerifierDiagnosticFailureKind::AssertionMismatch, 2);
        let cluster_ids: Vec<_> = report
            .failure_clusters
            .iter()
            .map(|c| c.cluster_key.clone())
            .collect();
        let role = report.preferred_repair_role;

        // Ledger already contains BOTH clusters (the whole report is burnt).
        let mut job = RepairJob {
            exhausted_attempts: vec![
                (cluster_ids[0].clone(), role),
                (cluster_ids[1].clone(), role),
            ],
            ..RepairJob::new_for_test()
        };
        let any_plan = SemanticRepairPlan {
            semantic_report: report.clone(),
            failure_cluster_id: cluster_ids[0].clone(),
            semantic_cause: report.failure_kind,
            spec_authority: SpecAuthority::ImplementationContract,
            preferred_repair_role: role,
            repair_hypothesis: report.repair_hypothesis.clone(),
            expected_improvement: None,
        };

        assign_semantic_plan_preserving_exhausted(&mut job, Some(any_plan), Some(&report));

        assert!(
            job.semantic_plan.is_none(),
            "fully exhausted report must clear the slot (caller falls back to re-diagnostic)",
        );
    }

    /// MF2 V3.1: helper does NOT touch legacy retry counters
    /// (`assessment_attempts`, `repair_attempt`). Mirrors the S7-005 boundary
    /// on `apply_semantic_repair_dispatch_after_rerun`.
    #[test]
    fn assign_semantic_plan_preserving_exhausted_never_bumps_retry_counters() {
        let report =
            multi_cluster_report_fixture(VerifierDiagnosticFailureKind::AssertionMismatch, 2);
        let cluster_ids: Vec<_> = report
            .failure_clusters
            .iter()
            .map(|c| c.cluster_key.clone())
            .collect();
        let role = report.preferred_repair_role;

        let mut job = RepairJob {
            exhausted_attempts: vec![(cluster_ids[0].clone(), role)],
            assessment_attempts: 1,
            repair_attempt: 2,
            ..RepairJob::new_for_test()
        };
        let doomed = SemanticRepairPlan {
            semantic_report: report.clone(),
            failure_cluster_id: cluster_ids[0].clone(),
            semantic_cause: report.failure_kind,
            spec_authority: SpecAuthority::ImplementationContract,
            preferred_repair_role: role,
            repair_hypothesis: report.repair_hypothesis.clone(),
            expected_improvement: None,
        };

        assign_semantic_plan_preserving_exhausted(&mut job, Some(doomed), Some(&report));

        // Counters untouched even though we walked clusters.
        assert_eq!(job.assessment_attempts, 1);
        assert_eq!(job.repair_attempt, 2);
    }

    /// MF2 V3.4 (production integration scenario #1):
    /// `mf2_v3_cluster_a_exhausted_after_no_progress_leads_to_cluster_b`.
    ///
    /// Simulates the failing production flow end-to-end at the helper layer:
    ///   1. Diagnostic produces a plan targeting cluster A.
    ///   2. Repair runs; rerun returns `SameFailureRemaining` (no progress).
    ///   3. `apply_semantic_repair_dispatch_after_rerun` pushes A into the
    ///      ledger and advances the slot to cluster B (legacy behaviour).
    ///   4. The next turn's diagnostic re-proposes cluster A (the LLM has no
    ///      memory of the previous turn) → without MF2 V3.1 this would
    ///      overwrite cluster B with cluster A and burn the entire
    ///      previous-turn progress.
    ///   5. `assign_semantic_plan_preserving_exhausted` recognises cluster A
    ///      is exhausted and walks back to cluster B — progress preserved.
    #[test]
    fn mf2_v3_cluster_a_exhausted_after_no_progress_leads_to_cluster_b() {
        let report =
            multi_cluster_report_fixture(VerifierDiagnosticFailureKind::AssertionMismatch, 2);
        let cluster_ids: Vec<_> = report
            .failure_clusters
            .iter()
            .map(|c| c.cluster_key.clone())
            .collect();
        let role = report.preferred_repair_role;

        // (1) Initial diagnostic → plan targets cluster A.
        let mut job = job_with_first_cluster_plan(&report);
        let preserved_assessment = super::super::VerifierRepairAssessment {
            failure_kind: VerifierDiagnosticFailureKind::AssertionMismatch,
            failure_type: VerifierFailureType::Unknown,
            probable_cause_role: Some(role),
            needed_reads: Vec::new(),
            repair_target_hint: None,
            repair_plan: Vec::new(),
            summary: None,
            source: super::super::VerifierRepairAssessmentSource::DiagnosticPass,
        };
        job.assessment = Some(preserved_assessment.clone());

        // (2/3) Repair → rerun SameFailureRemaining → dispatch advances to B.
        job.rerun_outcome = Some(VerifierRepairRerunOutcome::SameFailureRemaining);
        apply_semantic_repair_dispatch_after_rerun(&mut job, Some(&cluster_ids[0]));
        assert_eq!(
            job.semantic_plan.as_ref().unwrap().failure_cluster_id,
            cluster_ids[1],
            "dispatch must advance to cluster B after no-progress rerun",
        );
        assert_eq!(
            job.exhausted_attempts,
            vec![(cluster_ids[0].clone(), role)],
            "ledger must record cluster A as exhausted",
        );

        // (4) New diagnostic produces a fresh plan that targets cluster A
        // again (LLM doesn't remember cluster A is doomed).
        let re_diagnostic_plan = SemanticRepairPlan {
            semantic_report: report.clone(),
            failure_cluster_id: cluster_ids[0].clone(),
            semantic_cause: report.failure_kind,
            spec_authority: SpecAuthority::ImplementationContract,
            preferred_repair_role: role,
            repair_hypothesis: report.repair_hypothesis.clone(),
            expected_improvement: None,
        };

        // (5) MF2 V3.1 router → must skip A and land on B.
        assign_semantic_plan_preserving_exhausted(
            &mut job,
            Some(re_diagnostic_plan),
            Some(&report),
        );
        assert_eq!(
            job.semantic_plan.as_ref().unwrap().failure_cluster_id,
            cluster_ids[1],
            "MF2 V3.1 must preserve cluster-B progress when re-diagnostic re-proposes cluster A",
        );
        // Ledger still records A as exhausted.
        assert!(
            job.exhausted_attempts
                .contains(&(cluster_ids[0].clone(), role)),
            "cluster A must remain in exhausted_attempts ledger after re-diagnostic",
        );
    }

    /// MF2 V3.4 (production integration scenario #2):
    /// `mf2_v3_exhausted_a_persists_after_re_diagnostic`.
    ///
    /// Stronger variant: after several re-diagnostic rounds that all
    /// re-propose cluster A, the exhausted-A entry must persist and the
    /// slot must continue to advance to cluster B (never regressing).
    #[test]
    fn mf2_v3_exhausted_a_persists_after_re_diagnostic() {
        let report =
            multi_cluster_report_fixture(VerifierDiagnosticFailureKind::AssertionMismatch, 2);
        let cluster_ids: Vec<_> = report
            .failure_clusters
            .iter()
            .map(|c| c.cluster_key.clone())
            .collect();
        let role = report.preferred_repair_role;

        // Job state mirrors the snapshot after dispatch advanced once:
        // cluster A in ledger, cluster B in slot.
        let plan_b = SemanticRepairPlan {
            semantic_report: report.clone(),
            failure_cluster_id: cluster_ids[1].clone(),
            semantic_cause: report.failure_kind,
            spec_authority: SpecAuthority::ImplementationContract,
            preferred_repair_role: role,
            repair_hypothesis: report.repair_hypothesis.clone(),
            expected_improvement: None,
        };
        let mut job = RepairJob {
            semantic_plan: Some(plan_b),
            exhausted_attempts: vec![(cluster_ids[0].clone(), role)],
            ..RepairJob::new_for_test()
        };

        // Re-diagnostic loop: 3 successive calls all propose cluster A.
        for _ in 0..3 {
            let re_diag_plan = SemanticRepairPlan {
                semantic_report: report.clone(),
                failure_cluster_id: cluster_ids[0].clone(),
                semantic_cause: report.failure_kind,
                spec_authority: SpecAuthority::ImplementationContract,
                preferred_repair_role: role,
                repair_hypothesis: report.repair_hypothesis.clone(),
                expected_improvement: None,
            };
            assign_semantic_plan_preserving_exhausted(&mut job, Some(re_diag_plan), Some(&report));
            // After every router call, A remains exhausted; B is the slot.
            assert!(
                job.exhausted_attempts
                    .contains(&(cluster_ids[0].clone(), role)),
                "cluster A must persist in exhausted_attempts ledger across re-diagnostics",
            );
            assert_eq!(
                job.semantic_plan.as_ref().unwrap().failure_cluster_id,
                cluster_ids[1],
                "slot must remain on cluster B (never regress to A)",
            );
        }
    }

    // ---- Issue #647 (CB-009): advance_to_next_cluster carries forward SpecAuthority ---- //

    /// CB-009.3 (regression #1): when the current `SemanticRepairPlan` was
    /// elected with `SpecAuthority::UserRequest` (e.g. the user explicitly
    /// described the contract), advancing to the next cluster of the same
    /// report must **preserve** the `UserRequest` authority — only the
    /// `failure_cluster_id` and per-cluster fields change. The previous
    /// implementation re-ran `select_authority(&[Impl, LlmGenTest], None)`
    /// and silently demoted `UserRequest` → `ImplementationContract`
    /// (Codex CB-009).
    #[test]
    fn cb009_advance_to_next_cluster_carries_forward_user_request_authority() {
        let report =
            multi_cluster_report_fixture(VerifierDiagnosticFailureKind::AssertionMismatch, 2);
        let cluster_ids: Vec<_> = report
            .failure_clusters
            .iter()
            .map(|c| c.cluster_key.clone())
            .collect();
        let role = report.preferred_repair_role;

        // Initial plan: cluster A under UserRequest authority.
        let plan = SemanticRepairPlan {
            semantic_report: report.clone(),
            failure_cluster_id: cluster_ids[0].clone(),
            semantic_cause: report.failure_kind,
            spec_authority: SpecAuthority::UserRequest,
            preferred_repair_role: role,
            repair_hypothesis: report.repair_hypothesis.clone(),
            expected_improvement: None,
        };
        let mut job = RepairJob {
            semantic_plan: Some(plan),
            ..RepairJob::new_for_test()
        };

        assert!(advance_to_next_cluster(&mut job, &report));

        let advanced = job.semantic_plan.as_ref().expect("slot now holds plan B");
        assert_eq!(
            advanced.failure_cluster_id, cluster_ids[1],
            "advance must move the slot to the next cluster",
        );
        assert_eq!(
            advanced.spec_authority,
            SpecAuthority::UserRequest,
            "CB-009: UserRequest authority must be carried forward across slot reuse",
        );
    }

    /// CB-009.3 (regression #2): same invariant for `SpecAuthority::BehaviorContract`
    /// — when the initial plan was elected via the deterministic
    /// `RequiredBehaviorContract`, that authority must survive cluster
    /// transitions intact (no demotion to ImplementationContract).
    #[test]
    fn cb009_advance_to_next_cluster_carries_forward_behavior_contract() {
        let report =
            multi_cluster_report_fixture(VerifierDiagnosticFailureKind::AssertionMismatch, 2);
        let cluster_ids: Vec<_> = report
            .failure_clusters
            .iter()
            .map(|c| c.cluster_key.clone())
            .collect();
        let role = report.preferred_repair_role;

        let plan = SemanticRepairPlan {
            semantic_report: report.clone(),
            failure_cluster_id: cluster_ids[0].clone(),
            semantic_cause: report.failure_kind,
            spec_authority: SpecAuthority::BehaviorContract,
            preferred_repair_role: role,
            repair_hypothesis: report.repair_hypothesis.clone(),
            expected_improvement: None,
        };
        let mut job = RepairJob {
            semantic_plan: Some(plan),
            ..RepairJob::new_for_test()
        };

        assert!(advance_to_next_cluster(&mut job, &report));

        let advanced = job.semantic_plan.as_ref().expect("slot now holds plan B");
        assert_eq!(advanced.failure_cluster_id, cluster_ids[1]);
        assert_eq!(
            advanced.spec_authority,
            SpecAuthority::BehaviorContract,
            "CB-009: BehaviorContract authority must be carried forward across slot reuse",
        );
    }

    /// CB-009.3 (regression #3): when the initial plan's authority was
    /// elected via consensus (here represented by a `BehaviorContract`
    /// outcome — the test/usage-docs vs impl tie-break path in
    /// `consensus_to_authority_for_resolve`, see CB-008), advancing to the
    /// next cluster must keep that consensus-decided authority intact —
    /// `advance_to_next_cluster` is **not** allowed to re-derive a new
    /// authority from a fixed candidate base, because the candidate base
    /// does not include `BehaviorContract` and would silently overwrite it.
    #[test]
    fn cb009_advance_to_next_cluster_carries_forward_consensus_decided_authority() {
        let report =
            multi_cluster_report_fixture(VerifierDiagnosticFailureKind::AssertionMismatch, 3);
        let cluster_ids: Vec<_> = report
            .failure_clusters
            .iter()
            .map(|c| c.cluster_key.clone())
            .collect();
        let role = report.preferred_repair_role;

        // Plan elected via consensus → BehaviorContract on cluster A.
        let plan = SemanticRepairPlan {
            semantic_report: report.clone(),
            failure_cluster_id: cluster_ids[0].clone(),
            semantic_cause: report.failure_kind,
            spec_authority: SpecAuthority::BehaviorContract,
            preferred_repair_role: role,
            repair_hypothesis: report.repair_hypothesis.clone(),
            expected_improvement: None,
        };
        let mut job = RepairJob {
            semantic_plan: Some(plan),
            ..RepairJob::new_for_test()
        };

        // First advance: A → B. Authority must remain BehaviorContract.
        assert!(advance_to_next_cluster(&mut job, &report));
        assert_eq!(
            job.semantic_plan.as_ref().unwrap().failure_cluster_id,
            cluster_ids[1],
        );
        assert_eq!(
            job.semantic_plan.as_ref().unwrap().spec_authority,
            SpecAuthority::BehaviorContract,
            "CB-009: consensus-decided BehaviorContract must persist after first advance",
        );

        // Second advance: B → C. Authority must STILL remain BehaviorContract
        // (carry-forward survives chained slot reuse).
        assert!(advance_to_next_cluster(&mut job, &report));
        assert_eq!(
            job.semantic_plan.as_ref().unwrap().failure_cluster_id,
            cluster_ids[2],
        );
        assert_eq!(
            job.semantic_plan.as_ref().unwrap().spec_authority,
            SpecAuthority::BehaviorContract,
            "CB-009: consensus-decided BehaviorContract must persist across chained advances",
        );
    }

    /// Issue #647 (CB-013): `has_stale_assessment_after_cluster_advance`
    /// must return `true` exactly when:
    ///   1. `assessment.is_some()`
    ///   2. `semantic_plan.is_some()`
    ///   3. `!exhausted_attempts.is_empty()`
    ///   4. `verifier_repair_context_target_path` returns `None` (the
    ///      CB-007 guard fires)
    ///
    /// This is the same predicate the CB-012 decision-layer guard uses.
    /// `run_verifier_diagnostic_pass` consults this helper to clear the
    /// stale assessment before its own `assessment.is_some()` Skipped
    /// short-circuit fires (without the clear, CB-012's NeedDiagnostic
    /// would be neutralized at the production layer).
    #[test]
    fn cb013_has_stale_assessment_after_cluster_advance_fires_for_stale_state() {
        use tempfile::tempdir;
        let temp = tempdir().unwrap();
        let work_root = temp.path().to_path_buf();

        let report =
            multi_cluster_report_fixture(VerifierDiagnosticFailureKind::AssertionMismatch, 2);
        let cluster_ids: Vec<_> = report
            .failure_clusters
            .iter()
            .map(|c| c.cluster_key.clone())
            .collect();
        let role = report.preferred_repair_role;

        // Job state: active semantic_plan + non-empty exhausted_attempts +
        // assessment Some. No work_root file exists, and the CB-007 guard
        // in `verifier_repair_effective_target_hint` returns None whenever
        // `semantic_plan.is_some() && !exhausted_attempts.is_empty()` —
        // so `verifier_repair_context_target_path` returns None too.
        let plan = SemanticRepairPlan {
            semantic_report: report.clone(),
            failure_cluster_id: cluster_ids[1].clone(),
            semantic_cause: report.failure_kind,
            spec_authority: SpecAuthority::ImplementationContract,
            preferred_repair_role: role,
            repair_hypothesis: report.repair_hypothesis.clone(),
            expected_improvement: None,
        };
        let stale_hint = RecoveryTargetHint {
            role: super::super::task_contract::ArtifactRole::Implementation,
            path: "app/stale.py".to_string(),
            reason: "stale cluster A".to_string(),
        };
        let job = RepairJob {
            semantic_plan: Some(plan),
            exhausted_attempts: vec![(cluster_ids[0].clone(), role)],
            assessment: Some(super::super::VerifierRepairAssessment {
                failure_kind: VerifierDiagnosticFailureKind::AssertionMismatch,
                failure_type: VerifierFailureType::Unknown,
                probable_cause_role: Some(role),
                needed_reads: Vec::new(),
                repair_target_hint: Some(stale_hint.clone()),
                repair_plan: vec![stale_hint],
                summary: None,
                source: super::super::VerifierRepairAssessmentSource::DiagnosticPass,
            }),
            ..RepairJob::new_for_test()
        };

        assert!(
            has_stale_assessment_after_cluster_advance(&job, &work_root),
            "CB-013: stale state (advanced plan + non-empty exhausted_attempts + Some assessment) must be detected",
        );
    }

    /// Issue #647 (CB-013): the helper must NOT fire when
    /// `exhausted_attempts` is empty — fresh semantic_plan flow is
    /// unaffected so the Skipped short-circuit keeps its normal behavior.
    #[test]
    fn cb013_has_stale_assessment_does_not_fire_when_no_exhausted_attempts() {
        use tempfile::tempdir;
        let temp = tempdir().unwrap();
        let work_root = temp.path().to_path_buf();

        let report =
            multi_cluster_report_fixture(VerifierDiagnosticFailureKind::AssertionMismatch, 2);
        let role = report.preferred_repair_role;
        let cluster_ids: Vec<_> = report
            .failure_clusters
            .iter()
            .map(|c| c.cluster_key.clone())
            .collect();
        let plan = SemanticRepairPlan {
            semantic_report: report.clone(),
            failure_cluster_id: cluster_ids[0].clone(),
            semantic_cause: report.failure_kind,
            spec_authority: SpecAuthority::ImplementationContract,
            preferred_repair_role: role,
            repair_hypothesis: report.repair_hypothesis.clone(),
            expected_improvement: None,
        };
        let job = RepairJob {
            semantic_plan: Some(plan),
            // exhausted_attempts empty — CB-013 must NOT engage.
            assessment: Some(super::super::VerifierRepairAssessment {
                failure_kind: VerifierDiagnosticFailureKind::AssertionMismatch,
                failure_type: VerifierFailureType::Unknown,
                probable_cause_role: Some(role),
                needed_reads: Vec::new(),
                repair_target_hint: None,
                repair_plan: Vec::new(),
                summary: None,
                source: super::super::VerifierRepairAssessmentSource::DiagnosticPass,
            }),
            ..RepairJob::new_for_test()
        };
        assert!(
            !has_stale_assessment_after_cluster_advance(&job, &work_root),
            "CB-013: fresh state (empty exhausted_attempts) must NOT engage the stale-clear helper",
        );
    }

    /// Issue #647 (CB-013): the helper must NOT fire when `assessment` is
    /// already None — there is nothing to clear, and the diagnostic
    /// runner's normal path already handles this case.
    #[test]
    fn cb013_has_stale_assessment_does_not_fire_when_assessment_already_none() {
        use tempfile::tempdir;
        let temp = tempdir().unwrap();
        let work_root = temp.path().to_path_buf();

        let report =
            multi_cluster_report_fixture(VerifierDiagnosticFailureKind::AssertionMismatch, 2);
        let role = report.preferred_repair_role;
        let cluster_ids: Vec<_> = report
            .failure_clusters
            .iter()
            .map(|c| c.cluster_key.clone())
            .collect();
        let plan = SemanticRepairPlan {
            semantic_report: report.clone(),
            failure_cluster_id: cluster_ids[1].clone(),
            semantic_cause: report.failure_kind,
            spec_authority: SpecAuthority::ImplementationContract,
            preferred_repair_role: role,
            repair_hypothesis: report.repair_hypothesis.clone(),
            expected_improvement: None,
        };
        let job = RepairJob {
            semantic_plan: Some(plan),
            exhausted_attempts: vec![(cluster_ids[0].clone(), role)],
            assessment: None,
            ..RepairJob::new_for_test()
        };
        assert!(
            !has_stale_assessment_after_cluster_advance(&job, &work_root),
            "CB-013: assessment already None → helper must not fire",
        );
    }

    /// Issue #647 (CB-013): the helper must NOT fire when no
    /// `semantic_plan` is active — the legacy / SetupRepair path keeps its
    /// existing Skipped short-circuit behavior.
    #[test]
    fn cb013_has_stale_assessment_does_not_fire_when_no_semantic_plan() {
        use tempfile::tempdir;
        let temp = tempdir().unwrap();
        let work_root = temp.path().to_path_buf();

        let job = RepairJob {
            semantic_plan: None,
            // Even with an assessment present, the legacy / SetupRepair
            // path must keep its existing Skipped short-circuit behavior.
            assessment: Some(super::super::VerifierRepairAssessment {
                failure_kind: VerifierDiagnosticFailureKind::AssertionMismatch,
                failure_type: VerifierFailureType::Unknown,
                probable_cause_role: None,
                needed_reads: Vec::new(),
                repair_target_hint: None,
                repair_plan: Vec::new(),
                summary: None,
                source: super::super::VerifierRepairAssessmentSource::DiagnosticPass,
            }),
            ..RepairJob::new_for_test()
        };
        assert!(
            !has_stale_assessment_after_cluster_advance(&job, &work_root),
            "CB-013: semantic_plan None → helper must not fire",
        );
    }

    // -- Phase G grep / structure tests (Issue #647 acceptance closure) -- //

    /// Helper: split the module source into the production prefix
    /// (everything before `#[cfg(test)]\nmod tests` at column 0).
    fn production_source() -> &'static str {
        let source = include_str!("repair_job.rs");
        match source.find("\n#[cfg(test)]\nmod tests") {
            Some(idx) => &source[..idx],
            None => source,
        }
    }

    #[test]
    fn no_framework_literal_in_repair_job_production_code() {
        // S1-012 (拡張): production code (everything before `mod tests`)
        // must not embed framework-specific literals. Test-only literals
        // (e.g. `command: "pytest"` inside #[cfg(test)] fixtures) are
        // explicitly exempt.
        let prod = production_source();
        for lit in &[
            "\"422\"",
            "\"404\"",
            "/items/nonexistent",
            "FastAPI",
            "\"pytest\"",
        ] {
            assert!(
                !prod.contains(lit),
                "repair_job.rs production code must not contain framework literal {lit:?}",
            );
        }
    }

    #[test]
    fn no_unsafe_in_repair_job_production_code() {
        // DR4-003: no unsafe / FFI in new production code.
        let prod = production_source();
        assert!(
            !prod.contains("unsafe "),
            "repair_job.rs production code must not contain `unsafe `",
        );
        assert!(
            !prod.contains("extern \"C\""),
            "repair_job.rs production code must not declare FFI",
        );
    }

    #[test]
    fn session_store_does_not_serialize_semantic_failure_report() {
        // S1-011 / S3-006 / DR3-002: SemanticFailureReport &
        // FailureCluster are turn-local — they must not appear in the
        // session persistence layer. If a future Issue moves them into
        // `session::store`, this test forces an explicit review of the
        // serialization + masking SSOT path.
        let store_path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/session/store.rs");
        let body = std::fs::read_to_string(&store_path)
            .unwrap_or_else(|e| panic!("read {store_path:?}: {e}"));
        for forbidden in &[
            "SemanticFailureReport",
            "FailureCluster",
            "FailureClusterKey",
            "SpecAuthority",
            "SemanticRepairPlan",
        ] {
            assert!(
                !body.contains(forbidden),
                "src/session/store.rs must not reference turn-local type {forbidden:?} \
                 (S1-011 / S3-006: SemanticFailureReport is turn-local)",
            );
        }
    }
}
