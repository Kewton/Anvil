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
pub(super) const MAX_REPAIR_ATTEMPT_OUTCOMES: usize = 16;

/// repair attempt の結末を表す coarse variant (payload 付き enum)。
/// `VerifierRepairRerunOutcome` を置換せず、lifecycle 集約レイヤーとして
/// 並列に持つ (S1-001)。
///
/// Applied 系: edit が適用された後の rerun 結果 (no payload)。
/// Rejected 系: edit を適用する前の deterministic safety reject (payload あり / 無し)。
///
/// **Issue #662**: 新 3 variant (`RejectedMalformed` / `RejectedNoop` /
/// `RejectedDuplicate`) を additive 追加。`#[non_exhaustive]` を新規付与し
/// forward compat を確立。
///
/// **stable label 命名予約** (実装は Issue #666 で wire):
///   - `RejectedMalformed`  → `rejected_malformed`
///   - `RejectedNoop`       → `rejected_noop`
///   - `RejectedDuplicate`  → `rejected_duplicate`
///   - `AppliedImproved`    → `applied_improved`
///   - `AppliedNoProgress`  → `applied_no_progress`
///   - `AppliedWorsened`    → `applied_worsened`
///   - `RejectedUnsafe`     → `rejected_unsafe`
///   - `RejectedNoCandidate` → `rejected_no_candidate`
///
/// **DR1-004 反映**: `rejection_kind` / `detail` を並列 `Option<...>` field
/// として持たず、payload 付き variant で illegal states unrepresentable
/// (Applied 系で `Some(...)` が混入する余地を型で排除)。
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
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
    /// Issue #662: parse 段階での malformed reject。
    /// `parse_verifier_repair_intents_reply` / `parse_verifier_repair_intent_object`
    /// failure 経由で `record_controller_verifier_repair_invalid` に流す (no payload)。
    RejectedMalformed,
    /// Issue #662: `VerifierRepairIntent.old_string == new_string` または
    /// apply 後 `contents == original_contents` 検出時の reject (no payload)。
    /// `validate_verifier_repair_intents` 配下で検出 (検出順序 SSOT 優先度 2)。
    RejectedNoop,
    /// Issue #662: `verifier_repair_intents_fingerprint` が `applied_repair_intents`
    /// と一致した時点の reject (no payload)。raw `old_string` / `new_string` /
    /// raw path は variant payload に含めない (DR4-001)。
    RejectedDuplicate,
}

/// `RejectedUnsafe` の root cause coarse category (S1-004)。
/// **string field は持たない** (Phase 0 invariant (a) コンパイル時保証)。
///
/// **DR1-003 反映**: `NoCandidate` / `OtherUnsafe` variant は削除し、weakening
/// 系の 2 variant のみ。`RejectedNoCandidate` は `RepairAttemptOutcomeKind`
/// 側の独立 variant、`OtherUnsafe` は production emission site が無いため
/// premature variant として導入しない (将来必要になれば別 Issue で追加)。
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

/// Issue #662: promotion bucket (5 variant) for non-`RejectedUnsafe` outcomes.
///
/// Each variant groups `RepairAttemptOutcomeKind` entries that should count
/// against the same `(cluster, role)` promotion budget. Two outcomes
/// promote to `exhausted_attempts` when they fall into the **same** bucket
/// twice for the same `(cluster, role)`:
///   - `NoProgress` — `RepairAttemptOutcomeKind::AppliedNoProgress`
///     (edit applied but the rerun showed the same failure remaining).
///   - `Worsened`   — `RepairAttemptOutcomeKind::AppliedWorsened`
///     (edit applied but the rerun produced a new / worse failure).
///   - `Malformed`  — `RepairAttemptOutcomeKind::RejectedMalformed`
///     (5-4-1 priority 1: parse-stage rejection of the LLM intent payload).
///   - `Noop`       — `RepairAttemptOutcomeKind::RejectedNoop`
///     (5-4-1 priority 2: `old_string == new_string` or
///     `contents == original_contents` after apply).
///   - `Duplicate`  — `RepairAttemptOutcomeKind::RejectedDuplicate`
///     (5-4-1 priority 3: fingerprint matches `applied_repair_intents`).
///
/// `RejectedUnsafe` is intentionally excluded — its same-rejection-kind
/// counting lives in `should_promote_unsafe` (Issue #653 SSOT, unchanged) so
/// that `RepairRejectionKind` does not leak into this enum's payload (DRY /
/// KISS / Security Stage 4 — #666 observability never emits raw rejection
/// variants from this bucket).
///
/// **stable label naming reservation (Issue #666 observability wiring)**:
/// the bucket name strings (`"no_progress"` / `"worsened"` / `"malformed"` /
/// `"noop"` / `"duplicate"`) follow the snake_case projection of the variant
/// identifiers and stay aligned with the `RepairAttemptOutcomeKind` stable
/// labels documented above. No raw LLM text enters bucket payloads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PromotionBucket {
    NoProgress,
    Worsened,
    Malformed,
    Noop,
    Duplicate,
}

