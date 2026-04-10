use std::collections::HashMap;
use std::fmt;
use std::str::FromStr;

/// Edit/write fallback strategy (Issue #158).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EditStrategy {
    /// Prefer file.edit, fallback to file.write after consecutive failures.
    #[default]
    EditFirst,
    /// Prefer file.write from the start (advisory via system prompt).
    WriteFirst,
}

impl fmt::Display for EditStrategy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EditFirst => write!(f, "edit-first"),
            Self::WriteFirst => write!(f, "write-first"),
        }
    }
}

impl FromStr for EditStrategy {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "edit-first" | "edit_first" => Ok(Self::EditFirst),
            "write-first" | "write_first" => Ok(Self::WriteFirst),
            other => Err(format!("invalid edit strategy: {other}")),
        }
    }
}

/// Recommended action after a file.edit failure (Issue #158, #321).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditFallbackAction {
    /// Normal continuation (failure count < reread_threshold).
    Continue,
    /// Recommend re-reading the file before retrying edit.
    ReRead,
    /// Recommend switching to file.write.
    WriteFallback,
    /// Escalate to agent.fix_slice for bounded single-file repair (Issue #321).
    FixSliceEscalation,
}

/// Determine the fallback action based on failure count and thresholds.
///
/// Separated from `EditFailTracker` to maintain SRP (counter vs policy).
pub fn determine_fallback_action(
    count: u32,
    reread_threshold: u32,
    write_fallback_threshold: u32,
    fixslice_threshold: u32,
) -> EditFallbackAction {
    if fixslice_threshold > 0 && count >= fixslice_threshold {
        EditFallbackAction::FixSliceEscalation
    } else if count >= write_fallback_threshold {
        EditFallbackAction::WriteFallback
    } else if count >= reread_threshold {
        EditFallbackAction::ReRead
    } else {
        EditFallbackAction::Continue
    }
}

/// Decide whether to escalate to the worker path based on cumulative failures
/// and stagnation score (Issue #332).
///
/// This is the broadened escalation policy: fires when the model spreads
/// edit failures across multiple paths so no single path reaches
/// `edit_fixslice_threshold`, but the session is clearly stagnating.
///
/// `failures_threshold == 0` disables the trigger.
pub fn should_escalate_for_stagnation(
    total_failures: u32,
    stagnation_score: u32,
    failures_threshold: u32,
    score_threshold: u32,
) -> bool {
    failures_threshold > 0
        && total_failures >= failures_threshold
        && stagnation_score >= score_threshold
}

/// Tracks consecutive file.edit failures per file path.
/// Used to detect when the LLM is stuck retrying edits on the same file
/// and should be prompted to try an alternative approach (Issue #158, #321).
pub struct EditFailTracker {
    consecutive_failures: HashMap<String, u32>,
    reread_threshold: u32,
    write_fallback_threshold: u32,
    /// Threshold for escalating to agent.fix_slice (Issue #321).
    /// 0 = disabled.
    fixslice_threshold: u32,
    /// Cumulative failure count across all paths (Issue #332).
    ///
    /// Not reset on `record_success`, because the broadened stagnation
    /// escalation uses it to detect the drift pattern where the model
    /// scatters failures across multiple files and no single path accumulates
    /// enough to hit `fixslice_threshold`.
    total_failures: u32,
}

impl EditFailTracker {
    pub fn new(
        reread_threshold: u32,
        write_fallback_threshold: u32,
        fixslice_threshold: u32,
    ) -> Self {
        Self {
            consecutive_failures: HashMap::new(),
            reread_threshold,
            write_fallback_threshold,
            fixslice_threshold,
            total_failures: 0,
        }
    }

    /// Record a file.edit failure for the given path.
    /// Returns the recommended fallback action based on the cumulative failure count.
    pub fn record_failure(&mut self, path: &str) -> EditFallbackAction {
        let count = self
            .consecutive_failures
            .entry(path.to_string())
            .or_insert(0);
        *count += 1;
        self.total_failures = self.total_failures.saturating_add(1);
        determine_fallback_action(
            *count,
            self.reread_threshold,
            self.write_fallback_threshold,
            self.fixslice_threshold,
        )
    }

    /// Record a successful file.edit, resetting the failure count for that path.
    pub fn record_success(&mut self, path: &str) {
        self.consecutive_failures.remove(path);
    }

    /// Get the current failure count for a path.
    pub fn failure_count(&self, path: &str) -> u32 {
        self.consecutive_failures.get(path).copied().unwrap_or(0)
    }

