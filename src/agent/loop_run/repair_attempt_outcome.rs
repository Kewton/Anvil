//! Issue #653: RepairAttemptOutcome lifecycle ledger.
//!
//! Visibility: every export here is `pub(super)` and **must not** be
//! re-exported from `src/agent/loop_run.rs` (CLAUDE.md DR3-001).
//!
//! Security: variant 名は LLM 由来文字列を保持しない (Phase 0 invariant
//! (a))。`log_llm_event` payload に `repair_attempt_outcomes` を含める
//! 場合は、呼び出し側で `logging::mask_payload_inplace` を最終防衛線
//! として通す (Phase 0 invariant (b))。
//!
//! Persistence: in-memory 専用。`Serialize` / `Deserialize` derive は
//! **付与しない** (S3-004)。session restart 時の `RepairJob` は破棄され、
//! 空 ledger で再構築される。

use super::semantic_failure::FailureClusterKey;
use super::spec_authority::{RepairRole, WeakeningPattern};

/// 1 RepairJob あたりに保持する outcome 上限 (S1-003)。
/// 超過時は **oldest を drop** (FIFO) し、`tracing::warn!` で
/// drop metadata (件数のみ、内容は出さない) を emit する。
#[allow(dead_code)] // wired into repair_job.rs in Phase 2.
pub(super) const MAX_REPAIR_ATTEMPT_OUTCOMES: usize = 16;

/// repair attempt の結末を表す coarse 5 variant (payload 付き enum)。
/// `VerifierRepairRerunOutcome` を置換せず、lifecycle 集約レイヤーとして
/// 並列に持つ (S1-001)。
///
/// Applied 系: edit が適用された後の rerun 結果 (no payload)。
/// Rejected 系: edit を適用する前の deterministic safety reject (payload あり)。
///
/// **DR1-004 反映**: `rejection_kind` / `detail` を並列 `Option<...>` field
/// として持たず、payload 付き variant で illegal states unrepresentable
/// (Applied 系で `Some(...)` が混入する余地を型で排除)。
#[allow(dead_code)] // wired into turn.rs in Phase 3.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum RepairAttemptOutcomeKind {
    /// `VerifierRepairRerunOutcome::Improved` 1:1 (no payload)。
    AppliedImproved,
    /// `VerifierRepairRerunOutcome::SameFailureRemaining` 1:1 + `NewFailure` を畳む (S5-002, no payload)。
    AppliedNoProgress,
    /// `VerifierRepairRerunOutcome::Worsened` 1:1 (no payload)。
    AppliedWorsened,
    /// `validate_verifier_repair_intents` 内で `detect_test_weakening` /
    /// `detect_impl_weakening` が non-empty を返した時点での reject (S5-003)。
    /// `rejection` は `TestWeakening | ImplWeakening` のいずれか。
    /// `pattern` に細粒度の `WeakeningPattern` が入る (`AssertionDeleted` 等)。
    RejectedUnsafe {
        rejection: RepairRejectionKind,
        pattern: WeakeningPattern,
    },
    /// active `SemanticRepairPlan` がある (= semantic 経路に乗っている) のに
    /// `verifier_repair_effective_target_hint(&job)` が `None` を返した、
    /// すなわち「safe な repair target が一つも残っていない」状態 (S7-002, no payload)。
    RejectedNoCandidate,
}

/// `RejectedUnsafe` の root cause coarse category (S1-004)。
/// **string field は持たない** (Phase 0 invariant (a) コンパイル時保証)。
///
/// **DR1-003 反映**: `NoCandidate` / `OtherUnsafe` variant は削除し、weakening
/// 系の 2 variant のみ。`RejectedNoCandidate` は `RepairAttemptOutcomeKind`
/// 側の独立 variant、`OtherUnsafe` は production emission site が無いため
/// premature variant として導入しない (将来必要になれば別 Issue で追加)。
#[allow(dead_code)] // wired into turn.rs in Phase 3.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RepairRejectionKind {
    /// `detect_test_weakening` 由来 (5 pattern)。
    TestWeakening,
    /// `detect_impl_weakening` 由来 (4 pattern)。
    ImplWeakening,
}

