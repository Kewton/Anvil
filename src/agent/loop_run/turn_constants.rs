//! Module-level constants extracted from `turn.rs` (parent #680).
//!
//! Hosts the five shared `pub(super) const` items that other
//! `loop_run` siblings reference for retry budgets, log truncation
//! caps, and sentinel error strings:
//!
//! - `LOG_ARGS_MAX_CHARS` — maximum number of characters of
//!   tool-call arguments retained in trace logs.
//! - `PLAN_REPEATED_EXPLORATION_BLOCK_THRESHOLD` — the
//!   `actor_loop_flow` plan-mode repeated-exploration detector
//!   threshold.
//! - `TASK_CONTRACT_VERIFIER_ATTEMPT_LIMIT` — task-contract verifier
//!   diagnostic attempt budget.
//! - `TASK_CONTRACT_VERIFIER_REPAIR_ATTEMPT_LIMIT` — task-contract
//!   verifier repair attempt budget.
//! - `USER_INTERRUPT_ERROR` — sentinel error string emitted on user
//!   interrupt; routed through the retry / streaming paths as an
//!   early-return marker.
//!
//! `pub(super)` limited / no facade re-export (DR3-001).

/// Maximum number of characters of tool-call arguments retained in
/// trace logs.
pub(super) const LOG_ARGS_MAX_CHARS: usize = 200;

// Issue #634: SSOT for specialized-fallback ログ event 名。emit 側 / test 側の
// 双方が参照し、typo による検証無効化を防ぐ。文字列値そのものは既存テスト互換の
// ため不変。`EVENT_DETERMINISTIC_PYTHON_TEST_FALLBACK` は本 Issue で新規追加。
pub(super) const PLAN_REPEATED_EXPLORATION_BLOCK_THRESHOLD: usize = 2;
pub(super) const TASK_CONTRACT_VERIFIER_ATTEMPT_LIMIT: usize = 3;
pub(super) const TASK_CONTRACT_VERIFIER_REPAIR_ATTEMPT_LIMIT: usize = 6;
pub(super) const USER_INTERRUPT_ERROR: &str = "__anvil_user_interrupt__";
