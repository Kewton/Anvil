//! Agent loop and event processing.
//!
//! Defines the [`AgentEvent`] lifecycle and the [`BasicAgentLoop`] that
//! bridges provider responses into structured tool calls.

pub mod model_classifier;
pub mod subagent;
pub mod tag_spec;

pub use model_classifier::ToolProtocolMode;

use crate::contracts::InferencePerformanceView;
use crate::contracts::tokens::{ContentKind, estimate_tokens};
use crate::provider::{
    ImageContent, ProviderClient, ProviderEvent, ProviderMessage, ProviderMessageRole,
    ProviderTurnError, ProviderTurnRequest,
};
use crate::session::{MessageRole, SessionRecord};
use crate::tooling::{ToolCallRequest, ToolInput, detect_image_mime};
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectLanguage {
    Rust,
    NodeJs,
}

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
        #[serde(default)]
        inference_performance: Option<InferencePerformanceView>,
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

/// Minimum token budget reserved for messages even when the system prompt
/// consumes most of the budget.  Guarantees at least ~1 message is included.
const MINIMUM_MESSAGE_BUDGET: usize = 256;

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
        system_prompt: &str,
    ) -> ProviderTurnRequest {
        let token_budget = derive_context_budget(context_window);
        Self::build_turn_request_with_token_budget(
            model,
            session,
            stream,
            token_budget,
            system_prompt,
        )
    }

    pub fn build_turn_request_with_limit(
        model: impl Into<String>,
        session: &SessionRecord,
        stream: bool,
        max_messages: usize,
        system_prompt: &str,
    ) -> ProviderTurnRequest {
        let len = session.messages.len();
        let start = len.saturating_sub(max_messages);
        let sandbox_root = std::fs::canonicalize(&session.metadata.cwd).ok();

        let messages: Vec<ProviderMessage> = std::iter::once(ProviderMessage::new(
            ProviderMessageRole::System,
            system_prompt,
        ))
        .chain(
            session.messages[start..]
                .iter()
                .map(|sm| to_provider_message_with_images(sm, sandbox_root.as_deref())),
        )
        .collect();

        ProviderTurnRequest::new(model.into(), messages, stream)
    }

    pub fn build_turn_request_with_token_budget(
        model: impl Into<String>,
        session: &SessionRecord,
        stream: bool,
        token_budget: usize,
        system_prompt: &str,
    ) -> ProviderTurnRequest {
        let system_prompt_tokens = estimate_tokens(system_prompt, ContentKind::Text);
        let budget_for_messages = token_budget
            .saturating_sub(system_prompt_tokens)
            .max(MINIMUM_MESSAGE_BUDGET);

        let mut selected = Vec::new();
        let mut used_tokens = 0usize;

        for message in session.messages.iter().rev() {
            let kind = ContentKind::from_message_role(message.role);
            let estimated = estimate_tokens(message.effective_content(), kind);
            if !selected.is_empty() && used_tokens + estimated > budget_for_messages {
                break;
            }
            used_tokens += estimated;
            selected.push(message);
        }

        selected.reverse();

        tracing::debug!(
            selected_messages = selected.len(),
            used_tokens = used_tokens,
            system_prompt_tokens = system_prompt_tokens,
            budget_for_messages = budget_for_messages,
            budget = token_budget,
            "built turn request"
        );

        let sandbox_root = std::fs::canonicalize(&session.metadata.cwd).ok();

        let messages: Vec<ProviderMessage> = std::iter::once(ProviderMessage::new(
            ProviderMessageRole::System,
            system_prompt,
        ))
        .chain(
            selected
                .into_iter()
                .map(|sm| to_provider_message_with_images(sm, sandbox_root.as_deref())),
        )
        .collect();

        ProviderTurnRequest::new(model.into(), messages, stream)
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
        Err(err) => {
            repair_tool_call_block(block).ok_or_else(|| format!("invalid ANVIL_TOOL JSON: {err}"))
        }
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
    let input = ToolInput::from_json(tool_name, value)?;

    Ok(ToolCallRequest::new(
        tool_call_id.to_string(),
        tool_name.to_string(),
        input,
    ))
}

