//! Parallel tool progress tracking types.
//!
//! Provides [`ToolProgressStatus`] and [`ToolProgressEntry`] for tracking
//! individual tool execution progress during parallel execution.
//! These types are independent of the existing [`super::ToolExecutionStatus`]
//! to avoid breaking backward compatibility.

use std::time::Instant;

use super::ToolExecutionStatus;

/// Parallel progress tracking status enum.
///
/// Independent from [`ToolExecutionStatus`] — used only during parallel
/// execution for real-time progress display.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolProgressStatus {
    /// Entry created but execution has not started.
    Pending,
    /// Execution in progress.
    Running,
    /// Execution completed successfully.
    Completed,
    /// Execution failed with a reason.
    Failed(String),
}

/// Individual tool progress tracking entry for parallel execution.
#[derive(Debug, Clone)]
pub struct ToolProgressEntry {
    pub tool_call_id: String,
    pub tool_name: String,
    pub status: ToolProgressStatus,
    pub started_at: Option<Instant>,
    /// Elapsed time in milliseconds (same type as `ToolExecutionResult.elapsed_ms`).
    pub elapsed_ms: Option<u128>,
}

impl ToolProgressEntry {
    /// Convert `elapsed_ms` to `u64` for `ToolLogView` compatibility.
    ///
    /// Returns `None` if `elapsed_ms` is `None`. Saturates at `u64::MAX`.
    pub fn elapsed_ms_u64(&self) -> Option<u64> {
        self.elapsed_ms.map(|ms| ms.min(u64::MAX as u128) as u64)
    }
}

impl From<&ToolProgressStatus> for ToolExecutionStatus {
    fn from(status: &ToolProgressStatus) -> Self {
        match status {
            ToolProgressStatus::Completed => ToolExecutionStatus::Completed,
            ToolProgressStatus::Failed(_) => ToolExecutionStatus::Failed,
            // Pending/Running are abnormal terminal states — map to Interrupted
            ToolProgressStatus::Pending | ToolProgressStatus::Running => {
                ToolExecutionStatus::Interrupted
            }
        }
    }
}