    /// Cumulative failure count across all paths in this session (Issue #332).
    pub fn total_failures(&self) -> u32 {
        self.total_failures
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- EditStrategy tests ---

    #[test]
    fn test_edit_strategy_display() {
        assert_eq!(EditStrategy::EditFirst.to_string(), "edit-first");
        assert_eq!(EditStrategy::WriteFirst.to_string(), "write-first");
    }

    #[test]
    fn test_edit_strategy_from_str() {
        assert_eq!(
            "edit-first".parse::<EditStrategy>().unwrap(),
            EditStrategy::EditFirst
        );
        assert_eq!(
            "edit_first".parse::<EditStrategy>().unwrap(),
            EditStrategy::EditFirst
        );
        assert_eq!(
            "write-first".parse::<EditStrategy>().unwrap(),
            EditStrategy::WriteFirst
        );
        assert_eq!(
            "write_first".parse::<EditStrategy>().unwrap(),
            EditStrategy::WriteFirst
        );
        assert!("invalid".parse::<EditStrategy>().is_err());
    }

    #[test]
    fn test_edit_strategy_default() {
        assert_eq!(EditStrategy::default(), EditStrategy::EditFirst);
    }

    // --- determine_fallback_action tests ---

    #[test]
    fn test_determine_fallback_action() {
        // N=3, M=5, fixslice=0 (disabled)
        assert_eq!(
            determine_fallback_action(0, 3, 5, 0),
            EditFallbackAction::Continue
        );
        assert_eq!(
            determine_fallback_action(1, 3, 5, 0),
            EditFallbackAction::Continue
        );
        assert_eq!(
            determine_fallback_action(2, 3, 5, 0),
            EditFallbackAction::Continue
        );
        assert_eq!(
            determine_fallback_action(3, 3, 5, 0),
            EditFallbackAction::ReRead
        );
        assert_eq!(
            determine_fallback_action(4, 3, 5, 0),
            EditFallbackAction::ReRead
        );
        assert_eq!(
            determine_fallback_action(5, 3, 5, 0),
            EditFallbackAction::WriteFallback
        );
        assert_eq!(
            determine_fallback_action(6, 3, 5, 0),
            EditFallbackAction::WriteFallback
        );
        assert_eq!(
            determine_fallback_action(100, 3, 5, 0),
            EditFallbackAction::WriteFallback
        );
    }

    #[test]
    fn test_determine_fallback_action_fixslice_escalation() {
        // N=3, M=5, fixslice=7
        assert_eq!(
            determine_fallback_action(5, 3, 5, 7),
            EditFallbackAction::WriteFallback
        );
        assert_eq!(
            determine_fallback_action(6, 3, 5, 7),
            EditFallbackAction::WriteFallback
        );
        assert_eq!(
            determine_fallback_action(7, 3, 5, 7),
            EditFallbackAction::FixSliceEscalation
        );
        assert_eq!(
            determine_fallback_action(10, 3, 5, 7),
            EditFallbackAction::FixSliceEscalation
        );
    }

    // --- EditFailTracker tests ---

    #[test]
    fn test_tracker_basic_flow() {
        let mut tracker = EditFailTracker::new(3, 5, 0);

        // 1st and 2nd failure: Continue
        assert_eq!(
            tracker.record_failure("foo.rs"),
            EditFallbackAction::Continue
        );
        assert_eq!(
            tracker.record_failure("foo.rs"),
            EditFallbackAction::Continue
        );
        // 3rd failure: ReRead
        assert_eq!(tracker.record_failure("foo.rs"), EditFallbackAction::ReRead);
        assert_eq!(tracker.failure_count("foo.rs"), 3);
        // 4th failure: ReRead
        assert_eq!(tracker.record_failure("foo.rs"), EditFallbackAction::ReRead);
        // 5th failure: WriteFallback
        assert_eq!(
            tracker.record_failure("foo.rs"),
            EditFallbackAction::WriteFallback
        );
        assert_eq!(tracker.failure_count("foo.rs"), 5);
    }

    #[test]
    fn test_tracker_success_resets() {
        let mut tracker = EditFailTracker::new(3, 5, 0);

        tracker.record_failure("foo.rs");
        tracker.record_failure("foo.rs");
        tracker.record_success("foo.rs");
        assert_eq!(tracker.failure_count("foo.rs"), 0);

        // After reset, starts from Continue again
        assert_eq!(
            tracker.record_failure("foo.rs"),
            EditFallbackAction::Continue
        );
    }

    #[test]
    fn test_tracker_independent_paths() {
        let mut tracker = EditFailTracker::new(3, 5, 0);

        tracker.record_failure("foo.rs");
        assert_eq!(
            tracker.record_failure("bar.rs"),
            EditFallbackAction::Continue
        ); // bar: 1st
        assert_eq!(
            tracker.record_failure("foo.rs"),
            EditFallbackAction::Continue
        ); // foo: 2nd
        assert_eq!(
            tracker.record_failure("bar.rs"),
            EditFallbackAction::Continue
        ); // bar: 2nd
        assert_eq!(tracker.record_failure("foo.rs"), EditFallbackAction::ReRead); // foo: 3rd
        assert_eq!(tracker.record_failure("bar.rs"), EditFallbackAction::ReRead); // bar: 3rd
    }

    #[test]
    fn test_tracker_custom_thresholds() {
        let mut tracker = EditFailTracker::new(2, 4, 0);
        assert_eq!(tracker.record_failure("a.rs"), EditFallbackAction::Continue); // 1st
        assert_eq!(tracker.record_failure("a.rs"), EditFallbackAction::ReRead); // 2nd = reread
        assert_eq!(tracker.record_failure("a.rs"), EditFallbackAction::ReRead); // 3rd
        assert_eq!(
            tracker.record_failure("a.rs"),
            EditFallbackAction::WriteFallback
        ); // 4th = write
    }

    #[test]
    fn test_tracker_write_fallback_persists() {
        let mut tracker = EditFailTracker::new(3, 5, 0);
        for _ in 0..5 {
            tracker.record_failure("a.rs");
        }
        // Beyond threshold, still WriteFallback (fixslice disabled)
        assert_eq!(
            tracker.record_failure("a.rs"),
            EditFallbackAction::WriteFallback
        );
        assert_eq!(
            tracker.record_failure("a.rs"),
            EditFallbackAction::WriteFallback
        );
        assert_eq!(tracker.failure_count("a.rs"), 7);
    }

    // --- FixSlice escalation tests (Issue #321) ---

    #[test]
    fn test_tracker_fixslice_escalation() {
        // reread=3, write_fallback=5, fixslice=7
        let mut tracker = EditFailTracker::new(3, 5, 7);

        // 1-2: Continue
        assert_eq!(tracker.record_failure("a.rs"), EditFallbackAction::Continue);
        assert_eq!(tracker.record_failure("a.rs"), EditFallbackAction::Continue);
        // 3-4: ReRead
        assert_eq!(tracker.record_failure("a.rs"), EditFallbackAction::ReRead);
        assert_eq!(tracker.record_failure("a.rs"), EditFallbackAction::ReRead);
        // 5-6: WriteFallback
        assert_eq!(
            tracker.record_failure("a.rs"),
            EditFallbackAction::WriteFallback
        );
        assert_eq!(
            tracker.record_failure("a.rs"),
            EditFallbackAction::WriteFallback
        );
        // 7+: FixSliceEscalation
        assert_eq!(
            tracker.record_failure("a.rs"),
            EditFallbackAction::FixSliceEscalation
        );
        assert_eq!(tracker.failure_count("a.rs"), 7);
        // Persists beyond threshold
        assert_eq!(
            tracker.record_failure("a.rs"),
            EditFallbackAction::FixSliceEscalation
        );
    }

    #[test]
    fn test_tracker_fixslice_escalation_resets_on_success() {
        let mut tracker = EditFailTracker::new(3, 5, 7);
        for _ in 0..7 {
            tracker.record_failure("a.rs");
        }
        assert_eq!(
            tracker.record_failure("a.rs"),
            EditFallbackAction::FixSliceEscalation
        );

        tracker.record_success("a.rs");
        assert_eq!(tracker.failure_count("a.rs"), 0);
        assert_eq!(tracker.record_failure("a.rs"), EditFallbackAction::Continue);
    }

    // --- Issue #332: cumulative total_failures & stagnation helper ---

    #[test]
    fn test_total_failures_accumulates_across_paths_and_survives_success() {
        let mut tracker = EditFailTracker::new(3, 5, 7);
        assert_eq!(tracker.total_failures(), 0);
        tracker.record_failure("a.rs");
        tracker.record_failure("b.rs");
        tracker.record_failure("c.rs");
        assert_eq!(tracker.total_failures(), 3);
        tracker.record_success("a.rs");
        assert_eq!(
            tracker.total_failures(),
            3,
            "total_failures must not be reset by per-path success"
        );
        tracker.record_failure("a.rs");
        assert_eq!(tracker.total_failures(), 4);
    }

    #[test]
    fn test_should_escalate_for_stagnation_boundary() {
        // disabled
        assert!(!should_escalate_for_stagnation(100, 4, 0, 2));
        // below failures
        assert!(!should_escalate_for_stagnation(3, 4, 4, 2));
        // below score
        assert!(!should_escalate_for_stagnation(4, 1, 4, 2));
        // exactly at thresholds
        assert!(should_escalate_for_stagnation(4, 2, 4, 2));
        // well above
        assert!(should_escalate_for_stagnation(10, 4, 4, 2));
    }
}