fn repair_tool_call_block(block: &str) -> Option<ToolCallRequest> {
    let tool_name = extract_simple_string_field(block, "tool")?;
    let tool_call_id = extract_simple_string_field(block, "id")
        .unwrap_or_else(|| "call_generated_001".to_string());

    let input = ToolInput::repair_from_block(
        &tool_name,
        block,
        extract_simple_string_field,
        extract_trailing_string_field,
    )?;

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

/// Resolve image paths to base64-encoded [`ImageContent`] values.
///
/// Each path is canonicalized and checked against `sandbox_root` to prevent
/// path-traversal attacks.  On error (file missing, outside sandbox) the
/// caller is expected to log and skip.
fn resolve_image_content(
    image_paths: &[String],
    sandbox_root: &Path,
) -> Result<Vec<ImageContent>, std::io::Error> {
    image_paths
        .iter()
        .map(|path| {
            let canonical = std::fs::canonicalize(path)?;
            if !canonical.starts_with(sandbox_root) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    format!("image path outside sandbox: {}", path),
                ));
            }
            let data = std::fs::read(&canonical)?;
            let b64 = base64::engine::general_purpose::STANDARD.encode(&data);
            let mime_type = detect_image_mime(&canonical)
                .unwrap_or("application/octet-stream")
                .to_string();
            Ok(ImageContent {
                base64: b64,
                mime_type,
            })
        })
        .collect()
}

/// Convert a session message to a provider message, optionally resolving
/// attached image paths into base64-encoded [`ImageContent`].
fn to_provider_message_with_images(
    message: &crate::session::SessionMessage,
    sandbox_root: Option<&Path>,
) -> ProviderMessage {
    let role = match message.role {
        MessageRole::System => ProviderMessageRole::System,
        MessageRole::User => ProviderMessageRole::User,
        MessageRole::Assistant => ProviderMessageRole::Assistant,
        MessageRole::Tool => ProviderMessageRole::Tool,
    };
    let mut msg = ProviderMessage::new(role, message.effective_content().to_string());
    if let Some(ref paths) = message.image_paths
        && let Some(root) = sandbox_root
    {
        match resolve_image_content(paths, root) {
            Ok(images) => msg.images = Some(images),
            Err(err) => {
                tracing::warn!(error = %err, "failed to resolve image content, sending without images");
            }
        }
    }
    msg
}

fn derive_context_budget(context_window: u32) -> usize {
    if let Ok(override_val) = std::env::var("ANVIL_CONTEXT_BUDGET")
        && let Ok(budget) = override_val.parse::<usize>()
    {
        return budget;
    }
    let quarter = (context_window / 4) as usize;
    let half = (context_window / 2) as usize;
    let budget = quarter.clamp(256, half);
    tracing::debug!(
        budget = budget,
        context_window = context_window,
        "context budget derived"
    );
    budget
}

// --- Basic tool constants (always included) ---

const TOOL_DESC_FILE_READ: &str = concat!(
    "1. file.read — read a file or list a directory (also supports image files: PNG/JPG/JPEG/GIF/WebP, max 20MB):\n",
    "```ANVIL_TOOL\n",
    "{\"id\":\"call_001\",\"tool\":\"file.read\",\"path\":\"./relative/path\"}\n",
    "```\n",
    "\n",
);

const TOOL_DESC_FILE_WRITE: &str = concat!(
    "2. file.write — create or overwrite a file:\n",
    "```ANVIL_TOOL\n",
    "{\"id\":\"call_002\",\"tool\":\"file.write\",\"path\":\"./relative/path\",\"content\":\"file content here\"}\n",
    "```\n",
    "\n",
);

const TOOL_DESC_FILE_EDIT: &str = concat!(
    "3. file.edit — edit a file by replacing a specific string:\n",
    "```ANVIL_TOOL\n",
    "{\"id\":\"call_007\",\"tool\":\"file.edit\",\"path\":\"./relative/path\",\"old_string\":\"text to find\",\"new_string\":\"replacement text\"}\n",
    "```\n",
    "\n",
);

const TOOL_DESC_FILE_SEARCH: &str = concat!(
    "4. file.search — search for files by name or content:\n",
    "```ANVIL_TOOL\n",
    "{\"id\":\"call_003\",\"tool\":\"file.search\",\"root\":\".\",\"pattern\":\"search term\"}\n",
    "```\n",
    "\n",
);

