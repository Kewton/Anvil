//! Turn-local ANVIL_PLAN stall detection (Issue #391).
//!
//! Local models (7B–13B) sometimes drift into repeating ANVIL_PLAN blocks
//! without ever emitting a `file.write` / `file.edit` tool call, stalling
//! implementation until the runner times out. `PlanStallTracker` observes
//! both events within a single agentic turn and flags the stall after the
//! second ANVIL_PLAN block when no mutation tool has executed yet.
//!
//! ## Lifecycle
//!
//! The tracker is a turn-local value — it is constructed at the start of
//! each `complete_structured_response` invocation and discarded at its end.
//! Do **not** hold it across turns: a fresh turn is expected to re-register
//! its own ANVIL_PLAN.
//!
//! ## Replan compatibility
//!
//! Once the first mutation tool has fired this turn,
//! [`PlanStallTracker::record_first_tool_call`] latches `first_tool_call_seen`
//! to `true` and all subsequent ANVIL_PLAN blocks are ignored by the
//! counter. This preserves Issue #305's replan flow — follow-up
//! ANVIL_PLAN blocks after real implementation progress are legitimate and
//! must not trigger the stall guard.

/// Tracks ANVIL_PLAN emissions within a single agentic turn and decides
/// whether the turn has stalled on repeated planning without any mutation.
///
/// See the module-level docs for the full lifecycle contract.
#[derive(Debug, Default)]
pub struct PlanStallTracker {
    /// Number of ANVIL_PLAN blocks seen in the current turn **before** the
    /// first successful `file.write` / `file.edit` tool call. Once
    /// [`Self::first_tool_call_seen`] latches to `true`, this counter stops
    /// being incremented.
    plan_count_in_turn: usize,
    /// Latched on the first successful mutation tool call of the turn. Any
    /// subsequent ANVIL_PLAN blocks are considered legitimate replan
    /// attempts (Issue #305) and MUST NOT contribute to the stall count.
    first_tool_call_seen: bool,
}

impl PlanStallTracker {
    /// Create a fresh tracker with no recorded state.
    pub fn new() -> Self {
        Self::default()
    }

    /// Reset all state at the start of a new agentic turn.
    ///
    /// Equivalent to replacing the value with [`Self::new`]; provided as
    /// an explicit method so callers can keep a stack-allocated tracker
    /// across nested call sites in the same turn if needed.
    pub fn reset_for_turn(&mut self) {
        self.plan_count_in_turn = 0;
        self.first_tool_call_seen = false;
    }

    /// Record a newly parsed ANVIL_PLAN (or ANVIL_PLAN replan) block.
    ///
    /// Only counts the block while no mutation tool has fired yet in this
    /// turn. Post-mutation ANVIL_PLAN blocks are follow-up replans and are
    /// intentionally ignored.
    pub fn record_plan_block(&mut self) {
        if !self.first_tool_call_seen {
            self.plan_count_in_turn = self.plan_count_in_turn.saturating_add(1);
        }
    }

    /// Latch the "a mutation tool has executed this turn" flag.
    ///
    /// Called on every successful `file.write` / `file.edit` invocation; the
    /// flag only needs to be set once per turn, additional calls are no-ops.
    pub fn record_first_tool_call(&mut self) {
        self.first_tool_call_seen = true;
    }

    /// Returns `true` when the turn has stalled: ANVIL_PLAN was observed at
    /// least twice before any `file.write` / `file.edit` call landed.
    ///
    /// The threshold is `>= 2` — the first ANVIL_PLAN is legitimate (it is
    /// the normal pre-mutation announcement), while a second block without
    /// any intervening tool call is the earliest reliable signal of drift.
    pub fn is_stalled(&self) -> bool {
        !self.first_tool_call_seen && self.plan_count_in_turn >= 2
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_tracker_is_not_stalled() {
        let tracker = PlanStallTracker::new();
        assert!(!tracker.is_stalled());
    }

    #[test]
    fn single_plan_block_is_not_stalled() {
        let mut tracker = PlanStallTracker::new();
        tracker.record_plan_block();
        assert!(!tracker.is_stalled());
    }

    #[test]
    fn second_plan_block_is_stalled() {
        let mut tracker = PlanStallTracker::new();
        tracker.record_plan_block();
        tracker.record_plan_block();
        assert!(tracker.is_stalled());
    }

    #[test]
    fn tool_call_before_second_plan_prevents_stall() {
        let mut tracker = PlanStallTracker::new();
        tracker.record_plan_block();
        tracker.record_first_tool_call();
        tracker.record_plan_block();
        assert!(!tracker.is_stalled());
    }

    #[test]
    fn reset_restores_fresh_state() {
        let mut tracker = PlanStallTracker::new();
        tracker.record_plan_block();
        tracker.record_plan_block();
        assert!(tracker.is_stalled());

        tracker.reset_for_turn();
        assert!(!tracker.is_stalled());
        tracker.record_plan_block();
        assert!(!tracker.is_stalled());
    }
}