/// Issue #662: explicit enumeration of every `RepairAttemptOutcomeKind`
/// variant. Returns the bucket only when the kind participates in the
/// non-unsafe promotion path. `RejectedUnsafe` / `AppliedImproved` /
/// `RejectedNoCandidate` return `None` and are handled either by the unsafe
/// helper or excluded from promotion entirely (CLAUDE.md DR1-003 — never
/// `_ => None`).
fn promotion_bucket_non_unsafe(kind: &RepairAttemptOutcomeKind) -> Option<PromotionBucket> {
    match kind {
        RepairAttemptOutcomeKind::AppliedNoProgress => Some(PromotionBucket::NoProgress),
        RepairAttemptOutcomeKind::AppliedWorsened => Some(PromotionBucket::Worsened),
        RepairAttemptOutcomeKind::RejectedMalformed => Some(PromotionBucket::Malformed),
        RepairAttemptOutcomeKind::RejectedNoop => Some(PromotionBucket::Noop),
        RepairAttemptOutcomeKind::RejectedDuplicate => Some(PromotionBucket::Duplicate),
        // Handled by `should_promote_unsafe` (SSOT for same-rejection-kind count).
        RepairAttemptOutcomeKind::RejectedUnsafe { .. } => None,
        // Improvement is forward progress — never promotes.
        RepairAttemptOutcomeKind::AppliedImproved => None,
        // No safe target exists — promotion is meaningless (handled by the
        // semantic plan slot reuse path instead).
        RepairAttemptOutcomeKind::RejectedNoCandidate => None,
    }
}

