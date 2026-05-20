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
}
