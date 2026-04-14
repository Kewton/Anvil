//! Termination finite-state machine for the agentic loop (Issue #381).
//!
//! This module provides a pure state + decision-table layer that governs
//! termination decisions inside `complete_structured_response` and on the
//! Done path.  It has two parts:
//!
//! * [`TerminationLoopState`] — per-turn mutable state bag whose fields are
//!   private and can only be updated through dedicated methods that enforce
//!   invariants (e.g. `final_guard_retries <= MAX_FINAL_GUARD_RETRIES`).
//! * [`decide_no_tool_call`] — a pure decision table that determines how to
//!   classify a no-tool-call LLM response given a fully-materialised
//!   [`NoToolCallInput`].
//!
//! The orchestration layer in `agentic.rs` is responsible for all
//! side-effects (message injection, telemetry, assistant output recording).
//! The FSM itself performs no I/O and holds no references to `App`.
//!
//! # Design notes
//!
//! The module is named `termination_fsm` for consistency with Issue #381,
//! even though the implementation is a "state bag + pure decision table"
//! rather than a classical Mealy/Moore FSM.  See
//! `dev-reports/design/issue-381-termination-state-machine-design-policy.md`
//! for details.

use super::phase_estimator::PhaseAction;

/// Maximum number of ANVIL_FINAL guard retries (no-file-modification detection).
pub const MAX_FINAL_GUARD_RETRIES: u8 = 1;

/// Maximum number of plan-gate suppression events per turn (Issue #285 A2).
const MAX_PLAN_SUPPRESSIONS: u8 = 1;

/// Maximum number of task-semantics gate suppression events per turn (Issue #382).
const MAX_TASK_SEMANTICS_SUPPRESSIONS: u8 = 1;

/// All valid outcomes of evaluating a no-tool-call LLM response.
///
/// This enum is exposed publicly so that integration tests in
/// `tests/termination_semantics.rs` can use `decide_no_tool_call` as the
/// single source of truth for decision semantics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoToolCallDecision {
    /// Response follows a synthetic guidance injection; accepted as follow-up.
    GuidanceFollowupComplete,
    /// ANVIL_FINAL guard fires: no file modifications detected, retry injected.
    FinalGuardRetry,
    /// Phase estimator detected the write+verify+empty completion pattern.
    FallbackCompleted,
    /// Execution plan has incomplete items; termination suppressed.
    PlanGateSuppression,
    /// Task requires implementation but no file changes occurred; retry injected.
    TaskSemanticsSuppression,
    /// Unconditional acceptance as the final answer.
    FinalAnswer,
}

/// Transition result returned by FSM entry points.
///
/// The orchestration layer in `agentic.rs` consumes this to decide what
/// side-effect (if any) to perform: break out of the loop, continue
/// iterating, or inject a retry message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TerminationTransition {
    /// Exit the agentic loop with the given terminal outcome.
    Break(EndOfTurnOutcome),
    /// Continue the loop (suppression applied or retry injected).
    Continue,
    /// Activate a retry turn of the given kind (caller performs the side
    /// effect — message injection etc.).
    Retry(RetryKind),
}

/// Retry kind carried by [`TerminationTransition::Retry`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RetryKind {
    /// ANVIL_FINAL guard retry (no file modifications detected).
    FinalGuard,
    /// Plan gate retry (Done path: incomplete plan requires more work).
    PlanGate,
    /// Task-semantics gate retry (Issue #382).
    TaskSemantics,
    /// Guidance follow-up retry (Issue #372 (A)).
    ///
    /// Currently no code path emits this variant — the guidance retry is
    /// treated as a `Continue` transition because the orchestration layer
    /// has already injected the guidance message in
    /// `handle_post_tool_anvil_final_gate`.  The variant is kept for API
    /// completeness and for future refactors that may surface guidance as
    /// an explicit retry.
    #[allow(dead_code)]
    GuidanceFollowup,
}

