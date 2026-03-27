//! Loop detection for the agentic tool-use loop (Issue #145).
//!
//! Detects repeated identical tool calls using a ring-buffer of fingerprints
//! and responds with escalating actions: Warn → StrongWarn → Break.

use std::collections::VecDeque;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

/// Ring buffer maximum size (fixed for Phase 1).
pub const DEFAULT_MAX_HISTORY: usize = 20;

/// Action to take based on loop detection results.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoopAction {
    /// No loop detected, continue normally.
    Continue,
    /// First detection: warning message inserted as synthetic tool result.
    Warn(String),
    /// Second detection: strong warning message.
    StrongWarn(String),
    /// Third detection: terminate the agentic loop.
    Break(String),
}

impl LoopAction {
    /// Return the more severe of two `LoopAction` values.
    ///
    /// Priority: Break > StrongWarn > Warn > Continue.
    pub fn merge(self, other: LoopAction) -> LoopAction {
        match (&self, &other) {
            (LoopAction::Break(_), _) | (_, LoopAction::Continue) => self,
            (_, LoopAction::Break(_)) | (LoopAction::Continue, _) => other,
            (LoopAction::StrongWarn(_), _) => self,
            (_, LoopAction::StrongWarn(_)) => other,
            _ => self, // Both Warn: keep the first
        }
    }
}

/// Detects repeated identical tool calls within the agentic loop.
pub struct LoopDetector {
    /// Recent tool call fingerprints: (tool_name, input_hash).
    history: VecDeque<(String, u64)>,
    /// Maximum ring buffer size.
    max_history: usize,
    /// Number of identical consecutive calls before detection triggers.
    threshold: usize,
    /// Current escalation level (number of times detection has triggered).
    escalation_count: usize,
}

/// Compute a deterministic fingerprint from tool name and input JSON.
///
/// Uses `DefaultHasher` for simplicity. The same tool_name + input_json
/// combination always produces the same hash value within a single process.
pub fn fingerprint(tool_name: &str, input_json: &serde_json::Value) -> u64 {
    let mut hasher = DefaultHasher::new();
    tool_name.hash(&mut hasher);
    if let Ok(s) = serde_json::to_string(input_json) {
        s.hash(&mut hasher);
    }
    hasher.finish()
}

impl LoopDetector {
    /// Create a new LoopDetector with the given threshold.
    ///
    /// # Panics
    /// Panics if `threshold < 2` or `threshold > DEFAULT_MAX_HISTORY`.
    pub fn new(threshold: usize) -> Self {
        assert!(
            (2..=DEFAULT_MAX_HISTORY).contains(&threshold),
            "threshold must be in 2..={DEFAULT_MAX_HISTORY}, got {threshold}"
        );
        Self {
            history: VecDeque::with_capacity(DEFAULT_MAX_HISTORY),
            max_history: DEFAULT_MAX_HISTORY,
            threshold,
            escalation_count: 0,
        }
    }

    /// Reset all state. Called at the start of each `complete_structured_response()` turn.
    pub fn reset(&mut self) {
        self.history.clear();
        self.escalation_count = 0;
    }

    /// Record a tool call and check for loop patterns.
    ///
    /// This is the only public API that `agentic.rs` calls.
    pub fn record_and_check(
        &mut self,
        tool_name: &str,
        input_json: &serde_json::Value,
    ) -> LoopAction {
        let hash = fingerprint(tool_name, input_json);
        self.record(tool_name.to_string(), hash);
        self.check()
    }

    /// Append a fingerprint to the ring buffer, evicting oldest if full.
    fn record(&mut self, tool_name: String, hash: u64) {
        if self.history.len() >= self.max_history {
            self.history.pop_front();
        }
        self.history.push_back((tool_name, hash));
    }

    /// Check the tail of the history for repeated identical fingerprints.
    fn check(&mut self) -> LoopAction {
        if self.history.len() < self.threshold {
            return LoopAction::Continue;
        }

        // Check if the last `threshold` entries all share the same fingerprint.
        let len = self.history.len();
        let tail_start = len - self.threshold;
        let (ref last_name, last_hash) = self.history[len - 1];

        let all_same = self
            .history
            .iter()
            .skip(tail_start)
            .all(|(name, hash)| name == last_name && *hash == last_hash);

        if !all_same {
            return LoopAction::Continue;
        }

        self.escalation_count += 1;

        match self.escalation_count {
            1 => LoopAction::Warn(format!(
                "[Loop Detection] The same tool call '{}' has been repeated {} times consecutively. \
                 Consider trying a different approach or tool.",
                last_name, self.threshold
            )),
            2 => LoopAction::StrongWarn(format!(
                "[Loop Detection - WARNING] Tool '{}' is being called repeatedly with identical arguments. \
                 You MUST change your approach immediately. Try a completely different strategy.",
                last_name
            )),
            _ => LoopAction::Break(format!(
                "[Loop Detection - TERMINATED] Agentic loop terminated due to repeated identical calls to '{}'. \
                 The loop has been stopped to prevent infinite repetition.",
                last_name
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_is_deterministic() {
        let val = serde_json::json!({"path": "src/main.rs"});
        let h1 = fingerprint("file.read", &val);
        let h2 = fingerprint("file.read", &val);
        assert_eq!(h1, h2);
    }
}
