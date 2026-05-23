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

use super::repair_attempt_outcome::{
    MAX_REPAIR_ATTEMPT_OUTCOMES, RepairAttemptOutcome, RepairAttemptOutcomeKind,
    RepairRejectionKind, should_promote_to_exhausted_after_push,
};
use super::semantic_failure::{FailureClusterKey, SemanticFailureReport};
use super::spec_authority::{RepairRole, SpecAuthority, WeakeningPattern};
use super::task_contract::RecoveryTargetHint;
use super::{
    VerifierDiagnosticFailureKind, VerifierFailureType, VerifierRepairAssessment,
    VerifierRepairRerunOutcome,
};
use crate::session::store::ConversationMessage;

/// Maximum byte length retained for sanitized snapshot text fields. Consumed
/// by `truncate_for_snapshot` and the `failure_snapshot` production path (Issue #638).
pub(super) const SNAPSHOT_FIELD_BYTE_CAP: usize = 4096;

/// v0.4.10: bounded target-path lifecycle ledger cap. This ledger is
/// turn-local / in-memory like `repair_attempt_outcomes`; it stores only
/// closed enum buckets and admitted relative paths, never raw patch text.
const MAX_REPAIR_TARGET_ATTEMPTS: usize = 24;
const APPLIED_IMPROVED_TARGET_EXHAUSTION_THRESHOLD: usize = 3;

/// v0.4.10: target-local bucket used to decide when a selected repair target
/// should be skipped for the active cluster. This is deliberately separate
/// from `RepairAttemptOutcomeKind`: target exhaustion needs the exact unsafe
/// weakening pattern, while the cluster-level promotion path only needs the
/// coarse outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RepairTargetAttemptBucket {
    Unsafe {
        rejection: RepairRejectionKind,
        pattern: WeakeningPattern,
    },
    Malformed,
    Noop,
    Duplicate,
    ImprovedStillFailing,
    NoProgress,
    Worsened,
}

impl RepairTargetAttemptBucket {
    fn from_outcome_kind(kind: &RepairAttemptOutcomeKind) -> Option<Self> {
        match kind {
            RepairAttemptOutcomeKind::RejectedUnsafe { rejection, pattern } => Some(Self::Unsafe {
                rejection: *rejection,
                pattern: *pattern,
            }),
            RepairAttemptOutcomeKind::RejectedMalformed => Some(Self::Malformed),
            RepairAttemptOutcomeKind::RejectedNoop => Some(Self::Noop),
            RepairAttemptOutcomeKind::RejectedDuplicate => Some(Self::Duplicate),
            RepairAttemptOutcomeKind::AppliedNoProgress => Some(Self::NoProgress),
            RepairAttemptOutcomeKind::AppliedWorsened => Some(Self::Worsened),
            RepairAttemptOutcomeKind::AppliedImproved => Some(Self::ImprovedStillFailing),
            RepairAttemptOutcomeKind::RejectedNoCandidate => None,
        }
    }