/// 1 attempt の結末。
///
/// derive: `Debug, Clone, PartialEq, Eq` (S3-007) — `WeakeningPattern` は
/// `#[derive(... Eq)]` 済、`FailureClusterKey` も `Eq`、`RepairRejectionKind`
/// も `Eq`。
///
/// **DR1-004 反映**: `rejection_kind` / `detail` 並列 Option field を撤廃し、
/// `kind: RepairAttemptOutcomeKind` の payload variant に統合 (illegal states
/// unrepresentable)。
///
/// **DR1-007 反映**: `observed_at_generation: u32` field は本 Issue の受入
/// 条件で消費されないため premature field として削除。#654 が per-generation
/// grouping を必要とした時点で additive に再導入する (`RepairAttemptOutcome`
/// への field 追加は破壊的でない)。
#[allow(dead_code)] // wired into repair_job.rs (Phase 2) and turn.rs (Phase 3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RepairAttemptOutcome {
    /// active `SemanticRepairPlan.failure_cluster_id` からのみ取得 (S5-004 / Phase 0)。
    /// LLM 由来 cluster 識別子は **入れない**。
    pub(super) cluster: FailureClusterKey,
    /// active `SemanticRepairPlan.preferred_repair_role` からのみ取得 (S5-004)。
    pub(super) role: RepairRole,
    /// 5 variant のいずれか。`RejectedUnsafe` には payload (`rejection` + `pattern`) が含まれる。
    pub(super) kind: RepairAttemptOutcomeKind,
}

#[cfg(test)]
impl RepairAttemptOutcome {
    /// テストヘルパ: deterministic な (cluster, role, kind) で構築。
    pub(super) fn for_test(
        cluster: FailureClusterKey,
        role: RepairRole,
        kind: RepairAttemptOutcomeKind,
    ) -> Self {
        Self {
            cluster,
            role,
            kind,
        }
    }
}