/// Terminal outcome carried by [`TerminationTransition::Break`].
///
/// Named `EndOfTurnOutcome` (not `TerminationReason`) to avoid naming
/// collision with [`crate::contracts::TerminationReason`], which is the
/// sub-agent protocol type (out of scope for this module).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EndOfTurnOutcome {
    /// Clean final answer — no guards fired.
    FinalAnswer,
    /// Fallback completion accepted (phase estimator).
    FallbackCompleted,
    /// Post-tool ANVIL_FINAL was accepted after tool execution.
    PostToolAnvilFinalAccepted,
    /// Guidance follow-up completed (no retry needed / budget exhausted).
    GuidanceFollowupComplete,
}

/// Unified input for [`decide_no_tool_call`].
///
/// Fields are kept private; instances are constructed via one of the three
/// named constructors (`for_agentic_loop`, `for_done_path`, `for_testing`).
///
/// This avoids the 8-positional-argument footgun: six of the fields are
/// booleans, and getting two of them in the wrong order is an easy and
/// silent source of bugs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoToolCallInput {
    /// Whether ANVIL_FINAL was detected in this response.
    anvil_final_detected: bool,
    /// Whether we are awaiting a guidance follow-up response.
    awaiting_guidance_followup: bool,
    /// Number of final-guard retries already used.
    final_guard_retries: u8,
    /// Whether `touched_files` is empty (no file modifications this turn).
    touched_files_empty: bool,
    /// Phase action from the phase estimator.
    phase_action: PhaseAction,
    /// Whether the execution plan has incomplete items (Branch 3 fallthrough).
    plan_has_incomplete_items: bool,
    /// Whether the plan gate should fire (Branch 4).
    plan_gate_fires: bool,
    /// Whether the task-semantics gate should fire (Branch 5).
    task_semantics_fires: bool,
}

impl NoToolCallInput {
    /// Constructor used on the agentic-loop path.
    ///
    /// Derives `anvil_final_detected`, `awaiting_guidance_followup` and
    /// `final_guard_retries` from the supplied [`TerminationLoopState`].
    ///
    /// Note: visibility is `pub(crate)` because `TerminationLoopState` is
    /// a `pub(crate)` type — exposing this constructor at `pub` would leak
    /// a private type through its signature.
    pub(crate) fn for_agentic_loop(
        state: &TerminationLoopState,
        touched_files_empty: bool,
        phase_action: PhaseAction,
        plan_has_incomplete_items: bool,
        plan_gate_fires: bool,
        task_semantics_fires: bool,
    ) -> Self {
        Self {
            anvil_final_detected: state.is_anvil_final_seen(),
            awaiting_guidance_followup: state.awaiting_guidance_followup(),
            final_guard_retries: state.final_guard_retries(),
            touched_files_empty,
            phase_action,
            plan_has_incomplete_items,
            plan_gate_fires,
            task_semantics_fires,
        }
    }

    /// Constructor used on the Done path.
    ///
    /// Hardcodes Done-path invariants: `awaiting_guidance_followup=false`,
    /// `final_guard_retries=0`, `plan_gate_fires=false` (the plan gate is
    /// handled directly inside [`decide_done_path`] before this constructor
    /// is called, preserving the `plan gate → final guard → task semantics`
    /// ordering from Issue #253).
    ///
    /// `phase_action` and `plan_has_incomplete_items` are passed through
    /// from the caller.  The caller contract for Done-path invocations is
    /// `phase_action=Continue` and `plan_has_incomplete_items=false`;
    /// [`decide_done_path`] will panic via `unreachable!` if these invariants
    /// are violated, so callers must uphold them.
    pub fn for_done_path(
        anvil_final_detected: bool,
        touched_files_empty: bool,
        phase_action: PhaseAction,
        plan_has_incomplete_items: bool,
        task_semantics_fires: bool,
    ) -> Self {
        Self {
            anvil_final_detected,
            awaiting_guidance_followup: false,
            final_guard_retries: 0,
            touched_files_empty,
            phase_action,
            plan_has_incomplete_items,
            plan_gate_fires: false,
            task_semantics_fires,
        }
    }