    fn exhaustion_threshold(self) -> usize {
        match self {
            Self::ImprovedStillFailing => APPLIED_IMPROVED_TARGET_EXHAUSTION_THRESHOLD,
            Self::Unsafe { .. }
            | Self::Malformed
            | Self::Noop
            | Self::Duplicate
            | Self::NoProgress
            | Self::Worsened => 2,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RepairTargetAttemptOutcome {
    pub(super) cluster: FailureClusterKey,
    pub(super) role: RepairRole,
    pub(super) path: String,
    pub(super) bucket: RepairTargetAttemptBucket,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ExhaustedRepairTarget {
    pub(super) cluster: FailureClusterKey,
    pub(super) role: RepairRole,
    pub(super) path: String,
    pub(super) bucket: RepairTargetAttemptBucket,
}

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
    /// Issue #647 (CB-015): `RepairJob.assessment_generation` の値を、
    /// この `SemanticRepairPlan` が構築された **時点** で coil した snapshot。
    ///
    /// re-diagnostic が完了して新しい assessment が書き込まれると
    /// `RepairJob.assessment_generation` は `+1` される — それより前に
    /// 作られた plan は `plan.assessment_generation_at_creation <
    /// repair_job.assessment_generation` となり、stale ではなく
    /// **fresh** assessment と組み合わさった "post re-diagnostic" 状態
    /// として区別される (CB-007 / CB-012 / CB-013 / CB-014 ガードは
    /// この比較で stale を判定する)。
    pub(super) assessment_generation_at_creation: u32,
}

impl SemanticRepairPlan {
    /// CB-017 A''': locate the `FailureCluster` matching this plan's
    /// `failure_cluster_id` inside the embedded `semantic_report`.
    ///
    /// Returns `None` when no cluster with that id is present (defensive —
    /// production code always seeds the plan from a cluster that already
    /// lives in the report). This SSOT lookup keeps the cluster-key match
    /// in one place so `rebind_legacy_assessment_to_current_cluster` and
    /// any future consumer agree on the predicate.
    pub(super) fn current_cluster(&self) -> Option<&super::semantic_failure::FailureCluster> {
        self.semantic_report
            .failure_clusters
            .iter()
            .find(|c| c.cluster_key == self.failure_cluster_id)
    }

    /// CB-017 A''' (CR-7 V2): the admitted, Owned-validated targets for the
    /// plan's current cluster. Returns an empty slice when the cluster is
    /// not found or when the cluster has no admitted targets (targetless
    /// cluster — caller MUST skip rebinding).
    pub(super) fn current_cluster_targets(&self) -> &[RecoveryTargetHint] {
        self.current_cluster()
            .map(|c| c.admitted_cluster_targets.as_slice())
            .unwrap_or(&[])
    }
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
    /// Issue #647 (CB-015): "assessment は何代目か" を first-class state に
    /// した generation counter。`run_verifier_diagnostic_pass` が新 assessment
    /// を書き込むたびに `+= 1`、`verifier_repair_context_from_failure` は
    /// previous_context から carry over する。
    ///
    /// `SemanticRepairPlan.assessment_generation_at_creation` と比較する
    /// ことで CB-007/CB-012/CB-013/CB-014 のガードは「plan は新 assessment
    /// より古い (stale)」か「plan は新 assessment と同じ世代 (まだ re-diagnostic
    /// が走っていない / advance 直後の stale state)」かを区別する。
    pub(super) assessment_generation: u32,
    /// CB-017 A''': cluster_key-based transition detection.
    /// `rebind_legacy_assessment_to_current_cluster` でのみ書き換えられ、
    /// `cluster_key` の差分で `applied_repair_intents.clear()` を判定する。
    ///
    /// lifecycle:
    /// - 初期値 `None` (新規 assessment / legacy fallback)
    /// - `rebind_legacy_assessment_to_current_cluster` で
    ///   `Some(plan.failure_cluster_id.clone())` に更新
    /// - `verifier_repair_context_from_failure` で previous_context から
    ///   carry over
    /// - assessment が `None` に落ちる経路 (re-diagnostic 切替) で `None`
    ///   に reset
    ///
    /// turn-local field (session.messages には persist しない)。
    pub(super) assessment_bound_cluster_id: Option<FailureClusterKey>,
    /// Issue #653: per-job 累積 lifecycle ledger。
    /// FIFO bounded (cap = `MAX_REPAIR_ATTEMPT_OUTCOMES = 16`)、
    /// overflow は oldest drop + `tracing::warn!` metadata (S1-003)。
    /// session 永続化対象外 (`Serialize`/`Deserialize` 非付与、S3-004)。
    /// 同一 `(cluster, role, RejectedUnsafe { rejection })` が 2 回検出された
    /// 時点で `exhausted_attempts` にも昇格 push する (S1-006(a))。
    pub(super) repair_attempt_outcomes: Vec<RepairAttemptOutcome>,
    /// v0.4.10: target-path lifecycle ledger. Cluster-level
    /// `exhausted_attempts` is intentionally coarse; this ledger lets the
    /// controller stop retrying the same weakening / malformed / no-progress
    /// target while another admitted target for the same cluster may still
    /// be repairable.
    pub(super) repair_target_attempt_outcomes: Vec<RepairTargetAttemptOutcome>,
    /// v0.4.10: target paths that should no longer be selected for the
    /// corresponding `(cluster, role)`. This is in-memory only and carries
    /// bounded relative paths plus closed enum buckets.
    pub(super) exhausted_repair_targets: Vec<ExhaustedRepairTarget>,
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

/// Issue #662: structured return type for `record_repair_attempt_outcome`.
///
/// **Precondition** (enforced by caller via `record_repair_attempt_outcome`):
/// `RepairJob.semantic_plan = Some(_)` at the moment of the push. The
/// `debug_assert!` in `record_repair_attempt_outcome` will trip in debug
/// builds when the precondition is violated; release builds short-circuit
/// to `PromotionResult { promoted: false, all_clusters_exhausted: false }`.
///
/// **Postcondition** (fields):
/// - `promoted = true` when this push caused a new `(cluster, role)` to land
///   in `exhausted_attempts` (idempotent on duplicates — `contains`-guarded
///   inside `record_repair_attempt_outcome`).
/// - `all_clusters_exhausted = true` when, after the push, no cluster in
///   `semantic_plan.semantic_report` remains repairable under the plan's
///   `preferred_repair_role` (= `next_repairable_cluster` returns `None`).
///   When `semantic_plan = None` (release-build precondition violation),
///   this field is `false`.
///
/// **Caller contract** (`turn.rs::maybe_emit_repair_exhausted_from_promotion`,
/// the single chokepoint called from both production observation sites —
/// Applied path in `drive_task_contract_verifier` and Invalid path in
/// `record_controller_verifier_repair_invalid`):
/// observe `all_clusters_exhausted` and, if `true`, emit
/// `StopReason::RepairExhausted` via the existing `record_safe_stop_report`
/// SSOT. The Issue #654 `Agent.safe_stop_report_emitted: HashSet<StopReason>`
/// dedup makes the call safe to repeat from multiple caller sites in the
/// same turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct PromotionResult {
    pub(super) promoted: bool,
    pub(super) all_clusters_exhausted: bool,
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
            // Issue #654 / DR1-003: route through `safe_relative_path_string`
            // SSOT so the syntactic safety check (absolute / `..` / control
            // char / empty / non-UTF8) lives in one place. Behavior is
            // equivalent to the prior inline check (Codex CB-003 reflected).
            target_path: self
                .target_hint
                .as_ref()
                .and_then(|hint| safe_relative_path_string(hint.path.as_str()))
                .map(PathBuf::from),
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
        Self::empty_synthetic()
    }

    /// Issue #654 (E.2) — production-side minimal constructor used when an
    /// `artifact_completion_failed` emit point needs to surface a
    /// `SafeStopReport` without a verifier-driven `RepairJob`. All fields are
    /// empty / `None` / `Unknown`; the safe-stop builder fills in the
    /// `stop_reason` / `current_role` / `expected_target` from `SafeStopContext`
    /// so the synthetic shell never leaks misleading verifier metadata.
    ///
    /// Callers MUST set `target_hint` after construction if they want the
    /// `expected_target` field of the emitted payload to be populated.
    pub(super) fn empty_synthetic() -> Self {
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
            assessment_generation: 0,
            assessment_bound_cluster_id: None,
            repair_attempt_outcomes: Vec::new(),
            repair_target_attempt_outcomes: Vec::new(),
            exhausted_repair_targets: Vec::new(),
        }
    }

    /// Issue #653 (S7-003) / #662 (S5-003): ledger mutation SSOT (orchestration only)。
    ///
    /// 薄い orchestration として以下を担保する:
    /// 1. FIFO cap (`MAX_REPAIR_ATTEMPT_OUTCOMES = 16`) — 超過時は oldest drop
    ///    + `tracing::warn!` (件数 metadata のみ、内容は出さない)
    /// 2. `repair_attempt_outcomes` に push
    /// 3. `should_promote_to_exhausted_after_push` (pure-fn) を呼んで `Some` なら
    ///    `exhausted_attempts` に push (idempotent — `contains` チェック)
    /// 4. `assessment_generation` は **bump しない** (S3-005)
    /// 5. Issue #662: `PromotionResult { promoted, all_clusters_exhausted }`
    ///    を返す。caller (`turn.rs`) は `all_clusters_exhausted = true` 時に
    ///    `record_safe_stop_report(SafeStopInput::FromRepair { stop_reason:
    ///    StopReason::RepairExhausted, .. }, ctx)` を呼ぶ。
    ///
    /// **Precondition (Issue #662 DR1-001)**: `self.semantic_plan = Some` 経路
    /// に限定。legacy path (semantic_plan = None) では outcome を作らず本関数
    /// は呼ばれない。debug build では `debug_assert!` で gate、release build
    /// では `all_clusters_exhausted = false` を返すフォールバック。
    ///
    /// 昇格判定本体は pure-fn 側 (`repair_attempt_outcome::should_promote_to_exhausted_after_push`)
    /// にあるため、ここではフロー制御のみ。
    pub(super) fn record_repair_attempt_outcome(
        &mut self,
        outcome: RepairAttemptOutcome,
    ) -> PromotionResult {
        debug_assert!(
            self.semantic_plan.is_some(),
            "record_repair_attempt_outcome must be called only when semantic_plan = Some \
             (Issue #662 5-3 precondition)"
        );

        // 1. FIFO cap → oldest drop + tracing::warn! (closed metadata, DR4-002)
        if self.repair_attempt_outcomes.len() >= MAX_REPAIR_ATTEMPT_OUTCOMES {
            self.repair_attempt_outcomes.remove(0);
            // Issue #653 DR3-005 / DR4-002: structured event with closed metadata
            // (event + count). cluster key / hypothesis / WeakeningPattern / LLM
            // text は payload に含めない。
            tracing::warn!(
                event = "agent.repair_attempt_outcomes.fifo_drop",
                count = MAX_REPAIR_ATTEMPT_OUTCOMES,
                "repair_attempt_outcomes FIFO drop: oldest entry evicted"
            );
        }

        // 2. push
        self.repair_attempt_outcomes.push(outcome.clone());

        // 3. pure-fn predicate で昇格判定 → 必要なら exhausted_attempts に push
        let mut promoted = false;
        if let Some(entry) =
            should_promote_to_exhausted_after_push(&self.repair_attempt_outcomes, &outcome)
            && !self.exhausted_attempts.contains(&entry)
        {
            self.exhausted_attempts.push(entry);
            promoted = true;
        }
        // 4. assessment_generation 不変 (no bump — S3-005)

        // 5. Issue #662: detect "all repairable clusters in the active plan are
        // now exhausted under their preferred role". `next_repairable_cluster`
        // returns `None` when no cluster remains repairable. legacy `None`
        // path (precondition violation in release build) returns false.
        let all_clusters_exhausted = match self.semantic_plan.as_ref() {
            Some(plan) => next_repairable_cluster(
                &plan.semantic_report,
                plan.preferred_repair_role,
                &self.exhausted_attempts,
            )
            .is_none(),
            None => false,
        };

        PromotionResult {
            promoted,
            all_clusters_exhausted,
        }
    }

    /// v0.4.10: record the same repair attempt at target-path granularity.
    ///
    /// This wraps the existing cluster-level ledger so old safe-stop behavior
    /// stays intact, then adds a second signal: if the same `(cluster, role,
    /// target path, bucket)` fails twice, that target path is marked
    /// exhausted. If every admitted target for the active cluster is now
    /// exhausted, the cluster-level `(cluster, role)` entry is promoted too.
    pub(super) fn record_repair_attempt_outcome_for_target(
        &mut self,
        outcome: RepairAttemptOutcome,
        target_hint: &RecoveryTargetHint,
    ) -> PromotionResult {
        let mut result = self.record_repair_attempt_outcome(outcome.clone());
        // The legacy cluster-level ledger promotes on repeated
        // `(cluster, role, bucket)` regardless of target path. In the
        // target-aware path that is too coarse: a second weakening proposal for
        // tests/a.py should not exhaust tests/b.py. If this call just caused
        // a cluster-level promotion but not every current target is exhausted
        // yet, downgrade the promotion and let the target-level rule below
        // re-promote only when all admitted targets are exhausted.
        let cluster_entry = (outcome.cluster.clone(), outcome.role);
        if result.promoted
            && !self.current_cluster_role_targets_all_exhausted(&outcome.cluster, outcome.role)
        {
            if let Some(pos) = self
                .exhausted_attempts
                .iter()
                .position(|entry| entry == &cluster_entry)
            {
                self.exhausted_attempts.remove(pos);
            }
            result.promoted = false;
        }
        let Some(bucket) = RepairTargetAttemptBucket::from_outcome_kind(&outcome.kind) else {
            result.all_clusters_exhausted = self.all_clusters_exhausted_for_active_plan();
            return result;
        };
        let target_path = target_hint.path.clone();
        self.push_repair_target_attempt(RepairTargetAttemptOutcome {
            cluster: outcome.cluster.clone(),
            role: outcome.role,
            path: target_path.clone(),
            bucket,
        });
        let repeated_same_target_bucket = self
            .repair_target_attempt_outcomes
            .iter()
            .filter(|candidate| {
                candidate.cluster == outcome.cluster
                    && candidate.role == outcome.role
                    && candidate.path == target_path
                    && candidate.bucket == bucket
            })
            .count()
            >= bucket.exhaustion_threshold();
        if repeated_same_target_bucket
            && !self.is_repair_target_exhausted(&outcome.cluster, outcome.role, &target_path)
        {
            self.exhausted_repair_targets.push(ExhaustedRepairTarget {
                cluster: outcome.cluster.clone(),
                role: outcome.role,
                path: target_path,
                bucket,
            });
            if self.promote_cluster_if_current_targets_exhausted(&outcome.cluster, outcome.role) {
                result.promoted = true;
            }
        }
        result.all_clusters_exhausted = self.all_clusters_exhausted_for_active_plan();
        result
    }

    fn push_repair_target_attempt(&mut self, attempt: RepairTargetAttemptOutcome) {
        if self.repair_target_attempt_outcomes.len() >= MAX_REPAIR_TARGET_ATTEMPTS {
            self.repair_target_attempt_outcomes.remove(0);
            tracing::warn!(
                event = "agent.repair_target_attempt_outcomes.fifo_drop",
                count = MAX_REPAIR_TARGET_ATTEMPTS,
                "repair_target_attempt_outcomes FIFO drop: oldest entry evicted"
            );
        }
        self.repair_target_attempt_outcomes.push(attempt);
    }

    pub(super) fn has_exhausted_repair_targets(&self) -> bool {
        !self.exhausted_repair_targets.is_empty()
    }

    pub(super) fn is_repair_target_exhausted(
        &self,
        cluster: &FailureClusterKey,
        role: RepairRole,
        path: &str,
    ) -> bool {
        self.exhausted_repair_targets
            .iter()
            .any(|target| &target.cluster == cluster && target.role == role && target.path == path)
    }

    pub(super) fn is_repair_hint_exhausted(&self, hint: &RecoveryTargetHint) -> bool {
        let Some(plan) = self.semantic_plan.as_ref() else {
            return false;
        };
        self.is_repair_target_exhausted(&plan.failure_cluster_id, hint.role, &hint.path)
    }

    /// Return the first admitted target for the active semantic cluster that
    /// has not been target-exhausted. This helper is intentionally only about
    /// semantic plans; legacy `assessment` fallback stays in `turn.rs`.
    pub(super) fn current_unexhausted_semantic_target(&self) -> Option<&RecoveryTargetHint> {
        let plan = self.semantic_plan.as_ref()?;
        plan.current_cluster_targets().iter().find(|target| {
            target.role == plan.preferred_repair_role
                && !self.is_repair_target_exhausted(
                    &plan.failure_cluster_id,
                    plan.preferred_repair_role,
                    &target.path,
                )
        })
    }

    /// True only when the active semantic cluster has at least one admitted
    /// target for its role and all such targets are exhausted.
    pub(super) fn current_semantic_targets_all_exhausted(&self) -> bool {
        let Some(plan) = self.semantic_plan.as_ref() else {
            return false;
        };
        self.current_cluster_role_targets_all_exhausted(
            &plan.failure_cluster_id,
            plan.preferred_repair_role,
        )
    }

    pub(super) fn needs_diagnostic_after_target_exhaustion(&self) -> bool {
        if !self.has_exhausted_repair_targets() {
            return false;
        }
        if self.current_semantic_targets_all_exhausted() {
            return true;
        }
        let Some(assessment) = self.assessment.as_ref() else {
            return false;
        };
        let current_hint = assessment
            .repair_plan
            .get(self.applied_repair_intents.len())
            .or(assessment.repair_target_hint.as_ref());
        current_hint.is_some_and(|hint| self.is_repair_hint_exhausted(hint))
    }

    fn promote_cluster_if_current_targets_exhausted(
        &mut self,
        cluster: &FailureClusterKey,
        role: RepairRole,
    ) -> bool {
        if !self.current_cluster_role_targets_all_exhausted(cluster, role) {
            return false;
        }
        let entry = (cluster.clone(), role);
        if self.exhausted_attempts.contains(&entry) {
            return false;
        }
        self.exhausted_attempts.push(entry);
        true
    }

    fn current_cluster_role_targets_all_exhausted(
        &self,
        cluster: &FailureClusterKey,
        role: RepairRole,
    ) -> bool {
        let Some(plan) = self.semantic_plan.as_ref() else {
            return false;
        };
        if &plan.failure_cluster_id != cluster || plan.preferred_repair_role != role {
            return false;
        }
        let mut saw_target = false;
        for target in plan
            .current_cluster_targets()
            .iter()
            .filter(|target| target.role == role)
        {
            saw_target = true;
            if !self.is_repair_target_exhausted(cluster, role, &target.path) {
                return false;
            }
        }
        saw_target
    }

    fn all_clusters_exhausted_for_active_plan(&self) -> bool {
        self.semantic_plan.as_ref().is_some_and(|plan| {
            next_repairable_cluster(
                &plan.semantic_report,
                plan.preferred_repair_role,
                &self.exhausted_attempts,
            )
            .is_none()
        })
    }

    /// Issue #653 (S1-007, DR1-006 命名統一): #654 (bounded stop report) が消費する
    /// read-only snapshot。`pub` への昇格はしない (DR3-001)。
    ///
    /// field 名 `repair_attempt_outcomes` と揃え、命名を
    /// `snapshot_repair_attempt_outcomes` に統一。
    #[allow(dead_code)] // consumed by #654 (bounded verifier_failed_safe_stop report).
    pub(super) fn snapshot_repair_attempt_outcomes(&self) -> Vec<RepairAttemptOutcome> {
        self.repair_attempt_outcomes.clone()
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

/// Issue #647 (CB-015): SSOT predicate for "the active `semantic_plan` is
/// stale" — i.e. the plan was constructed under the *same* assessment that
/// is currently sitting in `RepairJob.assessment`, and at least one cluster
/// has already been pushed onto `exhausted_attempts`.
///
/// "Stale" means: the cluster pointer has advanced (e.g. cluster B is now in
/// the slot after A was exhausted), but the assessment / repair-target hint
/// was generated for the **previous** cluster and no re-diagnostic has run
/// yet to refresh it. Returning `true` here is the precondition for CB-007
/// (hint guard returns `None`), CB-012/CB-014 (decision routes to
/// `NeedDiagnostic` / `DiagnosticUnavailable`), and CB-013 (diagnostic
/// runner clears the stale assessment before its `Skipped` short-circuit
/// fires).
///
/// "Fresh" means: after a re-diagnostic has bumped
/// `RepairJob.assessment_generation`, the plan's
/// `assessment_generation_at_creation` is strictly less than the job's
/// generation — the assessment in the slot was rebuilt *after* the plan
/// was last advanced, so the hint is no longer stale and CB-007 should
/// step out of the way so the controller can route to `NeedFreshRead` /
/// `NeedEdit` on cluster B.
///
/// Returns `false` when:
/// - `semantic_plan` is `None` (legacy / SetupRepair path),
/// - `exhausted_attempts` is empty (fresh first-cluster state, never advanced),
/// - the plan's `assessment_generation_at_creation` is strictly less than
///   `RepairJob.assessment_generation` (re-diagnostic produced a fresh
///   assessment after the last advance).
pub(super) fn semantic_plan_is_stale(job: &RepairJob) -> bool {
    let Some(plan) = job.semantic_plan.as_ref() else {
        return false;
    };
    if job.exhausted_attempts.is_empty() {
        return false;
    }
    // Plan was created at or after the current assessment generation →
    // no re-diagnostic has refreshed the assessment since the plan was
    // advanced. The plan is stale relative to its own assessment.
    plan.assessment_generation_at_creation >= job.assessment_generation
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
    // Issue #647 (CB-012 / CB-014 / CB-015): When semantic_plan is active
    // but the assessment was constructed before the plan advanced to its
    // current cluster (= `semantic_plan_is_stale` returns `true`), the
    // assessment is stale and the repair-target hint guard in
    // `verifier_repair_context_target_path` returns `None`. Two branches:
    //
    //   * **CB-012** (attempts remain): force a fresh diagnostic so
    //     the next assessment reflects the advanced cluster. Without
    //     this branch we would fall through to
    //     `latest_successful_read_existing_path` and route the repair
    //     pass to an unrelated turn-local read target.
    //   * **CB-014** (attempts exhausted): fail closed with
    //     `DiagnosticUnavailable`. Without this branch the same
    //     stale-target fallback path reopens at the diagnostic budget
    //     boundary because `assessment.is_some()` keeps the earlier
    //     `assessment.is_none() -> DiagnosticUnavailable` arm from
    //     firing.
    //
    // **CB-015** (architectural refactor): the stale-state predicate now
    // uses `semantic_plan_is_stale` which compares
    // `plan.assessment_generation_at_creation` against
    // `RepairJob.assessment_generation`. After a re-diagnostic bumps the
    // job's generation, the predicate flips to `false` (fresh state) and
    // the controller can advance to `NeedFreshRead` / `NeedEdit` on the
    // current cluster — closing the liveness gap where cluster B repair
    // could never start because the same exhausted_attempts-based predicate
    // kept routing back to `NeedDiagnostic`.
    if job.is_some_and(semantic_plan_is_stale) {
        if job.is_some_and(|job| {
            job.assessment_attempts
                < crate::agent::loop_run::turn::VERIFIER_DIAGNOSTIC_ATTEMPT_LIMIT
        }) {
            return VerifierRepairDecision::NeedDiagnostic;
        }
        return VerifierRepairDecision::DiagnosticUnavailable;
    }
    if job.is_some_and(|job| job.needs_diagnostic_after_target_exhaustion()) {
        if job.is_some_and(|job| {
            job.assessment_attempts
                < crate::agent::loop_run::turn::VERIFIER_DIAGNOSTIC_ATTEMPT_LIMIT
        }) {
            return VerifierRepairDecision::NeedDiagnostic;
        }
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

/// Issue #647 (CB-013 / CB-015): detect the "advanced semantic_plan +
/// stale assessment" state that CB-012 catches in the decision layer.
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
/// The predicate composes:
/// * `assessment.is_some()` — a stale assessment exists to clear
/// * `semantic_plan_is_stale(job)` — CB-015 generation-aware staleness
///   (subsumes the legacy `semantic_plan.is_some() &&
///   !exhausted_attempts.is_empty()` shape, and additionally flips to
///   `false` once a re-diagnostic has refreshed the assessment).
///
/// `work_root` is no longer consulted: stale detection lives entirely
/// on the `RepairJob` generation fields, so the predicate is now
/// O(1) and independent of filesystem state. The argument is retained
/// for the existing call-site signature; future cleanup may remove it.
pub(super) fn has_stale_assessment_after_cluster_advance(
    job: &RepairJob,
    _work_root: &Path,
) -> bool {
    job.assessment.is_some() && semantic_plan_is_stale(job)
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

/// Issue #647 (CB-016): semantic distinction between "advance because the
/// rerun showed no progress and we should walk to the next cluster with
/// the same (now stale) assessment" vs "advance because a fresh
/// diagnostic re-proposed an already-exhausted cluster and we skip to
/// the next one with the fresh assessment in hand".
///
/// The two triggers differ in how the new plan's
/// `assessment_generation_at_creation` is stamped:
///   - `RerunNoProgress`: stamp with `job.assessment_generation` (same gen,
///     stale plan; the controller will route through CB-012/CB-014 for a
///     fresh diagnostic).
///   - `DiagnosticSkip`: stamp with
///     `job.assessment_generation.saturating_sub(1)` (one generation older
///     than current → fresh plan; the controller proceeds to the
///     cluster-B repair pass).
///
/// Without this distinction (V8 posture), both call sites stamped the
/// post-bump `job.assessment_generation`, which made the
/// `DiagnosticSkip` path look stale even though the caller had just
/// landed a fresh diagnostic and the assessment in `job.assessment` was
/// already current. The result was a livelock between
/// `assign_semantic_plan_preserving_exhausted` advancing to cluster B
/// and `verifier_repair_decision` immediately routing back to
/// `NeedDiagnostic` (because `plan.gen == job.gen` looked stale).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AdvanceTrigger {
    /// Verifier rerun produced no progress; advance with the still-stale
    /// assessment. The new plan's
    /// `assessment_generation_at_creation` equals the current
    /// `job.assessment_generation`, so the plan looks stale to
    /// CB-007/CB-012/CB-014 and the controller will run a fresh
    /// diagnostic before proceeding.
    RerunNoProgress,
    /// A fresh diagnostic re-proposed an exhausted cluster; skip to the
    /// next cluster with the fresh assessment. The new plan's
    /// `assessment_generation_at_creation` is one generation older than
    /// the current `job.assessment_generation`, so the plan is fresh
    /// and the controller proceeds directly to the cluster-B repair
    /// pass without re-running the diagnostic.
    DiagnosticSkip,
}

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
/// `trigger` (Issue #647 CB-016) controls how the new plan's
/// `assessment_generation_at_creation` is stamped — see `AdvanceTrigger`.
///
/// Returns `false` (and clears `semantic_plan`) when the report has no
/// remaining clusters; callers MUST treat that as "no more clusters to
/// attack in this report" and switch back to `re_diagnostic`.
#[allow(dead_code)] // wired into turn.rs by a subsequent task; exercised here via unit tests.
pub(super) fn advance_to_next_cluster(
    repair_job: &mut RepairJob,
    report: &SemanticFailureReport,
    trigger: AdvanceTrigger,
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
    //    under the report's preferred_repair_role. CB-017 A''' (Commit 3,
    //    CR-6 V2): route through `next_repairable_cluster` so targetless
    //    clusters (admitted_cluster_targets empty after enrich) are skipped
    //    and the ledger comparison uses `(cluster_key, RepairRole)` exact
    //    pairs.
    let role = report.preferred_repair_role;
    let next_cluster = next_repairable_cluster(report, role, &repair_job.exhausted_attempts);

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

    // Issue #647 (CB-015 / CB-016): the new plan's generation snapshot is
    // chosen by `trigger`:
    //
    //   * `AdvanceTrigger::RerunNoProgress` — stamp with the **current**
    //     `RepairJob.assessment_generation` (CB-015 default). This
    //     intentionally creates a "stale" state immediately after a
    //     cluster advance:
    //     `plan.assessment_generation_at_creation == job.assessment_generation`
    //     → `semantic_plan_is_stale` returns `true` → CB-007/CB-012 force
    //     a re-diagnostic against the new cluster. Once that re-diagnostic
    //     increments `RepairJob.assessment_generation`, the comparison
    //     flips and the controller proceeds to repair the freshly-diagnosed
    //     cluster (the liveness gap CB-015 closes).
    //   * `AdvanceTrigger::DiagnosticSkip` — stamp with
    //     `repair_job.assessment_generation.saturating_sub(1)`. The caller
    //     has just landed a fresh diagnostic (post-bump
    //     `job.assessment_generation`) and is only walking past an
    //     already-exhausted cluster proposed by the LLM. The fresh
    //     assessment in `job.assessment` already corresponds to the
    //     new cluster, so the new plan must look **fresh**
    //     (`plan.gen < job.gen`) — that lets CB-012 step out of the way
    //     and the controller routes directly to `NeedFreshRead` /
    //     `NeedEdit`.
    let assessment_generation_at_creation = match trigger {
        AdvanceTrigger::RerunNoProgress => repair_job.assessment_generation,
        AdvanceTrigger::DiagnosticSkip => repair_job.assessment_generation.saturating_sub(1),
    };
    repair_job.semantic_plan = Some(SemanticRepairPlan {
        semantic_cause: report.failure_kind,
        spec_authority,
        preferred_repair_role: role,
        repair_hypothesis: report.repair_hypothesis.clone(),
        failure_cluster_id: next_cluster.cluster_key.clone(),
        expected_improvement: None,
        semantic_report: report.clone(),
        assessment_generation_at_creation,
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

/// Issue #647 / CB-017 A''' (Commit 3, CR-6 V2): return the first cluster
/// of `report` that is *repairable* — i.e. has a non-empty
/// `admitted_cluster_targets` list. Targetless clusters (no LLM-supplied
/// target_paths AND no merged legacy targets, or all candidates rejected by
/// the admission gate) are skipped.
///
/// Used by `turn.rs::build_semantic_repair_plan_from_report_with_authority_input`
/// to pick the initial cluster for the new plan.
pub(super) fn first_repairable_cluster(
    report: &SemanticFailureReport,
) -> Option<&super::semantic_failure::FailureCluster> {
    report
        .failure_clusters
        .iter()
        .find(|c| !c.admitted_cluster_targets.is_empty())
}

/// Issue #647 / CB-017 A''' (Commit 3, CR-6 V2): return the next cluster
/// that:
///
/// 1. Has a non-empty `admitted_cluster_targets` list (repairable), AND
/// 2. Is not already in `exhausted` under the **exact** `(cluster_key,
///    preferred_role)` pair.
///
/// CR-6 V2 distinguishes attacks on the same cluster under different
/// roles: an entry `(cluster_A, Implementation)` in `exhausted` does NOT
/// block a later attempt on `(cluster_A, Test)`.
pub(super) fn next_repairable_cluster<'a>(
    report: &'a SemanticFailureReport,
    preferred_role: RepairRole,
    exhausted: &[(FailureClusterKey, RepairRole)],
) -> Option<&'a super::semantic_failure::FailureCluster> {
    report.failure_clusters.iter().find(|c| {
        !c.admitted_cluster_targets.is_empty()
            && !exhausted
                .iter()
                .any(|(id, role)| id == &c.cluster_key && *role == preferred_role)
    })
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
        // CB-017 A''' (CR-4 V2): assessment が None に落ちる経路では
        // `assessment_bound_cluster_id` も None に reset しておく — 次の
        // diagnostic が新 assessment を書き込む直後の rebind 呼び出しで
        // "新 bind" として扱われ、`applied_repair_intents` が clear される。
        repair_job.assessment_bound_cluster_id = None;
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

    // CB-016: dispatch-after-rerun is the `RerunNoProgress` semantic — the
    // verifier rerun returned no progress and we are walking to the next
    // cluster with the still-stale assessment. The new plan must look stale
    // so the controller routes through CB-012/CB-014 for a fresh
    // diagnostic before attempting the next repair pass.
    let advanced = advance_to_next_cluster(repair_job, &report, AdvanceTrigger::RerunNoProgress);

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
        // CB-017 A''': after `RerunNoProgress` advance, rebind the legacy
        // assessment slice to the new cluster's admitted targets so the
        // controller does not keep routing to cluster A's path via
        // `needed_reads` / `repair_plan`. The plan stamp is "stale" under
        // this trigger (CB-016), so CB-007/CB-012 will still force a fresh
        // diagnostic before edits land — but if the diagnostic budget is
        // exhausted, the rebind already realigned the assessment.
        rebind_legacy_assessment_to_current_cluster(repair_job);
    } else {
        // No remaining clusters in the report → fall back to re-diagnostic.
        repair_job.assessment = None;
        repair_job.assessment_attempts = repair_job.assessment_attempts.saturating_add(1);
        // CB-017 A''' (CR-4 V2): re-diagnostic 経路では bound も None に reset。
        repair_job.assessment_bound_cluster_id = None;
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
    // CB-016: this path is the `DiagnosticSkip` semantic — a freshly-built
    // plan from the diagnostic LLM targets an already-exhausted cluster,
    // so we skip ahead with the **fresh** assessment in hand. The new
    // plan must look fresh (plan.gen < job.gen) so the controller does
    // NOT route back to `NeedDiagnostic` (which would be a livelock —
    // we have already produced a fresh diagnostic this turn).
    advance_to_next_cluster(repair_job, &report, AdvanceTrigger::DiagnosticSkip);
    // `advance_to_next_cluster` already mutates `semantic_plan`
    // (either to the next unexhausted cluster or to `None`) and updates the
    // ledger — no further bookkeeping required here.
}

/// Issue #647 / CB-017 A''' (CR-4 V2 / CR-7 V2): rebind the legacy
/// `VerifierRepairAssessment.repair_target_hint` / `repair_plan` /
/// `needed_reads` triple to the current semantic-plan cluster's admitted
/// targets — SSOT for the §0.1 invariant ("active cluster id ↔ legacy
/// target set rebound together").
///
/// Behaviour:
/// 1. `semantic_plan == None` → no-op (legacy path is respected as-is).
/// 2. `current_cluster_targets()` is empty → no-op (caller MUST skip
///    targetless clusters via `first_repairable_cluster` /
///    `next_repairable_cluster`).
/// 3. Otherwise: overwrite `assessment.repair_target_hint` with
///    `targets[0].clone()` and `assessment.repair_plan` /
///    `assessment.needed_reads` with `targets.clone()` — the previous
///    cluster's stale paths are dropped so the verifier-decision routes
///    only to the current cluster (CR-7 V2 prevents stale cluster A
///    paths from being re-promoted by the `needed_reads` fallback).
/// 4. Cluster transition detection uses `assessment_bound_cluster_id`
///    (CR-4 V2 cluster-key SSOT, not path/role comparison): when the
///    bound id is `None` OR differs from `plan.failure_cluster_id`,
///    `applied_repair_intents.clear()` so a previous cluster's repair
///    history does not gate the new cluster's edit pass.
/// 5. After the rebind, `assessment_bound_cluster_id` is set to
///    `Some(plan.failure_cluster_id.clone())` so future calls within the
///    same cluster do not clear the ledger.
///
/// Note: this helper does NOT mutate `applied_repair_intents` beyond the
/// transition-`clear()` above, and does NOT touch
/// `assessment_generation` / `assessment_attempts` / `semantic_plan` —
/// those remain owned by the diagnostic / dispatch machinery (S7-005
/// boundary).
pub(super) fn rebind_legacy_assessment_to_current_cluster(repair_job: &mut RepairJob) {
    let Some(plan) = repair_job.semantic_plan.as_ref() else {
        return; // semantic_plan = None → legacy 経路尊重
    };
    let plan_cluster_id = plan.failure_cluster_id.clone();
    let targets = plan.current_cluster_targets().to_vec();
    if targets.is_empty() {
        return; // targetless cluster は rebind しない (caller が skip 済み想定)
    }
    // CR-4 V2: cluster_key ベースで transition 判定。
    // 未 bind (None) は "new bind" 扱い = transition と見なし
    // applied_repair_intents を clear する。
    let is_cluster_transition = repair_job
        .assessment_bound_cluster_id
        .as_ref()
        .map(|bound| bound != &plan_cluster_id)
        .unwrap_or(true);
    if let Some(assessment) = repair_job.assessment.as_mut() {
        // CR-7 V2: Vec<RecoveryTargetHint> を clone で代入。
        assessment.repair_target_hint = Some(targets[0].clone());
        assessment.repair_plan = targets.clone();
        assessment.needed_reads = targets.clone();
    }
    if is_cluster_transition {
        repair_job.applied_repair_intents.clear();
    }
    // CR-4 V2: rebind 後は新しい cluster_id に bind。
    repair_job.assessment_bound_cluster_id = Some(plan_cluster_id);
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

// ============================================================================
// Issue #654 — Bounded Safe Stop Report
// ============================================================================
//
// All types and helpers in this section are `pub(super)` (DR3-001). They are
// consumed exclusively by `turn.rs::record_safe_stop_report` and unit tests
// inside this module. They MUST NOT be re-exported from
// `src/agent/loop_run.rs`.
//
// SSOT pipeline (defense-in-depth):
//   raw -> sanitize_repair_job_text* -> safe_relative_path_string ->
//          SafeStopReport::build_from -> build_safe_stop_payload ->
//          log_llm_event -> mask_payload_inplace
//
// The `SafeStopReport` struct intentionally **flat-copies** the 4 overlapping
// fields from `VerifierFailureSnapshot` (failure_signature / command /
// output_excerpt / failure_type) for FromRepair input, so that the report has
// no nested `Option<VerifierFailureSnapshot>` shape and the FromMissingVerifier
// branch can default these fields explicitly (DR2-008).

use super::task_contract::ArtifactRole;

/// Hard upper bound on a single `safe_relative_path_string` projection (chars).
/// `expected_target` / `owned_test_artifacts[]` are first projected through
/// `safe_relative_path_string` and then re-sanitized through
/// `sanitize_repair_job_text_with_char_cap(_, SAFE_STOP_PATH_CHAR_CAP)`
/// (DR2-001).
pub(super) const SAFE_STOP_PATH_CHAR_CAP: usize = 240;

/// Hard upper bound on a single sanitized `actual_actions[]` entry (chars).
pub(super) const SAFE_STOP_ACTION_CHAR_CAP: usize = 120;

/// Maximum number of `actual_actions[]` entries kept after capping.
pub(super) const SAFE_STOP_ACTUAL_ACTIONS_MAX: usize = 8;

/// Maximum number of `owned_test_artifacts[]` entries kept after Owned-validation.
pub(super) const SAFE_STOP_OWNED_TEST_ARTIFACTS_MAX: usize = 8;

/// `last_repair_hypothesis` char cap (matches `MAX_REPAIR_HYPOTHESIS_CHARS`).
pub(super) const SAFE_STOP_LAST_REPAIR_HYPOTHESIS_CHAR_CAP: usize = 240;

/// Maximum number of clusters tracked in `ExhaustedAttemptsSummary.per_cluster`.
pub(super) const SAFE_STOP_PER_CLUSTER_MAX: usize = 8;

/// Maximum number of role labels per cluster in `ExhaustedAttemptsSummary.per_cluster`.
pub(super) const SAFE_STOP_PER_CLUSTER_ROLE_MAX: usize = 4;

/// Issue #654 — per-turn dedup key. Each variant maps 1:1 to a `stop_reason`
/// label that appears in the `agent.safe_stop.report` event payload.
///
/// `Hash + Eq + Copy` allow `HashSet<StopReason>` to act as the per-turn
/// dedup marker on `Agent` (DR1-006). Issue #662 added `RepairExhausted`,
/// bringing the closed-fixed cardinality to 6 variants.
#[derive(Clone, Copy, Hash, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub(super) enum StopReason {
    ArtifactCompletionFailed,
    VerifierFailedSafeStop,
    VerifierWeak,
    VerifierMissing,
    DiagnosticTargetMissing,
    /// Issue #662: same (cluster, role) failure was attacked >= 2 times via
    /// any promotion bucket (NoProgress / Worsened / Malformed / Noop /
    /// Duplicate / Unsafe) and all repairable clusters in the active
    /// `SemanticRepairPlan` are now in `exhausted_attempts`. Emit happens at
    /// the production caller observing `PromotionResult.all_clusters_exhausted`.
    RepairExhausted,
}

impl StopReason {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            StopReason::ArtifactCompletionFailed => "artifact_completion_failed",
            StopReason::VerifierFailedSafeStop => "verifier_failed_safe_stop",
            StopReason::VerifierWeak => "verifier_weak",
            StopReason::VerifierMissing => "verifier_missing",
            StopReason::DiagnosticTargetMissing => "diagnostic_target_missing",
            StopReason::RepairExhausted => "repair_exhausted",
        }
    }
}

/// Issue #654 — why diagnostic target selection failed (4 priorities).
/// Computed deterministically by `select_diagnostic_target_missing_reason`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum DiagnosticTargetMissingReason {
    /// priority 1: at least one candidate exists, but ALL are rejected by scope.
    ScopeExcluded,
    /// priority 2: candidates exist but resolve/read failed for all.
    AllCandidatesUnreadable,
    /// priority 3: no successful Read history and no changed_file_hints.
    ReadHistoryEmpty,
    /// priority 4 (default fallback): assessment missing / parse failed / retry exhaust.
    AssessmentMissing,
}

impl DiagnosticTargetMissingReason {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            DiagnosticTargetMissingReason::ScopeExcluded => "scope_excluded",
            DiagnosticTargetMissingReason::AllCandidatesUnreadable => "all_candidates_unreadable",
            DiagnosticTargetMissingReason::ReadHistoryEmpty => "read_history_empty",
            DiagnosticTargetMissingReason::AssessmentMissing => "assessment_missing",
        }
    }
}

/// Issue #654 — bounded summary of `RepairJob.exhausted_attempts` for the
/// safe stop report. std-only implementation (DR3-003): linear scans against
/// small bounded `Vec`s instead of pulling in `indexmap` / `enumset`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ExhaustedAttemptsSummary {
    pub(super) total: usize,
    /// `Vec<(cluster_key_16hex, Vec<role_label>)>` — preserves insertion
    /// order, deduplicates within a cluster, and caps at
    /// `SAFE_STOP_PER_CLUSTER_MAX` × `SAFE_STOP_PER_CLUSTER_ROLE_MAX`.
    pub(super) per_cluster: Vec<(String, Vec<&'static str>)>,
    /// Sanitized + 240-char-capped repair hypothesis. `None` when no plan / no hypothesis.
    pub(super) last_repair_hypothesis: Option<String>,
}

impl ExhaustedAttemptsSummary {
    /// Build a bounded summary from a `RepairJob` and an optional last
    /// hypothesis text (raw, will be sanitized + capped here).
    pub(super) fn from_repair_job(job: &RepairJob, last_hyp: Option<&str>) -> Self {
        let total = job.exhausted_attempts.len();
        let mut per_cluster: Vec<(String, Vec<&'static str>)> = Vec::new();
        for (cluster_key, role) in job.exhausted_attempts.iter() {
            let key_str = cluster_key.as_str().to_string();
            let role_label = role.label();
            match per_cluster.iter_mut().find(|(k, _)| k == &key_str) {
                Some((_, roles)) => {
                    if roles.len() < SAFE_STOP_PER_CLUSTER_ROLE_MAX && !roles.contains(&role_label)
                    {
                        roles.push(role_label);
                    }
                }
                None => {
                    if per_cluster.len() < SAFE_STOP_PER_CLUSTER_MAX {
                        per_cluster.push((key_str, vec![role_label]));
                    }
                }
            }
        }
        let last_repair_hypothesis = last_hyp.map(|raw| {
            sanitize_repair_job_text_with_char_cap(raw, SAFE_STOP_LAST_REPAIR_HYPOTHESIS_CHAR_CAP)
        });
        Self {
            total,
            per_cluster,
            last_repair_hypothesis,
        }
    }
}

/// Issue #654 / DR1-003 — single SSOT for the "raw path string -> safe
/// workspace-relative String" projection. Returns `None` when the input is:
/// - empty
/// - contains any C0 / DEL control char
/// - absolute path (Unix `/foo`, Windows `C:\…`, UNC `\\?\…` / `//host/share`)
/// - contains a `\` backslash separator (Windows path style)
/// - contains `..` (`ParentDir`) component after normalizing separators
///
/// CB-004 (Codex review, Issue #654): the previous implementation relied on
/// `Path::components()`, which delegates to the host OS's path grammar. On
/// Unix builds that means `..\secret`, `C:\Users\…`, `\\?\C:\foo`, and
/// `//host/share` all pass as a single normal component. This helper now
/// rejects backslash separators and Windows-style absolute prefixes at the
/// string level before falling back to the host `Path` traversal check, so
/// the projection is OS-independent.
///
/// The returned `String` is the raw projected path. Callers MUST then
/// run it through `sanitize_repair_job_text_with_char_cap(_,
/// SAFE_STOP_PATH_CHAR_CAP)` for byte-cap and secret-masking.
pub(super) fn safe_relative_path_string(raw: &str) -> Option<String> {
    if raw.is_empty() {
        return None;
    }
    if raw.chars().any(|c| c.is_control()) {
        return None;
    }
    // CB-004: reject Windows path styles before the host `Path` parser sees
    // them. The serialization-only consumers downstream still treat the
    // emitted payload as workspace-relative POSIX paths, so any `\`,
    // `<drive>:`, `\\?\`, or `//host/share` prefix is a hard reject.
    if raw.contains('\\') {
        return None;
    }
    if raw.starts_with("//") {
        return None;
    }
    let mut chars = raw.chars();
    if let (Some(first), Some(second)) = (chars.next(), chars.next())
        && first.is_ascii_alphabetic()
        && second == ':'
    {
        return None;
    }
    let path = Path::new(raw);
    if path.is_absolute() {
        return None;
    }
    if path
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return None;
    }
    Some(raw.to_string())
}

/// Issue #654 — sanitize + bound the `actual_actions[]` array. Takes the
/// **most recent** entries up to `SAFE_STOP_ACTUAL_ACTIONS_MAX` and clamps
/// each to `SAFE_STOP_ACTION_CHAR_CAP` characters via the existing
/// `sanitize_repair_job_text_with_char_cap` SSOT.
///
/// Input order convention: oldest-first. Output preserves new-to-old order
/// (i.e. the most recent action appears first) so downstream consumers see
/// the freshest context up front.
pub(super) fn build_actual_actions(raw: &[String]) -> Vec<String> {
    raw.iter()
        .rev()
        .take(SAFE_STOP_ACTUAL_ACTIONS_MAX)
        .map(|s| sanitize_repair_job_text_with_char_cap(s, SAFE_STOP_ACTION_CHAR_CAP))
        .collect()
}

/// Issue #654 / DR1-002 — deterministic priority selector for the
/// `diagnostic_target_missing_reason` field of a `SafeStopReport` produced by
/// the `diagnostic_target_missing` path.
///
/// Priority order (first matching wins):
/// 1. `ScopeExcluded` — at least one candidate path exists AND every candidate
///    is rejected by `scope.contains(&str)`.
/// 2. `AllCandidatesUnreadable` — at least one candidate exists (otherwise we
///    would have stopped at priority 1) — reaching this branch means scope
///    accepted at least one path but resolve/read still failed for all.
/// 3. `ReadHistoryEmpty` — no candidates **and** no `latest_successful_read`
///    **and** `job.changed_file_hints` is empty.
/// 4. `AssessmentMissing` — fallback default.
pub(super) fn select_diagnostic_target_missing_reason(
    job: &RepairJob,
    scope: &super::task_workspace_scope::TaskWorkspaceScope,
    candidates: &[String],
    latest_successful_read: Option<&Path>,
) -> DiagnosticTargetMissingReason {
    if !candidates.is_empty() && candidates.iter().all(|rel| !scope.contains(rel)) {
        return DiagnosticTargetMissingReason::ScopeExcluded;
    }
    if !candidates.is_empty() {
        return DiagnosticTargetMissingReason::AllCandidatesUnreadable;
    }
    if latest_successful_read.is_none() && job.changed_file_hints.is_empty() {
        return DiagnosticTargetMissingReason::ReadHistoryEmpty;
    }
    DiagnosticTargetMissingReason::AssessmentMissing
}

/// Issue #654 / DR1-001 — bicephalous input to `SafeStopReport::build_from`.
/// The two variants exist because `verifier_missing` (the only path that
/// fires `MissingVerifierJob`) does not carry a `RepairJob`.
pub(super) enum SafeStopInput<'a> {
    FromRepair {
        job: &'a RepairJob,
        stop_reason: StopReason,
        /// Owned-validated test artifact paths (verifier_weak / verifier_missing).
        owned_test_artifacts: Vec<String>,
    },
    FromMissingVerifier {
        #[allow(dead_code)]
        job: &'a MissingVerifierJob,
        owned_test_artifacts: Vec<String>,
    },
}

