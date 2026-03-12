#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeState {
    Ready,
    Thinking,
    Working,
    AwaitingApproval,
    Interrupted,
    Done,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppEvent {
    ConfigLoaded,
    ProviderBootstrapped,
    StartupCompleted,
    StateChanged,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusView {
    pub line: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanView {
    pub items: Vec<String>,
    pub active_index: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalView {
    pub tool_name: String,
    pub summary: String,
    pub risk: String,
    pub tool_call_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterruptView {
    pub interrupted_what: String,
    pub saved_status: String,
    pub next_actions: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppStateSnapshot {
    pub state: RuntimeState,
    pub last_event: Option<AppEvent>,
    pub status: StatusView,
    pub plan: Option<PlanView>,
    pub reasoning_summary: Vec<String>,
    pub approval: Option<ApprovalView>,
    pub interrupt: Option<InterruptView>,
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
        self.plan = Some(PlanView { items, active_index });
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
}
