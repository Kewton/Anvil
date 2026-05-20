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
/// LLM emits one role per report). `spec_authority` is re-selected via
/// `select_authority` so the same Phase-D fallback set
/// (`ImplementationContract` + `LlmGeneratedTest`) governs the new plan
/// deterministically.
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
    //    we drop it, so slot reuse preserves history (S3-010).
    if let Some(current_plan) = repair_job.semantic_plan.as_ref() {
        let entry = (
            current_plan.failure_cluster_id.clone(),
            current_plan.preferred_repair_role,
        );
        if !repair_job.exhausted_attempts.contains(&entry) {
            repair_job.exhausted_attempts.push(entry);
        }
    }

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

    // 4. Re-pick the spec authority for the new cluster using the same
    //    Phase-D fallback candidate set so the deterministic behaviour is
    //    preserved across slot reuse.
    let candidates = [
        SpecAuthority::ImplementationContract,
        SpecAuthority::LlmGeneratedTest,
    ];
    let Some(spec_authority) = super::spec_authority::select_authority(&candidates, None) else {
        // select_authority on a non-empty filtered candidate list cannot
        // return None today, but if a future variant change makes it
        // possible, fail closed (no plan).
        repair_job.semantic_plan = None;
        return false;
    };

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
}
