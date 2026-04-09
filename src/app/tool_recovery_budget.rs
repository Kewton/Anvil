use std::collections::HashMap;

/// Manages per-path recovery read budgets for file.edit failure recovery.
///
/// After a file.edit (or file.edit_anchor) failure, the LLM typically needs
/// to re-read the file to get the current content before retrying.  These
/// recovery reads should not be counted by generic exploration detectors
/// (loop detector, phase estimator, etc.) because they are a legitimate
/// part of the edit retry workflow.
///
/// `ToolRecoveryBudget` grants a small budget of suppressed reads per path
/// on edit failure and consumes it on subsequent file.read calls.
pub(crate) struct ToolRecoveryBudget {
    /// Canonical/normalized per-path recovery read remaining counts.
    budgets: HashMap<String, u32>,
    /// Maximum recovery reads permitted per grant (configurable).
    max_budget: u32,
}

impl ToolRecoveryBudget {
    /// Create a new budget manager with the given per-path maximum.
    pub(crate) fn new(max_budget: u32) -> Self {
        Self {
            budgets: HashMap::new(),
            max_budget,
        }
    }

    /// Check whether this tool call is a recovery read that should suppress
    /// detector signals.  Returns `true` (and decrements the budget) when
    /// `tool_name` is `"file.read"` and `path` has remaining budget.
    pub(crate) fn should_suppress_detectors(&mut self, tool_name: &str, path: &str) -> bool {
        if tool_name != "file.read" {
            return false;
        }
        let normalized = normalize_path(path);
        match self.budgets.get_mut(&normalized) {
            Some(remaining) if *remaining > 0 => {
                *remaining -= 1;
                true
            }
            _ => false,
        }
    }

    /// Grant recovery budget for the given path.
    pub(crate) fn grant(&mut self, path: &str) {
        let normalized = normalize_path(path);
        self.budgets.insert(normalized, self.max_budget);
    }

    /// Clear (remove) budget for the given path (e.g. on edit success).
    pub(crate) fn clear(&mut self, path: &str) {
        let normalized = normalize_path(path);
        self.budgets.remove(&normalized);
    }

    /// Check whether the path currently has any remaining budget.
    pub(crate) fn has_budget(&self, path: &str) -> bool {
        let normalized = normalize_path(path);
        self.budgets.get(&normalized).is_some_and(|&v| v > 0)
    }
}

/// Normalize a file path for consistent budget key usage.
///
/// Handles common variations so that `./foo.rs` and `foo.rs` map to the
/// same key.  Also normalizes backslashes to forward slashes.
fn normalize_path(raw: &str) -> String {
    let s = raw.replace('\\', "/");
    let s = s.trim_start_matches("./");
    s.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_grant_and_suppress() {
        let mut budget = ToolRecoveryBudget::new(3);
        budget.grant("foo.rs");
        assert!(budget.has_budget("foo.rs"));
        assert!(budget.should_suppress_detectors("file.read", "foo.rs"));
        assert!(budget.has_budget("foo.rs")); // still 2 remaining
    }

    #[test]
    fn test_consume_decrements() {
        let mut budget = ToolRecoveryBudget::new(2);
        budget.grant("foo.rs");

        assert!(budget.should_suppress_detectors("file.read", "foo.rs"));
        assert!(budget.should_suppress_detectors("file.read", "foo.rs"));
        // Budget exhausted
        assert!(!budget.should_suppress_detectors("file.read", "foo.rs"));
        assert!(!budget.has_budget("foo.rs"));
    }

    #[test]
    fn test_clear_removes() {
        let mut budget = ToolRecoveryBudget::new(3);
        budget.grant("foo.rs");
        assert!(budget.has_budget("foo.rs"));

        budget.clear("foo.rs");
        assert!(!budget.has_budget("foo.rs"));
        assert!(!budget.should_suppress_detectors("file.read", "foo.rs"));
    }

    #[test]
    fn test_path_normalization() {
        let mut budget = ToolRecoveryBudget::new(3);

        // Grant with "./" prefix, check without
        budget.grant("./src/main.rs");
        assert!(budget.has_budget("src/main.rs"));
        assert!(budget.should_suppress_detectors("file.read", "src/main.rs"));

        // Grant without prefix, check with "./"
        budget.grant("lib.rs");
        assert!(budget.has_budget("./lib.rs"));

        // Backslash normalization
        budget.grant("src\\app\\mod.rs");
        assert!(budget.has_budget("src/app/mod.rs"));
    }

    #[test]
    fn test_non_file_read_returns_false() {
        let mut budget = ToolRecoveryBudget::new(3);
        budget.grant("foo.rs");

        assert!(!budget.should_suppress_detectors("file.write", "foo.rs"));
        assert!(!budget.should_suppress_detectors("file.edit", "foo.rs"));
        assert!(!budget.should_suppress_detectors("shell.exec", "foo.rs"));
        // Budget should still be intact
        assert!(budget.has_budget("foo.rs"));
    }

    #[test]
    fn test_no_budget_returns_false() {
        let mut budget = ToolRecoveryBudget::new(3);
        assert!(!budget.should_suppress_detectors("file.read", "foo.rs"));
    }

    #[test]
    fn test_independent_paths() {
        let mut budget = ToolRecoveryBudget::new(1);
        budget.grant("a.rs");
        budget.grant("b.rs");

        assert!(budget.should_suppress_detectors("file.read", "a.rs"));
        // a.rs exhausted
        assert!(!budget.should_suppress_detectors("file.read", "a.rs"));
        // b.rs still has budget
        assert!(budget.should_suppress_detectors("file.read", "b.rs"));
    }

    #[test]
    fn test_regrant_resets_budget() {
        let mut budget = ToolRecoveryBudget::new(2);
        budget.grant("foo.rs");
        budget.should_suppress_detectors("file.read", "foo.rs"); // consume 1

        // Re-grant resets to max
        budget.grant("foo.rs");
        assert!(budget.should_suppress_detectors("file.read", "foo.rs"));
        assert!(budget.should_suppress_detectors("file.read", "foo.rs"));
        assert!(!budget.should_suppress_detectors("file.read", "foo.rs"));
    }
}
