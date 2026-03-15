//! Agent loop and event processing.
//!
//! Defines the [`AgentEvent`] lifecycle and the [`BasicAgentLoop`] that
//! bridges provider responses into structured tool calls.

use crate::provider::{
    ProviderClient, ProviderEvent, ProviderMessage, ProviderMessageRole, ProviderTurnError,
    ProviderTurnRequest,
};
use crate::session::{MessageRole, SessionRecord};
use crate::tooling::{ToolCallRequest, ToolInput};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Events emitted by the agent during a single turn.
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
    /// Pending structured tool calls awaiting approval in the agentic loop.
    #[serde(default)]
    pub pending_tool_calls: Vec<ToolCallRequest>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentRuntime {
    script: AgentRuntimeScript,
}

impl Default for AgentRuntime {
    fn default() -> Self {
        Self::new()
    }
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
        // Try strict extraction first, fall back to lenient for unclosed blocks.
        let final_block = extract_final_block(content, "ANVIL_FINAL")
            .or_else(|| extract_final_block_lenient(content, "ANVIL_FINAL"));

        let mut tool_calls = Vec::new();
        for block in tool_blocks {
            tool_calls.push(parse_tool_call_block(&block)?);
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

fn parse_tool_call_block(block: &str) -> Result<ToolCallRequest, String> {
    match serde_json::from_str::<Value>(block) {
        Ok(value) => parse_tool_call_value(&value),
        Err(err) => repair_tool_call_block(block)
            .ok_or_else(|| format!("invalid ANVIL_TOOL JSON: {err}")),
    }
}

fn parse_tool_call_value(value: &Value) -> Result<ToolCallRequest, String> {
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
                .or_else(|| value.get("path"))
                .and_then(Value::as_str)
                .ok_or_else(|| "missing root in file.search tool block".to_string())?
                .to_string(),
            pattern: value
                .get("pattern")
                .or_else(|| value.get("content"))
                .or_else(|| value.get("query"))
                .and_then(Value::as_str)
                .ok_or_else(|| "missing pattern in file.search tool block".to_string())?
                .to_string(),
        },
        "shell.exec" | "shell" => ToolInput::ShellExec {
            command: value
                .get("command")
                .and_then(Value::as_str)
                .ok_or_else(|| "missing command in shell.exec tool block".to_string())?
                .to_string(),
        },
        other => return Err(format!("unsupported tool in ANVIL_TOOL block: {other}")),
    };

    Ok(ToolCallRequest::new(
        tool_call_id.to_string(),
        tool_name.to_string(),
        input,
    ))
}

fn repair_tool_call_block(block: &str) -> Option<ToolCallRequest> {
    let tool_name = extract_simple_string_field(block, "tool")?;
    let tool_call_id =
        extract_simple_string_field(block, "id").unwrap_or_else(|| "call_generated_001".to_string());

    let input = match tool_name.as_str() {
        "file.write" => ToolInput::FileWrite {
            path: extract_simple_string_field(block, "path")?,
            content: extract_trailing_string_field(block, "content")?,
        },
        "file.read" => ToolInput::FileRead {
            path: extract_simple_string_field(block, "path")?,
        },
        "file.search" => ToolInput::FileSearch {
            root: extract_simple_string_field(block, "root")
                .or_else(|| extract_simple_string_field(block, "path"))?,
            pattern: extract_simple_string_field(block, "pattern")
                .or_else(|| extract_simple_string_field(block, "content"))
                .or_else(|| extract_simple_string_field(block, "query"))?,
        },
        "shell.exec" | "shell" => ToolInput::ShellExec {
            command: extract_simple_string_field(block, "command")?,
        },
        _ => return None,
    };

    Some(ToolCallRequest::new(tool_call_id, tool_name, input))
}

fn extract_simple_string_field(block: &str, key: &str) -> Option<String> {
    let marker = format!("\"{key}\":\"");
    let start = block.find(&marker)? + marker.len();
    let tail = &block[start..];
    let mut result = String::new();
    let mut escaped = false;

    for ch in tail.chars() {
        if escaped {
            result.push(match ch {
                'n' => '\n',
                'r' => '\r',
                't' => '\t',
                '"' => '"',
                '\\' => '\\',
                other => other,
            });
            escaped = false;
            continue;
        }

        match ch {
            '\\' => escaped = true,
            '"' => return Some(result),
            other => result.push(other),
        }
    }

    None
}

fn extract_trailing_string_field(block: &str, key: &str) -> Option<String> {
    let marker = format!("\"{key}\":\"");
    let start = block.find(&marker)? + marker.len();
    let closing_brace = block.rfind('}')?;
    let before_brace = &block[..closing_brace];
    let end = before_brace.rfind('"')?;
    (end >= start).then(|| loose_unescape(&block[start..end]))
}

