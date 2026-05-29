//! Issue #688 (parent #680, Phase 8): photon feedback derive core
//! (`PhotonOutcomeInputs` / `PhotonFeedbackOutcome` / `case_f_condition_met`
//! + the static-allowlist `outcome_detail` const) extracted from `turn.rs`.
//!
//! Phase 8 scope (Issue #688): this PR migrates the **type + Case F
//! predicate + const only**. The rerun-trigger keyword detector
//! (`is_rerun_trigger` / `normalize_rerun_trigger_input` /
//! `contains_rerun_with_word_boundary` / `build_rerun_prompt_hint_if_eligible`
//! / `is_runnable_rerun_hint`), the outcome derivation core
//! (`derive_photon_feedback_outcome` / `is_eligible_failure_kind`), and
//! the photon injection helpers (`build_photon_injection_message` /
//! `request_explicitly_requests_script_execution`) stay in `turn.rs` for
//! now and will be migrated in follow-up PRs.
//!
//! The `PHOTON_OUTCOME_DETAIL_NO_PROGRESS_DESPITE_INJECT` const is
//! re-exported from `loop_run.rs` (`pub use`) so the integration test
//! `tests/photon_evaluate_signal_smoke.rs` can keep importing it via
//! `anvil::agent::loop_run::PHOTON_OUTCOME_DETAIL_NO_PROGRESS_DESPITE_INJECT`.
//! That re-export is pre-existing; this PR only updates its source path.
//!
//! `pub(super)` limited / no facade re-export (DR3-001 — except the one
//! pre-existing `pub use` of the const). `turn.rs` is the only in-crate
//! consumer for the `PhotonOutcomeInputs` / `PhotonFeedbackOutcome` types.

use crate::session::feedback::FeedbackKind;

/// Issue #591 (AS-04 / 設計判断 #3): inputs to the
/// `derive_photon_feedback_outcome` pure helper. Bundles only the data
/// the helper is allowed to inspect — `FeedbackKind` enum value,
/// `AnvilScore` booleans / counts, same-turn flag, adopted-id count,
/// and shadow flag. Free-text fields (feedback excerpt, command, model
/// output) are NOT in scope here (security threat: outcome helper must
/// not leak secrets into the static-allowlist return value).
///
/// Lifetime parameter ties the refs to `Agent` state that produced them.
pub(crate) struct PhotonOutcomeInputs<'a> {
    /// Latest recorded `FeedbackKind`, if any. Stale across turn boundaries:
    /// the helper must only trust this when `eligible_feedback_recorded_this_turn`
    /// is `true` (DR3-NEW-002).
    pub last_feedback_kind: Option<&'a FeedbackKind>,
    /// `SessionSnapshot.eligible_feedback_recorded_this_turn` flag (Issue #455).
    /// Guards `failure` / `safety_violation` derivation against stale feedback.
    /// `success` derivation (via `AnvilScore.user_visible_artifact`) is NOT
    /// gated by this flag (DR3-NEW-002).
    pub eligible_feedback_recorded_this_turn: bool,
    /// Current turn's computed `AnvilScore`, if available. `None` for
    /// TransportError / pre-compute paths — see `success.rs` facade.
    pub anvil_score: Option<&'a crate::session::anvil_score::AnvilScore>,
    /// Number of `summary_ids_adopted` actually sent to photon `/v1/evaluate`
    /// (post sanitize + post cap). 0 short-circuits to `None`.
    pub adopted_id_count: usize,
    /// Shadow mode flag from `Config.photon_shadow_mode`. When `true`, the
    /// helper short-circuits to `None` — Anvil must not stamp an adoption
    /// outcome on shadow turns (AS-04 / 設計判断 #7).
    pub shadow_mode: bool,
    /// Issue #601: 1-based actor loop iteration count for this turn, populated
    /// at `run_actor_loop` tail from local `last_iter.min(max_iterations)`
    /// (S5-002 — `self.last_iter` field does not exist). Used by Case F
    /// no-progress detection (`<= 1` is one of the four AND conditions).
    pub iter_count_this_turn: usize,
    /// Issue #601: number of prepared tool calls dispatched this turn, populated
    /// at `run_actor_loop` tail from local `tool_calls_made_this_turn`.
    /// Used by Case F no-progress detection (`== 0` is one of the four AND
    /// conditions).
    pub tool_calls_this_turn: usize,
    /// Issue #601: `SessionSnapshot.repo_edit_succeeded_this_turn` flag (Issue #456).
    /// Used by Case F no-progress detection (`== false` is one of the four AND
    /// conditions).
    pub repo_edit_succeeded_this_turn: bool,
    /// Issue #601: indicates whether the current turn's WorkMode resolved to
    /// `AnswerOnly` (Issue #576). Populate **must** go through
    /// `Agent::answer_only_mode_active()` SSOT helper (DR3-001), never a
    /// direct `mode_state.work_mode == WorkMode::AnswerOnly` comparison.
    /// Used by Case F no-progress detection (`== false` is one of the four
    /// AND conditions — AnswerOnly turns are expected to make zero edits).
    pub work_mode_is_answer_only: bool,
    /// Issue #608 Phase α-2 (AP-10 / 設計判断 #2 + #3): same-turn signal
    /// that the agent observed a `VerifierExitZero` evidence entry (i.e.
    /// a BuildTest invocation exited 0 and passed the DR4-002 gate).
    /// Derived from `evidence_set_this_turn` at the production callsite
    /// (turn.rs:3529 周辺) — no separate SessionSnapshot flag (design
    /// 設計判断 #3 (B)). Case E in `derive_photon_feedback_outcome` ORs
    /// this signal with `AnvilScore.user_visible_artifact` so a successful
    /// verifier run still earns a `success` outcome even when no Write /
    /// Edit produced an on-disk artifact (e.g. read-only repos).
    pub verifier_exit_zero_this_turn: bool,
}