impl SafeStopInput<'_> {
    /// SSOT extractor for the `StopReason` carried by this input.
    /// `FromMissingVerifier` is hard-coded to `StopReason::VerifierMissing`
    /// (DR2-004).
    pub(super) fn stop_reason(&self) -> StopReason {
        match self {
            SafeStopInput::FromRepair { stop_reason, .. } => *stop_reason,
            SafeStopInput::FromMissingVerifier { .. } => StopReason::VerifierMissing,
        }
    }
}

/// Issue #654 — read-only context plumbed from `turn.rs` into `build_from`.
/// Lifetimes mirror the borrowed view the caller already has (no clone).
pub(super) struct SafeStopContext<'a> {
    /// Resolved per §6.4 priority: semantic_plan -> task_contract -> None.
    pub(super) current_role: Option<ArtifactRole>,
    /// Raw expected-target path (caller-provided). Will be projected through
    /// `safe_relative_path_string` + `sanitize_repair_job_text_with_char_cap`.
    pub(super) expected_target: Option<&'a Path>,
    /// Raw, unsanitized action labels (oldest-first). Will be capped/sanitized
    /// by `build_actual_actions`.
    pub(super) actual_actions_raw: Vec<String>,
    /// For `diagnostic_target_missing` reason selection (DR1-002 / DR2-003).
    /// Caller converts owned `PathBuf` to `&Path` via `.as_deref()`.
    pub(super) latest_successful_read: Option<&'a Path>,
    /// For `diagnostic_target_missing` reason selection (DR1-002).
    pub(super) task_workspace_scope: &'a super::task_workspace_scope::TaskWorkspaceScope,
    /// Candidate paths considered during diagnostic target selection.
    pub(super) candidates: Vec<String>,
    /// For event-payload `session_id` and `turn_index` fields.
    pub(super) session_id: &'a str,
    pub(super) turn_index: u64,
}

/// Issue #654 — bounded structured safe stop report. Built by
/// `SafeStopReport::build_from` and consumed by `build_safe_stop_payload`
/// in `turn.rs`. All string fields are SSOT-sanitized (defense-in-depth).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct SafeStopReport {
    pub(super) failure_signature: String,
    pub(super) command: String,
    pub(super) output_excerpt: String,
    pub(super) failure_type: VerifierFailureType,
    pub(super) stop_reason: StopReason,
    pub(super) current_role: Option<ArtifactRole>,
    pub(super) expected_target: Option<String>,
    pub(super) actual_actions: Vec<String>,
    pub(super) exhausted_attempts_summary: Option<ExhaustedAttemptsSummary>,
    pub(super) diagnostic_target_missing_reason: Option<DiagnosticTargetMissingReason>,
    pub(super) owned_test_artifacts: Vec<String>,
    pub(super) session_id: String,
    pub(super) turn_index: u64,
}

impl SafeStopReport {
    /// SSOT 5-path / 2-input-variant builder. All masking + capping happens
    /// here, even when source data has already been sanitized upstream
    /// (defense-in-depth, R7).
    pub(super) fn build_from(input: SafeStopInput<'_>, ctx: SafeStopContext<'_>) -> Self {
        let stop_reason = input.stop_reason();
        let expected_target = ctx
            .expected_target
            .and_then(|p| p.to_str())
            .and_then(safe_relative_path_string)
            .map(|s| sanitize_repair_job_text_with_char_cap(&s, SAFE_STOP_PATH_CHAR_CAP));
        let actual_actions = build_actual_actions(&ctx.actual_actions_raw);

        match input {
            SafeStopInput::FromRepair {
                job,
                stop_reason: _,
                owned_test_artifacts,
            } => {
                // Re-use the existing `failure_snapshot()` SSOT — it already
                // composes `sanitize_repair_job_text` /
                // `redact_verifier_command_for_storage` and projects
                // `target_path` through the same syntactic safety check.
                let snapshot = job.failure_snapshot();
                let mut failure_type = snapshot.failure_type;
                // DR3-005: `DiagnosticTargetMissing` is only ever set by the
                // diagnostic_target_missing emit path. Mapping helpers never
                // produce this variant, so we set it here explicitly.
                if stop_reason == StopReason::DiagnosticTargetMissing {
                    failure_type = VerifierFailureType::DiagnosticTargetMissing;
                }
                // Issue #662 (design judgment #5 (b)): `RepairExhausted` is a
                // meta-state ("same failure attacked >= 2 times") distinct
                // from the underlying verifier failure type. Upgrade the
                // payload's `failure_type` here so downstream `/bug-fix`
                // consumers can branch on `failure_type == "repair_exhausted"`
                // without inspecting `stop_reason`. The original
                // `failure_signature` / `output_excerpt` continue to carry the
                // root-cause verifier text.
                if stop_reason == StopReason::RepairExhausted {
                    failure_type = VerifierFailureType::RepairExhausted;
                }
                let diagnostic_reason = if stop_reason == StopReason::DiagnosticTargetMissing {
                    Some(select_diagnostic_target_missing_reason(
                        job,
                        ctx.task_workspace_scope,
                        &ctx.candidates,
                        ctx.latest_successful_read,
                    ))
                } else {
                    None
                };
                let last_hyp = job
                    .semantic_plan
                    .as_ref()
                    .map(|plan| plan.repair_hypothesis.as_str());
                let summary = Some(ExhaustedAttemptsSummary::from_repair_job(job, last_hyp));
                Self {
                    failure_signature: snapshot.failure_signature,
                    command: snapshot.command,
                    output_excerpt: snapshot.output_excerpt,
                    failure_type,
                    stop_reason,
                    current_role: ctx.current_role,
                    expected_target,
                    actual_actions,
                    exhausted_attempts_summary: summary,
                    diagnostic_target_missing_reason: diagnostic_reason,
                    owned_test_artifacts: sanitize_and_filter_owned_paths(&owned_test_artifacts),
                    session_id: ctx.session_id.to_string(),
                    turn_index: ctx.turn_index,
                }
            }
            SafeStopInput::FromMissingVerifier {
                job: _,
                owned_test_artifacts,
            } => Self {
                // Explicit defaults (R8): downstream consumers expect non-empty
                // failure_signature / failure_type even for the missing-verifier
                // path so empty-string detection in `/bug-fix` does not misfire.
                failure_signature: "missing_verifier_or_config".to_string(),
                command: String::new(),
                output_excerpt: String::new(),
                failure_type: VerifierFailureType::MissingVerifierOrConfig,
                stop_reason,
                current_role: ctx.current_role,
                expected_target,
                actual_actions,
                exhausted_attempts_summary: None,
                diagnostic_target_missing_reason: None,
                owned_test_artifacts: sanitize_and_filter_owned_paths(&owned_test_artifacts),
                session_id: ctx.session_id.to_string(),
                turn_index: ctx.turn_index,
            },
        }
    }
}

