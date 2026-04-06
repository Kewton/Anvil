//! Progress tracking types for parallel tool execution display.
//!
//! These types are display-only and do not affect tool execution logic.

/// Status of an individual tool during parallel execution (display only).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolProgressStatus {
    /// Tool is currently running.
    Running,
    /// Tool completed successfully.
    Completed,
    /// Tool execution failed.
    Failed,
}

/// Progress entry for an individual tool during parallel execution (display only).
#[derive(Debug, Clone)]
pub struct ToolProgressEntry {
    /// Tool name (e.g. "file.read").
    pub tool_name: String,
    /// Current status.
    pub status: ToolProgressStatus,
    /// When execution started.
    pub started_at: std::time::Instant,
    /// Elapsed time in milliseconds. `None` while `Running`;
    /// set on `Completed` or `Failed`.
    ///
    /// Uses `u64` (sufficient for display; u64::MAX ms ≈ 584 million years).
    pub elapsed_ms: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn status_equality() {
        assert_eq!(ToolProgressStatus::Running, ToolProgressStatus::Running);
        assert_eq!(ToolProgressStatus::Completed, ToolProgressStatus::Completed);
        assert_eq!(ToolProgressStatus::Failed, ToolProgressStatus::Failed);
        assert_ne!(ToolProgressStatus::Running, ToolProgressStatus::Completed);
        assert_ne!(ToolProgressStatus::Running, ToolProgressStatus::Failed);
        assert_ne!(ToolProgressStatus::Completed, ToolProgressStatus::Failed);
    }

    #[test]
    fn status_is_copy() {
        let s = ToolProgressStatus::Running;
        let s2 = s; // Copy
        assert_eq!(s, s2);
    }

    #[test]
    fn entry_creation_running() {
        let entry = ToolProgressEntry {
            tool_name: "file.read".to_string(),
            status: ToolProgressStatus::Running,
            started_at: Instant::now(),
            elapsed_ms: None,
        };
        assert_eq!(entry.tool_name, "file.read");
        assert_eq!(entry.status, ToolProgressStatus::Running);
        assert!(entry.elapsed_ms.is_none());
    }

    #[test]
    fn entry_creation_completed() {
        let entry = ToolProgressEntry {
            tool_name: "git.status".to_string(),
            status: ToolProgressStatus::Completed,
            started_at: Instant::now(),
            elapsed_ms: Some(1234),
        };
        assert_eq!(entry.status, ToolProgressStatus::Completed);
        assert_eq!(entry.elapsed_ms, Some(1234));
    }

    #[test]
    fn entry_creation_failed() {
        let entry = ToolProgressEntry {
            tool_name: "shell.exec".to_string(),
            status: ToolProgressStatus::Failed,
            started_at: Instant::now(),
            elapsed_ms: Some(500),
        };
        assert_eq!(entry.status, ToolProgressStatus::Failed);
        assert_eq!(entry.elapsed_ms, Some(500));
    }

    #[test]
    fn entry_clone() {
        let entry = ToolProgressEntry {
            tool_name: "file.write".to_string(),
            status: ToolProgressStatus::Running,
            started_at: Instant::now(),
            elapsed_ms: None,
        };
        let cloned = entry.clone();
        assert_eq!(cloned.tool_name, entry.tool_name);
        assert_eq!(cloned.status, entry.status);
    }
}