/// Issue #653 (DR1-002): push 後の outcomes slice を見て「同一
/// `(cluster, role, RejectedUnsafe { rejection: TestWeakening | ImplWeakening, .. })`
/// が 2 回以上検出されていれば `(cluster, role)` を返す」pure-fn predicate。
///
/// caller (`record_repair_attempt_outcome`) は `Some((cluster, role))` を受け取ったら
/// `exhausted_attempts` に push する (idempotent — caller 側で `contains` チェック)。
///
/// **副作用なし**。`outcomes` / `pushed` を mutate しない。これにより helper を
/// 経由せず slice + outcome literal で unit test 可能 (受入条件 S1-006(a))。
#[allow(dead_code)] // wired into repair_job.rs in Phase 2.
pub(super) fn should_promote_to_exhausted_after_push(
    outcomes: &[RepairAttemptOutcome],
    pushed: &RepairAttemptOutcome,
) -> Option<(FailureClusterKey, RepairRole)> {
    let rejection = match &pushed.kind {
        RepairAttemptOutcomeKind::RejectedUnsafe { rejection, .. } => *rejection,
        _ => return None,
    };
    let count = outcomes
        .iter()
        .filter(|o| {
            o.cluster == pushed.cluster
                && o.role == pushed.role
                && matches!(
                    &o.kind,
                    RepairAttemptOutcomeKind::RejectedUnsafe { rejection: r, .. }
                        if *r == rejection
                )
        })
        .count();
    if count >= 2 {
        Some((pushed.cluster.clone(), pushed.role))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::loop_run::semantic_failure::cluster_key_for_test;
    use crate::agent::loop_run::task_contract::ArtifactRole;

    fn cluster(label: &str) -> FailureClusterKey {
        cluster_key_for_test(label)
    }

    fn rejected_unsafe(
        cluster_label: &str,
        role: RepairRole,
        rejection: RepairRejectionKind,
        pattern: WeakeningPattern,
    ) -> RepairAttemptOutcome {
        RepairAttemptOutcome::for_test(
            cluster(cluster_label),
            role,
            RepairAttemptOutcomeKind::RejectedUnsafe { rejection, pattern },
        )
    }

    #[test]
    fn should_promote_returns_some_on_two_same_test_weakening() {
        let first = rejected_unsafe(
            "A",
            ArtifactRole::Test,
            RepairRejectionKind::TestWeakening,
            WeakeningPattern::AssertionDeleted,
        );
        let second = first.clone();
        let outcomes = vec![first.clone(), second.clone()];
        let promoted = should_promote_to_exhausted_after_push(&outcomes, &second);
        assert_eq!(promoted, Some((cluster("A"), ArtifactRole::Test)));
    }

    #[test]
    fn should_promote_returns_some_on_two_same_impl_weakening() {
        let first = rejected_unsafe(
            "A",
            ArtifactRole::Implementation,
            RepairRejectionKind::ImplWeakening,
            WeakeningPattern::ValidatorDeleted,
        );
        let second = first.clone();
        let outcomes = vec![first.clone(), second.clone()];
        let promoted = should_promote_to_exhausted_after_push(&outcomes, &second);
        assert_eq!(promoted, Some((cluster("A"), ArtifactRole::Implementation)));
    }

    #[test]
    fn should_promote_returns_none_for_single_occurrence() {
        let only = rejected_unsafe(
            "A",
            ArtifactRole::Test,
            RepairRejectionKind::TestWeakening,
            WeakeningPattern::AssertionDeleted,
        );
        let outcomes = vec![only.clone()];
        let promoted = should_promote_to_exhausted_after_push(&outcomes, &only);
        assert_eq!(promoted, None);
    }

    #[test]
    fn should_promote_returns_none_for_applied_kinds() {
        let applied = RepairAttemptOutcome::for_test(
            cluster("A"),
            ArtifactRole::Implementation,
            RepairAttemptOutcomeKind::AppliedNoProgress,
        );
        let outcomes = vec![
            applied.clone(),
            applied.clone(),
            applied.clone(),
            applied.clone(),
            applied.clone(),
        ];
        let promoted = should_promote_to_exhausted_after_push(&outcomes, &applied);
        assert_eq!(promoted, None);
    }

    #[test]
    fn should_promote_returns_none_for_distinct_clusters() {
        let a = rejected_unsafe(
            "A",
            ArtifactRole::Test,
            RepairRejectionKind::TestWeakening,
            WeakeningPattern::AssertionDeleted,
        );
        let b = rejected_unsafe(
            "B",
            ArtifactRole::Test,
            RepairRejectionKind::TestWeakening,
            WeakeningPattern::AssertionDeleted,
        );
        let outcomes = vec![a.clone(), b.clone()];
        let promoted = should_promote_to_exhausted_after_push(&outcomes, &b);
        assert_eq!(promoted, None);
    }

    #[test]
    fn should_promote_returns_none_for_distinct_roles() {
        let test_attempt = rejected_unsafe(
            "A",
            ArtifactRole::Test,
            RepairRejectionKind::TestWeakening,
            WeakeningPattern::AssertionDeleted,
        );
        let impl_attempt = rejected_unsafe(
            "A",
            ArtifactRole::Implementation,
            RepairRejectionKind::ImplWeakening,
            WeakeningPattern::ValidatorDeleted,
        );
        let outcomes = vec![test_attempt.clone(), impl_attempt.clone()];
        let promoted = should_promote_to_exhausted_after_push(&outcomes, &impl_attempt);
        assert_eq!(promoted, None);
    }

    #[test]
    fn max_repair_attempt_outcomes_constant_is_sixteen() {
        // Issue #653 S1-003: cap = 16 hardcode invariant lock.
        assert_eq!(MAX_REPAIR_ATTEMPT_OUTCOMES, 16);
    }
}