const TOOL_DESC_SHELL_EXEC: &str = concat!(
    "5. shell.exec — run a shell command and capture its output:\n",
    "```ANVIL_TOOL\n",
    "{\"id\":\"call_004\",\"tool\":\"shell.exec\",\"command\":\"ls -la\"}\n",
    "```\n",
    "\n",
);

// --- Optional tool constants (included when used) ---

const TOOL_DESC_WEB_FETCH: &str = concat!(
    "6. web.fetch — fetch the contents of a URL:\n",
    "```ANVIL_TOOL\n",
    "{\"id\":\"call_005\",\"tool\":\"web.fetch\",\"url\":\"https://example.com\"}\n",
    "```\n",
    "\n",
);

const TOOL_DESC_WEB_SEARCH: &str = concat!(
    "7. web.search — search the web by keyword:\n",
    "```ANVIL_TOOL\n",
    "{\"id\":\"call_006\",\"tool\":\"web.search\",\"query\":\"search keywords here\"}\n",
    "```\n",
    "Use web.search when you need to look up error messages, library usage, or any information not available locally.\n",
    "\n",
);

const TOOL_DESC_AGENT_EXPLORE: &str = concat!(
    "8. agent.explore — launch a read-only sub-agent to explore the codebase:\n",
    "```ANVIL_TOOL\n",
    "{\"id\":\"call_008\",\"tool\":\"agent.explore\",\"prompt\":\"Investigate the module structure under src/\",\"scope\":\"./src\"}\n",
    "```\n",
    "The sub-agent can only use file.read and file.search within the given scope directory.\n",
    "\n",
);

const TOOL_DESC_AGENT_PLAN: &str = concat!(
    "9. agent.plan — launch a read-only sub-agent to create an implementation plan:\n",
    "```ANVIL_TOOL\n",
    "{\"id\":\"call_009\",\"tool\":\"agent.plan\",\"prompt\":\"Create a plan to add error handling\",\"scope\":\"./src\"}\n",
    "```\n",
    "The sub-agent can use file.read, file.search, and web.fetch within the given scope directory.\n",
    "\n",
);

/// Data-driven definition of optional tools: (tool_name, tool_description).
const OPTIONAL_TOOLS: &[(&str, &str)] = &[
    ("web.fetch", TOOL_DESC_WEB_FETCH),
    ("web.search", TOOL_DESC_WEB_SEARCH),
    ("agent.explore", TOOL_DESC_AGENT_EXPLORE),
    ("agent.plan", TOOL_DESC_AGENT_PLAN),
];

/// Names of all optional tools, derived from [`OPTIONAL_TOOLS`].
fn optional_tool_names() -> impl Iterator<Item = &'static str> {
    OPTIONAL_TOOLS.iter().map(|(name, _)| *name)
}

// --- Static section constants ---

const PROMPT_WORK_APPROACH: &str = concat!(
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
);

const PROMPT_TOOL_RULES: &str = concat!(
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
    "- Do not assume files like README.md exist — verify first.\n",
    "- For dev servers and watch processes (npm run dev, cargo watch, etc.), use background execution with '&' so the command returns immediately.\n",
    "- shell.exec output is streamed to the terminal in real-time. The user can press Ctrl+C to cancel.\n",
    "\n",
    "## GitHub Insights\n",
    "When asked about repository statistics, use shell.exec with gh api:\n",
    "- Contributors: gh api repos/{owner}/{repo}/stats/contributors\n",
    "- Commit activity: gh api repos/{owner}/{repo}/stats/commit_activity\n",
    "- Code frequency: gh api repos/{owner}/{repo}/stats/code_frequency\n",
    "- Detect repo: gh repo view --json owner,name\n",
    "- GitHub stats endpoints (contributors, commit_activity) may return {} on first request. If you get an empty response, wait 3 seconds with shell.exec sleep 3 and retry the same API call.",
);

/// Guidance for LLMs on confirm-class tool behavior.
///
/// Prevents models from asking for permission in natural language
/// when a tool requires user approval. Anvil handles approval inline.
const PROMPT_CONFIRM_CLASS_GUIDANCE: &str = concat!(
    "\n## Tool approval\n",
    "Some tools require user approval before execution.\n",
    "Anvil automatically shows an approval prompt when you call these tools.\n",
    "Do NOT ask the user for permission in natural language.\n",
    "Always emit the tool call directly using ANVIL_TOOL blocks — Anvil handles the rest.\n",
    "If a tool call is denied, you will receive \"denied by user\" as the result.\n",
);