fn loose_unescape(value: &str) -> String {
    value
        .replace("\\n", "\n")
        .replace("\\r", "\r")
        .replace("\\t", "\t")
        .replace("\\\"", "\"")
        .replace("\\\\", "\\")
}

fn derive_context_budget(context_window: u32) -> usize {
    if let Ok(override_val) = std::env::var("ANVIL_CONTEXT_BUDGET")
        && let Ok(budget) = override_val.parse::<usize>()
    {
        return budget;
    }
    let quarter = (context_window / 4) as usize;
    let half = (context_window / 2) as usize;
    quarter.clamp(256, half)
}

fn estimate_message_tokens(content: &str) -> usize {
    let chars = content.chars().count();
    chars.div_ceil(4).max(1)
}

fn tool_protocol_system_prompt() -> &'static str {
    concat!(
        "You are Anvil, a local coding agent for serious terminal work.\n",
        "\n",
        "## Work approach\n",
        "When given a task, follow this approach:\n",
        "1. Start by understanding the current state: list directories (file.read on \".\") or search (file.search) before assuming files exist.\n",
        "2. Plan your work: break complex tasks into steps. State your plan before executing.\n",
        "3. Execute iteratively: use tools to gather information, then act on what you learned. Do NOT guess file paths — discover them first.\n",
        "4. If a tool call fails (e.g. file not found), adapt your plan based on the error rather than stopping.\n",
        "5. Summarize what you accomplished and what remains.\n",
        "\n",
        "## Tool protocol\n",
        "When a task requires file operations, respond using fenced blocks.\n",
        "\n",
        "Available tools:\n",
        "\n",
        "1. file.read — read a file or list a directory:\n",
        "```ANVIL_TOOL\n",
        "{\"id\":\"call_001\",\"tool\":\"file.read\",\"path\":\"./relative/path\"}\n",
        "```\n",
        "\n",
        "2. file.write — create or overwrite a file:\n",
        "```ANVIL_TOOL\n",
        "{\"id\":\"call_002\",\"tool\":\"file.write\",\"path\":\"./relative/path\",\"content\":\"file content here\"}\n",
        "```\n",
        "\n",
        "3. file.search — search for files by name or content:\n",
        "```ANVIL_TOOL\n",
        "{\"id\":\"call_003\",\"tool\":\"file.search\",\"root\":\".\",\"pattern\":\"search term\"}\n",
        "```\n",
        "\n",
        "4. shell.exec — run a shell command and capture its output:\n",
        "```ANVIL_TOOL\n",
        "{\"id\":\"call_004\",\"tool\":\"shell.exec\",\"command\":\"ls -la\"}\n",
        "```\n",
        "\n",
        "After ALL tool blocks, include exactly one final block with your summary:\n",
        "```ANVIL_FINAL\n",
        "User-facing summary and code review notes.\n",
        "```\n",
        "\n",
        "Rules:\n",
        "- All paths must be relative (start with ./ or a directory name).\n",
        "- Do not use any other tool syntax.\n",
        "- Always include ANVIL_FINAL after your tool blocks.\n",
        "- If no file operations are needed, just respond normally without tool blocks.\n",
        "- Start exploration with file.read on \".\" to list the project root before reading specific files.\n",
        "- Do not assume files like README.md exist — verify first."
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

/// Extract the ANVIL_FINAL block with strict closing (for streaming detection).
fn extract_final_block(content: &str, label: &str) -> Option<String> {
    let start_marker = format!("```{label}\n");
    let start = content.find(&start_marker)?;
    let block_start = start + start_marker.len();
    // Search for closing marker AFTER the block start, not from the end.
    content[block_start..]
        .find("\n```")
        .map(|pos| content[block_start..block_start + pos].to_string())
}

/// Lenient extraction: accept an unclosed ANVIL_FINAL block.
///
/// LLMs sometimes omit the closing ``` for the final block.  When called
/// from the Done-event path (where we know the response is complete),
/// this fallback captures everything after the opening marker.
fn extract_final_block_lenient(content: &str, label: &str) -> Option<String> {
    let start_marker = format!("```{label}\n");
    let start = content.find(&start_marker)?;
    let block_start = start + start_marker.len();
    let tail = content[block_start..].trim_end();
    // Strip a trailing ``` if present (model may close without preceding newline)
    let tail = tail.strip_suffix("```").unwrap_or(tail).trim_end();
    Some(tail.to_string())
}