/// Issue #662: `RejectedUnsafe` same-rejection-kind counting SSOT (Issue
/// #653 logic unchanged). Returns `Some((cluster, role))` when the pushed
/// outcome makes the same-rejection-kind count reach 2 for the same
/// `(cluster, role)`.
fn should_promote_unsafe(
    outcomes: &[RepairAttemptOutcome],
    pushed: &RepairAttemptOutcome,
) -> Option<(FailureClusterKey, RepairRole)> {
    let rejection = match &pushed.kind {
        RepairAttemptOutcomeKind::RejectedUnsafe { rejection, .. } => *rejection,
        // Every other variant is delegated to `should_promote_non_unsafe`.
        RepairAttemptOutcomeKind::AppliedImproved
        | RepairAttemptOutcomeKind::AppliedNoProgress
        | RepairAttemptOutcomeKind::AppliedWorsened
        | RepairAttemptOutcomeKind::RejectedNoCandidate
        | RepairAttemptOutcomeKind::RejectedMalformed
        | RepairAttemptOutcomeKind::RejectedNoop
        | RepairAttemptOutcomeKind::RejectedDuplicate => return None,
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

/// Issue #662: bucket-based same-bucket counting for `Applied{NoProgress,
/// Worsened}` and the new 3 `Rejected{Malformed, Noop, Duplicate}` variants.
fn should_promote_non_unsafe(
    outcomes: &[RepairAttemptOutcome],
    pushed: &RepairAttemptOutcome,
) -> Option<(FailureClusterKey, RepairRole)> {
    let bucket = promotion_bucket_non_unsafe(&pushed.kind)?;
    let count = outcomes
        .iter()
        .filter(|o| {
            o.cluster == pushed.cluster
                && o.role == pushed.role
                && promotion_bucket_non_unsafe(&o.kind) == Some(bucket)
        })
        .count();
    if count >= 2 {
        Some((pushed.cluster.clone(), pushed.role))
    } else {
        None
    }
}

/// Issue #653 / #662: push 後の outcomes slice を見て「同一 `(cluster, role)` に
/// 対し同一 promotion bucket (Unsafe x RepairRejectionKind / NoProgress /
/// Worsened / Malformed / Noop / Duplicate) が 2 回以上検出されていれば
/// `(cluster, role)` を返す」pure-fn predicate (2 述語合成)。
///
/// Issue #662 で `AppliedNoProgress` / `AppliedWorsened` / 新 3 `Rejected*`
/// variant を promotion 対象に拡張。`AppliedImproved` / `RejectedNoCandidate`
/// は除外したまま (forward progress / 無意味な reject)。
///
/// **2 述語合成の意味** (`or_else` 短絡):
///   1. `should_promote_unsafe` — Issue #653 SSOT (変更なし)。
///      `RejectedUnsafe { rejection, .. }` のみ参照し、同一
///      `RepairRejectionKind` の 2 回目検出で promote。`RepairRejectionKind`
///      enum 値は本 SSOT 内に閉じ込め、bucket 側 payload へ漏らさない
///      (DRY / Security Stage 4)。
///   2. `should_promote_non_unsafe` — Issue #662 で additive 追加。
///      `PromotionBucket` (5 variant) を経由した同一 bucket 2 回目検出で
///      promote。`AppliedImproved` / `RejectedNoCandidate` / `RejectedUnsafe`
///      は明示的に `None` を返す (CLAUDE.md DR1-003 — silent skip 回避)。
///
/// `RejectedUnsafe` を pushed しても `should_promote_non_unsafe` は `None` を
/// 返すため、2 述語が同一 push に対し double-fire することはない。
///
/// caller (`record_repair_attempt_outcome`) は `Some((cluster, role))` を受け取ったら
/// `exhausted_attempts` に push する (idempotent — caller 側で `contains` チェック)。
///
/// **副作用なし**。`outcomes` / `pushed` を mutate しない。これにより helper を
/// 経由せず slice + outcome literal で unit test 可能 (受入条件 S1-006(a))。
pub(super) fn should_promote_to_exhausted_after_push(
    outcomes: &[RepairAttemptOutcome],
    pushed: &RepairAttemptOutcome,
) -> Option<(FailureClusterKey, RepairRole)> {
    should_promote_unsafe(outcomes, pushed).or_else(|| should_promote_non_unsafe(outcomes, pushed))
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

    // Issue #662: expectation flip — `AppliedNoProgress` now promotes after 2
    // pushes (was: never promoted under Issue #653).
    #[test]
    fn should_promote_returns_some_on_two_same_applied_no_progress() {
        let applied = RepairAttemptOutcome::for_test(
            cluster("A"),
            ArtifactRole::Implementation,
            RepairAttemptOutcomeKind::AppliedNoProgress,
        );
        let outcomes = vec![applied.clone(), applied.clone()];
        let promoted = should_promote_to_exhausted_after_push(&outcomes, &applied);
        assert_eq!(
            promoted,
            Some((cluster("A"), ArtifactRole::Implementation)),
            "Issue #662: AppliedNoProgress x 2 must promote"
        );
    }

    #[test]
    fn should_promote_returns_some_on_two_same_applied_worsened() {
        let applied = RepairAttemptOutcome::for_test(
            cluster("A"),
            ArtifactRole::Implementation,
            RepairAttemptOutcomeKind::AppliedWorsened,
        );
        let outcomes = vec![applied.clone(), applied.clone()];
        let promoted = should_promote_to_exhausted_after_push(&outcomes, &applied);
        assert_eq!(
            promoted,
            Some((cluster("A"), ArtifactRole::Implementation)),
            "Issue #662: AppliedWorsened x 2 must promote"
        );
    }

    #[test]
    fn should_promote_returns_some_on_two_same_rejected_malformed() {
        let outcome = RepairAttemptOutcome::for_test(
            cluster("A"),
            ArtifactRole::Implementation,
            RepairAttemptOutcomeKind::RejectedMalformed,
        );
        let outcomes = vec![outcome.clone(), outcome.clone()];
        let promoted = should_promote_to_exhausted_after_push(&outcomes, &outcome);
        assert_eq!(
            promoted,
            Some((cluster("A"), ArtifactRole::Implementation)),
            "Issue #662: RejectedMalformed x 2 must promote"
        );
    }

    #[test]
    fn should_promote_returns_some_on_two_same_rejected_noop() {
        let outcome = RepairAttemptOutcome::for_test(
            cluster("A"),
            ArtifactRole::Implementation,
            RepairAttemptOutcomeKind::RejectedNoop,
        );
        let outcomes = vec![outcome.clone(), outcome.clone()];
        let promoted = should_promote_to_exhausted_after_push(&outcomes, &outcome);
        assert_eq!(
            promoted,
            Some((cluster("A"), ArtifactRole::Implementation)),
            "Issue #662: RejectedNoop x 2 must promote"
        );
    }

    #[test]
    fn should_promote_returns_some_on_two_same_rejected_duplicate() {
        let outcome = RepairAttemptOutcome::for_test(
            cluster("A"),
            ArtifactRole::Implementation,
            RepairAttemptOutcomeKind::RejectedDuplicate,
        );
        let outcomes = vec![outcome.clone(), outcome.clone()];
        let promoted = should_promote_to_exhausted_after_push(&outcomes, &outcome);
        assert_eq!(
            promoted,
            Some((cluster("A"), ArtifactRole::Implementation)),
            "Issue #662: RejectedDuplicate x 2 must promote"
        );
    }

    #[test]
    fn should_promote_returns_none_for_applied_improved() {
        // Issue #662 regression guard: AppliedImproved is forward progress
        // and must never promote, even after many repetitions.
        let applied = RepairAttemptOutcome::for_test(
            cluster("A"),
            ArtifactRole::Implementation,
            RepairAttemptOutcomeKind::AppliedImproved,
        );
        let outcomes = vec![
            applied.clone(),
            applied.clone(),
            applied.clone(),
            applied.clone(),
            applied.clone(),
        ];
        let promoted = should_promote_to_exhausted_after_push(&outcomes, &applied);
        assert_eq!(
            promoted, None,
            "AppliedImproved must never promote (forward progress)"
        );
    }

    #[test]
    fn should_promote_returns_none_for_rejected_no_candidate() {
        // Issue #662 regression guard: RejectedNoCandidate is not a count
        // signal — the semantic plan slot reuse path handles target
        // exhaustion separately.
        let outcome = RepairAttemptOutcome::for_test(
            cluster("A"),
            ArtifactRole::Implementation,
            RepairAttemptOutcomeKind::RejectedNoCandidate,
        );
        let outcomes = vec![outcome.clone(), outcome.clone(), outcome.clone()];
        let promoted = should_promote_to_exhausted_after_push(&outcomes, &outcome);
        assert_eq!(
            promoted, None,
            "RejectedNoCandidate must never promote (orthogonal path)"
        );
    }

    #[test]
    fn should_promote_returns_none_for_single_applied_no_progress() {
        // Issue #662: count=1 must not promote (count >= 2 SSOT preserved).
        let applied = RepairAttemptOutcome::for_test(
            cluster("A"),
            ArtifactRole::Implementation,
            RepairAttemptOutcomeKind::AppliedNoProgress,
        );
        let outcomes = vec![applied.clone()];
        let promoted = should_promote_to_exhausted_after_push(&outcomes, &applied);
        assert_eq!(promoted, None, "count=1 must not promote");
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
