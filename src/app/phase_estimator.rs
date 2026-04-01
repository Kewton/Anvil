//! Phase estimation based on tool call patterns (Issue #159).
//!
//! For LLM models that do not emit ANVIL_FINAL markers, this module estimates
//! the current agent phase (exploring vs implementing) from the sequence of
//! tool calls and provides fallback completion detection.

/// Read-category tool names.
const READ_TOOLS: &[&str] = &["file.read", "file.search", "web.fetch"];
/// Write-category tool names.
const WRITE_TOOLS: &[&str] = &["file.edit", "file.edit_anchor", "file.write"];

/// Action recommended by the phase estimator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PhaseAction {
    /// No action needed.
    Continue,
    /// Inject a system message to nudge the LLM toward implementation.
    ForceTransition(String),
    /// Fallback completion detected (ANVIL_FINAL was never observed).
    FallbackComplete,
}

/// Estimated agent phase (for logging/debugging).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Not enough data to determine phase.
    Unknown,
    /// Predominantly reading files.
    Exploring,
    /// Has performed write operations.
    Implementing,
}

impl std::fmt::Display for Phase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Phase::Unknown => write!(f, "unknown"),
            Phase::Exploring => write!(f, "exploring"),
            Phase::Implementing => write!(f, "implementing"),
        }
    }
}

/// Tool category for phase estimation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToolCategory {
    Read,
    Write,
    Other,
}

fn classify_tool(tool_name: &str) -> ToolCategory {
    if READ_TOOLS.contains(&tool_name) {
        ToolCategory::Read
    } else if WRITE_TOOLS.contains(&tool_name) {
        ToolCategory::Write
    } else {
        ToolCategory::Other
    }
}

/// Estimates the agent phase from tool call patterns and provides
/// fallback completion detection when ANVIL_FINAL is not emitted.
pub struct PhaseEstimator {
    /// Consecutive read-category tool calls (reset on write success or reset()).
    consecutive_reads: usize,
    /// Whether a write-category tool has succeeded at least once.
    has_written: bool,
    /// Whether ANVIL_FINAL has been observed at least once in this session.
    anvil_final_observed: bool,
    /// Threshold N: consecutive reads to enter "exploring" phase.
    explore_threshold: usize,
    /// Threshold M: consecutive reads to trigger forced transition (M > N).
    force_transition_threshold: usize,
    /// Threshold K: consecutive reads after last write for fallback completion.
    completion_read_threshold: usize,
}

impl PhaseEstimator {
    /// Create a new PhaseEstimator with the given thresholds.
    pub fn new(
        explore_threshold: usize,
        force_transition_threshold: usize,
        completion_read_threshold: usize,
    ) -> Self {
        Self {
            consecutive_reads: 0,
            has_written: false,
            anvil_final_observed: false,
            explore_threshold,
            force_transition_threshold,
            completion_read_threshold,
        }
    }

    /// Reset per-turn counters. Called at the start of each
    /// `complete_structured_response()` turn.
    ///
    /// `has_written` and `anvil_final_observed` are **preserved** across turns.
    pub fn reset(&mut self) {
        self.consecutive_reads = 0;
    }

    /// Record a tool call and return a recommended action.
    ///
    /// Returns `Continue` or `ForceTransition`. Never returns `FallbackComplete`
    /// (use [`check_empty_response`] for that).
    pub fn record_tool_call(&mut self, tool_name: &str, success: bool) -> PhaseAction {
        match classify_tool(tool_name) {
            ToolCategory::Read => {
                self.consecutive_reads += 1;
                if self.consecutive_reads >= self.force_transition_threshold {
                    return PhaseAction::ForceTransition(
                        "You have been reading files extensively. \
                         Please proceed to implementation using file.edit or file.write."
                            .to_string(),
                    );
                }
                PhaseAction::Continue
            }
            ToolCategory::Write => {
                if success {
                    self.consecutive_reads = 0;
                    self.has_written = true;
                }
                PhaseAction::Continue
            }
            ToolCategory::Other => PhaseAction::Continue,
        }
    }

    /// Check whether an empty-tool-calls response indicates fallback completion.
    ///
    /// Returns `FallbackComplete` when:
    /// - `has_written` is true (at least one successful write)
    /// - `consecutive_reads >= completion_read_threshold` (verification reads after write)
    /// - `anvil_final_observed` is false (ANVIL_FINAL was never seen)
    ///
    /// Otherwise returns `Continue`.
    pub fn check_empty_response(&self) -> PhaseAction {
        if self.anvil_final_observed {
            return PhaseAction::Continue;
        }
        if self.has_written && self.consecutive_reads >= self.completion_read_threshold {
            return PhaseAction::FallbackComplete;
        }
        PhaseAction::Continue
    }

    /// Record that ANVIL_FINAL was observed. Disables fallback completion
    /// for the remainder of the session.
    pub fn observe_anvil_final(&mut self) {
        self.anvil_final_observed = true;
    }

    /// Reset model-dependent state (called on `/model` switch).
    /// Clears `anvil_final_observed` since a new model may not emit ANVIL_FINAL.
    #[allow(dead_code)]
    pub fn reset_model_state(&mut self) {
        self.anvil_final_observed = false;
    }

