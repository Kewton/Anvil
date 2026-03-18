//! Shared contract types used across modules.
//!
//! These types form the schema for snapshots, console rendering, and
//! persistent session state.  They are intentionally plain data with
//! `Serialize`/`Deserialize` support.

pub mod tokens;

use serde::{Deserialize, Serialize};

/// The runtime lifecycle states of the application.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RuntimeState {
    Ready,
    Thinking,
    Working,
    AwaitingApproval,
    Interrupted,
    Done,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AppEvent {
    ConfigLoaded,
    ProviderBootstrapped,
    StartupCompleted,
    StateChanged,
    PlanItemAdded,
    PlanFocusChanged,
    PlanCleared,
    PlanCheckpointSaved,
    SessionCompacted,
    SessionLoaded,
    SessionSaved,
    SessionNormalizedAfterInterrupt,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusView {
    pub line: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanView {
    pub items: Vec<String>,
    pub active_index: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalView {
    pub tool_name: String,
    pub summary: String,
    pub risk: String,
    pub tool_call_id: String,
    #[serde(skip)]
    pub diff_preview: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InterruptView {
    pub interrupted_what: String,
    pub saved_status: String,
    pub next_actions: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolLogView {
    pub tool_name: String,
    pub action: String,
    pub target: String,
}

/// Context usage warning level based on threshold evaluation.
///
/// Used as `Option<ContextWarningLevel>` where `None` means no warning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ContextWarningLevel {
    /// Usage >= 80%: warning
    Warning,
    /// Usage >= 90%: critical
    Critical,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextUsageView {
    pub estimated_tokens: usize,
    pub max_tokens: u32,
}

impl ContextUsageView {
    /// Context usage ratio clamped to 0.0..=1.0. Returns 0.0 if max_tokens is 0.
    pub fn usage_ratio(&self) -> f64 {
        if self.max_tokens == 0 {
            return 0.0;
        }
        let ratio = self.estimated_tokens as f64 / self.max_tokens as f64;
        ratio.clamp(0.0, 1.0)
    }

    /// Warning level based on usage thresholds (>=0.9 Critical, >=0.8 Warning, else None).
    pub fn warning_level(&self) -> Option<ContextWarningLevel> {
        let ratio = self.usage_ratio();
        if ratio >= 0.9 {
            Some(ContextWarningLevel::Critical)
        } else if ratio >= 0.8 {
            Some(ContextWarningLevel::Warning)
        } else {
            None
        }
    }

    /// Usage percentage for display (0..=100).
    pub fn usage_percent(&self) -> u32 {
        (self.usage_ratio() * 100.0).round() as u32
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConsoleMessageRole {
    User,
    Assistant,
    Tool,
    System,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsoleMessageView {
    pub role: ConsoleMessageRole,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsoleRenderContext {
    pub snapshot: AppStateSnapshot,
    pub model_name: String,
    pub messages: Vec<ConsoleMessageView>,
    pub history_summary: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppStateSnapshot {
    /// Current lifecycle state. Set in every state.
    pub state: RuntimeState,
    /// Most recent event that caused this snapshot. Set in every state.
    #[serde(default)]
    pub last_event: Option<AppEvent>,
    /// Human-readable status line. Set in every state.
    #[serde(default)]
    pub status: StatusView,
    /// Active plan items. Used in: Thinking, Working, Done.
    #[serde(default)]
    pub plan: Option<PlanView>,
    /// LLM reasoning steps. Used in: Thinking.
    #[serde(default)]
    pub reasoning_summary: Vec<String>,
    /// Pending approval details. Used in: AwaitingApproval.
    #[serde(default)]
    pub approval: Option<ApprovalView>,
    /// Interrupt details. Used in: Interrupted.
    #[serde(default)]
    pub interrupt: Option<InterruptView>,
    /// Tool execution log entries. Used in: Working, Done.
    #[serde(default)]
    pub tool_logs: Vec<ToolLogView>,
    /// Wall-clock milliseconds for the current turn. Used in: Thinking, Working, Done, Interrupted, Error.
    #[serde(default)]
    pub elapsed_ms: Option<u128>,
    /// Token budget usage. Set in every state.
    #[serde(default)]
    pub context_usage: Option<ContextUsageView>,
    /// Summary of what was accomplished. Used in: Done.
    #[serde(default)]
    pub completion_summary: Option<String>,
    /// Session persistence status. Used in: Done, Interrupted.
    #[serde(default)]
    pub saved_status: Option<String>,
    /// Error description. Used in: Error.
    #[serde(default)]
    pub error_summary: Option<String>,
    /// Suggested recovery actions. Used in: Error, Interrupted.
    #[serde(default)]
    pub recommended_actions: Vec<String>,
    /// Context overflow warning level. Used in: Done.
    #[serde(default)]
    pub context_warning: Option<ContextWarningLevel>,
}

impl AppStateSnapshot {
    pub fn new(state: RuntimeState) -> Self {
        Self {
            state,
            last_event: None,
            status: StatusView {
                line: String::new(),
            },
            plan: None,
            reasoning_summary: Vec::new(),
            approval: None,
            interrupt: None,
            tool_logs: Vec::new(),
            elapsed_ms: None,
            context_usage: None,
            completion_summary: None,
            saved_status: None,
            error_summary: None,
            recommended_actions: Vec::new(),
            context_warning: None,
        }
    }

    pub fn with_event(mut self, event: AppEvent) -> Self {
        self.last_event = Some(event);
        self
    }

    pub fn with_status(mut self, status: String) -> Self {
        self.status = StatusView { line: status };
        self
    }

    pub fn with_plan(mut self, items: Vec<String>, active_index: Option<usize>) -> Self {
        self.plan = Some(PlanView {
            items,
            active_index,
        });
        self
    }

    pub fn with_reasoning_summary(mut self, reasoning_summary: Vec<String>) -> Self {
        self.reasoning_summary = reasoning_summary;
        self
    }

    pub fn with_approval(
        mut self,
        tool_name: String,
        summary: String,
        risk: String,
        tool_call_id: String,
    ) -> Self {
        self.approval = Some(ApprovalView {
            tool_name,
            summary,
            risk,
            tool_call_id,
            diff_preview: None,
        });
        self
    }

    pub fn with_diff_preview(mut self, preview: Option<String>) -> Self {
        if let Some(ref mut approval) = self.approval {
            approval.diff_preview = preview;
        }
        self
    }

    pub fn with_interrupt(
        mut self,
        interrupted_what: String,
        saved_status: String,
        next_actions: Vec<String>,
    ) -> Self {
        self.interrupt = Some(InterruptView {
            interrupted_what,
            saved_status,
            next_actions,
        });
        self
    }

    pub fn with_tool_logs(mut self, tool_logs: Vec<ToolLogView>) -> Self {
        self.tool_logs = tool_logs;
        self
    }

    pub fn with_elapsed_ms(mut self, elapsed_ms: u128) -> Self {
        self.elapsed_ms = Some(elapsed_ms);
        self
    }

    pub fn with_context_usage(mut self, estimated_tokens: usize, max_tokens: u32) -> Self {
        self.context_usage = Some(ContextUsageView {
            estimated_tokens,
            max_tokens,
        });
        self
    }

    pub fn with_completion_summary(
        mut self,
        completion_summary: impl Into<String>,
        saved_status: impl Into<String>,
    ) -> Self {
        self.completion_summary = Some(completion_summary.into());
        self.saved_status = Some(saved_status.into());
        self
    }

    pub fn with_error_summary(
        mut self,
        error_summary: impl Into<String>,
        recommended_actions: Vec<String>,
    ) -> Self {
        self.error_summary = Some(error_summary.into());
        self.recommended_actions = recommended_actions;
        self
    }

    pub fn with_context_warning(mut self, level: ContextWarningLevel) -> Self {
        self.context_warning = Some(level);
        self
    }
}

#[cfg(test)]
impl AppStateSnapshot {
    /// Assert that the snapshot has the expected fields populated for its state.
    pub fn assert_valid_for_state(&self) {
        match self.state {
            RuntimeState::AwaitingApproval => {
                assert!(
                    self.approval.is_some(),
                    "AwaitingApproval must have approval"
                );
            }
            RuntimeState::Done => {
                assert!(
                    self.completion_summary.is_some(),
                    "Done must have completion_summary"
                );
            }
            RuntimeState::Error => {
                assert!(
                    self.error_summary.is_some(),
                    "Error must have error_summary"
                );
            }
            RuntimeState::Interrupted => {
                assert!(self.interrupt.is_some(), "Interrupted must have interrupt");
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_ratio_normal() {
        let usage = ContextUsageView {
            estimated_tokens: 5000,
            max_tokens: 10000,
        };
        assert!((usage.usage_ratio() - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn usage_ratio_zero_max_tokens() {
        let usage = ContextUsageView {
            estimated_tokens: 100,
            max_tokens: 0,
        };
        assert!((usage.usage_ratio() - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn usage_ratio_clamped_above_one() {
        let usage = ContextUsageView {
            estimated_tokens: 15000,
            max_tokens: 10000,
        };
        assert!((usage.usage_ratio() - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn usage_ratio_boundary_values() {
        let usage_80 = ContextUsageView {
            estimated_tokens: 8000,
            max_tokens: 10000,
        };
        assert!((usage_80.usage_ratio() - 0.8).abs() < f64::EPSILON);

        let usage_90 = ContextUsageView {
            estimated_tokens: 9000,
            max_tokens: 10000,
        };
        assert!((usage_90.usage_ratio() - 0.9).abs() < f64::EPSILON);
    }

    #[test]
    fn warning_level_below_80_is_none() {
        let usage = ContextUsageView {
            estimated_tokens: 7999,
            max_tokens: 10000,
        };
        assert_eq!(usage.warning_level(), None);
    }

    #[test]
    fn warning_level_at_80_is_warning() {
        let usage = ContextUsageView {
            estimated_tokens: 8000,
            max_tokens: 10000,
        };
        assert_eq!(usage.warning_level(), Some(ContextWarningLevel::Warning));
    }

    #[test]
    fn warning_level_at_89_is_warning() {
        let usage = ContextUsageView {
            estimated_tokens: 8999,
            max_tokens: 10000,
        };
        assert_eq!(usage.warning_level(), Some(ContextWarningLevel::Warning));
    }

    #[test]
    fn warning_level_at_90_is_critical() {
        let usage = ContextUsageView {
            estimated_tokens: 9000,
            max_tokens: 10000,
        };
        assert_eq!(usage.warning_level(), Some(ContextWarningLevel::Critical));
    }

    #[test]
    fn warning_level_at_100_is_critical() {
        let usage = ContextUsageView {
            estimated_tokens: 10000,
            max_tokens: 10000,
        };
        assert_eq!(usage.warning_level(), Some(ContextWarningLevel::Critical));
    }

    #[test]
    fn usage_percent_normal() {
        let usage = ContextUsageView {
            estimated_tokens: 2200,
            max_tokens: 10000,
        };
        assert_eq!(usage.usage_percent(), 22);
    }

    #[test]
    fn usage_percent_rounding() {
        let usage = ContextUsageView {
            estimated_tokens: 3333,
            max_tokens: 10000,
        };
        assert_eq!(usage.usage_percent(), 33);
    }

    #[test]
    fn usage_percent_zero_max() {
        let usage = ContextUsageView {
            estimated_tokens: 100,
            max_tokens: 0,
        };
        assert_eq!(usage.usage_percent(), 0);
    }
}