    /// Full-control constructor used by integration tests.
    ///
    /// This constructor is intentionally not cfg-gated so that
    /// `tests/termination_semantics.rs` can build arbitrary inputs without
    /// relying on a `testing` feature flag.  It is `#[doc(hidden)]` to
    /// discourage production callers from using it directly.
    #[doc(hidden)]
    #[allow(clippy::too_many_arguments)]
    pub fn for_testing(
        anvil_final_detected: bool,
        awaiting_guidance_followup: bool,
        final_guard_retries: u8,
        touched_files_empty: bool,
        phase_action: PhaseAction,
        plan_has_incomplete_items: bool,
        plan_gate_fires: bool,
        task_semantics_fires: bool,
    ) -> Self {
        Self {
            anvil_final_detected,
            awaiting_guidance_followup,
            final_guard_retries,
            touched_files_empty,
            phase_action,
            plan_has_incomplete_items,
            plan_gate_fires,
            task_semantics_fires,
        }
    }
}

/// Single source of truth for evaluating a no-tool-call response.
///
/// Pure function with no side-effects.  Side-effects (message injection,
/// telemetry, assistant output recording) are the responsibility of the
/// orchestration layer in `agentic.rs`.
pub fn decide_no_tool_call(input: NoToolCallInput) -> NoToolCallDecision {
    // Branch 1: guidance follow-up
    if input.awaiting_guidance_followup {
        // Guidance escalation uses the same retry budget as Branch 2 but
        // does NOT require `anvil_final_detected`.  ANVIL_FINAL may have
        // been suppressed via `suppress_anvil_final()` when the guidance
        // injection was posted.
        if input.touched_files_empty && input.final_guard_retries < MAX_FINAL_GUARD_RETRIES {
            return NoToolCallDecision::FinalGuardRetry;
        }
        return NoToolCallDecision::GuidanceFollowupComplete;
    }

    // Branch 2: ANVIL_FINAL guard (requires anvil_final_detected, DR2-003)
    if input.anvil_final_detected
        && input.touched_files_empty
        && input.final_guard_retries < MAX_FINAL_GUARD_RETRIES
    {
        return NoToolCallDecision::FinalGuardRetry;
    }

    // Branch 3: fallback completion (Issue #307: if plan has incomplete
    // items, fall through to plan gate instead).
    if matches!(input.phase_action, PhaseAction::FallbackComplete)
        && !input.plan_has_incomplete_items
    {
        return NoToolCallDecision::FallbackCompleted;
    }

    // Branch 4: plan gate
    if input.plan_gate_fires {
        return NoToolCallDecision::PlanGateSuppression;
    }

    // Branch 5: task-semantics gate
    if input.task_semantics_fires {
        return NoToolCallDecision::TaskSemanticsSuppression;
    }

    // Final: accept as answer
    NoToolCallDecision::FinalAnswer
}

/// Loop-local state governing termination decisions in the agentic loop.
///
/// All fields are initialised at the start of `complete_structured_response`
/// and consumed only within that method (or within
/// `handle_done_path_anvil_final_guard`, where a fresh instance is created
/// each call).  App-level session state (`forced_mode_active`,
/// `repair_closure_active`, `proactive_delegation_pending`) is intentionally
/// excluded — see Issue #385 scope boundary.
#[derive(Debug)]
pub(crate) struct TerminationLoopState {
    anvil_final_seen: bool,
    final_guard_retries: u8,
    guidance_retry_used: bool,
    awaiting_guidance_followup: bool,
    noplan_suppression_count: u8,
    pre_exit_repair_injected: bool,
    task_semantics_suppression_count: u8,
}

