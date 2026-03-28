use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WriteRepeatAction {
    Continue,
    Warn,
    StrongWarn,
}

pub(crate) struct WriteRepeatTracker {
    consecutive_writes: HashMap<String, u32>,
    warn_threshold: u32,
    strong_warn_threshold: u32,
}

impl WriteRepeatTracker {
    pub(crate) fn new(warn_threshold: u32, strong_warn_threshold: u32) -> Self {
        Self {
            consecutive_writes: HashMap::new(),
            warn_threshold,
            strong_warn_threshold,
        }
    }

    /// Record a successful file.write for the given path.
    /// Returns the recommended action based on the cumulative write count.
    pub(crate) fn record_write(&mut self, path: &str) -> WriteRepeatAction {
        let count = self.consecutive_writes.entry(path.to_string()).or_insert(0);
        *count += 1;
        if *count >= self.strong_warn_threshold {
            WriteRepeatAction::StrongWarn
        } else if *count >= self.warn_threshold {
            WriteRepeatAction::Warn
        } else {
            WriteRepeatAction::Continue
        }
    }

    /// Reset the write count for a path (called on file.edit/file.read success).
    pub(crate) fn reset_for_path(&mut self, path: &str) {
        self.consecutive_writes.remove(path);
    }

    /// Get the current write count for a path.
    pub(crate) fn write_count(&self, path: &str) -> u32 {
        self.consecutive_writes.get(path).copied().unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_flow() {
        let mut tracker = WriteRepeatTracker::new(3, 4);

        assert_eq!(tracker.record_write("foo.rs"), WriteRepeatAction::Continue); // 1
        assert_eq!(tracker.record_write("foo.rs"), WriteRepeatAction::Continue); // 2
        assert_eq!(tracker.record_write("foo.rs"), WriteRepeatAction::Warn); // 3
        assert_eq!(
            tracker.record_write("foo.rs"),
            WriteRepeatAction::StrongWarn
        ); // 4
    }

    #[test]
    fn test_threshold_boundary() {
        let mut tracker = WriteRepeatTracker::new(3, 4);

        // Below warn threshold
        assert_eq!(tracker.record_write("a.rs"), WriteRepeatAction::Continue); // 1
        assert_eq!(tracker.record_write("a.rs"), WriteRepeatAction::Continue); // 2

        // Exactly at warn threshold
        assert_eq!(tracker.record_write("a.rs"), WriteRepeatAction::Warn); // 3

        // Exactly at strong warn threshold
        assert_eq!(tracker.record_write("a.rs"), WriteRepeatAction::StrongWarn); // 4
    }

    #[test]
    fn test_reset_on_path() {
        let mut tracker = WriteRepeatTracker::new(3, 4);

        tracker.record_write("foo.rs");
        tracker.record_write("foo.rs");
        assert_eq!(tracker.write_count("foo.rs"), 2);

        tracker.reset_for_path("foo.rs");
        assert_eq!(tracker.write_count("foo.rs"), 0);

        // After reset, needs full count again
        assert_eq!(tracker.record_write("foo.rs"), WriteRepeatAction::Continue); // 1
    }

    #[test]
    fn test_independent_paths() {
        let mut tracker = WriteRepeatTracker::new(3, 4);

        tracker.record_write("foo.rs");
        tracker.record_write("foo.rs");
        tracker.record_write("bar.rs");

        assert_eq!(tracker.write_count("foo.rs"), 2);
        assert_eq!(tracker.write_count("bar.rs"), 1);

        // Warn triggers independently
        assert_eq!(tracker.record_write("foo.rs"), WriteRepeatAction::Warn); // foo: 3
        assert_eq!(tracker.record_write("bar.rs"), WriteRepeatAction::Continue); // bar: 2
    }

    #[test]
    fn test_strong_warn_persists() {
        let mut tracker = WriteRepeatTracker::new(3, 4);

        for _ in 0..4 {
            tracker.record_write("foo.rs");
        }
        assert_eq!(
            tracker.record_write("foo.rs"),
            WriteRepeatAction::StrongWarn
        ); // 5th
        assert_eq!(
            tracker.record_write("foo.rs"),
            WriteRepeatAction::StrongWarn
        ); // 6th
        assert_eq!(
            tracker.record_write("foo.rs"),
            WriteRepeatAction::StrongWarn
        ); // 7th
    }
}
