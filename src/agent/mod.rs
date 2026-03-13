use crate::provider::{
    ProviderClient, ProviderEvent, ProviderMessage, ProviderMessageRole, ProviderTurnError,
    ProviderTurnRequest,
};
use crate::session::{MessageRole, SessionRecord};
use crate::tooling::{ToolCallRequest, ToolInput};
use serde::{Deserialize, Serialize};
use serde_json::Value;

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StructuredAssistantResponse {
    pub tool_calls: Vec<ToolCallRequest>,
    pub final_response: String,
}

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
            std::iter::once(ProviderMessage::new(
                ProviderMessageRole::System,
                tool_protocol_system_prompt(),
            ))
            .chain(selected.into_iter().map(|message| {
                let role = match message.role {
                    MessageRole::System => ProviderMessageRole::System,
                    MessageRole::User => ProviderMessageRole::User,
                    MessageRole::Assistant => ProviderMessageRole::Assistant,
                    MessageRole::Tool => ProviderMessageRole::Tool,
                };
                ProviderMessage::new(role, message.content.clone())
            }))
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

    pub fn parse_structured_response(content: &str) -> Result<StructuredAssistantResponse, String> {
        let tool_blocks = extract_fenced_blocks(content, "ANVIL_TOOL");
        let final_block = extract_final_block(content, "ANVIL_FINAL");

        let mut tool_calls = Vec::new();
        for block in tool_blocks {
            let value: Value = serde_json::from_str(&block)
                .map_err(|err| format!("invalid ANVIL_TOOL JSON: {err}"))?;
            let tool_name = value
                .get("tool")
                .and_then(Value::as_str)
                .ok_or_else(|| "missing tool in ANVIL_TOOL block".to_string())?;
            let tool_call_id = value
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("call_generated_001");
            let input = match tool_name {
                "file.write" => ToolInput::FileWrite {
                    path: value
                        .get("path")
                        .and_then(Value::as_str)
                        .ok_or_else(|| "missing path in file.write tool block".to_string())?
                        .to_string(),
                    content: value
                        .get("content")
                        .and_then(Value::as_str)
                        .ok_or_else(|| "missing content in file.write tool block".to_string())?
                        .to_string(),
                },
                "file.read" => ToolInput::FileRead {
                    path: value
                        .get("path")
                        .and_then(Value::as_str)
                        .ok_or_else(|| "missing path in file.read tool block".to_string())?
                        .to_string(),
                },
                "file.search" => ToolInput::FileSearch {
                    root: value
                        .get("root")
                        .and_then(Value::as_str)
                        .ok_or_else(|| "missing root in file.search tool block".to_string())?
                        .to_string(),
                    pattern: value
                        .get("pattern")
                        .and_then(Value::as_str)
                        .ok_or_else(|| "missing pattern in file.search tool block".to_string())?
                        .to_string(),
                },
                other => return Err(format!("unsupported tool in ANVIL_TOOL block: {other}")),
            };
            tool_calls.push(ToolCallRequest::new(
                tool_call_id.to_string(),
                tool_name.to_string(),
                input,
            ));
        }

        let final_response = final_block
            .map(|block| block.trim().to_string())
            .unwrap_or_else(|| content.trim().to_string());

        Ok(StructuredAssistantResponse {
            tool_calls,
            final_response,
        })
    }

    pub fn is_complete_structured_response(content: &str) -> bool {
        extract_final_block(content, "ANVIL_FINAL").is_some()
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

fn tool_protocol_system_prompt() -> &'static str {
    concat!(
        "You are Anvil. When a task requires file changes, respond using this protocol.\n",
        "Use one or more fenced blocks exactly like:\n",
        "```ANVIL_TOOL\n",
        "{\"id\":\"call_001\",\"tool\":\"file.write\",\"path\":\"./relative/path\",\"content\":\"...\"}\n",
        "```\n",
        "Supported tools: file.write, file.read, file.search.\n",
        "After tool blocks, include exactly one final fenced block:\n",
        "```ANVIL_FINAL\n",
        "User-facing summary and code review notes.\n",
        "```\n",
        "Do not use any other tool syntax."
    )
}

fn extract_fenced_blocks(content: &str, label: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    let start_marker = format!("```{label}\n");
    let end_marker = "\n```";
    let mut cursor = 0usize;

    while let Some(start) = content[cursor..].find(&start_marker) {
        let block_start = cursor + start + start_marker.len();
        if let Some(end) = content[block_start..].find(end_marker) {
            let block_end = block_start + end;
            blocks.push(content[block_start..block_end].to_string());
            cursor = block_end + end_marker.len();
        } else {
            break;
        }
    }

    blocks
}

fn extract_final_block(content: &str, label: &str) -> Option<String> {
    let start_marker = format!("```{label}\n");
    let start = content.find(&start_marker)?;
    let block_start = start + start_marker.len();
    let block_end = content.rfind("\n```")?;
    (block_end >= block_start).then(|| content[block_start..block_end].to_string())
}
