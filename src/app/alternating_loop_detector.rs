//! Alternating/cyclic loop detector for the agentic tool-use loop (Issue #172).
//!
//! Detects repeating patterns of tool calls (e.g., A-B-A-B or A-B-C-A-B-C)
//! using a ring buffer of fingerprints and responds with escalating actions.

use std::collections::VecDeque;

use super::loop_detector::{LoopAction, fingerprint};

/// Ring buffer size for alternating pattern detection.
pub const DEFAULT_BUFFER_SIZE: usize = 20;
/// Maximum cycle length to check.
pub const MAX_CYCLE_LENGTH: usize = 5;
/// Minimum cycle length to check.
pub const MIN_CYCLE_LENGTH: usize = 2;
/// Default number of cycle repetitions before detection triggers.
pub const DEFAULT_CYCLE_THRESHOLD: usize = 3;

/// Detects alternating/cyclic tool call patterns within the agentic loop.
pub struct AlternatingLoopDetector {
    /// Recent tool call fingerprints: (tool_name, input_hash).
    history: VecDeque<(String, u64)>,
    /// Ring buffer maximum size.
    buffer_size: usize,
    /// Number of cycle repetitions required before detection triggers.
    cycle_threshold: usize,
    /// Current escalation level (number of times detection has triggered).
    escalation_count: usize,
}

impl AlternatingLoopDetector {
    /// Create a new AlternatingLoopDetector with the given cycle threshold.
    pub fn new(cycle_threshold: usize) -> Self {
        Self {
            history: VecDeque::with_capacity(DEFAULT_BUFFER_SIZE),
            buffer_size: DEFAULT_BUFFER_SIZE,
            cycle_threshold,
            escalation_count: 0,
        }
    }

    /// Reset all state. Called at the start of each `complete_structured_response()` turn.
    pub fn reset(&mut self) {
        self.history.clear();
        self.escalation_count = 0;
    }

    /// Record a tool call and check for alternating/cyclic loop patterns.
    pub fn record_and_check(
        &mut self,
        tool_name: &str,
        input_json: &serde_json::Value,
    ) -> LoopAction {
        let fp = fingerprint(tool_name, input_json);
        self.history.push_back((tool_name.to_string(), fp));
        if self.history.len() > self.buffer_size {
            self.history.pop_front();
        }

        if let Some(cycle_len) = self.detect_cycle() {
            self.escalation_count += 1;
            return self.escalate(cycle_len);
        }

        LoopAction::Continue
    }

    /// Detect repeating cycles of length `MIN_CYCLE_LENGTH..=MAX_CYCLE_LENGTH`
    /// at the tail of the history buffer.
    fn detect_cycle(&self) -> Option<usize> {
        let len = self.history.len();

        for cycle_len in MIN_CYCLE_LENGTH..=MAX_CYCLE_LENGTH {
            let required = cycle_len * self.cycle_threshold;
            if len < required {
                continue;
            }

            let start = len - required;
            let is_cycle = (0..required)
                .all(|j| self.history[start + j] == self.history[start + (j % cycle_len)]);
            if is_cycle {
                return Some(cycle_len);
            }
        }

        None
    }

    /// Escalate based on how many times detection has triggered.
    fn escalate(&self, cycle_len: usize) -> LoopAction {
        let msg = format!(
            "[Anvil] Alternating tool call pattern detected (cycle length: {}). \
             You are repeating the same sequence of tool calls. \
             Please try a different approach.",
            cycle_len
        );

        match self.escalation_count {
            1 => LoopAction::Warn(msg),
            2 => LoopAction::StrongWarn(msg),
            _ => LoopAction::Break(msg),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_creates_empty_detector() {
        let d = AlternatingLoopDetector::new(3);
        assert_eq!(d.history.len(), 0);
        assert_eq!(d.escalation_count, 0);
        assert_eq!(d.cycle_threshold, 3);
    }
}