const PROMPT_GIT_GUIDE: &str = concat!(
    "\n\n## Git operations\n",
    "When working with Git, follow these safety categories:\n",
    "\n",
    "**Safe (auto-approved):**\n",
    "- git status, git log, git diff, git branch, git show <ref>, git remote -v, git rev-parse\n",
    "\n",
    "**Change (requires confirmation):**\n",
    "- git add, git commit, git push, git checkout, git merge, git rebase, git stash\n",
    "\n",
    "**NEVER use these without explicit user request:**\n",
    "- git reset --hard — destroys uncommitted changes irreversibly\n",
    "- git clean -fd — deletes untracked files permanently\n",
    "- git push --force — rewrites remote history, can lose team members' work\n",
    "- git rebase on shared branches — rewrites history others depend on\n",
    "- --no-verify flag — skips safety hooks, always blocked by the system\n",
);

const PROMPT_ENV_GUIDE: &str = concat!(
    "\n## Environment inspection\n",
    "Use these to check the development environment:\n",
    "- which <tool> — check if a tool is installed\n",
    "- uname — identify the operating system\n",
    "- node -v, rustc --version, python --version, go version — check language versions\n",
);

const PROMPT_PROCESS_GUIDE: &str = concat!(
    "\n## Process management\n",
    "- lsof -i — check network port usage\n",
    "- For dev servers (npm run dev, cargo watch), use background execution with '&'\n",
);

const PROMPT_RUST_GUIDE: &str = concat!(
    "\n## Rust development\n",
    "Build, test, and lint commands (auto-approved):\n",
    "- cargo build — compile the project\n",
    "- cargo test — run all tests\n",
    "- cargo clippy --all-targets — run linter (aim for zero warnings)\n",
    "- cargo fmt --check — check formatting\n",
    "- cargo check — type-check without building\n",
    "When fixing issues, iterate: make changes, then cargo build, cargo test, cargo clippy.\n",
);

const PROMPT_NODEJS_GUIDE: &str = concat!(
    "\n## Node.js development\n",
    "Build, test, and lint commands (auto-approved):\n",
    "- npm test — run test suite\n",
    "- npx jest <path> — run specific tests\n",
    "- npx eslint <path> — run linter\n",
    "- npx prettier --check <path> — check formatting\n",
    "When fixing issues, iterate: make changes, then npm test, npx eslint.\n",
);

/// Generate the system prompt with dynamic tool selection based on used_tools.
///
/// Basic tools (file.read, file.write, file.edit, file.search, shell.exec)
/// are always included. Optional tools (web.fetch, web.search, agent.explore,
/// agent.plan) are only included when present in used_tools.
pub(crate) fn tool_protocol_system_prompt(
    languages: &[ProjectLanguage],
    mcp_tool_descriptions: Option<&str>,
    used_tools: &std::collections::HashSet<String>,
) -> String {
    let mut prompt = String::with_capacity(8192);

    // Work approach (static)
    prompt.push_str(PROMPT_WORK_APPROACH);

    // Basic tools (always included)
    prompt.push_str(TOOL_DESC_FILE_READ);
    prompt.push_str(TOOL_DESC_FILE_WRITE);
    prompt.push_str(TOOL_DESC_FILE_EDIT);
    prompt.push_str(TOOL_DESC_FILE_SEARCH);
    prompt.push_str(TOOL_DESC_SHELL_EXEC);

    // Optional tools (data-driven: included only when present in used_tools)
    for (tool_name, tool_desc) in OPTIONAL_TOOLS {
        if used_tools.contains(*tool_name) {
            prompt.push_str(tool_desc);
        }
    }

    // Tool rules and GitHub Insights (static)
    prompt.push_str(PROMPT_TOOL_RULES);

    // Confirm-class tool approval guidance (static)
    prompt.push_str(PROMPT_CONFIRM_CLASS_GUIDANCE);

    // Append MCP tool descriptions dynamically
    // [D4-010] mcp_tool_descriptions is sanitized by generate_mcp_tool_descriptions()
    if let Some(mcp_desc) = mcp_tool_descriptions {
        prompt.push_str("\n\n## MCP External Tools\n\n");
        prompt.push_str(mcp_desc);
    }

    // Git operations guide (static)
    prompt.push_str(PROMPT_GIT_GUIDE);

    // Environment inspection guide (static)
    prompt.push_str(PROMPT_ENV_GUIDE);

    // Process management guide (static)
    prompt.push_str(PROMPT_PROCESS_GUIDE);

    // Rust-specific guide (only when Rust detected)
    if languages.contains(&ProjectLanguage::Rust) {
        prompt.push_str(PROMPT_RUST_GUIDE);
    }

    // Node.js-specific guide (only when NodeJs detected)
    if languages.contains(&ProjectLanguage::NodeJs) {
        prompt.push_str(PROMPT_NODEJS_GUIDE);
    }

    prompt
}

