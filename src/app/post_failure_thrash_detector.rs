//! Post-failure tool-active thrash detector (Issue #351).
//!
//! After `agent.fix_slice` has failed at least once without a subsequent
//! worker success, the parent agent can enter a tool-active thrash loop:
//! it keeps calling tools (`file.read` / `file.search` / `shell.exec`) but
//! the execution plan never advances and no mutation lands. This shape is
//! outside PR #350's closure loop detector (which only fires on zero-
//! tool-call reasoning responses) and not caught by the generic
//! `LoopDetector` (which requires identical tool calls in a tight window).
//!
//! This detector counts consecutive turns while the guard is armed in
//! which (a) at least one tool was executed and (b) no execution plan
//! items were advanced and no mutation was recorded. When the count
//! reaches the configured thresholds, it escalates via the
//! `LoopAction` ladder: `Continue → Warn → StrongWarn → Break`.
//!
//! - Guard arming (the caller must verify these before calling):
//!     1. `worker_observed == false`
//!     2. `fixslice_worker_failure_count > 0`
//! - The detector is reset at the start of each top-level turn, when the
//!   guard disarms, and whenever a plan is (re-)registered.

use super::loop_detector::LoopAction;

/// Default number of consecutive no-progress turns before a Warn fires.
pub const DEFAULT_WARN_THRESHOLD: u32 = 3;
/// Default number of consecutive no-progress turns before a StrongWarn fires.
pub const DEFAULT_STRONG_WARN_THRESHOLD: u32 = 4;
/// Default number of consecutive no-progress turns before the detector
/// breaks the agentic loop.
pub const DEFAULT_BREAK_THRESHOLD: u32 = 5;

/// Detector for parent-side post-failure tool-active thrash.
#[derive(Debug)]
pub struct PostFailureThrashDetector {
    no_progress_turns: u32,
    warn_threshold: u32,
    strong_warn_threshold: u32,
    break_threshold: u32,
    escalation_count: u32,
}

impl Default for PostFailureThrashDetector {
    fn default() -> Self {
        Self::new(
            DEFAULT_WARN_THRESHOLD,
            DEFAULT_STRONG_WARN_THRESHOLD,
            DEFAULT_BREAK_THRESHOLD,
        )
    }
}

impl PostFailureThrashDetector {
    pub fn new(warn_threshold: u32, strong_warn_threshold: u32, break_threshold: u32) -> Self {
        Self {
            no_progress_turns: 0,
            warn_threshold,
            strong_warn_threshold,
            break_threshold,
            escalation_count: 0,
        }
    }

    /// Reset to a clean state.
    ///
    /// Called when the guard disarms (worker success observed, plan
    /// (re-)registered, or the turn made progress), and at the start of
    /// each top-level turn.
    pub fn reset(&mut self) {
        self.no_progress_turns = 0;
        self.escalation_count = 0;
    }

    /// Number of consecutive no-progress turns observed while armed.
    pub fn no_progress_turns(&self) -> u32 {
        self.no_progress_turns
    }

    /// Record a turn while the guard is armed and return the escalation
    /// action.
    ///
    /// `had_tool_calls` is true when the turn executed at least one tool
    /// call; `plan_advanced` is true when the turn advanced at least one
    /// plan item or recorded at least one mutation. Any turn that lacks
    /// tool calls or made progress resets the counter — this detector
    /// targets the tool-active-but-stuck shape specifically.
    pub fn record_turn(&mut self, had_tool_calls: bool, plan_advanced: bool) -> LoopAction {
        if plan_advanced || !had_tool_calls {
            self.no_progress_turns = 0;
            self.escalation_count = 0;
            return LoopAction::Continue;
        }

        self.no_progress_turns += 1;

        if self.no_progress_turns >= self.break_threshold {
            self.escalation_count += 1;
            return LoopAction::Break(
                "[Post-Failure Thrash Guard - TERMINATED] Agentic loop terminated: \
                 parent agent has been issuing tool calls with zero plan advancement \
                 after fix_slice worker failure. Classifying as post-failure thrash."
                    .to_string(),
            );
        }
        if self.no_progress_turns >= self.strong_warn_threshold {
            self.escalation_count += 1;
            return LoopAction::StrongWarn(
                "[Post-Failure Thrash Guard - WARNING] Tool calls are continuing but \
                 the execution plan has not advanced after a fix_slice worker failure. \
                 You MUST either re-invoke agent.fix_slice on the troubled target or \
                 emit ANVIL_FINAL and terminate. Do NOT keep reading / searching."
                    .to_string(),
            );
        }
        if self.no_progress_turns >= self.warn_threshold {
            self.escalation_count += 1;
            return LoopAction::Warn(
                "[Post-Failure Thrash Guard] The parent agent keeps calling tools after \
                 a fix_slice worker failure without advancing the execution plan. \
                 Decide now: either re-invoke agent.fix_slice with a specific target, \
                 or emit ANVIL_FINAL."
                    .to_string(),
            );
        }

        LoopAction::Continue
    }