    /// Return the estimated phase for logging/debugging.
    pub fn current_phase(&self) -> Phase {
        if self.has_written {
            Phase::Implementing
        } else if self.consecutive_reads >= self.explore_threshold {
            Phase::Exploring
        } else {
            Phase::Unknown
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_estimator() -> PhaseEstimator {
        PhaseEstimator::new(5, 10, 5)
    }

    #[test]
    fn classify_tool_read() {
        assert_eq!(classify_tool("file.read"), ToolCategory::Read);
        assert_eq!(classify_tool("file.search"), ToolCategory::Read);
        assert_eq!(classify_tool("web.fetch"), ToolCategory::Read);
    }

    #[test]
    fn classify_tool_write() {
        assert_eq!(classify_tool("file.edit"), ToolCategory::Write);
        assert_eq!(classify_tool("file.write"), ToolCategory::Write);
    }

    #[test]
    fn classify_tool_other() {
        assert_eq!(classify_tool("shell.exec"), ToolCategory::Other);
        assert_eq!(classify_tool("agent.explore"), ToolCategory::Other);
        assert_eq!(classify_tool("agent.plan"), ToolCategory::Other);
        assert_eq!(classify_tool("unknown.tool"), ToolCategory::Other);
    }

    #[test]
    fn consecutive_reads_trigger_exploring_phase() {
        let mut est = default_estimator();
        for _ in 0..5 {
            est.record_tool_call("file.read", true);
        }
        assert_eq!(est.current_phase(), Phase::Exploring);
    }

    #[test]
    fn write_success_resets_consecutive_reads() {
        let mut est = default_estimator();
        for _ in 0..4 {
            est.record_tool_call("file.read", true);
        }
        assert_eq!(est.consecutive_reads, 4);
        est.record_tool_call("file.edit", true);
        assert_eq!(est.consecutive_reads, 0);
        assert!(est.has_written);
    }

    #[test]
    fn write_failure_does_not_reset() {
        let mut est = default_estimator();
        for _ in 0..4 {
            est.record_tool_call("file.read", true);
        }
        est.record_tool_call("file.edit", false);
        assert_eq!(est.consecutive_reads, 4);
        assert!(!est.has_written);
    }

    #[test]
    fn force_transition_at_m_reads() {
        let mut est = default_estimator();
        for i in 0..10 {
            let action = est.record_tool_call("file.read", true);
            if i < 9 {
                assert_eq!(action, PhaseAction::Continue);
            } else {
                assert!(matches!(action, PhaseAction::ForceTransition(_)));
            }
        }
    }

    #[test]
    fn fallback_complete_conditions() {
        let mut est = default_estimator();
        // No write yet → no fallback
        for _ in 0..5 {
            est.record_tool_call("file.read", true);
        }
        assert_eq!(est.check_empty_response(), PhaseAction::Continue);

        // Write, then K reads → fallback
        est.record_tool_call("file.write", true);
        for _ in 0..5 {
            est.record_tool_call("file.read", true);
        }
        assert_eq!(est.check_empty_response(), PhaseAction::FallbackComplete);
    }

    #[test]
    fn anvil_final_observed_disables_fallback() {
        let mut est = default_estimator();
        est.record_tool_call("file.write", true);
        for _ in 0..5 {
            est.record_tool_call("file.read", true);
        }
        est.observe_anvil_final();
        assert_eq!(est.check_empty_response(), PhaseAction::Continue);
    }

    #[test]
    fn reset_preserves_has_written_and_anvil_final() {
        let mut est = default_estimator();
        est.record_tool_call("file.write", true);
        est.observe_anvil_final();
        est.record_tool_call("file.read", true);

        est.reset();

        assert_eq!(est.consecutive_reads, 0);
        assert!(est.has_written);
        assert!(est.anvil_final_observed);
    }

    #[test]
    fn reset_model_state_clears_anvil_final() {
        let mut est = default_estimator();
        est.observe_anvil_final();
        assert!(est.anvil_final_observed);

        est.reset_model_state();
        assert!(!est.anvil_final_observed);
    }

    #[test]
    fn current_phase_unknown_initially() {
        let est = default_estimator();
        assert_eq!(est.current_phase(), Phase::Unknown);
    }

    #[test]
    fn current_phase_implementing_after_write() {
        let mut est = default_estimator();
        est.record_tool_call("file.edit", true);
        assert_eq!(est.current_phase(), Phase::Implementing);
    }

    #[test]
    fn other_tools_do_not_affect_counts() {
        let mut est = default_estimator();
        est.record_tool_call("shell.exec", true);
        assert_eq!(est.consecutive_reads, 0);
        assert!(!est.has_written);
    }

    #[test]
    fn phase_display() {
        assert_eq!(Phase::Unknown.to_string(), "unknown");
        assert_eq!(Phase::Exploring.to_string(), "exploring");
        assert_eq!(Phase::Implementing.to_string(), "implementing");
    }

    #[test]
    fn record_tool_call_never_returns_fallback_complete() {
        let mut est = default_estimator();
        est.record_tool_call("file.write", true);
        for _ in 0..20 {
            let action = est.record_tool_call("file.read", true);
            assert_ne!(action, PhaseAction::FallbackComplete);
        }
    }
}