/// Generate a system prompt with no optional tools (basic tools only).
///
/// Useful for testing that optional tools are excluded when unused.
pub fn tool_protocol_system_prompt_basic_only(
    languages: &[ProjectLanguage],
    mcp_tool_descriptions: Option<&str>,
) -> String {
    let empty = std::collections::HashSet::new();
    tool_protocol_system_prompt(languages, mcp_tool_descriptions, &empty)
}

/// Generate a system prompt with all tools included (for test compatibility
/// and contexts where all tools should be available).
pub fn tool_protocol_system_prompt_all_tools(
    languages: &[ProjectLanguage],
    mcp_tool_descriptions: Option<&str>,
) -> String {
    let all_tools: std::collections::HashSet<String> =
        optional_tool_names().map(|s| s.to_string()).collect();
    tool_protocol_system_prompt(languages, mcp_tool_descriptions, &all_tools)
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

// --- MCP tool description generation ---

use crate::mcp::McpToolInfo;
use std::collections::HashMap;

/// Maximum characters for MCP tool descriptions in the system prompt.
/// [D3-009] Prevents system prompt bloat that compresses message budget.
const MAX_MCP_PROMPT_CHARS: usize = 8000;

/// Maximum characters per individual tool description.
const MAX_TOOL_DESC_CHARS: usize = 500;

/// Generate MCP tool descriptions for inclusion in the system prompt.
///
/// [D4-010] Sanitizes descriptions to remove ANVIL_TOOL/ANVIL_FINAL markers.
/// [D3-009] Falls back to tool-name-only list if total exceeds MAX_MCP_PROMPT_CHARS.
pub fn generate_mcp_tool_descriptions(tools: &HashMap<String, Vec<McpToolInfo>>) -> String {
    let mut full_descriptions = String::new();

    for (server_name, tool_list) in tools {
        for tool_info in tool_list {
            let mcp_name = format!("mcp__{server_name}__{}", tool_info.name);

            // [D4-010] Sanitize description: remove ANVIL_TOOL/ANVIL_FINAL markers
            let (mut desc, _) = crate::config::sanitize_markers(&tool_info.description);

            // Truncate per-tool description
            if desc.chars().count() > MAX_TOOL_DESC_CHARS {
                desc = desc.chars().take(MAX_TOOL_DESC_CHARS).collect::<String>();
                desc.push_str("...");
            }

            let schema_str = serde_json::to_string(&tool_info.input_schema).unwrap_or_default();

            full_descriptions.push_str(&format!(
                "- **{mcp_name}**: {desc}\n  Input schema: {schema_str}\n  Usage:\n  ```ANVIL_TOOL\n  {{\"id\":\"call_mcp\",\"tool\":\"{mcp_name}\",... }}\n  ```\n\n"
            ));
        }
    }

    // [D3-009] Check total size and fall back to name-only list if too large
    if full_descriptions.chars().count() > MAX_MCP_PROMPT_CHARS {
        eprintln!(
            "Warning: MCP tool descriptions exceed {} characters, falling back to tool-name-only list.",
            MAX_MCP_PROMPT_CHARS
        );
        let mut fallback = String::from("Available MCP tools (use ANVIL_TOOL blocks to call):\n");
        for (server_name, tool_list) in tools {
            for tool_info in tool_list {
                let mcp_name = format!("mcp__{server_name}__{}", tool_info.name);
                fallback.push_str(&format!("- {mcp_name}\n"));
            }
        }
        return fallback;
    }

    full_descriptions
}
