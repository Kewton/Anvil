use std::collections::HashMap;

/// Tracks consecutive file.write failures per file path.
/// Used to detect when the LLM is stuck retrying writes on the same file
/// and should be prompted to try an alternative approach.
pub(crate) struct WriteFailTracker {
    consecutive_failures: HashMap<String, u32>,
    threshold: u32,
}

impl WriteFailTracker {
    pub(crate) fn new(threshold: u32) -> Self {
        Self {
            consecutive_failures: HashMap::new(),
            threshold,
        }
    }

    /// Record a file.write failure for the given path.
    /// Returns true if the failure count has reached the threshold.
    pub(crate) fn record_failure(&mut self, path: &str) -> bool {
        let count = self
            .consecutive_failures
            .entry(path.to_string())
            .or_insert(0);
        *count += 1;
        *count >= self.threshold
    }

    /// Record a successful file.write, resetting the failure count for that path.
    pub(crate) fn record_success(&mut self, path: &str) {
        self.consecutive_failures.remove(path);
    }

    /// Get the current failure count for a path.
    pub(crate) fn failure_count(&self, path: &str) -> u32 {
        self.consecutive_failures.get(path).copied().unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tracker_basic_flow() {
        let mut tracker = WriteFailTracker::new(2);

        assert!(!tracker.record_failure("foo.rs"));
        assert!(tracker.record_failure("foo.rs")); // 2nd failure hits threshold
        assert_eq!(tracker.failure_count("foo.rs"), 2);
    }

    #[test]
    fn test_tracker_success_resets() {
        let mut tracker = WriteFailTracker::new(2);

        tracker.record_failure("foo.rs");
        tracker.record_success("foo.rs");
        assert_eq!(tracker.failure_count("foo.rs"), 0);

        // After reset, needs 2 more failures
        assert!(!tracker.record_failure("foo.rs"));
    }

    #[test]
    fn test_tracker_independent_paths() {
        let mut tracker = WriteFailTracker::new(2);

        tracker.record_failure("foo.rs");
        assert!(!tracker.record_failure("bar.rs")); // bar.rs: 1st
        assert!(tracker.record_failure("foo.rs")); // foo.rs: 2nd = threshold
        assert!(tracker.record_failure("bar.rs")); // bar.rs: 2nd = threshold
    }

    #[test]
    fn test_tracker_threshold_boundary() {
        let mut tracker = WriteFailTracker::new(2);
        assert!(!tracker.record_failure("a.rs")); // 1st, below threshold
        assert!(tracker.record_failure("a.rs")); // 2nd = threshold
        assert!(tracker.record_failure("a.rs")); // 3rd, still above threshold
    }
}