impl TerminationLoopState {
    /// Create a new state bag for the start of a turn.
    ///
    /// `anvil_final_already` captures whether a prior response (in the
    /// Done path) had already carried an ANVIL_FINAL marker;
    /// `initial_anvil_final_detected` captures whether the current
    /// structured response carries one.
    pub(crate) fn new(anvil_final_already: bool, initial_anvil_final_detected: bool) -> Self {
        Self {
            anvil_final_seen: anvil_final_already || initial_anvil_final_detected,
            final_guard_retries: 0,
            guidance_retry_used: false,
            awaiting_guidance_followup: false,
            noplan_suppression_count: 0,
            pre_exit_repair_injected: false,
            task_semantics_suppression_count: 0,
        }
    }

    // --- Read-only accessors --------------------------------------------------

    /// Has ANVIL_FINAL been observed (and not subsequently suppressed)?
    pub(crate) fn is_anvil_final_seen(&self) -> bool {
        self.anvil_final_seen
    }

    /// Number of final-guard retries already used this turn.
    pub(crate) fn final_guard_retries(&self) -> u8 {
        self.final_guard_retries
    }

    /// Has the one-shot guidance retry been consumed?
    pub(crate) fn guidance_retry_used(&self) -> bool {
        self.guidance_retry_used
    }

    /// Are we awaiting a guidance follow-up response?
    pub(crate) fn awaiting_guidance_followup(&self) -> bool {
        self.awaiting_guidance_followup
    }

    /// Current plan-gate suppression count (capped at `MAX_PLAN_SUPPRESSIONS`).
    pub(crate) fn noplan_suppression_count(&self) -> u8 {
        self.noplan_suppression_count
    }

    /// Has the pre-exit repair turn been injected (Issue #325)?
    pub(crate) fn pre_exit_repair_injected(&self) -> bool {
        self.pre_exit_repair_injected
    }

    /// Current task-semantics gate suppression count
    /// (capped at `MAX_TASK_SEMANTICS_SUPPRESSIONS`).
    pub(crate) fn task_semantics_suppression_count(&self) -> u8 {
        self.task_semantics_suppression_count
    }

    // --- Mutating methods -----------------------------------------------------

    /// Record that ANVIL_FINAL was newly detected in an LLM response.
    pub(crate) fn observe_anvil_final(&mut self) {
        self.anvil_final_seen = true;
    }

    /// Suppress the current ANVIL_FINAL detection (plan gate / guidance
    /// retry).  The state may be re-observed later in the same turn.
    pub(crate) fn suppress_anvil_final(&mut self) {
        self.anvil_final_seen = false;
    }

    /// Increment the final-guard retry counter.
    ///
    /// Invariant: the counter never exceeds [`MAX_FINAL_GUARD_RETRIES`].
    pub(crate) fn increment_final_guard_retries(&mut self) {
        debug_assert!(
            self.final_guard_retries < MAX_FINAL_GUARD_RETRIES,
            "final_guard_retries exceeded MAX_FINAL_GUARD_RETRIES"
        );
        self.final_guard_retries = self.final_guard_retries.saturating_add(1);
    }

    /// Mark the one-shot guidance retry as consumed.  Monotonic: once set,
    /// never cleared within the same turn.
    pub(crate) fn mark_guidance_retry_used(&mut self) {
        self.guidance_retry_used = true;
    }

    /// Record that we posted a guidance injection and are now awaiting the
    /// follow-up response.
    pub(crate) fn set_awaiting_guidance_followup(&mut self) {
        self.awaiting_guidance_followup = true;
    }

    /// Clear the awaiting-guidance-followup flag (called after the
    /// follow-up arrives).
    pub(crate) fn clear_awaiting_guidance_followup(&mut self) {
        self.awaiting_guidance_followup = false;
    }

