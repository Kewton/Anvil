use crate::provider::{
    ProviderClient, ProviderEvent, ProviderMessage, ProviderMessageRole, ProviderTurnError,
    ProviderTurnRequest,
};
use crate::session::{MessageRole, SessionRecord};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgentEvent {
    Thinking {
        status: String,
        plan_items: Vec<String>,
        active_index: Option<usize>,
        reasoning_summary: Vec<String>,
        elapsed_ms: u128,
    },
    ApprovalRequested {
        status: String,
        tool_name: String,
        summary: String,
        risk: String,
        tool_call_id: String,
        elapsed_ms: u128,
    },
    Working {
        status: String,
        plan_items: Vec<String>,
        active_index: Option<usize>,
        tool_logs: Vec<(String, String, String)>,
        elapsed_ms: u128,
    },
    Done {
        status: String,
        assistant_message: String,
        completion_summary: String,
        saved_status: String,
        tool_logs: Vec<(String, String, String)>,
        elapsed_ms: u128,
    },
    Interrupted {
        status: String,
        interrupted_what: String,
        saved_status: String,
        next_actions: Vec<String>,
        elapsed_ms: u128,
    },
    Failed {
        status: String,
        error_summary: String,
        recommended_actions: Vec<String>,
        elapsed_ms: u128,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentRuntimeScript {
    events: Vec<AgentEvent>,
}

impl AgentRuntimeScript {
    pub fn new(events: Vec<AgentEvent>) -> Self {
        Self { events }
    }

    pub fn events(&self) -> &[AgentEvent] {
        &self.events
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingTurnState {
    pub waiting_tool_call_id: String,
    pub remaining_events: Vec<AgentEvent>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentRuntime {
    script: AgentRuntimeScript,
}

impl AgentRuntime {
    pub fn new() -> Self {
        Self {
            script: AgentRuntimeScript::new(Vec::new()),
        }
    }

    pub fn from_script(script: AgentRuntimeScript) -> Self {
        Self { script }
    }

    pub fn events(&self) -> &[AgentEvent] {
        self.script.events()
    }
}

pub struct BasicAgentLoop;

impl BasicAgentLoop {
    pub fn build_turn_request(
        model: impl Into<String>,
        session: &SessionRecord,
        stream: bool,
    ) -> ProviderTurnRequest {
        Self::build_turn_request_with_limit(model, session, stream, 12)
    }

    pub fn build_turn_request_with_limit(
        model: impl Into<String>,
        session: &SessionRecord,
        stream: bool,
        max_messages: usize,
    ) -> ProviderTurnRequest {
        let len = session.messages.len();
        let start = len.saturating_sub(max_messages);
        ProviderTurnRequest::new(
            model.into(),
            session.messages[start..]
                .iter()
                .map(|message| {
                    let role = match message.role {
                        MessageRole::System => ProviderMessageRole::System,
                        MessageRole::User => ProviderMessageRole::User,
                        MessageRole::Assistant => ProviderMessageRole::Assistant,
                        MessageRole::Tool => ProviderMessageRole::Tool,
                    };
                    ProviderMessage::new(role, message.content.clone())
                })
                .collect(),
            stream,
        )
    }

    pub fn run_turn<C: ProviderClient>(
        provider: &C,
        request: &ProviderTurnRequest,
    ) -> Result<Vec<ProviderEvent>, ProviderTurnError> {
        let mut events = Vec::new();
        provider.stream_turn(request, &mut |event| events.push(event))?;
        Ok(events)
    }
}