/// Issue #654 — defense-in-depth re-walk of caller-supplied owned test
/// artifact paths. Each candidate must survive `safe_relative_path_string`
/// before being sanitized + capped. Empty / `..` / absolute paths are
/// silently dropped (caller is supposed to upstream-validate via
/// `classify_ownership`; this is the last line of defense, DR2-006).
fn sanitize_and_filter_owned_paths(raw: &[String]) -> Vec<String> {
    raw.iter()
        .filter_map(|p| safe_relative_path_string(p))
        .map(|p| sanitize_repair_job_text_with_char_cap(&p, SAFE_STOP_PATH_CHAR_CAP))
        .take(SAFE_STOP_OWNED_TEST_ARTIFACTS_MAX)
        .collect()
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
            assessment_generation_at_creation: 0,
        };
        let plan_b = SemanticRepairPlan {
            semantic_report: report,
            failure_cluster_id: cluster_id,
            semantic_cause: VerifierDiagnosticFailureKind::AssertionMismatch,
            spec_authority: SpecAuthority::BehaviorContract,
            preferred_repair_role: super::super::task_contract::ArtifactRole::Implementation,
            repair_hypothesis: "h".to_string(),
            expected_improvement: None,
            assessment_generation_at_creation: 0,
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
            assessment_generation_at_creation: 0,
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
            assessment_generation_at_creation: 0,
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
        let mut report = super::super::semantic_failure::parse_semantic_failure_report(&json)
            .expect("multi-cluster fixture parses");
        // CB-017 A''' (Commit 3): seed every cluster with a synthetic admitted
        // target so the new `next_repairable_cluster` skip rule does not
        // accidentally drop clusters in pre-CB-017 fixtures. Production code
        // fills this slot via `enrich_failure_clusters_with_admitted_targets`
        // in `turn.rs`; tests that exercise the cluster-walker rely on the
        // pre-Commit-3 "every cluster is repairable" expectation.
        for (idx, cluster) in report.failure_clusters.iter_mut().enumerate() {
            cluster.admitted_cluster_targets =
                vec![super::super::task_contract::RecoveryTargetHint {
                    role: super::super::task_contract::ArtifactRole::Implementation,
                    path: format!("app/main_{idx}.py"),
                    reason: "fixture-admitted target".to_string(),
                }];
        }
        report
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
            assessment_generation_at_creation: 0,
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
        assert!(advance_to_next_cluster(
            &mut job,
            &report,
            AdvanceTrigger::RerunNoProgress
        ));
        assert_eq!(
            job.semantic_plan.as_ref().unwrap().failure_cluster_id,
            cluster_ids[1]
        );

        // Second advance: cluster 2 → cluster 3.
        assert!(advance_to_next_cluster(
            &mut job,
            &report,
            AdvanceTrigger::RerunNoProgress
        ));
        assert_eq!(
            job.semantic_plan.as_ref().unwrap().failure_cluster_id,
            cluster_ids[2]
        );

        // Third advance: no more clusters — slot cleared, return false.
        assert!(!advance_to_next_cluster(
            &mut job,
            &report,
            AdvanceTrigger::RerunNoProgress
        ));
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
        assert!(advance_to_next_cluster(
            &mut job,
            &report,
            AdvanceTrigger::RerunNoProgress
        ));
        assert_eq!(job.exhausted_attempts.len(), 1);
        assert_eq!(job.exhausted_attempts[0], (cluster_ids[0].clone(), role));
        // Slot now targets cluster 2.
        assert_eq!(
            job.semantic_plan.as_ref().unwrap().failure_cluster_id,
            cluster_ids[1]
        );

        // Advance 2: cluster 2 → cluster 3; ledger preserves cluster 1.
        assert!(advance_to_next_cluster(
            &mut job,
            &report,
            AdvanceTrigger::RerunNoProgress
        ));
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
        assert!(!advance_to_next_cluster(
            &mut job,
            &report,
            AdvanceTrigger::RerunNoProgress
        ));
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
        assert!(!advance_to_next_cluster(
            &mut job,
            &report,
            AdvanceTrigger::RerunNoProgress
        ));
        let after_first_len = job.exhausted_attempts.len();
        assert_eq!(after_first_len, 1);

        // Second call: semantic_plan is None, so no new ledger entry.
        assert!(!advance_to_next_cluster(
            &mut job,
            &report,
            AdvanceTrigger::RerunNoProgress
        ));
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
            assessment_generation_at_creation: 0,
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
            assessment_generation_at_creation: 0,
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
        assert!(advance_to_next_cluster(
            &mut job,
            &report,
            AdvanceTrigger::RerunNoProgress
        )); // 1 → 2
        assert!(advance_to_next_cluster(
            &mut job,
            &report,
            AdvanceTrigger::RerunNoProgress
        )); // 2 → 3
        // Third advance: no more clusters.
        assert!(!advance_to_next_cluster(
            &mut job,
            &report,
            AdvanceTrigger::RerunNoProgress
        ));

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
            assessment_generation_at_creation: 0,
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
            assessment_generation_at_creation: 0,
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
            assessment_generation_at_creation: 0,
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
            assessment_generation_at_creation: 0,
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
            assessment_generation_at_creation: 0,
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
            assessment_generation_at_creation: 0,
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
            assessment_generation_at_creation: 0,
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
                assessment_generation_at_creation: 0,
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
            assessment_generation_at_creation: 0,
        };
        let mut job = RepairJob {
            semantic_plan: Some(plan),
            ..RepairJob::new_for_test()
        };

        assert!(advance_to_next_cluster(
            &mut job,
            &report,
            AdvanceTrigger::RerunNoProgress
        ));

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
            assessment_generation_at_creation: 0,
        };
        let mut job = RepairJob {
            semantic_plan: Some(plan),
            ..RepairJob::new_for_test()
        };

        assert!(advance_to_next_cluster(
            &mut job,
            &report,
            AdvanceTrigger::RerunNoProgress
        ));

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
            assessment_generation_at_creation: 0,
        };
        let mut job = RepairJob {
            semantic_plan: Some(plan),
            ..RepairJob::new_for_test()
        };

        // First advance: A → B. Authority must remain BehaviorContract.
        assert!(advance_to_next_cluster(
            &mut job,
            &report,
            AdvanceTrigger::RerunNoProgress
        ));
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
        assert!(advance_to_next_cluster(
            &mut job,
            &report,
            AdvanceTrigger::RerunNoProgress
        ));
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
            assessment_generation_at_creation: 0,
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
            assessment_generation_at_creation: 0,
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
            assessment_generation_at_creation: 0,
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

    // ---- Issue #647 (CB-015): stale↔fresh distinction via assessment_generation ---- //

    /// CB-015.1: a freshly-built RepairJob has `assessment_generation == 0`
    /// and `semantic_plan_is_stale` is `false` (no plan, no exhausted
    /// attempts). The legacy zero-init posture is preserved.
    #[test]
    fn cb015_new_job_starts_with_generation_zero_and_is_not_stale() {
        let job = RepairJob::new_for_test();
        assert_eq!(job.assessment_generation, 0);
        assert!(!semantic_plan_is_stale(&job));
    }

    /// CB-015.2 (core stale state): after a cluster advance, the new plan is
    /// `assessment_generation_at_creation == job.assessment_generation`
    /// AND `!exhausted_attempts.is_empty()` → `semantic_plan_is_stale`
    /// fires. This is the precondition where CB-007 / CB-012 must force
    /// re-diagnostic.
    #[test]
    fn cb015_stale_state_after_cluster_advance_returns_true() {
        let report =
            multi_cluster_report_fixture(VerifierDiagnosticFailureKind::AssertionMismatch, 2);
        let mut job = job_with_first_cluster_plan(&report);
        // The fixture sets job.assessment_generation = 0 (default). The
        // initial plan is also at generation 0 — but `exhausted_attempts`
        // is empty, so the plan is not yet stale.
        assert!(!semantic_plan_is_stale(&job));

        // Simulate a "diagnostic landed at generation 1" baseline (so the
        // initial plan from `job_with_first_cluster_plan` looks fresh
        // post-bump). This mirrors the production sequencing in
        // `run_verifier_diagnostic_pass`.
        job.assessment_generation = 1;
        assert!(!semantic_plan_is_stale(&job)); // empty exhausted_attempts

        // Now advance to cluster B → the new plan is created with
        // `assessment_generation_at_creation = job.assessment_generation = 1`
        // AND cluster A is pushed onto `exhausted_attempts`. The stale
        // predicate must fire.
        assert!(advance_to_next_cluster(
            &mut job,
            &report,
            AdvanceTrigger::RerunNoProgress
        ));
        assert!(
            semantic_plan_is_stale(&job),
            "CB-015: post-advance, plan.gen == job.gen and exhausted non-empty → stale must be true",
        );
        assert_eq!(
            job.semantic_plan
                .as_ref()
                .unwrap()
                .assessment_generation_at_creation,
            1,
            "advance must coil the current job.gen into the new plan",
        );
    }

    /// CB-015.3 (fresh state): once a re-diagnostic bumps
    /// `RepairJob.assessment_generation`, the plan's
    /// `assessment_generation_at_creation` becomes strictly less than the
    /// job's generation → `semantic_plan_is_stale` flips back to `false`.
    /// This is the architectural change that lets CB-007 / CB-012 step
    /// out of the way after a re-diagnostic has refreshed the assessment.
    #[test]
    fn cb015_post_re_diagnostic_fresh_state_is_not_stale() {
        let report =
            multi_cluster_report_fixture(VerifierDiagnosticFailureKind::AssertionMismatch, 2);
        let mut job = job_with_first_cluster_plan(&report);
        job.assessment_generation = 1;
        // Advance to cluster B → stale precondition.
        assert!(advance_to_next_cluster(
            &mut job,
            &report,
            AdvanceTrigger::RerunNoProgress
        ));
        assert!(semantic_plan_is_stale(&job));

        // Simulate `run_verifier_diagnostic_pass` writing a new assessment:
        // bump `assessment_generation` by 1. The slot still holds the
        // cluster-B plan with `assessment_generation_at_creation == 1`.
        job.assessment_generation = 2;
        assert!(
            !semantic_plan_is_stale(&job),
            "CB-015: after re-diagnostic bump, plan.gen < job.gen → stale must be false",
        );
    }

    /// CB-015.4 (decision integration): the `verifier_repair_decision`
    /// state machine routes a stale state through `NeedDiagnostic`, and a
    /// fresh state (post-re-diagnostic) through the normal target
    /// resolution path (`NeedTargetDiscovery` here because the test
    /// fixture's `repair_target_hint` path does not exist on disk; the
    /// essential invariant is "no longer `NeedDiagnostic`").
    #[test]
    fn cb015_decision_routes_fresh_state_past_need_diagnostic() {
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

        let stale_hint = super::super::task_contract::RecoveryTargetHint {
            role: super::super::task_contract::ArtifactRole::Implementation,
            path: "app/stale.py".to_string(),
            reason: "stale".to_string(),
        };

        // Stale state: plan.gen == job.gen, exhausted non-empty.
        let mut job = RepairJob {
            semantic_plan: Some(SemanticRepairPlan {
                semantic_report: report.clone(),
                failure_cluster_id: cluster_ids[1].clone(),
                semantic_cause: report.failure_kind,
                spec_authority: SpecAuthority::ImplementationContract,
                preferred_repair_role: role,
                repair_hypothesis: report.repair_hypothesis.clone(),
                expected_improvement: None,
                assessment_generation_at_creation: 1,
            }),
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
            assessment_attempts: 0,
            assessment_generation: 1,
            ..RepairJob::new_for_test()
        };

        // Stale → NeedDiagnostic (CB-012 path via CB-015 predicate).
        let dec = verifier_repair_decision(true, Some(&job), &[], &work_root, Some(1), 1);
        assert_eq!(
            dec,
            VerifierRepairDecision::NeedDiagnostic,
            "CB-015 stale state must route to NeedDiagnostic",
        );

        // Now simulate re-diagnostic bumping `assessment_generation` →
        // plan becomes fresh.
        job.assessment_generation = 2;
        assert!(!semantic_plan_is_stale(&job));

        // Fresh → no longer NeedDiagnostic. The decision falls through to
        // the normal target-resolution branches.
        let dec_fresh = verifier_repair_decision(true, Some(&job), &[], &work_root, Some(1), 1);
        assert_ne!(
            dec_fresh,
            VerifierRepairDecision::NeedDiagnostic,
            "CB-015 fresh state must NOT route to NeedDiagnostic — \
             the controller can now proceed to NeedFreshRead/NeedEdit/etc.",
        );
        assert_ne!(
            dec_fresh,
            VerifierRepairDecision::DiagnosticUnavailable,
            "CB-015 fresh state must NOT fail closed as DiagnosticUnavailable",
        );
    }

    /// CB-015.5 (CB-013 helper integration): once the assessment is fresh
    /// (post-re-diagnostic generation bump), `has_stale_assessment_after_cluster_advance`
    /// no longer fires — the diagnostic runner's `Skipped` short-circuit
    /// returns to its normal posture instead of being pre-empted.
    #[test]
    fn cb015_has_stale_assessment_fires_only_for_stale_generation() {
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

        let mut job = RepairJob {
            semantic_plan: Some(SemanticRepairPlan {
                semantic_report: report.clone(),
                failure_cluster_id: cluster_ids[1].clone(),
                semantic_cause: report.failure_kind,
                spec_authority: SpecAuthority::ImplementationContract,
                preferred_repair_role: role,
                repair_hypothesis: report.repair_hypothesis.clone(),
                expected_improvement: None,
                assessment_generation_at_creation: 1,
            }),
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
            assessment_generation: 1,
            ..RepairJob::new_for_test()
        };

        // Stale: helper fires.
        assert!(has_stale_assessment_after_cluster_advance(&job, &work_root));

        // Fresh (re-diagnostic bumped gen): helper must NOT fire — the
        // assessment now belongs to the current cluster, no need to
        // clear it before the next diagnostic call.
        job.assessment_generation = 2;
        assert!(
            !has_stale_assessment_after_cluster_advance(&job, &work_root),
            "CB-015: post-re-diagnostic fresh assessment must NOT trigger the stale-clear helper",
        );
    }

    /// CB-015.6 (carryover invariant): `verifier_repair_context_from_failure`
    /// carries `assessment_generation` over the turn boundary. This pin
    /// guards the SSOT carryover so a future refactor cannot silently
    /// drop the generation field across slot reuse.
    ///
    /// The actual production carryover lives in `turn.rs`; we verify the
    /// field shape here by constructing a job with a non-zero generation,
    /// cloning the field through the public API, and asserting it survives.
    #[test]
    fn cb015_assessment_generation_field_is_copyable_and_survives_clone() {
        let mut job = RepairJob::new_for_test();
        job.assessment_generation = 7;
        let cloned = job.clone();
        assert_eq!(cloned.assessment_generation, 7);
        // PartialEq still holds across the new field.
        assert_eq!(job, cloned);
        // Mutating the new field breaks equality, confirming the field
        // participates in `PartialEq`.
        let mut bumped = job.clone();
        bumped.assessment_generation = 8;
        assert_ne!(job, bumped);
    }

    // ---- Issue #647 (CB-016): AdvanceTrigger semantic distinction ---- //

    /// CB-016 (helper unit): from the same initial job, `DiagnosticSkip`
    /// stamps the new plan one generation behind the current job, while
    /// `RerunNoProgress` stamps it equal to the current job — yielding a
    /// stale plan for the latter and a fresh plan for the former.
    #[test]
    fn cb016_advance_trigger_stamps_correct_generation() {
        let report =
            multi_cluster_report_fixture(VerifierDiagnosticFailureKind::AssertionMismatch, 2);

        // Use job.assessment_generation = 2 so we can distinguish the two
        // stamps unambiguously (saturating_sub(1) yields 1, not the same
        // value as the RerunNoProgress branch).
        let mut job_rerun = job_with_first_cluster_plan(&report);
        job_rerun.assessment_generation = 2;
        assert!(advance_to_next_cluster(
            &mut job_rerun,
            &report,
            AdvanceTrigger::RerunNoProgress,
        ));
        let rerun_plan = job_rerun.semantic_plan.as_ref().expect("rerun advanced");
        assert_eq!(
            rerun_plan.assessment_generation_at_creation, 2,
            "RerunNoProgress must stamp plan.gen = job.gen (stale post-advance)",
        );
        assert!(
            semantic_plan_is_stale(&job_rerun),
            "RerunNoProgress post-advance must produce a stale plan",
        );

        let mut job_skip = job_with_first_cluster_plan(&report);
        job_skip.assessment_generation = 2;
        assert!(advance_to_next_cluster(
            &mut job_skip,
            &report,
            AdvanceTrigger::DiagnosticSkip,
        ));
        let skip_plan = job_skip.semantic_plan.as_ref().expect("skip advanced");
        assert_eq!(
            skip_plan.assessment_generation_at_creation, 1,
            "DiagnosticSkip must stamp plan.gen = job.gen - 1 (fresh post-advance)",
        );
        assert!(
            !semantic_plan_is_stale(&job_skip),
            "DiagnosticSkip post-advance must produce a fresh plan",
        );
    }

    /// CB-016 (boundary): when `job.assessment_generation == 0`,
    /// `DiagnosticSkip` saturates at 0 instead of underflowing. The plan
    /// is then equal to the job (not less than), so the stale predicate
    /// fires the same way as `RerunNoProgress` — defensive posture for a
    /// caller that wires the new trigger in before the first
    /// re-diagnostic bump has happened.
    #[test]
    fn cb016_diagnostic_skip_saturates_at_zero_generation() {
        let report =
            multi_cluster_report_fixture(VerifierDiagnosticFailureKind::AssertionMismatch, 2);
        let mut job = job_with_first_cluster_plan(&report);
        // job.assessment_generation defaults to 0 (RepairJob::new_for_test).
        assert_eq!(job.assessment_generation, 0);

        assert!(advance_to_next_cluster(
            &mut job,
            &report,
            AdvanceTrigger::DiagnosticSkip,
        ));
        let plan = job.semantic_plan.as_ref().expect("advanced");
        assert_eq!(
            plan.assessment_generation_at_creation, 0,
            "DiagnosticSkip with job.gen=0 must saturate at 0 (no underflow)",
        );
    }

    /// CB-016.1 (production integration scenario): `assign_semantic_plan_preserving_exhausted`
    /// is the `DiagnosticSkip` callsite. When a fresh diagnostic
    /// re-proposes an already-exhausted cluster, the helper walks to the
    /// next unexhausted cluster — and the new plan must look **fresh**
    /// (plan.gen < job.gen) so the controller does NOT route back to
    /// `NeedDiagnostic`. That would be a livelock: we just finished a
    /// fresh diagnostic, and routing back would consume the diagnostic
    /// budget for nothing.
    ///
    /// CB-017 A''' (Commit 5): natural-flow variant. Earlier revisions of
    /// this test hand-tuned `assessment.repair_target_hint` to a cluster B
    /// path so the post-advance `verifier_repair_decision` could resolve a
    /// target file on disk. Commit 4 introduced
    /// `rebind_legacy_assessment_to_current_cluster`, which is now invoked
    /// at the production `DiagnosticSkip` callsite immediately after the
    /// preserve-exhausted helper. The rebind transparently overwrites the
    /// assessment slice with cluster B's `admitted_cluster_targets`, so
    /// the test no longer needs to seed the cluster B path manually —
    /// removing the "manual B target" blind spot the Codex review called
    /// out. The test now drives the production sequence verbatim
    /// (preserve-exhausted → rebind) and asserts that the assessment ends
    /// up aligned with cluster B's fixture-admitted target (`app/main_1.py`).
    #[test]
    fn cb016_diagnostic_skip_advance_produces_fresh_plan() {
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

        // The fixture seeds every cluster with a synthetic admitted target
        // at `app/main_{idx}.py`. After the natural rebind, cluster B's
        // assessment slot will point at `app/main_1.py` — we materialise
        // that file on disk so `verifier_repair_context_target_path` can
        // resolve a real path for the decision check below. NO manual
        // assessment.repair_target_hint setting (CB-017 A''' natural flow).
        let cluster_b_fixture_target = "app/main_1.py";
        let fresh_target_path = work_root.join(cluster_b_fixture_target);
        if let Some(parent) = fresh_target_path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&fresh_target_path, "# fresh cluster B target\n").unwrap();

        // Job posture: cluster A exhausted (verifier rerun no-progress
        // pushed it onto the ledger and walked to cluster B on a prior
        // turn). The fresh diagnostic just ran and bumped the assessment
        // generation to 2. The assessment slot still carries cluster A's
        // **stale** target (no manual cluster B seeding) — the rebind
        // helper will realign it to cluster B's admitted target during
        // the production sequence below.
        let stale_cluster_a_hint = super::super::task_contract::RecoveryTargetHint {
            role: super::super::task_contract::ArtifactRole::Implementation,
            path: "app/main_0.py".to_string(),
            reason: "stale cluster A target — rebind must overwrite this".to_string(),
        };
        let plan_b_before = SemanticRepairPlan {
            semantic_report: report.clone(),
            failure_cluster_id: cluster_ids[1].clone(),
            semantic_cause: report.failure_kind,
            spec_authority: SpecAuthority::ImplementationContract,
            preferred_repair_role: role,
            repair_hypothesis: report.repair_hypothesis.clone(),
            expected_improvement: None,
            // Plan was created at the previous gen (CB-015 RerunNoProgress
            // path); pre-diagnostic generation = 1.
            assessment_generation_at_creation: 1,
        };
        let mut job = RepairJob {
            semantic_plan: Some(plan_b_before),
            exhausted_attempts: vec![(cluster_ids[0].clone(), role)],
            assessment: Some(super::super::VerifierRepairAssessment {
                failure_kind: VerifierDiagnosticFailureKind::AssertionMismatch,
                failure_type: VerifierFailureType::Unknown,
                probable_cause_role: Some(role),
                needed_reads: Vec::new(),
                repair_target_hint: Some(stale_cluster_a_hint.clone()),
                repair_plan: vec![stale_cluster_a_hint],
                summary: None,
                source: super::super::VerifierRepairAssessmentSource::DiagnosticPass,
            }),
            // Fresh diagnostic just landed → assessment_generation bumped to 2.
            assessment_generation: 2,
            // Prior bind: cluster A (transition to B will land via rebind).
            assessment_bound_cluster_id: Some(cluster_ids[0].clone()),
            ..RepairJob::new_for_test()
        };

        // Fresh diagnostic produced a plan that re-proposes cluster A
        // (LLM has no memory of the previous turn's exhaustion). The
        // plan's stamp matches the current generation (= 2) because
        // `run_verifier_diagnostic_pass` threads the pre-bump value
        // (which was 1) into the candidate plan after the assessment
        // landed; but the precise stamp is what
        // `build_semantic_repair_plan_from_report_with_authority_input`
        // produces — we only need the (cluster_id, role) to be exhausted
        // to drive the skip path.
        let proposed_plan_a = SemanticRepairPlan {
            semantic_report: report.clone(),
            failure_cluster_id: cluster_ids[0].clone(),
            semantic_cause: report.failure_kind,
            spec_authority: SpecAuthority::ImplementationContract,
            preferred_repair_role: role,
            repair_hypothesis: report.repair_hypothesis.clone(),
            expected_improvement: None,
            assessment_generation_at_creation: 1,
        };

        // Mirror the production sequence in run_verifier_diagnostic_pass:
        //   1. assign_semantic_plan_preserving_exhausted (DiagnosticSkip)
        //      walks past cluster A to cluster B.
        //   2. rebind_legacy_assessment_to_current_cluster realigns the
        //      assessment slice to cluster B's admitted targets — NO
        //      manual cluster B seeding required (CB-017 A''' blind-spot fix).
        assign_semantic_plan_preserving_exhausted(&mut job, Some(proposed_plan_a), Some(&report));
        rebind_legacy_assessment_to_current_cluster(&mut job);

        let advanced = job
            .semantic_plan
            .as_ref()
            .expect("DiagnosticSkip must land on cluster B");
        assert_eq!(
            advanced.failure_cluster_id, cluster_ids[1],
            "DiagnosticSkip must walk past exhausted cluster A to cluster B",
        );
        // CB-017 A''' natural flow: the rebind helper automatically
        // realigned the assessment slot to cluster B's fixture-admitted
        // target. No manual cluster B seeding was performed on the
        // assessment — the test would have failed under the pre-Commit-4
        // implementation that left assessment.repair_target_hint stuck on
        // cluster A.
        let assessment = job
            .assessment
            .as_ref()
            .expect("rebind preserves assessment");
        let cluster_b_admitted = report.failure_clusters[1].admitted_cluster_targets[0].clone();
        assert_eq!(
            assessment.repair_target_hint.as_ref(),
            Some(&cluster_b_admitted),
            "CB-017 A''' natural flow: rebind must point repair_target_hint at cluster B's admitted target without manual seeding",
        );
        assert_eq!(
            assessment
                .repair_target_hint
                .as_ref()
                .map(|h| h.path.as_str()),
            Some(cluster_b_fixture_target),
            "CB-017 A''' natural flow: cluster B fixture target path is app/main_1.py — not a manually injected app/cluster_b.py",
        );
        assert_eq!(
            assessment.repair_plan,
            vec![cluster_b_admitted.clone()],
            "rebind must overwrite repair_plan with cluster B targets clone",
        );
        assert_eq!(
            assessment.needed_reads,
            vec![cluster_b_admitted],
            "rebind must overwrite needed_reads with cluster B targets clone",
        );
        assert_eq!(
            job.assessment_bound_cluster_id.as_ref(),
            Some(&cluster_ids[1]),
            "bound id must transition to cluster B after rebind",
        );
        // CB-016 core invariant: the new plan looks fresh
        // (plan.gen < job.gen) so the stale predicate does NOT fire.
        assert!(
            !semantic_plan_is_stale(&job),
            "CB-016: DiagnosticSkip post-advance plan must be fresh (plan.gen < job.gen)",
        );

        // The decision must NOT route back to NeedDiagnostic /
        // DiagnosticUnavailable — that would be the V8 livelock CB-016
        // closes.
        let dec = verifier_repair_decision(true, Some(&job), &[], &work_root, Some(0), 0);
        assert_ne!(
            dec,
            VerifierRepairDecision::NeedDiagnostic,
            "CB-016: post DiagnosticSkip, the controller must NOT re-route through NeedDiagnostic",
        );
        assert_ne!(
            dec,
            VerifierRepairDecision::DiagnosticUnavailable,
            "CB-016: post DiagnosticSkip, the controller must NOT fail closed as DiagnosticUnavailable",
        );
    }

    /// CB-016.2 (production integration scenario): the legacy `RerunNoProgress`
    /// path through `apply_semantic_repair_dispatch_after_rerun` must keep
    /// its stale-stamp behaviour intact — that is the precondition for
    /// CB-012/CB-014 to force a fresh diagnostic between cluster A and
    /// cluster B repair attempts.
    #[test]
    fn cb016_rerun_no_progress_advance_produces_stale_plan() {
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

        // Job posture: a successful diagnostic produced a plan targeting
        // cluster A at generation 1; the assessment is in the slot too.
        let stale_hint = super::super::task_contract::RecoveryTargetHint {
            role: super::super::task_contract::ArtifactRole::Implementation,
            path: "app/cluster_a.py".to_string(),
            reason: "cluster A repair".to_string(),
        };
        let plan_a = SemanticRepairPlan {
            semantic_report: report.clone(),
            failure_cluster_id: cluster_ids[0].clone(),
            semantic_cause: report.failure_kind,
            spec_authority: SpecAuthority::ImplementationContract,
            preferred_repair_role: role,
            repair_hypothesis: report.repair_hypothesis.clone(),
            expected_improvement: None,
            assessment_generation_at_creation: 1,
        };
        let mut job = RepairJob {
            semantic_plan: Some(plan_a),
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
            assessment_generation: 1,
            rerun_outcome: Some(VerifierRepairRerunOutcome::SameFailureRemaining),
            ..RepairJob::new_for_test()
        };

        // RerunNoProgress path: dispatch advances the slot to cluster B
        // with the stale assessment carried over.
        apply_semantic_repair_dispatch_after_rerun(&mut job, Some(&cluster_ids[0]));

        let advanced = job
            .semantic_plan
            .as_ref()
            .expect("RerunNoProgress must land on cluster B");
        assert_eq!(
            advanced.failure_cluster_id, cluster_ids[1],
            "RerunNoProgress must advance past cluster A to cluster B",
        );
        // CB-016 core invariant for the legacy path: the new plan is
        // stale (plan.gen == job.gen) so CB-012 forces a fresh diagnostic.
        assert_eq!(
            advanced.assessment_generation_at_creation, job.assessment_generation,
            "CB-016: RerunNoProgress post-advance must stamp plan.gen = job.gen",
        );
        assert!(
            semantic_plan_is_stale(&job),
            "CB-016: RerunNoProgress post-advance plan must be stale (CB-015 invariant)",
        );

        // The decision must route through NeedDiagnostic (CB-012 path) —
        // that is the existing behaviour the legacy `RerunNoProgress`
        // semantic preserves.
        let dec = verifier_repair_decision(true, Some(&job), &[], &work_root, Some(0), 0);
        assert_eq!(
            dec,
            VerifierRepairDecision::NeedDiagnostic,
            "CB-016: post RerunNoProgress, controller must route to NeedDiagnostic (CB-012)",
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

    // ─── CB-017 A''' (Commit 3): first/next_repairable_cluster ─────────────

    /// CR-6 V2: `first_repairable_cluster` skips clusters whose
    /// `admitted_cluster_targets` is empty, returning the first cluster with
    /// at least one admitted target.
    #[test]
    fn cb017_first_repairable_cluster_skips_targetless() {
        // Build a 3-cluster fixture (every cluster pre-seeded with admitted
        // targets), then clear the first cluster's admitted list. The helper
        // must walk past the empty first cluster and return the second.
        let mut report =
            multi_cluster_report_fixture(VerifierDiagnosticFailureKind::AssertionMismatch, 3);
        report.failure_clusters[0].admitted_cluster_targets.clear();
        let chosen = super::first_repairable_cluster(&report).expect("second cluster repairable");
        assert_eq!(chosen.cluster_key, report.failure_clusters[1].cluster_key);

        // If we clear every cluster, the helper returns None.
        for cluster in report.failure_clusters.iter_mut() {
            cluster.admitted_cluster_targets.clear();
        }
        assert!(super::first_repairable_cluster(&report).is_none());
    }

    /// CR-6 V2: `next_repairable_cluster` distinguishes the same cluster
    /// under different `RepairRole`s — an entry `(cluster_A, Implementation)`
    /// does NOT block a later attempt on `(cluster_A, Test)`.
    #[test]
    fn cb017_next_repairable_distinguishes_same_cluster_different_role() {
        let report =
            multi_cluster_report_fixture(VerifierDiagnosticFailureKind::AssertionMismatch, 2);
        let cluster_a = report.failure_clusters[0].cluster_key.clone();
        // Cluster A exhausted under Implementation; query under Test should
        // still admit Cluster A.
        let exhausted = vec![(
            cluster_a.clone(),
            super::super::task_contract::ArtifactRole::Implementation,
        )];
        let chosen = super::next_repairable_cluster(
            &report,
            super::super::task_contract::ArtifactRole::Test,
            &exhausted,
        )
        .expect("cluster A still repairable under Test role");
        assert_eq!(chosen.cluster_key, cluster_a);
        // Same query under Implementation must skip A and pick B.
        let chosen = super::next_repairable_cluster(
            &report,
            super::super::task_contract::ArtifactRole::Implementation,
            &exhausted,
        )
        .expect("cluster B repairable under Implementation");
        assert_eq!(chosen.cluster_key, report.failure_clusters[1].cluster_key);
    }

    /// CR-6 V2 (fallback test): when every cluster of the report is targetless
    /// (no LLM-supplied target_paths, no merged legacy targets, or all
    /// candidates rejected by admission), the controller falls back to the
    /// legacy assessment — represented here by `first_repairable_cluster`
    /// returning `None`.
    #[test]
    fn cb017_all_clusters_targetless_falls_back_to_legacy_assessment() {
        let mut report =
            multi_cluster_report_fixture(VerifierDiagnosticFailureKind::AssertionMismatch, 3);
        for cluster in report.failure_clusters.iter_mut() {
            cluster.admitted_cluster_targets.clear();
        }
        assert!(super::first_repairable_cluster(&report).is_none());
        assert!(
            super::next_repairable_cluster(
                &report,
                super::super::task_contract::ArtifactRole::Implementation,
                &[]
            )
            .is_none(),
            "next_repairable_cluster must skip targetless clusters",
        );
    }

    // ─── CB-017 A''' (Commit 4): rebind + assessment_bound_cluster_id ─────

    /// Helper: build a `VerifierRepairAssessment` with caller-supplied
    /// target slices so transition tests can compare pre/post rebind state
    /// without re-typing the struct literal at every call site.
    fn assessment_with_targets(
        repair_target_hint: Option<RecoveryTargetHint>,
        repair_plan: Vec<RecoveryTargetHint>,
        needed_reads: Vec<RecoveryTargetHint>,
    ) -> super::super::VerifierRepairAssessment {
        super::super::VerifierRepairAssessment {
            failure_kind: VerifierDiagnosticFailureKind::AssertionMismatch,
            failure_type: VerifierFailureType::Unknown,
            probable_cause_role: Some(super::super::task_contract::ArtifactRole::Implementation),
            needed_reads,
            repair_target_hint,
            repair_plan,
            summary: None,
            source: super::super::VerifierRepairAssessmentSource::DiagnosticPass,
        }
    }

    fn impl_hint(path: &str) -> RecoveryTargetHint {
        RecoveryTargetHint {
            role: super::super::task_contract::ArtifactRole::Implementation,
            path: path.to_string(),
            reason: "rebind fixture".to_string(),
        }
    }

    /// CR-4 V2: cluster_key 変化時のみ `applied_repair_intents.clear()`。
    /// bound = cluster A、plan = cluster B → transition と判定 → clear。
    #[test]
    fn cb017_rebind_clears_applied_intents_on_cluster_id_transition() {
        let report =
            multi_cluster_report_fixture(VerifierDiagnosticFailureKind::AssertionMismatch, 2);
        let cluster_ids: Vec<_> = report
            .failure_clusters
            .iter()
            .map(|c| c.cluster_key.clone())
            .collect();
        let role = report.preferred_repair_role;

        // semantic_plan は cluster B を指している (admitted targets が seed されている)
        let plan_b = SemanticRepairPlan {
            semantic_report: report.clone(),
            failure_cluster_id: cluster_ids[1].clone(),
            semantic_cause: report.failure_kind,
            spec_authority: SpecAuthority::ImplementationContract,
            preferred_repair_role: role,
            repair_hypothesis: report.repair_hypothesis.clone(),
            expected_improvement: None,
            assessment_generation_at_creation: 0,
        };
        let stale_hint = impl_hint("app/cluster_a.py");
        let mut job = RepairJob {
            semantic_plan: Some(plan_b),
            assessment: Some(assessment_with_targets(
                Some(stale_hint.clone()),
                vec![stale_hint.clone()],
                vec![stale_hint],
            )),
            applied_repair_intents: vec!["intent-from-cluster-a".to_string()],
            assessment_bound_cluster_id: Some(cluster_ids[0].clone()),
            ..RepairJob::new_for_test()
        };

        rebind_legacy_assessment_to_current_cluster(&mut job);

        assert!(
            job.applied_repair_intents.is_empty(),
            "cluster_key transition (A → B) must clear applied_repair_intents",
        );
        assert_eq!(
            job.assessment_bound_cluster_id.as_ref(),
            Some(&cluster_ids[1]),
            "bound id must be re-bound to the new cluster",
        );
    }

    /// CR-4 V2: bound と plan の cluster_id が一致するときは
    /// `applied_repair_intents` を維持。
    #[test]
    fn cb017_rebind_does_not_clear_applied_intents_within_same_cluster() {
        let report =
            multi_cluster_report_fixture(VerifierDiagnosticFailureKind::AssertionMismatch, 2);
        let cluster_ids: Vec<_> = report
            .failure_clusters
            .iter()
            .map(|c| c.cluster_key.clone())
            .collect();
        let role = report.preferred_repair_role;

        let plan_b = SemanticRepairPlan {
            semantic_report: report.clone(),
            failure_cluster_id: cluster_ids[1].clone(),
            semantic_cause: report.failure_kind,
            spec_authority: SpecAuthority::ImplementationContract,
            preferred_repair_role: role,
            repair_hypothesis: report.repair_hypothesis.clone(),
            expected_improvement: None,
            assessment_generation_at_creation: 0,
        };
        // bound = B (same cluster) → no transition.
        let mut job = RepairJob {
            semantic_plan: Some(plan_b),
            assessment: Some(assessment_with_targets(
                Some(impl_hint("app/cluster_b.py")),
                vec![impl_hint("app/cluster_b.py")],
                vec![impl_hint("app/cluster_b.py")],
            )),
            applied_repair_intents: vec!["intent-for-cluster-b".to_string()],
            assessment_bound_cluster_id: Some(cluster_ids[1].clone()),
            ..RepairJob::new_for_test()
        };

        rebind_legacy_assessment_to_current_cluster(&mut job);

        assert_eq!(
            job.applied_repair_intents,
            vec!["intent-for-cluster-b".to_string()],
            "no cluster_key transition → applied_repair_intents must be preserved",
        );
        assert_eq!(
            job.assessment_bound_cluster_id.as_ref(),
            Some(&cluster_ids[1]),
            "bound id stays on cluster B",
        );
    }

    /// CR-4 V2: path/role が同じでも `cluster_key` が違えば transition。
    /// path/role 比較ではなく cluster_key の差分で判定することを固定する。
    #[test]
    fn cb017_rebind_handles_same_file_role_in_different_clusters() {
        let mut report =
            multi_cluster_report_fixture(VerifierDiagnosticFailureKind::AssertionMismatch, 2);
        // 両 cluster の admitted_cluster_targets を同じ path/role にする
        // (multi_cluster_report_fixture は idx で path を差別化するので
        // 手動で揃える)。
        let shared_hint = impl_hint("app/shared.py");
        for cluster in report.failure_clusters.iter_mut() {
            cluster.admitted_cluster_targets = vec![shared_hint.clone()];
        }
        let cluster_ids: Vec<_> = report
            .failure_clusters
            .iter()
            .map(|c| c.cluster_key.clone())
            .collect();
        assert_ne!(
            cluster_ids[0], cluster_ids[1],
            "cluster_keys must differ even when paths are identical",
        );
        let role = report.preferred_repair_role;

        let plan_b = SemanticRepairPlan {
            semantic_report: report.clone(),
            failure_cluster_id: cluster_ids[1].clone(),
            semantic_cause: report.failure_kind,
            spec_authority: SpecAuthority::ImplementationContract,
            preferred_repair_role: role,
            repair_hypothesis: report.repair_hypothesis.clone(),
            expected_improvement: None,
            assessment_generation_at_creation: 0,
        };
        // bound = cluster A (same path/role as cluster B).
        let mut job = RepairJob {
            semantic_plan: Some(plan_b),
            assessment: Some(assessment_with_targets(
                Some(shared_hint.clone()),
                vec![shared_hint.clone()],
                vec![shared_hint],
            )),
            applied_repair_intents: vec!["intent-from-cluster-a".to_string()],
            assessment_bound_cluster_id: Some(cluster_ids[0].clone()),
            ..RepairJob::new_for_test()
        };

        rebind_legacy_assessment_to_current_cluster(&mut job);

        assert!(
            job.applied_repair_intents.is_empty(),
            "CR-4 V2: same path/role but different cluster_key must still be detected as transition",
        );
        assert_eq!(
            job.assessment_bound_cluster_id.as_ref(),
            Some(&cluster_ids[1])
        );
    }

    /// CR-7 V2: `needed_reads` is overwritten with the current cluster's
    /// admitted targets (Vec<RecoveryTargetHint> clone), not the previous
    /// cluster's stale paths. Closes the `needed_reads` fallback that
    /// re-promoted cluster A paths after advance.
    #[test]
    fn cb017_rebind_writes_needed_reads_to_current_cluster_paths_only() {
        let mut report =
            multi_cluster_report_fixture(VerifierDiagnosticFailureKind::AssertionMismatch, 2);
        // cluster B の admitted targets を確定値に差し替え (multi_cluster fixture は
        // idx ベースなので、明示する)。
        let cluster_b_target = impl_hint("app/cluster_b.py");
        report.failure_clusters[1].admitted_cluster_targets = vec![cluster_b_target.clone()];
        let cluster_ids: Vec<_> = report
            .failure_clusters
            .iter()
            .map(|c| c.cluster_key.clone())
            .collect();
        let role = report.preferred_repair_role;

        let plan_b = SemanticRepairPlan {
            semantic_report: report.clone(),
            failure_cluster_id: cluster_ids[1].clone(),
            semantic_cause: report.failure_kind,
            spec_authority: SpecAuthority::ImplementationContract,
            preferred_repair_role: role,
            repair_hypothesis: report.repair_hypothesis.clone(),
            expected_improvement: None,
            assessment_generation_at_creation: 0,
        };
        let stale_a = impl_hint("app/cluster_a.py");
        let mut job = RepairJob {
            semantic_plan: Some(plan_b),
            assessment: Some(assessment_with_targets(
                Some(stale_a.clone()),
                vec![stale_a.clone()],
                vec![stale_a.clone()],
            )),
            assessment_bound_cluster_id: Some(cluster_ids[0].clone()),
            ..RepairJob::new_for_test()
        };

        rebind_legacy_assessment_to_current_cluster(&mut job);

        let updated = job.assessment.as_ref().expect("assessment preserved");
        assert_eq!(
            updated.needed_reads,
            vec![cluster_b_target.clone()],
            "needed_reads must be overwritten with current cluster targets (no stale cluster A path)",
        );
        // Stale A path must not survive anywhere.
        assert!(
            !updated.needed_reads.iter().any(|h| h.path == stale_a.path),
            "stale cluster A path must not appear in needed_reads after rebind",
        );
    }

    /// CR-7 V2: `repair_plan` is written as a verbatim clone of the current
    /// cluster's admitted targets — preserving order and role exactly so the
    /// controller can iterate the slice without re-classifying.
    #[test]
    fn cb017_rebind_writes_repair_plan_as_targets_clone() {
        let mut report =
            multi_cluster_report_fixture(VerifierDiagnosticFailureKind::AssertionMismatch, 2);
        // Multiple targets on cluster B so we can assert order preservation.
        let t1 = impl_hint("app/b1.py");
        let t2 = impl_hint("app/b2.py");
        let t3 = impl_hint("app/b3.py");
        report.failure_clusters[1].admitted_cluster_targets =
            vec![t1.clone(), t2.clone(), t3.clone()];
        let cluster_ids: Vec<_> = report
            .failure_clusters
            .iter()
            .map(|c| c.cluster_key.clone())
            .collect();
        let role = report.preferred_repair_role;

        let plan_b = SemanticRepairPlan {
            semantic_report: report.clone(),
            failure_cluster_id: cluster_ids[1].clone(),
            semantic_cause: report.failure_kind,
            spec_authority: SpecAuthority::ImplementationContract,
            preferred_repair_role: role,
            repair_hypothesis: report.repair_hypothesis.clone(),
            expected_improvement: None,
            assessment_generation_at_creation: 0,
        };
        let mut job = RepairJob {
            semantic_plan: Some(plan_b),
            assessment: Some(assessment_with_targets(None, Vec::new(), Vec::new())),
            assessment_bound_cluster_id: None,
            ..RepairJob::new_for_test()
        };

        rebind_legacy_assessment_to_current_cluster(&mut job);

        let updated = job.assessment.as_ref().expect("assessment preserved");
        assert_eq!(
            updated.repair_plan,
            vec![t1.clone(), t2.clone(), t3.clone()],
            "repair_plan must be a verbatim clone of current cluster targets",
        );
        assert_eq!(
            updated.repair_target_hint.as_ref(),
            Some(&t1),
            "repair_target_hint must be the first target (clone)",
        );
    }

    /// Legacy path preservation: `semantic_plan = None` → rebind is a no-op.
    #[test]
    fn cb017_rebind_no_op_when_semantic_plan_none() {
        let stale_hint = impl_hint("app/legacy.py");
        let mut job = RepairJob {
            semantic_plan: None,
            assessment: Some(assessment_with_targets(
                Some(stale_hint.clone()),
                vec![stale_hint.clone()],
                vec![stale_hint.clone()],
            )),
            applied_repair_intents: vec!["legacy-intent".to_string()],
            assessment_bound_cluster_id: None,
            ..RepairJob::new_for_test()
        };

        let before = job.clone();
        rebind_legacy_assessment_to_current_cluster(&mut job);

        assert_eq!(
            job, before,
            "rebind must be a complete no-op when semantic_plan is None",
        );
    }

    /// Caller-skipped invariant: targetless cluster (admitted_cluster_targets
    /// empty) → rebind is a no-op. `first_repairable_cluster` /
    /// `next_repairable_cluster` already skip these, so reaching the rebind
    /// helper with an empty target list is defensive.
    #[test]
    fn cb017_rebind_no_op_when_cluster_targetless() {
        let mut report =
            multi_cluster_report_fixture(VerifierDiagnosticFailureKind::AssertionMismatch, 2);
        // Clear the second cluster's admitted targets to make it targetless.
        report.failure_clusters[1].admitted_cluster_targets.clear();
        let cluster_ids: Vec<_> = report
            .failure_clusters
            .iter()
            .map(|c| c.cluster_key.clone())
            .collect();
        let role = report.preferred_repair_role;

        let plan_b = SemanticRepairPlan {
            semantic_report: report.clone(),
            failure_cluster_id: cluster_ids[1].clone(),
            semantic_cause: report.failure_kind,
            spec_authority: SpecAuthority::ImplementationContract,
            preferred_repair_role: role,
            repair_hypothesis: report.repair_hypothesis.clone(),
            expected_improvement: None,
            assessment_generation_at_creation: 0,
        };
        let stale_a = impl_hint("app/cluster_a.py");
        let mut job = RepairJob {
            semantic_plan: Some(plan_b),
            assessment: Some(assessment_with_targets(
                Some(stale_a.clone()),
                vec![stale_a.clone()],
                vec![stale_a.clone()],
            )),
            applied_repair_intents: vec!["intent".to_string()],
            assessment_bound_cluster_id: Some(cluster_ids[0].clone()),
            ..RepairJob::new_for_test()
        };

        let before = job.clone();
        rebind_legacy_assessment_to_current_cluster(&mut job);

        assert_eq!(
            job, before,
            "rebind must be a complete no-op when the current cluster has no admitted targets",
        );
    }

    /// CR-4 V2 carry-over invariant: a fresh-built RepairJob preserves the
    /// `assessment_bound_cluster_id` field shape across `Clone` so
    /// `verifier_repair_context_from_failure` can carry it over previous
    /// turn contexts.
    #[test]
    fn cb017_assessment_bound_cluster_id_carry_over_from_previous_context() {
        let report =
            multi_cluster_report_fixture(VerifierDiagnosticFailureKind::AssertionMismatch, 2);
        let cluster_a = report.failure_clusters[0].cluster_key.clone();

        // Mutating the new field breaks PartialEq, confirming the field
        // participates in equality and `Clone` preserves it.
        let mut job = RepairJob::new_for_test();
        assert!(job.assessment_bound_cluster_id.is_none());
        job.assessment_bound_cluster_id = Some(cluster_a.clone());
        let cloned = job.clone();
        assert_eq!(
            cloned.assessment_bound_cluster_id.as_ref(),
            Some(&cluster_a)
        );
        assert_eq!(job, cloned);

        let mut diverged = job.clone();
        diverged.assessment_bound_cluster_id = None;
        assert_ne!(job, diverged);
    }

    /// CR-4 V2: `apply_semantic_repair_dispatch_after_rerun` must reset
    /// `assessment_bound_cluster_id` to `None` whenever it clears the
    /// assessment for a re-diagnostic round (both `should_re_diagnostic`
    /// path and "no more clusters" path).
    #[test]
    fn cb017_assessment_bound_cluster_id_reset_on_dispatch_re_diagnostic() {
        let report =
            multi_cluster_report_fixture(VerifierDiagnosticFailureKind::AssertionMismatch, 2);
        let cluster_ids: Vec<_> = report
            .failure_clusters
            .iter()
            .map(|c| c.cluster_key.clone())
            .collect();
        let role = report.preferred_repair_role;

        // 1. should_re_diagnostic = true path (plan targets exhausted cluster).
        let plan_a = SemanticRepairPlan {
            semantic_report: report.clone(),
            failure_cluster_id: cluster_ids[0].clone(),
            semantic_cause: report.failure_kind,
            spec_authority: SpecAuthority::ImplementationContract,
            preferred_repair_role: role,
            repair_hypothesis: report.repair_hypothesis.clone(),
            expected_improvement: None,
            assessment_generation_at_creation: 0,
        };
        let mut job = RepairJob {
            semantic_plan: Some(plan_a),
            exhausted_attempts: vec![(cluster_ids[0].clone(), role)],
            assessment: Some(assessment_with_targets(None, Vec::new(), Vec::new())),
            rerun_outcome: Some(VerifierRepairRerunOutcome::SameFailureRemaining),
            assessment_bound_cluster_id: Some(cluster_ids[0].clone()),
            ..RepairJob::new_for_test()
        };
        apply_semantic_repair_dispatch_after_rerun(&mut job, Some(&cluster_ids[0]));
        assert!(
            job.assessment.is_none(),
            "re-diagnostic path clears assessment"
        );
        assert!(
            job.assessment_bound_cluster_id.is_none(),
            "re-diagnostic (already-exhausted plan) must reset bound id to None",
        );

        // 2. "no more clusters" path: plan present, rerun bad, no remaining clusters.
        let report_one =
            multi_cluster_report_fixture(VerifierDiagnosticFailureKind::AssertionMismatch, 1);
        let only_cluster = report_one.failure_clusters[0].cluster_key.clone();
        let mut job2 = job_with_first_cluster_plan(&report_one);
        job2.assessment = Some(assessment_with_targets(None, Vec::new(), Vec::new()));
        job2.rerun_outcome = Some(VerifierRepairRerunOutcome::Worsened);
        job2.assessment_bound_cluster_id = Some(only_cluster.clone());

        apply_semantic_repair_dispatch_after_rerun(&mut job2, Some(&only_cluster));
        assert!(
            job2.semantic_plan.is_none(),
            "no more clusters → slot cleared"
        );
        assert!(
            job2.assessment.is_none(),
            "no more clusters → assessment cleared"
        );
        assert!(
            job2.assessment_bound_cluster_id.is_none(),
            "no-more-clusters fallback must reset bound id to None",
        );
    }

    /// CB-017 A''' (Commit 5, CR-4 V2 lifecycle): `assessment_bound_cluster_id`
    /// must be reset to `None` on every dispatch path that drops
    /// `assessment` to `None`. This fixes the bound id and `assessment`
    /// option in lock-step so the next diagnostic-pass rebind treats the
    /// new assessment as a "new bind" (transition → clears
    /// `applied_repair_intents`).
    ///
    /// Complements `cb017_assessment_bound_cluster_id_reset_on_dispatch_re_diagnostic`
    /// by isolating the invariant "whenever assessment falls to None, the
    /// bound id falls to None too" — explicitly required by Codex review 4.
    #[test]
    fn cb017_assessment_bound_cluster_id_reset_when_assessment_falls_to_none() {
        let report =
            multi_cluster_report_fixture(VerifierDiagnosticFailureKind::AssertionMismatch, 1);
        let only_cluster = report.failure_clusters[0].cluster_key.clone();
        let role = report.preferred_repair_role;

        // Construct a job that is in the "rerun bad, plan exhausted" posture
        // — should_re_diagnostic = true → dispatch will null the
        // assessment AND must null the bound id in lock-step.
        let plan_only = SemanticRepairPlan {
            semantic_report: report.clone(),
            failure_cluster_id: only_cluster.clone(),
            semantic_cause: report.failure_kind,
            spec_authority: SpecAuthority::ImplementationContract,
            preferred_repair_role: role,
            repair_hypothesis: report.repair_hypothesis.clone(),
            expected_improvement: None,
            assessment_generation_at_creation: 0,
        };
        let mut job = RepairJob {
            semantic_plan: Some(plan_only),
            exhausted_attempts: vec![(only_cluster.clone(), role)],
            assessment: Some(assessment_with_targets(
                Some(impl_hint("app/main_0.py")),
                vec![impl_hint("app/main_0.py")],
                vec![impl_hint("app/main_0.py")],
            )),
            rerun_outcome: Some(VerifierRepairRerunOutcome::SameFailureRemaining),
            assessment_bound_cluster_id: Some(only_cluster.clone()),
            ..RepairJob::new_for_test()
        };

        apply_semantic_repair_dispatch_after_rerun(&mut job, Some(&only_cluster));

        // Invariant: assessment-None ↔ bound-id-None (lock-step reset).
        assert!(
            job.assessment.is_none(),
            "dispatch must clear assessment when re-diagnostic is needed",
        );
        assert!(
            job.assessment_bound_cluster_id.is_none(),
            "CR-4 V2: assessment-None implies bound-id-None (lock-step lifecycle)",
        );
    }

    /// CB-017 A''' (Commit 5, CR-4 V2 lifecycle): the legacy path
    /// (`semantic_plan = None`) must not touch `assessment_bound_cluster_id`.
    /// `rebind_legacy_assessment_to_current_cluster` is a complete no-op in
    /// this case — including leaving the bound id untouched (whatever it
    /// was — `Some(...)` or `None` — before the call must match after).
    ///
    /// This explicitly locks the invariant Codex review 4 called out:
    /// "legacy 経路 (semantic_plan = None) では bound は変更されない".
    /// Complements `cb017_rebind_no_op_when_semantic_plan_none` (which
    /// fixes the entire job equality) by naming the bound-id invariant
    /// directly.
    #[test]
    fn cb017_assessment_bound_cluster_id_unaffected_when_semantic_plan_is_none() {
        let report =
            multi_cluster_report_fixture(VerifierDiagnosticFailureKind::AssertionMismatch, 1);
        let stale_bound = report.failure_clusters[0].cluster_key.clone();

        // Bound id was Some(...) before — must stay Some(...) (unchanged)
        // when the rebind sees semantic_plan = None.
        let mut job_with_some_bound = RepairJob {
            semantic_plan: None,
            assessment: Some(assessment_with_targets(
                Some(impl_hint("app/legacy.py")),
                vec![impl_hint("app/legacy.py")],
                vec![impl_hint("app/legacy.py")],
            )),
            assessment_bound_cluster_id: Some(stale_bound.clone()),
            ..RepairJob::new_for_test()
        };
        rebind_legacy_assessment_to_current_cluster(&mut job_with_some_bound);
        assert_eq!(
            job_with_some_bound.assessment_bound_cluster_id.as_ref(),
            Some(&stale_bound),
            "legacy path (semantic_plan=None): bound id Some(_) must remain unchanged",
        );

        // Bound id was None before — must stay None when the rebind sees
        // semantic_plan = None.
        let mut job_with_none_bound = RepairJob {
            semantic_plan: None,
            assessment: Some(assessment_with_targets(
                Some(impl_hint("app/legacy.py")),
                vec![impl_hint("app/legacy.py")],
                vec![impl_hint("app/legacy.py")],
            )),
            assessment_bound_cluster_id: None,
            ..RepairJob::new_for_test()
        };
        rebind_legacy_assessment_to_current_cluster(&mut job_with_none_bound);
        assert!(
            job_with_none_bound.assessment_bound_cluster_id.is_none(),
            "legacy path (semantic_plan=None): bound id None must remain None",
        );
    }

    /// Production-level (CB-017 core fix): `DiagnosticSkip` path rebinds
    /// the legacy assessment slice to cluster B's admitted targets. This is
    /// the scenario fired by `assign_semantic_plan_preserving_exhausted`
    /// inside `run_verifier_diagnostic_pass` when a fresh diagnostic
    /// re-proposes an already-exhausted cluster A.
    #[test]
    fn cb017_diagnostic_skip_rebinds_legacy_assessment_to_cluster_b() {
        let mut report =
            multi_cluster_report_fixture(VerifierDiagnosticFailureKind::AssertionMismatch, 2);
        let cluster_b_target = impl_hint("app/cluster_b.py");
        report.failure_clusters[1].admitted_cluster_targets = vec![cluster_b_target.clone()];
        let cluster_ids: Vec<_> = report
            .failure_clusters
            .iter()
            .map(|c| c.cluster_key.clone())
            .collect();
        let role = report.preferred_repair_role;

        // Posture: cluster A exhausted, fresh diagnostic re-proposed A,
        // dispatch will walk to B via DiagnosticSkip. Legacy assessment
        // currently still has cluster A paths.
        let stale_a = impl_hint("app/cluster_a.py");
        let mut job = RepairJob {
            exhausted_attempts: vec![(cluster_ids[0].clone(), role)],
            assessment: Some(assessment_with_targets(
                Some(stale_a.clone()),
                vec![stale_a.clone()],
                vec![stale_a.clone()],
            )),
            assessment_bound_cluster_id: Some(cluster_ids[0].clone()),
            ..RepairJob::new_for_test()
        };
        let doomed_plan_a = SemanticRepairPlan {
            semantic_report: report.clone(),
            failure_cluster_id: cluster_ids[0].clone(),
            semantic_cause: report.failure_kind,
            spec_authority: SpecAuthority::ImplementationContract,
            preferred_repair_role: role,
            repair_hypothesis: report.repair_hypothesis.clone(),
            expected_improvement: None,
            assessment_generation_at_creation: 0,
        };

        // Mirror the production sequence:
        //   1. assign_semantic_plan_preserving_exhausted walks past A to B.
        //   2. rebind_legacy_assessment_to_current_cluster realigns assessment.
        assign_semantic_plan_preserving_exhausted(&mut job, Some(doomed_plan_a), Some(&report));
        rebind_legacy_assessment_to_current_cluster(&mut job);

        let assessment = job.assessment.as_ref().expect("assessment preserved");
        assert_eq!(
            assessment.repair_target_hint.as_ref(),
            Some(&cluster_b_target),
            "DiagnosticSkip rebind must promote cluster B target as repair_target_hint",
        );
        assert_eq!(
            assessment.repair_plan,
            vec![cluster_b_target.clone()],
            "DiagnosticSkip rebind must overwrite repair_plan with cluster B targets",
        );
        assert_eq!(
            assessment.needed_reads,
            vec![cluster_b_target],
            "DiagnosticSkip rebind must overwrite needed_reads with cluster B targets",
        );
        assert_eq!(
            job.assessment_bound_cluster_id.as_ref(),
            Some(&cluster_ids[1]),
            "bound id must now point at cluster B",
        );
    }

    /// Production-level: `RerunNoProgress` path (dispatch helper) also
    /// rebinds the legacy assessment slice — without this, after a rerun
    /// no-progress advance to cluster B the controller would still route to
    /// cluster A paths via the legacy needed_reads fallback (the bug CB-017
    /// closes for the rerun path too).
    #[test]
    fn cb017_rerun_no_progress_also_rebinds_legacy_assessment() {
        let mut report =
            multi_cluster_report_fixture(VerifierDiagnosticFailureKind::AssertionMismatch, 2);
        let cluster_b_target = impl_hint("app/cluster_b.py");
        report.failure_clusters[1].admitted_cluster_targets = vec![cluster_b_target.clone()];
        let cluster_ids: Vec<_> = report
            .failure_clusters
            .iter()
            .map(|c| c.cluster_key.clone())
            .collect();
        let role = report.preferred_repair_role;

        let plan_a = SemanticRepairPlan {
            semantic_report: report.clone(),
            failure_cluster_id: cluster_ids[0].clone(),
            semantic_cause: report.failure_kind,
            spec_authority: SpecAuthority::ImplementationContract,
            preferred_repair_role: role,
            repair_hypothesis: report.repair_hypothesis.clone(),
            expected_improvement: None,
            assessment_generation_at_creation: 0,
        };
        let stale_a = impl_hint("app/cluster_a.py");
        let mut job = RepairJob {
            semantic_plan: Some(plan_a),
            assessment: Some(assessment_with_targets(
                Some(stale_a.clone()),
                vec![stale_a.clone()],
                vec![stale_a.clone()],
            )),
            rerun_outcome: Some(VerifierRepairRerunOutcome::SameFailureRemaining),
            assessment_bound_cluster_id: Some(cluster_ids[0].clone()),
            ..RepairJob::new_for_test()
        };

        apply_semantic_repair_dispatch_after_rerun(&mut job, Some(&cluster_ids[0]));

        // Slot walked to cluster B AND assessment slice was rebound.
        assert_eq!(
            job.semantic_plan.as_ref().unwrap().failure_cluster_id,
            cluster_ids[1],
        );
        let assessment = job.assessment.as_ref().expect("assessment preserved");
        assert_eq!(
            assessment.repair_target_hint.as_ref(),
            Some(&cluster_b_target),
            "RerunNoProgress advance must rebind repair_target_hint to cluster B",
        );
        assert_eq!(assessment.needed_reads, vec![cluster_b_target]);
        assert_eq!(
            job.assessment_bound_cluster_id.as_ref(),
            Some(&cluster_ids[1]),
        );
    }

    /// Production-level (CR-7 V2): after a `DiagnosticSkip` rebind, the
    /// stale cluster A target must no longer appear anywhere in the
    /// assessment slice — preventing the `verifier_repair_effective_target_hint`
    /// fallback from re-promoting it via `needed_reads`.
    #[test]
    fn cb017_stale_a_target_not_re_promoted_from_needed_reads() {
        let mut report =
            multi_cluster_report_fixture(VerifierDiagnosticFailureKind::AssertionMismatch, 2);
        let cluster_b_target = impl_hint("app/cluster_b.py");
        report.failure_clusters[1].admitted_cluster_targets = vec![cluster_b_target.clone()];
        let cluster_ids: Vec<_> = report
            .failure_clusters
            .iter()
            .map(|c| c.cluster_key.clone())
            .collect();
        let role = report.preferred_repair_role;

        let stale_a = impl_hint("app/cluster_a.py");
        let mut job = RepairJob {
            exhausted_attempts: vec![(cluster_ids[0].clone(), role)],
            assessment: Some(assessment_with_targets(
                Some(stale_a.clone()),
                // Multiple stale entries to make sure none survive.
                vec![stale_a.clone(), stale_a.clone()],
                vec![stale_a.clone(), stale_a.clone(), stale_a.clone()],
            )),
            assessment_bound_cluster_id: Some(cluster_ids[0].clone()),
            ..RepairJob::new_for_test()
        };
        let doomed_plan_a = SemanticRepairPlan {
            semantic_report: report.clone(),
            failure_cluster_id: cluster_ids[0].clone(),
            semantic_cause: report.failure_kind,
            spec_authority: SpecAuthority::ImplementationContract,
            preferred_repair_role: role,
            repair_hypothesis: report.repair_hypothesis.clone(),
            expected_improvement: None,
            assessment_generation_at_creation: 0,
        };

        assign_semantic_plan_preserving_exhausted(&mut job, Some(doomed_plan_a), Some(&report));
        rebind_legacy_assessment_to_current_cluster(&mut job);

        let assessment = job.assessment.as_ref().expect("assessment preserved");
        let stale_path = stale_a.path.as_str();
        assert!(
            !assessment.needed_reads.iter().any(|h| h.path == stale_path),
            "stale cluster A path must not survive in needed_reads",
        );
        assert!(
            !assessment.repair_plan.iter().any(|h| h.path == stale_path),
            "stale cluster A path must not survive in repair_plan",
        );
        assert!(
            assessment
                .repair_target_hint
                .as_ref()
                .is_none_or(|h| h.path != stale_path),
            "stale cluster A path must not remain as repair_target_hint",
        );
    }

    // ---- Issue #653 (Phase 2): RepairAttemptOutcome ledger ---- //

    use super::super::repair_attempt_outcome::{
        MAX_REPAIR_ATTEMPT_OUTCOMES, RepairAttemptOutcome, RepairAttemptOutcomeKind,
        RepairRejectionKind,
    };
    use super::super::semantic_failure::cluster_key_for_test;
    use super::super::spec_authority::WeakeningPattern;
    use super::super::task_contract::ArtifactRole;

    fn outcome_rejected_unsafe(
        cluster_label: &str,
        role: ArtifactRole,
        rejection: RepairRejectionKind,
        pattern: WeakeningPattern,
    ) -> RepairAttemptOutcome {
        RepairAttemptOutcome::for_test(
            cluster_key_for_test(cluster_label),
            role,
            RepairAttemptOutcomeKind::RejectedUnsafe { rejection, pattern },
        )
    }

    fn outcome_applied_no_progress(
        cluster_label: &str,
        role: ArtifactRole,
    ) -> RepairAttemptOutcome {
        RepairAttemptOutcome::for_test(
            cluster_key_for_test(cluster_label),
            role,
            RepairAttemptOutcomeKind::AppliedNoProgress,
        )
    }

    fn outcome_applied_improved(cluster_label: &str, role: ArtifactRole) -> RepairAttemptOutcome {
        RepairAttemptOutcome::for_test(
            cluster_key_for_test(cluster_label),
            role,
            RepairAttemptOutcomeKind::AppliedImproved,
        )
    }

    /// Issue #662: build a `RepairJob` with `semantic_plan = Some(...)` so the
    /// `record_repair_attempt_outcome` precondition (`debug_assert!`) is
    /// satisfied. `cluster_label` becomes the active plan's
    /// `failure_cluster_id` and `role` is its `preferred_repair_role`.
    /// `cluster_label` MUST match the outcome.cluster the test plans to push,
    /// otherwise `next_repairable_cluster` will report the test cluster as
    /// still repairable and `all_clusters_exhausted` will be false.
    #[cfg(test)]
    fn semantic_repair_job_for_test(cluster_label: &str, role: ArtifactRole) -> RepairJob {
        let report = semantic_report_fixture_with_cluster(
            cluster_label,
            VerifierDiagnosticFailureKind::AssertionMismatch,
            0.7,
        );
        let cluster_id = report.failure_clusters[0].cluster_key.clone();
        let plan = SemanticRepairPlan {
            semantic_report: report,
            failure_cluster_id: cluster_id,
            semantic_cause: VerifierDiagnosticFailureKind::AssertionMismatch,
            spec_authority: SpecAuthority::BehaviorContract,
            preferred_repair_role: role,
            repair_hypothesis: "h".to_string(),
            expected_improvement: None,
            assessment_generation_at_creation: 0,
        };
        RepairJob {
            semantic_plan: Some(plan),
            ..RepairJob::new_for_test()
        }
    }

    #[cfg(test)]
    fn semantic_repair_job_with_targets_for_test(
        cluster_label: &str,
        role: ArtifactRole,
        paths: &[&str],
    ) -> (
        RepairJob,
        Vec<super::super::task_contract::RecoveryTargetHint>,
    ) {
        let mut report = semantic_report_fixture_with_cluster(
            cluster_label,
            VerifierDiagnosticFailureKind::AssertionMismatch,
            0.7,
        );
        report.preferred_repair_role = role;
        let targets = paths
            .iter()
            .map(|path| super::super::task_contract::RecoveryTargetHint {
                role,
                path: (*path).to_string(),
                reason: "target exhaustion fixture".to_string(),
            })
            .collect::<Vec<_>>();
        if let Some(cluster) = report.failure_clusters.get_mut(0) {
            cluster.admitted_cluster_targets = targets.clone();
        }
        let cluster_id = report.failure_clusters[0].cluster_key.clone();
        let plan = SemanticRepairPlan {
            semantic_report: report,
            failure_cluster_id: cluster_id,
            semantic_cause: VerifierDiagnosticFailureKind::AssertionMismatch,
            spec_authority: SpecAuthority::BehaviorContract,
            preferred_repair_role: role,
            repair_hypothesis: "h".to_string(),
            expected_improvement: None,
            assessment_generation_at_creation: 0,
        };
        (
            RepairJob {
                semantic_plan: Some(plan),
                ..RepairJob::new_for_test()
            },
            targets,
        )
    }

    /// Issue #662: like `semantic_report_fixture` but with caller-supplied
    /// `cluster_label` and a synthetic admitted target so the cluster is
    /// considered repairable by `next_repairable_cluster`.
    ///
    /// The cluster's `cluster_key` is overridden to match
    /// `cluster_key_for_test(cluster_label)` (the test SSOT used by outcome
    /// fixtures) and `admitted_cluster_targets` is seeded with a single
    /// synthetic `RecoveryTargetHint`. In production these targets are
    /// populated by `turn.rs` after parsing; the test fixture short-circuits
    /// that step.
    #[cfg(test)]
    fn semantic_report_fixture_with_cluster(
        cluster_label: &str,
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
                    "observed": cluster_label,
                    "expected": "exp",
                    "input_shape": "shape",
                    "assertion_shape": "AssertEq",
                    "involved_artifacts": ["test"],
                    "affected_cases": ["case1"],
                }
            ],
        });
        let mut report = super::super::semantic_failure::parse_semantic_failure_report(&json)
            .expect("fixture parses");
        // Override the cluster_key so it matches `cluster_key_for_test(cluster_label)`
        // (which is the deterministic test SSOT used across outcome fixtures).
        // Also seed admitted_cluster_targets with a synthetic hint so
        // `next_repairable_cluster` considers the cluster repairable until
        // it lands in `exhausted_attempts`.
        if let Some(cluster) = report.failure_clusters.get_mut(0) {
            cluster.cluster_key = cluster_key_for_test(cluster_label);
            cluster.admitted_cluster_targets.push(
                super::super::task_contract::RecoveryTargetHint {
                    role: super::super::task_contract::ArtifactRole::Implementation,
                    path: format!("tests/{cluster_label}_smoke.rs"),
                    reason: "semantic fixture".to_string(),
                },
            );
        }
        report
    }

    #[test]
    fn phase3_record_repair_attempt_outcome_initial_state() {
        let mut job = semantic_repair_job_for_test("A", ArtifactRole::Implementation);
        assert!(job.repair_attempt_outcomes.is_empty());
        let outcome = outcome_applied_no_progress("A", ArtifactRole::Implementation);
        let _ = job.record_repair_attempt_outcome(outcome.clone());
        assert_eq!(job.repair_attempt_outcomes.len(), 1);
        assert_eq!(job.repair_attempt_outcomes[0], outcome);
    }

    #[test]
    fn target_path_exhaustion_skips_only_repeated_bad_target() {
        let (mut job, targets) = semantic_repair_job_with_targets_for_test(
            "A",
            ArtifactRole::Test,
            &["tests/a.py", "tests/b.py"],
        );
        let outcome = outcome_rejected_unsafe(
            "A",
            ArtifactRole::Test,
            RepairRejectionKind::TestWeakening,
            WeakeningPattern::AssertionDeleted,
        );

        let first = job.record_repair_attempt_outcome_for_target(outcome.clone(), &targets[0]);
        assert!(!first.promoted);
        assert!(!first.all_clusters_exhausted);
        assert!(!job.is_repair_target_exhausted(
            &cluster_key_for_test("A"),
            ArtifactRole::Test,
            "tests/a.py",
        ));

        let second = job.record_repair_attempt_outcome_for_target(outcome, &targets[0]);
        assert!(!second.promoted);
        assert!(!second.all_clusters_exhausted);
        assert!(job.is_repair_target_exhausted(
            &cluster_key_for_test("A"),
            ArtifactRole::Test,
            "tests/a.py",
        ));
        assert_eq!(
            job.current_unexhausted_semantic_target()
                .expect("tests/b.py remains repairable")
                .path,
            "tests/b.py"
        );
        assert!(
            !job.exhausted_attempts
                .contains(&(cluster_key_for_test("A"), ArtifactRole::Test)),
            "cluster-level exhaustion must wait until all admitted targets are exhausted"
        );
    }

    #[test]
    fn target_path_exhaustion_is_scoped_to_active_cluster() {
        let (mut job, targets) = semantic_repair_job_with_targets_for_test(
            "A",
            ArtifactRole::Test,
            &["tests/shared.py"],
        );
        let outcome = outcome_rejected_unsafe(
            "A",
            ArtifactRole::Test,
            RepairRejectionKind::TestWeakening,
            WeakeningPattern::AssertionDeleted,
        );

        let _ = job.record_repair_attempt_outcome_for_target(outcome.clone(), &targets[0]);
        let _ = job.record_repair_attempt_outcome_for_target(outcome, &targets[0]);
        assert!(job.is_repair_hint_exhausted(&targets[0]));

        let report = semantic_report_fixture_with_cluster(
            "B",
            VerifierDiagnosticFailureKind::AssertionMismatch,
            0.8,
        );
        let cluster_id = report.failure_clusters[0].cluster_key.clone();
        job.semantic_plan = Some(SemanticRepairPlan {
            semantic_report: report,
            failure_cluster_id: cluster_id,
            semantic_cause: VerifierDiagnosticFailureKind::AssertionMismatch,
            spec_authority: SpecAuthority::BehaviorContract,
            preferred_repair_role: ArtifactRole::Test,
            repair_hypothesis: "new cluster should not inherit target exhaustion".to_string(),
            expected_improvement: None,
            assessment_generation_at_creation: 0,
        });

        assert!(
            !job.is_repair_hint_exhausted(&targets[0]),
            "target exhaustion from cluster A must not suppress the same file for cluster B"
        );
    }

    #[test]
    fn target_path_exhaustion_promotes_cluster_after_all_targets_exhausted() {
        let (mut job, targets) = semantic_repair_job_with_targets_for_test(
            "A",
            ArtifactRole::Test,
            &["tests/a.py", "tests/b.py"],
        );
        let outcome = outcome_rejected_unsafe(
            "A",
            ArtifactRole::Test,
            RepairRejectionKind::TestWeakening,
            WeakeningPattern::AssertionDeleted,
        );

        let _ = job.record_repair_attempt_outcome_for_target(outcome.clone(), &targets[0]);
        let _ = job.record_repair_attempt_outcome_for_target(outcome.clone(), &targets[0]);
        let _ = job.record_repair_attempt_outcome_for_target(outcome.clone(), &targets[1]);
        let fourth = job.record_repair_attempt_outcome_for_target(outcome, &targets[1]);

        assert!(fourth.promoted);
        assert!(fourth.all_clusters_exhausted);
        assert!(job.current_semantic_targets_all_exhausted());
        assert!(job.current_unexhausted_semantic_target().is_none());
        assert!(
            job.exhausted_attempts
                .contains(&(cluster_key_for_test("A"), ArtifactRole::Test))
        );
    }

    #[test]
    fn target_path_exhaustion_counts_repeated_improvements_that_still_fail() {
        let (mut job, targets) = semantic_repair_job_with_targets_for_test(
            "A",
            ArtifactRole::Implementation,
            &["app/main.py"],
        );
        let outcome = outcome_applied_improved("A", ArtifactRole::Implementation);

        let first = job.record_repair_attempt_outcome_for_target(outcome.clone(), &targets[0]);
        let second = job.record_repair_attempt_outcome_for_target(outcome.clone(), &targets[0]);
        assert!(!first.promoted);
        assert!(!second.promoted);
        assert!(!job.current_semantic_targets_all_exhausted());

        let third = job.record_repair_attempt_outcome_for_target(outcome, &targets[0]);
        assert!(third.promoted);
        assert!(job.current_semantic_targets_all_exhausted());
        assert!(job.needs_diagnostic_after_target_exhaustion());
    }

    #[test]
    fn exhausted_target_routes_to_diagnostic_before_safe_stop() {
        use tempfile::tempdir;
        let temp = tempdir().unwrap();
        let work_root = temp.path().to_path_buf();
        let (mut job, targets) = semantic_repair_job_with_targets_for_test(
            "A",
            ArtifactRole::Implementation,
            &["app/main.py"],
        );
        let outcome = outcome_applied_improved("A", ArtifactRole::Implementation);
        let _ = job.record_repair_attempt_outcome_for_target(outcome.clone(), &targets[0]);
        let _ = job.record_repair_attempt_outcome_for_target(outcome.clone(), &targets[0]);
        let _ = job.record_repair_attempt_outcome_for_target(outcome, &targets[0]);
        job.assessment = Some(super::super::VerifierRepairAssessment {
            failure_kind: VerifierDiagnosticFailureKind::AssertionMismatch,
            failure_type: VerifierFailureType::Unknown,
            probable_cause_role: Some(ArtifactRole::Implementation),
            needed_reads: Vec::new(),
            repair_target_hint: Some(targets[0].clone()),
            repair_plan: vec![targets[0].clone()],
            summary: None,
            source: super::super::VerifierRepairAssessmentSource::DiagnosticPass,
        });
        job.assessment_attempts = 0;

        let decision = verifier_repair_decision(true, Some(&job), &[], &work_root, Some(0), 0);
        assert_eq!(decision, VerifierRepairDecision::NeedDiagnostic);

        job.assessment_attempts = crate::agent::loop_run::turn::VERIFIER_DIAGNOSTIC_ATTEMPT_LIMIT;
        let exhausted = verifier_repair_decision(true, Some(&job), &[], &work_root, Some(0), 0);
        assert_eq!(exhausted, VerifierRepairDecision::DiagnosticUnavailable);
    }

    #[test]
    fn phase3_record_repair_attempt_outcome_fifo_cap_at_16() {
        let mut job = semantic_repair_job_for_test("seed", ArtifactRole::Implementation);
        // Push 17 distinct outcomes (distinct clusters so no promotion fires).
        for i in 0..(MAX_REPAIR_ATTEMPT_OUTCOMES + 1) {
            let label = format!("cluster-{i}");
            let outcome = outcome_applied_no_progress(&label, ArtifactRole::Implementation);
            let _ = job.record_repair_attempt_outcome(outcome);
        }
        // Cap respected and oldest dropped (FIFO).
        assert_eq!(
            job.repair_attempt_outcomes.len(),
            MAX_REPAIR_ATTEMPT_OUTCOMES
        );
        let first_cluster = cluster_key_for_test("cluster-1");
        assert_eq!(job.repair_attempt_outcomes[0].cluster, first_cluster);
    }

    #[test]
    fn phase3_record_repair_attempt_outcome_does_not_bump_assessment_generation() {
        let mut job = semantic_repair_job_for_test("A", ArtifactRole::Test);
        job.assessment_generation = 7;
        let outcome = outcome_applied_no_progress("A", ArtifactRole::Test);
        let _ = job.record_repair_attempt_outcome(outcome);
        assert_eq!(job.assessment_generation, 7, "S3-005: no bump");
    }

    #[test]
    fn phase3_repeated_test_weakening_promotes_to_exhausted_attempts_after_two_pushes() {
        let mut job = semantic_repair_job_for_test("A", ArtifactRole::Test);
        let outcome = outcome_rejected_unsafe(
            "A",
            ArtifactRole::Test,
            RepairRejectionKind::TestWeakening,
            WeakeningPattern::AssertionDeleted,
        );
        let _ = job.record_repair_attempt_outcome(outcome.clone());
        assert!(
            job.exhausted_attempts.is_empty(),
            "1 outcome must not promote"
        );
        let _ = job.record_repair_attempt_outcome(outcome.clone());
        assert_eq!(
            job.exhausted_attempts,
            vec![(cluster_key_for_test("A"), ArtifactRole::Test)],
            "2 outcomes must promote"
        );
        // idempotent: 3rd push must not duplicate.
        let _ = job.record_repair_attempt_outcome(outcome);
        assert_eq!(
            job.exhausted_attempts,
            vec![(cluster_key_for_test("A"), ArtifactRole::Test)],
            "3rd push must remain idempotent"
        );
    }

    // ========================================================================
    // Issue #654 / #662 — Bounded Safe Stop Report unit tests
    // ========================================================================

    #[test]
    fn stop_reason_as_str_covers_all_six_variants() {
        assert_eq!(
            StopReason::ArtifactCompletionFailed.as_str(),
            "artifact_completion_failed"
        );
        assert_eq!(
            StopReason::VerifierFailedSafeStop.as_str(),
            "verifier_failed_safe_stop"
        );
        assert_eq!(StopReason::VerifierWeak.as_str(), "verifier_weak");
        assert_eq!(StopReason::VerifierMissing.as_str(), "verifier_missing");
        assert_eq!(
            StopReason::DiagnosticTargetMissing.as_str(),
            "diagnostic_target_missing"
        );
        assert_eq!(StopReason::RepairExhausted.as_str(), "repair_exhausted");
    }

    #[test]
    fn phase3_distinct_clusters_do_not_promote_exhausted() {
        let mut job = semantic_repair_job_for_test("A", ArtifactRole::Test);
        let _ = job.record_repair_attempt_outcome(outcome_rejected_unsafe(
            "A",
            ArtifactRole::Test,
            RepairRejectionKind::TestWeakening,
            WeakeningPattern::AssertionDeleted,
        ));
        let _ = job.record_repair_attempt_outcome(outcome_rejected_unsafe(
            "B",
            ArtifactRole::Test,
            RepairRejectionKind::TestWeakening,
            WeakeningPattern::AssertionDeleted,
        ));
        assert!(job.exhausted_attempts.is_empty());
    }

    #[test]
    fn phase3_applied_no_progress_promotes_under_issue_662() {
        // Issue #662: expectation flip — `AppliedNoProgress` x 2 now promotes
        // (was: never promoted under Issue #653).
        let mut job = semantic_repair_job_for_test("A", ArtifactRole::Implementation);
        for _ in 0..5 {
            let _ = job.record_repair_attempt_outcome(outcome_applied_no_progress(
                "A",
                ArtifactRole::Implementation,
            ));
        }
        assert_eq!(
            job.exhausted_attempts,
            vec![(cluster_key_for_test("A"), ArtifactRole::Implementation)],
            "Issue #662: AppliedNoProgress x 5 must promote"
        );
    }

    #[test]
    fn phase3_snapshot_repair_attempt_outcomes_returns_clone() {
        let mut job = semantic_repair_job_for_test("A", ArtifactRole::Implementation);
        let outcome = outcome_applied_no_progress("A", ArtifactRole::Implementation);
        let _ = job.record_repair_attempt_outcome(outcome);
        let mut snapshot = job.snapshot_repair_attempt_outcomes();
        snapshot.clear();
        assert_eq!(
            job.repair_attempt_outcomes.len(),
            1,
            "mutating snapshot must not affect ledger"
        );
    }

    #[test]
    fn phase3_new_for_test_initializes_empty_ledger() {
        let job = RepairJob::new_for_test();
        assert!(job.repair_attempt_outcomes.is_empty());
    }

    #[test]
    fn phase3_record_repair_attempt_outcome_idempotent_exhausted_push() {
        // Issue #653 T4.2: same RejectedUnsafe pushed 3 times → exhausted_attempts
        // contains exactly 1 entry (idempotent `contains` guard).
        let mut job = semantic_repair_job_for_test("A", ArtifactRole::Test);
        let outcome = outcome_rejected_unsafe(
            "A",
            ArtifactRole::Test,
            RepairRejectionKind::TestWeakening,
            WeakeningPattern::AssertionDeleted,
        );
        for _ in 0..3 {
            let _ = job.record_repair_attempt_outcome(outcome.clone());
        }
        assert_eq!(job.exhausted_attempts.len(), 1);
    }

    #[test]
    fn record_repair_attempt_outcome_returns_all_clusters_exhausted_when_last_cluster_promoted() {
        // Issue #662: the active semantic_plan has exactly 1 repairable
        // cluster ("A"). After 2 same-bucket outcomes that cluster lands in
        // `exhausted_attempts`, `next_repairable_cluster` returns `None`, and
        // the second push reports `all_clusters_exhausted = true`.
        let mut job = semantic_repair_job_for_test("A", ArtifactRole::Implementation);
        let outcome = outcome_applied_no_progress("A", ArtifactRole::Implementation);

        let first = job.record_repair_attempt_outcome(outcome.clone());
        assert!(!first.promoted, "1st push must not promote");
        assert!(
            !first.all_clusters_exhausted,
            "1st push must leave cluster repairable"
        );

        let second = job.record_repair_attempt_outcome(outcome.clone());
        assert!(second.promoted, "2nd push must promote");
        assert!(
            second.all_clusters_exhausted,
            "2nd push exhausts the only repairable cluster"
        );

        // Idempotent: 3rd push must not re-promote but all_clusters_exhausted
        // remains true (the ledger still says no clusters left).
        let third = job.record_repair_attempt_outcome(outcome);
        assert!(!third.promoted, "3rd push must be idempotent");
        assert!(
            third.all_clusters_exhausted,
            "exhaustion is sticky across idempotent pushes"
        );
    }

    #[test]
    fn stop_reason_supports_hashset_membership() {
        // DR1-006: HashSet<StopReason> is the Agent dedup marker.
        let mut set: std::collections::HashSet<StopReason> = std::collections::HashSet::new();
        assert!(set.insert(StopReason::VerifierWeak));
        // Inserting again should be a no-op.
        assert!(!set.insert(StopReason::VerifierWeak));
        assert!(set.contains(&StopReason::VerifierWeak));
        assert!(!set.contains(&StopReason::VerifierMissing));
    }

    #[test]
    fn diagnostic_target_missing_reason_as_str_covers_all_four_variants() {
        assert_eq!(
            DiagnosticTargetMissingReason::ScopeExcluded.as_str(),
            "scope_excluded"
        );
        assert_eq!(
            DiagnosticTargetMissingReason::AllCandidatesUnreadable.as_str(),
            "all_candidates_unreadable"
        );
        assert_eq!(
            DiagnosticTargetMissingReason::ReadHistoryEmpty.as_str(),
            "read_history_empty"
        );
        assert_eq!(
            DiagnosticTargetMissingReason::AssessmentMissing.as_str(),
            "assessment_missing"
        );
    }

    #[test]
    fn safe_relative_path_string_rejects_unsafe_inputs() {
        assert_eq!(safe_relative_path_string(""), None);
        assert_eq!(safe_relative_path_string("/absolute/path"), None);
        assert_eq!(safe_relative_path_string("../escape"), None);
        assert_eq!(safe_relative_path_string("dir/../foo"), None);
        assert_eq!(safe_relative_path_string("with\nnewline"), None);
        assert_eq!(safe_relative_path_string("with\x00nul"), None);
        assert_eq!(safe_relative_path_string("with\x7fdel"), None);
    }

    #[test]
    fn safe_relative_path_string_rejects_windows_style_inputs() {
        // CB-004 regression: the host `Path::components()` is OS-dependent.
        // On Unix builds these inputs previously passed through as a single
        // normal component; the SSOT now string-checks the grammar.
        assert_eq!(safe_relative_path_string("..\\secret"), None);
        assert_eq!(safe_relative_path_string("dir\\..\\foo"), None);
        assert_eq!(safe_relative_path_string("dir\\file.rs"), None);
        assert_eq!(safe_relative_path_string("C:\\Users\\me"), None);
        assert_eq!(safe_relative_path_string("c:/Users/me"), None);
        assert_eq!(safe_relative_path_string("D:foo"), None);
        assert_eq!(safe_relative_path_string("\\\\?\\C:\\foo"), None);
        assert_eq!(safe_relative_path_string("//host/share"), None);
        assert_eq!(safe_relative_path_string("//foo/bar"), None);
    }

    #[test]
    fn safe_relative_path_string_accepts_workspace_relative() {
        assert_eq!(
            safe_relative_path_string("src/lib.rs"),
            Some("src/lib.rs".to_string())
        );
        assert_eq!(
            safe_relative_path_string("tests/foo.rs"),
            Some("tests/foo.rs".to_string())
        );
    }

    #[test]
    fn safe_relative_path_string_then_cap_clamps_long_paths() {
        // DR2-001: 240 char cap is applied by caller via
        // `sanitize_repair_job_text_with_char_cap(_, SAFE_STOP_PATH_CHAR_CAP)`.
        let long = "a".repeat(300);
        let raw = safe_relative_path_string(&long).expect("safe");
        let capped = sanitize_repair_job_text_with_char_cap(&raw, SAFE_STOP_PATH_CHAR_CAP);
        // 240 chars + 3-char ellipsis = 243 chars total.
        assert!(capped.chars().count() <= SAFE_STOP_PATH_CHAR_CAP + 3);
        assert!(capped.ends_with("..."));
    }

    #[test]
    fn build_actual_actions_caps_to_eight_newest_first() {
        let raw: Vec<String> = (0..12).map(|i| format!("action_{i}")).collect();
        let out = build_actual_actions(&raw);
        assert_eq!(out.len(), SAFE_STOP_ACTUAL_ACTIONS_MAX);
        // Newest-first order: raw is oldest-first 0..12, so output starts at 11.
        assert_eq!(out[0], "action_11");
        assert_eq!(out[7], "action_4");
    }

    #[test]
    fn build_actual_actions_handles_empty_input() {
        let out = build_actual_actions(&[]);
        assert!(out.is_empty());
    }

    #[test]
    fn build_actual_actions_clamps_per_entry_chars() {
        let long = "x".repeat(500);
        let out = build_actual_actions(std::slice::from_ref(&long));
        assert_eq!(out.len(), 1);
        assert!(out[0].chars().count() <= SAFE_STOP_ACTION_CHAR_CAP + 3);
        assert!(out[0].ends_with("..."));
    }

    #[test]
    fn exhausted_attempts_summary_deduplicates_and_caps() {
        let mut job = RepairJob::new_for_test();
        let key_a = make_cluster_key("aaaa");
        let key_b = make_cluster_key("bbbb");
        // Same (cluster, role) added twice — should dedup.
        job.exhausted_attempts.push((
            key_a.clone(),
            super::super::task_contract::ArtifactRole::Implementation,
        ));
        job.exhausted_attempts.push((
            key_a.clone(),
            super::super::task_contract::ArtifactRole::Implementation,
        ));
        job.exhausted_attempts.push((
            key_a.clone(),
            super::super::task_contract::ArtifactRole::Test,
        ));
        job.exhausted_attempts.push((
            key_b.clone(),
            super::super::task_contract::ArtifactRole::Implementation,
        ));
        let summary = ExhaustedAttemptsSummary::from_repair_job(&job, None);
        assert_eq!(summary.total, 4);
        assert_eq!(summary.per_cluster.len(), 2);
        // Cluster A has 2 unique roles (Implementation + Test).
        let (_k, roles) = summary
            .per_cluster
            .iter()
            .find(|(k, _)| k == &key_a.as_str().to_string())
            .expect("key_a present");
        assert_eq!(roles.len(), 2);
        assert!(roles.contains(&"implementation"));
        assert!(roles.contains(&"test"));
    }

    #[test]
    fn exhausted_attempts_summary_caps_hypothesis_to_240_chars() {
        let job = RepairJob::new_for_test();
        let hyp = "h".repeat(500);
        let summary = ExhaustedAttemptsSummary::from_repair_job(&job, Some(&hyp));
        let stored = summary.last_repair_hypothesis.expect("hypothesis stored");
        assert!(stored.chars().count() <= SAFE_STOP_LAST_REPAIR_HYPOTHESIS_CHAR_CAP + 3);
    }

    #[test]
    fn select_diagnostic_target_missing_reason_priority_1_scope_excluded() {
        let job = RepairJob::new_for_test();
        let scope = super::super::task_workspace_scope::TaskWorkspaceScope {
            mode: super::super::task_workspace_scope::ScopeMode::Explicit {
                paths: vec![std::path::PathBuf::from("only/this/path")],
            },
        };
        let candidates = vec!["other/path.rs".to_string()];
        let reason = select_diagnostic_target_missing_reason(&job, &scope, &candidates, None);
        assert_eq!(reason, DiagnosticTargetMissingReason::ScopeExcluded);
    }

    #[test]
    fn select_diagnostic_target_missing_reason_priority_2_all_unreadable() {
        let job = RepairJob::new_for_test();
        let scope = super::super::task_workspace_scope::TaskWorkspaceScope {
            mode: super::super::task_workspace_scope::ScopeMode::SingleProjectRoot,
        };
        let candidates = vec!["any/path.rs".to_string()];
        let reason = select_diagnostic_target_missing_reason(&job, &scope, &candidates, None);
        assert_eq!(
            reason,
            DiagnosticTargetMissingReason::AllCandidatesUnreadable
        );
    }

    #[test]
    fn select_diagnostic_target_missing_reason_priority_3_read_history_empty() {
        let job = RepairJob::new_for_test();
        let scope = super::super::task_workspace_scope::TaskWorkspaceScope {
            mode: super::super::task_workspace_scope::ScopeMode::SingleProjectRoot,
        };
        let reason = select_diagnostic_target_missing_reason(&job, &scope, &[], None);
        assert_eq!(reason, DiagnosticTargetMissingReason::ReadHistoryEmpty);
    }

    #[test]
    fn select_diagnostic_target_missing_reason_priority_4_assessment_missing() {
        let mut job = RepairJob::new_for_test();
        job.changed_file_hints
            .push(super::super::task_contract::RecoveryTargetHint {
                role: super::super::task_contract::ArtifactRole::Implementation,
                path: "src/foo.rs".to_string(),
                reason: "hint".to_string(),
            });
        let scope = super::super::task_workspace_scope::TaskWorkspaceScope {
            mode: super::super::task_workspace_scope::ScopeMode::SingleProjectRoot,
        };
        let reason = select_diagnostic_target_missing_reason(&job, &scope, &[], None);
        assert_eq!(reason, DiagnosticTargetMissingReason::AssessmentMissing);
    }

    #[test]
    fn safe_stop_input_stop_reason_returns_carried_value_for_from_repair() {
        let job = RepairJob::new_for_test();
        let input = SafeStopInput::FromRepair {
            job: &job,
            stop_reason: StopReason::ArtifactCompletionFailed,
            owned_test_artifacts: Vec::new(),
        };
        assert_eq!(input.stop_reason(), StopReason::ArtifactCompletionFailed);
    }

    #[test]
    fn safe_stop_input_stop_reason_is_hard_coded_for_from_missing_verifier() {
        // DR2-004: `FromMissingVerifier` always maps to `VerifierMissing`.
        let job = MissingVerifierJob::new(3, 0);
        let input = SafeStopInput::FromMissingVerifier {
            job: &job,
            owned_test_artifacts: Vec::new(),
        };
        assert_eq!(input.stop_reason(), StopReason::VerifierMissing);
    }

    #[test]
    fn safe_stop_report_build_from_repair_propagates_snapshot_fields() {
        // DR3-006: 4 overlapping fields (failure_signature / command /
        // output_excerpt / failure_type) must match the snapshot exactly.
        let mut job = RepairJob::new_for_test();
        job.failure_signature = "pytest::test_foo::assertion".to_string();
        job.command = "pytest -q".to_string();
        job.output_excerpt = "AssertionError: 1 != 2".to_string();
        job.failure_type = VerifierFailureType::AssertionFailure;
        let snapshot = job.failure_snapshot();
        let scope = super::super::task_workspace_scope::TaskWorkspaceScope {
            mode: super::super::task_workspace_scope::ScopeMode::SingleProjectRoot,
        };
        let input = SafeStopInput::FromRepair {
            job: &job,
            stop_reason: StopReason::VerifierFailedSafeStop,
            owned_test_artifacts: Vec::new(),
        };
        let ctx = SafeStopContext {
            current_role: None,
            expected_target: None,
            actual_actions_raw: Vec::new(),
            latest_successful_read: None,
            task_workspace_scope: &scope,
            candidates: Vec::new(),
            session_id: "session-1",
            turn_index: 1,
        };
        let report = SafeStopReport::build_from(input, ctx);
        assert_eq!(report.failure_signature, snapshot.failure_signature);
        assert_eq!(report.command, snapshot.command);
        assert_eq!(report.output_excerpt, snapshot.output_excerpt);
        assert_eq!(report.failure_type, snapshot.failure_type);
        assert_eq!(report.stop_reason, StopReason::VerifierFailedSafeStop);
    }

    #[test]
    fn safe_stop_report_build_from_overrides_failure_type_for_diagnostic_target_missing() {
        let mut job = RepairJob::new_for_test();
        // Source failure_type is AssertionFailure (e.g. carried from a prior
        // diagnostic). The builder must override it to DiagnosticTargetMissing
        // (DR3-005) when the StopReason is DiagnosticTargetMissing.
        job.failure_type = VerifierFailureType::AssertionFailure;
        let scope = super::super::task_workspace_scope::TaskWorkspaceScope {
            mode: super::super::task_workspace_scope::ScopeMode::SingleProjectRoot,
        };
        let input = SafeStopInput::FromRepair {
            job: &job,
            stop_reason: StopReason::DiagnosticTargetMissing,
            owned_test_artifacts: Vec::new(),
        };
        let ctx = SafeStopContext {
            current_role: None,
            expected_target: None,
            actual_actions_raw: Vec::new(),
            latest_successful_read: None,
            task_workspace_scope: &scope,
            candidates: Vec::new(),
            session_id: "s",
            turn_index: 0,
        };
        let report = SafeStopReport::build_from(input, ctx);
        assert_eq!(
            report.failure_type,
            VerifierFailureType::DiagnosticTargetMissing
        );
        // diagnostic_target_missing_reason should be populated.
        assert!(report.diagnostic_target_missing_reason.is_some());
    }

    #[test]
    fn safe_stop_report_build_from_missing_verifier_sets_explicit_defaults() {
        // R8: builder default for FromMissingVerifier must populate
        // failure_signature with the explicit "missing_verifier_or_config"
        // marker so downstream consumers don't misfire on empty strings.
        let job = MissingVerifierJob::new(3, 0);
        let scope = super::super::task_workspace_scope::TaskWorkspaceScope {
            mode: super::super::task_workspace_scope::ScopeMode::SingleProjectRoot,
        };
        let input = SafeStopInput::FromMissingVerifier {
            job: &job,
            owned_test_artifacts: vec!["tests/smoke.rs".to_string()],
        };
        let ctx = SafeStopContext {
            current_role: None,
            expected_target: None,
            actual_actions_raw: Vec::new(),
            latest_successful_read: None,
            task_workspace_scope: &scope,
            candidates: Vec::new(),
            session_id: "s",
            turn_index: 0,
        };
        let report = SafeStopReport::build_from(input, ctx);
        assert_eq!(report.failure_signature, "missing_verifier_or_config");
        assert_eq!(report.command, "");
        assert_eq!(
            report.failure_type,
            VerifierFailureType::MissingVerifierOrConfig
        );
        assert_eq!(report.stop_reason, StopReason::VerifierMissing);
        assert!(report.exhausted_attempts_summary.is_none());
        assert_eq!(report.owned_test_artifacts, vec!["tests/smoke.rs"]);
    }

    #[test]
    fn safe_stop_report_build_from_rejects_unsafe_owned_test_artifacts() {
        // DR2-006: defense-in-depth: build_from must drop unsafe paths even
        // if upstream classification leaked them in.
        let job = MissingVerifierJob::new(3, 0);
        let scope = super::super::task_workspace_scope::TaskWorkspaceScope {
            mode: super::super::task_workspace_scope::ScopeMode::SingleProjectRoot,
        };
        let input = SafeStopInput::FromMissingVerifier {
            job: &job,
            owned_test_artifacts: vec![
                "tests/ok.rs".to_string(),
                "/abs/escape.rs".to_string(),
                "../traverse.rs".to_string(),
                "control\nchar.rs".to_string(),
            ],
        };
        let ctx = SafeStopContext {
            current_role: None,
            expected_target: None,
            actual_actions_raw: Vec::new(),
            latest_successful_read: None,
            task_workspace_scope: &scope,
            candidates: Vec::new(),
            session_id: "s",
            turn_index: 0,
        };
        let report = SafeStopReport::build_from(input, ctx);
        assert_eq!(report.owned_test_artifacts, vec!["tests/ok.rs"]);
    }

    /// Construct a `FailureClusterKey` for tests via the public parse path.
    /// We only need a deterministic 16-hex value, not the full report.
    fn make_cluster_key(seed: &str) -> super::super::semantic_failure::FailureClusterKey {
        // Use the same `parse_semantic_failure_report` -> first cluster's
        // `cluster_key` SSOT so we don't reach into private constructors.
        let json = serde_json::json!({
            "failure_kind": "assertion_mismatch",
            "confidence": 0.5,
            "preferred_repair_role": "implementation",
            "repair_hypothesis": "h",
            "failure_clusters": [
                {
                    "observed": format!("obs-{seed}"),
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
            .failure_clusters
            .into_iter()
            .next()
            .expect("cluster present")
            .cluster_key
    }
}