    /// Record a plan-gate suppression event.
    ///
    /// Invariant: count stays at or below [`MAX_PLAN_SUPPRESSIONS`]
    /// (Issue #285 A2).
    pub(crate) fn record_plan_suppression(&mut self) {
        debug_assert!(
            self.noplan_suppression_count < MAX_PLAN_SUPPRESSIONS,
            "noplan_suppression_count exceeded MAX_PLAN_SUPPRESSIONS"
        );
        self.noplan_suppression_count = self.noplan_suppression_count.saturating_add(1);
    }

    /// Record that the pre-exit repair turn was injected (Issue #325).
    pub(crate) fn mark_pre_exit_repair_injected(&mut self) {
        self.pre_exit_repair_injected = true;
    }

    /// Record a task-semantics gate suppression event.
    ///
    /// Invariant: count stays at or below
    /// [`MAX_TASK_SEMANTICS_SUPPRESSIONS`] (Issue #382).
    pub(crate) fn record_task_semantics_suppression(&mut self) {
        debug_assert!(
            self.task_semantics_suppression_count < MAX_TASK_SEMANTICS_SUPPRESSIONS,
            "task_semantics_suppression_count exceeded MAX_TASK_SEMANTICS_SUPPRESSIONS"
        );
        self.task_semantics_suppression_count =
            self.task_semantics_suppression_count.saturating_add(1);
    }
}

// ---------------------------------------------------------------------------
// Entry points
// ---------------------------------------------------------------------------

/// Done-path termination decision.
///
/// This entry point enforces the Done-path ordering (Issue #253 DR2-001):
///   plan gate → final guard → task semantics → FinalAnswer.
///
/// Unlike the agentic loop, Done-path state is not persisted across calls;
/// the caller creates a fresh [`TerminationLoopState`] for each invocation.
pub(crate) fn decide_done_path(
    state: &mut TerminationLoopState,
    anvil_final_detected: bool,
    plan_gate_fires: bool,
    task_semantics_fires: bool,
    touched_files_empty: bool,
    phase_action: PhaseAction,
    plan_has_incomplete_items: bool,
) -> TerminationTransition {
    // Mirror the caller-side bookkeeping: observe ANVIL_FINAL so subsequent
    // introspection of the state object is consistent.
    if anvil_final_detected {
        state.observe_anvil_final();
    }

    // Done-path ordering (Issue #253 DR2-001):
    // 1. Plan gate fires BEFORE the final guard.  Only fires when
    //    ANVIL_FINAL was detected and no file edits were made — this
    //    mirrors the existing `handle_done_path_anvil_final_guard` shape.
    if anvil_final_detected && touched_files_empty && plan_gate_fires {
        return TerminationTransition::Retry(RetryKind::PlanGate);
    }

    // 2. Delegate to the pure decision table for the remaining branches.
    //    The Done path always passes `plan_gate_fires=false` to
    //    `decide_no_tool_call` because the plan gate was handled above.
    let input = NoToolCallInput::for_done_path(
        anvil_final_detected,
        touched_files_empty,
        phase_action,
        plan_has_incomplete_items,
        task_semantics_fires,
    );

    match decide_no_tool_call(input) {
        NoToolCallDecision::FinalGuardRetry => TerminationTransition::Retry(RetryKind::FinalGuard),
        NoToolCallDecision::TaskSemanticsSuppression => {
            TerminationTransition::Retry(RetryKind::TaskSemantics)
        }
        NoToolCallDecision::FinalAnswer => {
            TerminationTransition::Break(EndOfTurnOutcome::FinalAnswer)
        }
        // The remaining variants are unreachable on the Done path because
        // we hardcode `awaiting_guidance_followup=false`,
        // `phase_action=Continue` (no fallback) and `plan_gate_fires=false`
        // (handled directly above).
        NoToolCallDecision::PlanGateSuppression => {
            unreachable!("Done path: plan gate is handled directly before decide_no_tool_call")
        }
        NoToolCallDecision::GuidanceFollowupComplete => {
            unreachable!("Done path never sets awaiting_guidance_followup")
        }
        NoToolCallDecision::FallbackCompleted => {
            unreachable!("Done path uses PhaseAction::Continue; no fallback completion")
        }
    }
}