/// Issue #601 (S5-001 / 設計判断 #4 (b)): unified return type for
/// `derive_photon_feedback_outcome`. Bundles the legacy `outcome` value
/// (`"success"` / `"failure"` / `"safety_violation"` / `None`) and the new
/// `outcome_detail` value (`"no_progress_despite_inject"` / `None`) so the
/// caller never has to recompute Case F conditions (DR1-001 SSOT).
///
/// Both fields are `Option<&'static str>` so the helper cannot leak runtime
/// data into the outbound payload — the audit boundary stays at type level
/// (DR4-NEW-004).
#[derive(Debug, Clone)]
pub(crate) struct PhotonFeedbackOutcome {
    /// Static-allowlist outcome value: `"success"` / `"failure"` /
    /// `"safety_violation"` / `None`.
    pub outcome: Option<&'static str>,
    /// Optional static-allowlist detail tag. Currently only
    /// `PHOTON_OUTCOME_DETAIL_NO_PROGRESS_DESPITE_INJECT` is defined; the
    /// `pub const` allowlist makes it grep-able and integration-test-callable.
    pub outcome_detail: Option<&'static str>,
}

/// Issue #601 (Case F SSOT, D1-001 / DR1-001): no-progress 4-condition AND.
///
/// Returns `true` iff the current turn matches the no-progress shape:
///   1. Not an AnswerOnly turn (`work_mode_is_answer_only == false`).
///   2. At most one actor loop iteration (`iter_count_this_turn <= 1`).
///   3. No successful repo edit (`repo_edit_succeeded_this_turn == false`).
///   4. No tool call dispatched (`tool_calls_this_turn == 0`).
///
/// This helper is the **only** place these four conditions appear in code.
/// `derive_photon_feedback_outcome` Case F is the **only** callsite in
/// production. Test safeguards (`empty_inputs()` defaults) intentionally
/// break at least one condition so existing tests that don't set the Case F
/// fields keep returning the previous outcome shape.
pub(crate) fn case_f_condition_met(inputs: &PhotonOutcomeInputs<'_>) -> bool {
    !inputs.work_mode_is_answer_only
        && inputs.iter_count_this_turn <= 1
        && !inputs.repo_edit_succeeded_this_turn
        && inputs.tool_calls_this_turn == 0
}

/// Issue #601 (Case F outcome_detail SSOT, DR4-NEW-004 audit boundary).
///
/// Static-allowlist literal for the new `outcome_detail` JSON value emitted
/// in the `context_pack_event` request body and the
/// `agent.photon_evaluate.completed` log payload. Currently exactly one value
/// is defined (`no_progress_despite_inject`); the `pub const` keeps it
/// grep-able and allows future cross-layer consistency checks (photon-side
/// `_FAILURE_DETAILS` allowlist) and integration-test imports.
///
/// `pub` (not `pub(crate)`) so `tests/photon_evaluate_signal_smoke.rs` can
/// import the symbol via the `pub use` re-export added in
/// `src/agent/loop_run.rs`.
pub const PHOTON_OUTCOME_DETAIL_NO_PROGRESS_DESPITE_INJECT: &str = "no_progress_despite_inject";