    #[cfg(test)]
    pub fn escalation_count(&self) -> u32 {
        self.escalation_count
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn new_default() -> PostFailureThrashDetector {
        PostFailureThrashDetector::default()
    }

    #[test]
    fn progress_resets_counter() {
        let mut det = new_default();
        assert_eq!(det.record_turn(true, false), LoopAction::Continue);
        assert_eq!(det.no_progress_turns(), 1);
        assert_eq!(det.record_turn(true, true), LoopAction::Continue);
        assert_eq!(det.no_progress_turns(), 0);
    }

    #[test]
    fn tool_inactive_turn_resets_counter() {
        let mut det = new_default();
        assert_eq!(det.record_turn(true, false), LoopAction::Continue);
        assert_eq!(det.record_turn(true, false), LoopAction::Continue);
        // Zero-tool-call turn: closure_loop_detector territory, not ours.
        assert_eq!(det.record_turn(false, false), LoopAction::Continue);
        assert_eq!(det.no_progress_turns(), 0);
    }

    #[test]
    fn warn_at_warn_threshold() {
        let mut det = new_default();
        for _ in 0..(DEFAULT_WARN_THRESHOLD - 1) {
            assert_eq!(det.record_turn(true, false), LoopAction::Continue);
        }
        let action = det.record_turn(true, false);
        assert!(
            matches!(action, LoopAction::Warn(_)),
            "expected Warn at turn {DEFAULT_WARN_THRESHOLD}, got {action:?}"
        );
    }

    #[test]
    fn strong_warn_at_strong_warn_threshold() {
        let mut det = new_default();
        for _ in 0..(DEFAULT_STRONG_WARN_THRESHOLD - 1) {
            det.record_turn(true, false);
        }
        let action = det.record_turn(true, false);
        assert!(
            matches!(action, LoopAction::StrongWarn(_)),
            "expected StrongWarn at turn {DEFAULT_STRONG_WARN_THRESHOLD}, got {action:?}"
        );
    }

    #[test]
    fn break_at_break_threshold() {
        let mut det = new_default();
        for _ in 0..(DEFAULT_BREAK_THRESHOLD - 1) {
            det.record_turn(true, false);
        }
        let action = det.record_turn(true, false);
        assert!(
            matches!(action, LoopAction::Break(_)),
            "expected Break at turn {DEFAULT_BREAK_THRESHOLD}, got {action:?}"
        );
    }

    #[test]
    fn reset_clears_state() {
        let mut det = new_default();
        for _ in 0..DEFAULT_WARN_THRESHOLD {
            det.record_turn(true, false);
        }
        assert!(det.escalation_count() > 0);
        det.reset();
        assert_eq!(det.no_progress_turns(), 0);
        assert_eq!(det.escalation_count(), 0);
        assert_eq!(det.record_turn(true, false), LoopAction::Continue);
    }

    #[test]
    fn progress_turn_after_warn_clears_escalation() {
        let mut det = new_default();
        for _ in 0..DEFAULT_WARN_THRESHOLD {
            det.record_turn(true, false);
        }
        // A turn that finally advanced the plan resets escalation.
        assert_eq!(det.record_turn(true, true), LoopAction::Continue);
        assert_eq!(det.no_progress_turns(), 0);
        // Future drift still works from a clean slate.
        for _ in 0..(DEFAULT_WARN_THRESHOLD - 1) {
            assert_eq!(det.record_turn(true, false), LoopAction::Continue);
        }
        assert!(matches!(det.record_turn(true, false), LoopAction::Warn(_)));
    }
}
