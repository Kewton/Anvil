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
        context_window: u32,
    ) -> ProviderTurnRequest {
        let token_budget = derive_context_budget(context_window);
        Self::build_turn_request_with_token_budget(model, session, stream, token_budget)
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

    pub fn build_turn_request_with_token_budget(
        model: impl Into<String>,
        session: &SessionRecord,
        stream: bool,
        token_budget: usize,
    ) -> ProviderTurnRequest {
        let mut selected = Vec::new();
        let mut used_tokens = 0usize;

        for message in session.messages.iter().rev() {
            let estimated = estimate_message_tokens(&message.content);
            if !selected.is_empty() && used_tokens + estimated > token_budget {
                break;
            }
            used_tokens += estimated;
            selected.push(message);
        }

        selected.reverse();

        ProviderTurnRequest::new(
            model.into(),
            selected
                .into_iter()
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

fn derive_context_budget(context_window: u32) -> usize {
    let quarter = (context_window / 4) as usize;
    quarter.clamp(256, 8_192)
}

fn estimate_message_tokens(content: &str) -> usize {
    let chars = content.chars().count();
    chars.div_ceil(4).max(1)
}