/// Agentic-loop termination decision for no-tool-call responses.
///
/// Mirrors `decide_no_tool_call` but returns a [`TerminationTransition`]
/// (instead of the raw decision enum) so that the orchestration layer can
/// dispatch on the appropriate control-flow action.
pub(crate) fn decide_agentic_loop(
    state: &mut TerminationLoopState,
    input: NoToolCallInput,
) -> TerminationTransition {
    match decide_no_tool_call(input) {
        NoToolCallDecision::FinalGuardRetry => TerminationTransition::Retry(RetryKind::FinalGuard),
        NoToolCallDecision::GuidanceFollowupComplete => {
            TerminationTransition::Break(EndOfTurnOutcome::GuidanceFollowupComplete)
        }
        NoToolCallDecision::FallbackCompleted => {
            TerminationTransition::Break(EndOfTurnOutcome::FallbackCompleted)
        }
        NoToolCallDecision::PlanGateSuppression => {
            // Plan gate fired — orchestration layer is responsible for
            // recording the suppression on the state (the FSM returns
            // Continue so the loop picks up the next LLM turn).
            let _ = state;
            TerminationTransition::Continue
        }
        NoToolCallDecision::TaskSemanticsSuppression => {
            TerminationTransition::Retry(RetryKind::TaskSemantics)
        }
        NoToolCallDecision::FinalAnswer => {
            TerminationTransition::Break(EndOfTurnOutcome::FinalAnswer)
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- TerminationLoopState unit tests (moved from agentic.rs, Issue #385) ---

    #[test]
    fn termination_loop_state_new_both_false() {
        let state = TerminationLoopState::new(false, false);
        assert!(!state.is_anvil_final_seen());
        assert_eq!(state.final_guard_retries(), 0);
        assert!(!state.guidance_retry_used());
        assert!(!state.awaiting_guidance_followup());
        assert_eq!(state.noplan_suppression_count(), 0);
        assert!(!state.pre_exit_repair_injected());
        assert_eq!(state.task_semantics_suppression_count(), 0);
    }

    #[test]
    fn termination_loop_state_new_already_true() {
        let state = TerminationLoopState::new(true, false);
        assert!(state.is_anvil_final_seen());
    }

    #[test]
    fn termination_loop_state_new_detected_true() {
        let state = TerminationLoopState::new(false, true);
        assert!(state.is_anvil_final_seen());
    }

    #[test]
    fn termination_loop_state_suppress_and_observe() {
        let mut state = TerminationLoopState::new(true, false);
        assert!(state.is_anvil_final_seen());
        state.suppress_anvil_final();
        assert!(!state.is_anvil_final_seen());
        state.observe_anvil_final();
        assert!(state.is_anvil_final_seen());
    }

    #[test]
    fn termination_loop_state_record_plan_suppression() {
        let mut state = TerminationLoopState::new(false, false);
        assert_eq!(state.noplan_suppression_count(), 0);
        state.record_plan_suppression();
        assert_eq!(state.noplan_suppression_count(), 1);
    }

    // --- New FSM-level tests (Issue #381) --------------------------------

    /// (a) Issue #173: ANVIL_FINAL detected + empty touched_files →
    /// FSM requests a final-guard retry.
    #[test]
    fn fsm_anvil_final_and_empty_touched_files_retries_final_guard() {
        let mut state = TerminationLoopState::new(false, true); // ANVIL_FINAL observed
        let input = NoToolCallInput::for_agentic_loop(
            &state,
            /*touched_files_empty=*/ true,
            PhaseAction::Continue,
            /*plan_has_incomplete_items=*/ false,
            /*plan_gate_fires=*/ false,
            /*task_semantics_fires=*/ false,
        );
        let transition = decide_agentic_loop(&mut state, input);
        assert_eq!(
            transition,
            TerminationTransition::Retry(RetryKind::FinalGuard),
            "ANVIL_FINAL + empty touched_files must trigger FinalGuard retry (Issue #173)"
        );
    }

    /// (b) Issue #307: FallbackCompleted + incomplete plan must NOT accept
    /// as final.  With plan_gate_fires=true (as the caller would compute
    /// when the plan is incomplete), FSM returns Continue (PlanGateSuppression).
    #[test]
    fn fsm_fallback_completed_with_incomplete_plan_suppresses() {
        let mut state = TerminationLoopState::new(false, false);
        let input = NoToolCallInput::for_agentic_loop(
            &state,
            /*touched_files_empty=*/ false,
            PhaseAction::FallbackComplete,
            /*plan_has_incomplete_items=*/ true,
            /*plan_gate_fires=*/ true,
            /*task_semantics_fires=*/ false,
        );
        let transition = decide_agentic_loop(&mut state, input);
        assert_eq!(
            transition,
            TerminationTransition::Continue,
            "FallbackCompleted + incomplete plan must be suppressed (Issue #307)"
        );
    }

    /// (c) Issue #382: implementation task + task-semantics gate fires →
    /// FSM requests a task-semantics retry.
    #[test]
    fn fsm_implementation_task_fires_task_semantics_retry() {
        let mut state = TerminationLoopState::new(false, false);
        let input = NoToolCallInput::for_agentic_loop(
            &state,
            /*touched_files_empty=*/ true,
            PhaseAction::Continue,
            /*plan_has_incomplete_items=*/ false,
            /*plan_gate_fires=*/ false,
            /*task_semantics_fires=*/ true,
        );
        let transition = decide_agentic_loop(&mut state, input);
        assert_eq!(
            transition,
            TerminationTransition::Retry(RetryKind::TaskSemantics),
            "implementation task with task_semantics_fires must trigger TaskSemantics retry \
             (Issue #382)"
        );
    }

    /// (d) Issue #285 / #249: plan gate budget exhausted on the second
    /// invocation.  Once `record_plan_suppression` has been called, the
    /// caller would pass `plan_gate_fires=false`, and with no other gate
    /// active the FSM must accept as FinalAnswer (Break).
    #[test]
    fn fsm_plan_gate_budget_exhausted_breaks() {
        let mut state = TerminationLoopState::new(false, false);
        // First invocation: plan gate fires, suppression recorded.
        let input1 = NoToolCallInput::for_agentic_loop(
            &state,
            /*touched_files_empty=*/ true,
            PhaseAction::Continue,
            /*plan_has_incomplete_items=*/ true,
            /*plan_gate_fires=*/ true,
            /*task_semantics_fires=*/ false,
        );
        let t1 = decide_agentic_loop(&mut state, input1);
        assert_eq!(t1, TerminationTransition::Continue);
        state.record_plan_suppression();

        // Second invocation: budget exceeded; caller passes plan_gate_fires=false.
        let input2 = NoToolCallInput::for_agentic_loop(
            &state,
            /*touched_files_empty=*/ true,
            PhaseAction::Continue,
            /*plan_has_incomplete_items=*/ true,
            /*plan_gate_fires=*/ false,
            /*task_semantics_fires=*/ false,
        );
        let t2 = decide_agentic_loop(&mut state, input2);
        assert_eq!(
            t2,
            TerminationTransition::Break(EndOfTurnOutcome::FinalAnswer),
            "plan gate exhausted + no other gates must break with FinalAnswer (Issue #285/#249)"
        );
    }

    /// (e) Issue #372: once `final_guard_retries >= MAX_FINAL_GUARD_RETRIES`,
    /// the FSM must NOT retry again.  The result is Break(FinalAnswer) when
    /// no other gates are active.
    #[test]
    fn fsm_final_guard_retries_exhausted_breaks_final_answer() {
        let mut state = TerminationLoopState::new(false, true);
        state.increment_final_guard_retries();
        assert_eq!(state.final_guard_retries(), MAX_FINAL_GUARD_RETRIES);

        let input = NoToolCallInput::for_agentic_loop(
            &state,
            /*touched_files_empty=*/ true,
            PhaseAction::Continue,
            /*plan_has_incomplete_items=*/ false,
            /*plan_gate_fires=*/ false,
            /*task_semantics_fires=*/ false,
        );
        let transition = decide_agentic_loop(&mut state, input);
        assert_eq!(
            transition,
            TerminationTransition::Break(EndOfTurnOutcome::FinalAnswer),
            "final_guard_retries >= MAX must not retry; break with FinalAnswer (Issue #372)"
        );
    }

    // --- Done-path ordering tests ----------------------------------------

    /// Done path: plan gate ordering (Issue #253 DR2-001) — plan gate
    /// fires BEFORE final guard.
    #[test]
    fn done_path_plan_gate_ordered_before_final_guard() {
        let mut state = TerminationLoopState::new(false, false);
        let transition = decide_done_path(
            &mut state,
            /*anvil_final_detected=*/ true,
            /*plan_gate_fires=*/ true,
            /*task_semantics_fires=*/ false,
            /*touched_files_empty=*/ true,
            PhaseAction::Continue,
            /*plan_has_incomplete_items=*/ true,
        );
        assert_eq!(
            transition,
            TerminationTransition::Retry(RetryKind::PlanGate),
            "Done path: plan gate must fire before final guard (Issue #253 DR2-001)"
        );
    }

    /// Done path: final guard fires when plan gate does not.
    #[test]
    fn done_path_final_guard_fires_when_plan_gate_inactive() {
        let mut state = TerminationLoopState::new(false, false);
        let transition = decide_done_path(
            &mut state,
            /*anvil_final_detected=*/ true,
            /*plan_gate_fires=*/ false,
            /*task_semantics_fires=*/ false,
            /*touched_files_empty=*/ true,
            PhaseAction::Continue,
            /*plan_has_incomplete_items=*/ false,
        );
        assert_eq!(
            transition,
            TerminationTransition::Retry(RetryKind::FinalGuard),
            "Done path: final guard must fire when plan gate is inactive"
        );
    }

    /// Done path: task-semantics gate fires last in the ordering.
    #[test]
    fn done_path_task_semantics_fires_after_final_guard() {
        let mut state = TerminationLoopState::new(false, false);
        // ANVIL_FINAL absent → plan gate doesn't fire; final guard doesn't
        // fire (requires anvil_final_detected).  Task semantics wins.
        let transition = decide_done_path(
            &mut state,
            /*anvil_final_detected=*/ false,
            /*plan_gate_fires=*/ true, // ignored because anvil_final_detected=false
            /*task_semantics_fires=*/ true,
            /*touched_files_empty=*/ true,
            PhaseAction::Continue,
            /*plan_has_incomplete_items=*/ false,
        );
        assert_eq!(
            transition,
            TerminationTransition::Retry(RetryKind::TaskSemantics),
            "Done path: task-semantics gate fires when plan gate/final guard inactive"
        );
    }

    /// Done path: clean final answer when no gates fire.
    #[test]
    fn done_path_clean_final_answer() {
        let mut state = TerminationLoopState::new(false, false);
        let transition = decide_done_path(
            &mut state,
            /*anvil_final_detected=*/ false,
            /*plan_gate_fires=*/ false,
            /*task_semantics_fires=*/ false,
            /*touched_files_empty=*/ true,
            PhaseAction::Continue,
            /*plan_has_incomplete_items=*/ false,
        );
        assert_eq!(
            transition,
            TerminationTransition::Break(EndOfTurnOutcome::FinalAnswer),
            "Done path: clean response must be accepted as FinalAnswer"
        );
    }
}
