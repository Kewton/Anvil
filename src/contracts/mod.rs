/// Shared contract types used across modules.
///
/// These types form the schema for snapshots, console rendering, and
/// persistent session state.  They are intentionally plain data with
/// `Serialize`/`Deserialize` support.

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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextUsageView {
    pub estimated_tokens: usize,
    pub max_tokens: u32,
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
    pub state: RuntimeState,
    #[serde(default)]
    pub last_event: Option<AppEvent>,
    #[serde(default)]
    pub status: StatusView,
    #[serde(default)]
    pub plan: Option<PlanView>,
    #[serde(default)]
    pub reasoning_summary: Vec<String>,
    #[serde(default)]
    pub approval: Option<ApprovalView>,
    #[serde(default)]
    pub interrupt: Option<InterruptView>,
    #[serde(default)]
    pub tool_logs: Vec<ToolLogView>,
    #[serde(default)]
    pub elapsed_ms: Option<u128>,
    #[serde(default)]
    pub context_usage: Option<ContextUsageView>,
    #[serde(default)]
    pub completion_summary: Option<String>,
    #[serde(default)]
    pub saved_status: Option<String>,
    #[serde(default)]
    pub error_summary: Option<String>,
    #[serde(default)]
    pub recommended_actions: Vec<String>,
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
        });
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
}
