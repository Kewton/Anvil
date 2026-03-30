use std::collections::HashMap;

/// Action recommended when a file has been read repeatedly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReadRepeatAction {
    /// Read count below threshold — no action needed.
    Continue,
    /// Read count reached warn threshold — suggest using cached context.
    Warn(u32),
    /// Read count reached strong-warn threshold — strongly urge to stop re-reading.
    StrongWarn(u32),
}

/// Tracks per-path file.read counts across turns within an agentic session.
///
/// Unlike LoopDetector (which resets every turn), this tracker persists across
/// the entire agentic loop to detect cross-turn file.read repetition (Issue #185).
pub(crate) struct ReadRepeatTracker {
    counts: HashMap<String, u32>,
    warn_threshold: u32,
    strong_warn_threshold: u32,
}

/// Pure policy function: determine action based on count and thresholds.
/// Separated from ReadRepeatTracker to maintain SRP (counter vs policy).
pub(crate) fn determine_read_repeat_action(
    count: u32,
    warn_threshold: u32,
    strong_warn_threshold: u32,
) -> ReadRepeatAction {
    if count >= strong_warn_threshold {
        ReadRepeatAction::StrongWarn(count)
    } else if count >= warn_threshold {
        ReadRepeatAction::Warn(count)
    } else {
        ReadRepeatAction::Continue
    }
}

impl ReadRepeatTracker {
    pub(crate) fn new(warn_threshold: u32, strong_warn_threshold: u32) -> Self {
        Self {
            counts: HashMap::new(),
            warn_threshold,
            strong_warn_threshold,
        }
    }

    /// Record a successful file.read and return the recommended action.
    pub(crate) fn record_read(&mut self, path: &str) -> ReadRepeatAction {
        let count = self.counts.entry(path.to_string()).or_insert(0);
        *count += 1;
        determine_read_repeat_action(*count, self.warn_threshold, self.strong_warn_threshold)
    }

    /// Reset the read count for a path (called after file.write/file.edit success).
    pub(crate) fn reset(&mut self, path: &str) {
        self.counts.remove(path);
    }

    /// Get current read count for a path.
    #[cfg(test)]
    pub(crate) fn read_count(&self, path: &str) -> u32 {
        self.counts.get(path).copied().unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_flow() {
        let mut tracker = ReadRepeatTracker::new(2, 4);
        assert_eq!(tracker.record_read("foo.rs"), ReadRepeatAction::Continue); // 1st
        assert!(matches!(
            tracker.record_read("foo.rs"),
            ReadRepeatAction::Warn(2)
        )); // 2nd
        assert!(matches!(
            tracker.record_read("foo.rs"),
            ReadRepeatAction::Warn(3)
        )); // 3rd
        assert!(matches!(
            tracker.record_read("foo.rs"),
            ReadRepeatAction::StrongWarn(4)
        )); // 4th
    }

    #[test]
    fn test_reset_clears_count() {
        let mut tracker = ReadRepeatTracker::new(2, 4);
        tracker.record_read("foo.rs");
        tracker.record_read("foo.rs"); // Warn
        tracker.reset("foo.rs");
        assert_eq!(tracker.record_read("foo.rs"), ReadRepeatAction::Continue); // reset: 1st
    }

    #[test]
    fn test_independent_paths() {
        let mut tracker = ReadRepeatTracker::new(2, 4);
        tracker.record_read("foo.rs");
        tracker.record_read("foo.rs"); // foo.rs: Warn
        assert_eq!(tracker.record_read("bar.rs"), ReadRepeatAction::Continue); // bar.rs: 1st
    }

    #[test]
    fn test_custom_thresholds() {
        let mut tracker = ReadRepeatTracker::new(3, 5);
        assert_eq!(tracker.record_read("a.rs"), ReadRepeatAction::Continue); // 1st
        assert_eq!(tracker.record_read("a.rs"), ReadRepeatAction::Continue); // 2nd
        assert!(matches!(
            tracker.record_read("a.rs"),
            ReadRepeatAction::Warn(3)
        )); // 3rd
        assert!(matches!(
            tracker.record_read("a.rs"),
            ReadRepeatAction::Warn(4)
        )); // 4th
        assert!(matches!(
            tracker.record_read("a.rs"),
            ReadRepeatAction::StrongWarn(5)
        )); // 5th
    }

    #[test]
    fn test_read_count() {
        let mut tracker = ReadRepeatTracker::new(2, 4);
        assert_eq!(tracker.read_count("foo.rs"), 0);
        tracker.record_read("foo.rs");
        assert_eq!(tracker.read_count("foo.rs"), 1);
        tracker.record_read("foo.rs");
        assert_eq!(tracker.read_count("foo.rs"), 2);
    }

    #[test]
    fn test_determine_read_repeat_action() {
        // Boundary value tests for pure function
        assert_eq!(
            determine_read_repeat_action(0, 2, 4),
            ReadRepeatAction::Continue
        );
        assert_eq!(
            determine_read_repeat_action(1, 2, 4),
            ReadRepeatAction::Continue
        );
        assert_eq!(
            determine_read_repeat_action(2, 2, 4),
            ReadRepeatAction::Warn(2)
        );
        assert_eq!(
            determine_read_repeat_action(3, 2, 4),
            ReadRepeatAction::Warn(3)
        );
        assert_eq!(
            determine_read_repeat_action(4, 2, 4),
            ReadRepeatAction::StrongWarn(4)
        );
        assert_eq!(
            determine_read_repeat_action(5, 2, 4),
            ReadRepeatAction::StrongWarn(5)
        );
    }

    #[test]
    fn test_strong_warn_persists() {
        let mut tracker = ReadRepeatTracker::new(2, 4);
        for _ in 0..4 {
            tracker.record_read("foo.rs");
        }
        // Beyond threshold, still returns StrongWarn
        assert!(matches!(
            tracker.record_read("foo.rs"),
            ReadRepeatAction::StrongWarn(5)
        ));
        assert!(matches!(
            tracker.record_read("foo.rs"),
            ReadRepeatAction::StrongWarn(6)
        ));
    }

    #[test]
    fn test_edit_anchor_reset() {
        // Simulates that file.edit_anchor success also resets the tracker
        let mut tracker = ReadRepeatTracker::new(2, 4);
        tracker.record_read("foo.rs");
        tracker.record_read("foo.rs"); // Warn
        tracker.record_read("foo.rs"); // Warn
        // Simulate edit_anchor success -> reset
        tracker.reset("foo.rs");
        assert_eq!(tracker.read_count("foo.rs"), 0);
        assert_eq!(tracker.record_read("foo.rs"), ReadRepeatAction::Continue);
    }
}
