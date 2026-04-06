//! Agent loop and event processing.
//!
//! Defines the [`AgentEvent`] lifecycle and the [`BasicAgentLoop`] that
//! bridges provider responses into structured tool calls.

pub mod model_classifier;
pub mod subagent;
pub mod tag_parser;
pub mod tag_spec;

pub use model_classifier::{ModelCapability, ModelSizeClass, PromptTier, ToolProtocolMode};

use crate::contracts::InferencePerformanceView;
use crate::contracts::tokens::{ContentKind, NO_CALIBRATION, estimate_tokens_calibrated};
use crate::provider::{
    ImageContent, ProviderClient, ProviderEvent, ProviderMessage, ProviderMessageRole,
    ProviderTurnError, ProviderTurnRequest,
};
use crate::session::{MessageRole, SessionMessage, SessionRecord};
use crate::tooling::{ToolCallRequest, ToolInput, detect_image_mime};
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};
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
    /// Whether an ANVIL_FINAL block was detected in this response (Issue #173).
    pub anvil_final_detected: bool,
    /// Provider の生レスポンス全文。ANVIL_PLAN 抽出専用。
    /// ログ・永続化に使わないこと。
    pub raw_content: String,
}

impl StructuredAssistantResponse {
    /// Fallback factory for cases that bypass `parse_structured_response`.
    pub fn empty(final_response: String) -> Self {
        Self {
            tool_calls: Vec::new(),
            raw_content: final_response.clone(),
            final_response,
            anvil_final_detected: false,
        }
    }
}

impl BasicAgentLoop {
    pub fn build_turn_request(
        model: impl Into<String>,
        session: &SessionRecord,
        stream: bool,
        context_window: u32,
        system_prompt: &str,
        context_budget_override: Option<u32>,
    ) -> ProviderTurnRequest {
        let token_budget = derive_context_budget(context_window, context_budget_override);
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
        let (request, _) = build_turn_request_with_calibration(
            model,
            session,
            stream,
            token_budget,
            system_prompt,
            NO_CALIBRATION,
        );
        request
    }

    /// Build a provider turn request with calibration ratio applied.
    ///
    /// Returns `(ProviderTurnRequest, estimated_prompt_tokens)`.
    /// `calibration_ratio` is obtained from `TokenCalibrationStore::get_ratio()`.
    pub fn build_turn_request_calibrated(
        model: impl Into<String>,
        session: &SessionRecord,
        stream: bool,
        context_window: u32,
        system_prompt: &str,
        calibration_ratio: f64,
        context_budget_override: Option<u32>,
    ) -> (ProviderTurnRequest, usize) {
        let token_budget = derive_context_budget(context_window, context_budget_override);
        build_turn_request_with_calibration(
            model,
            session,
            stream,
            token_budget,
            system_prompt,
            calibration_ratio,
        )
    }

    /// Estimate the number of session messages that would be pruned.
    /// Uses the same shared selection logic as `build_turn_request_with_calibration()`.
    /// Returns `(pruned_count, selected_tokens)`.
    pub fn estimate_pruned_message_count(
        session: &SessionRecord,
        context_window: u32,
        system_prompt_tokens: usize,
        calibration_ratio: f64,
        context_budget_override: Option<u32>,
    ) -> (usize, usize) {
        let token_budget = derive_context_budget(context_window, context_budget_override);
        let budget_for_messages = token_budget
            .saturating_sub(system_prompt_tokens)
            .max(MINIMUM_MESSAGE_BUDGET);

        let (selected, selected_tokens) = select_messages_within_budget(
            session.messages.iter(),
            budget_for_messages,
            calibration_ratio,
        );

        let pruned_count = session.messages.len().saturating_sub(selected.len());
        (pruned_count, selected_tokens)
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
        let empty = crate::tooling::ToolRegistry::new();
        Self::parse_structured_response_with_registry(content, &empty)
    }

    pub fn parse_structured_response_with_registry(
        content: &str,
        registry: &crate::tooling::ToolRegistry,
    ) -> Result<StructuredAssistantResponse, String> {
        let tool_blocks = extract_fenced_blocks(content, "ANVIL_TOOL");

        // ANVIL_FINALの位置を取得（カットオフポイント）
        let final_cutoff = content.find("```ANVIL_FINAL\n");
        let anvil_final_detected = final_cutoff.is_some();

        // Try strict extraction first, fall back to lenient for unclosed blocks.
        let final_block = extract_final_block(content, "ANVIL_FINAL")
            .or_else(|| extract_final_block_lenient(content, "ANVIL_FINAL"));

        let mut tool_calls = Vec::new();
        for (offset, block) in tool_blocks {
            // ANVIL_TOOLブロックの開始マーカー位置(offset)が
            // ANVIL_FINALの開始マーカー位置(cutoff)と同じかそれ以降にある場合に除外
            if let Some(cutoff) = final_cutoff
                && offset >= cutoff
            {
                continue;
            }
            tool_calls.push(parse_tool_call_block_multi_tier(&block, registry)?);
        }

        // Issue #186: 同一ターン内の重複ツール呼び出しを排除し、ID衝突を解消
        let tool_calls = dedup_tool_calls(tool_calls);

        let final_response = final_block
            .map(|block| block.trim().to_string())
            .unwrap_or_else(|| content.trim().to_string());

        Ok(StructuredAssistantResponse {
            tool_calls,
            final_response,
            anvil_final_detected,
            raw_content: content.to_string(),
        })
    }

    /// Strict ANVIL_FINAL detection (closed block only). Used during streaming.
    pub fn is_complete_structured_response(content: &str) -> bool {
        extract_final_block(content, "ANVIL_FINAL").is_some()
    }

    /// Lenient ANVIL_FINAL detection (accepts unclosed blocks). Used after response completion.
    pub fn is_complete_structured_response_lenient(content: &str) -> bool {
        extract_final_block(content, "ANVIL_FINAL").is_some()
            || extract_final_block_lenient(content, "ANVIL_FINAL").is_some()
    }
}

/// Issue #186: 同一ターン内の重複ツール呼び出しを排除し、ID衝突を解消する。
///
/// 1. セマンティック重複排除: (tool_name, input) が同一のツール呼び出しは最初の出現のみ保持
/// 2. ID衝突解消: 同一IDが複数存在する場合、2番目以降にサフィックスを付与
fn dedup_tool_calls(tool_calls: Vec<ToolCallRequest>) -> Vec<ToolCallRequest> {
    // Step 1: セマンティック重複排除
    let mut seen_fingerprints: HashSet<u64> = HashSet::new();
    let mut deduped: Vec<ToolCallRequest> = Vec::new();

    for call in tool_calls {
        let fp = tool_call_fingerprint(&call);
        if seen_fingerprints.insert(fp) {
            deduped.push(call);
        }
    }

    // Step 2: ID衝突解消 — 同一IDが複数ある場合にリナンバリング
    let mut id_counts: HashMap<String, usize> = HashMap::new();
    for call in &mut deduped {
        let count = id_counts.entry(call.tool_call_id.clone()).or_insert(0);
        if *count > 0 {
            call.tool_call_id = format!("{}_{}", call.tool_call_id, count);
        }
        *count += 1;
    }

    deduped
}

/// (tool_name, serialized input) のハッシュでセマンティック一致を判定
fn tool_call_fingerprint(call: &ToolCallRequest) -> u64 {
    let mut hasher = DefaultHasher::new();
    call.tool_name.hash(&mut hasher);
    if let Ok(s) = serde_json::to_string(&call.input) {
        s.hash(&mut hasher);
    }
    hasher.finish()
}

/// Multi-tier tool call parser.
///
/// Tier 1: Strict JSON (existing parse path)
/// Tier 2: Tag-based (XML-like) format via tag_parser
/// Tier 3: Repair fallback (existing repair path)
fn parse_tool_call_block_multi_tier(
    block: &str,
    registry: &crate::tooling::ToolRegistry,
) -> Result<ToolCallRequest, String> {
    // Tier 1: strict JSON
    if let Ok(value) = serde_json::from_str::<Value>(block) {
        match parse_tool_call_value(&value, registry) {
            Ok(call) => return Ok(call),
            Err(json_err) => {
                // JSON parsed but field extraction failed — this is a definitive error
                // (the tool name was recognized but required fields are missing).
                // Only fall through if the JSON didn't even have a "tool" field.
                if value.get("tool").and_then(Value::as_str).is_some() {
                    return Err(json_err);
                }
            }
        }
    }

    // Tier 2: tag-based
    if tag_parser::is_tag_format(block)
        && let Ok((tool_name, input)) = tag_parser::parse_tag_tool_block(block)
    {
        let id = format!("tag_{}", tool_name.replace('.', "_"));
        return Ok(ToolCallRequest::new(id, tool_name, input));
    }

    // Tier 3: repair fallback
    repair_tool_call_block(block)
        .ok_or_else(|| "Failed to parse tool call in any format".to_string())
}

fn parse_tool_call_value(
    value: &Value,
    registry: &crate::tooling::ToolRegistry,
) -> Result<ToolCallRequest, String> {
    let tool_name = value
        .get("tool")
        .and_then(Value::as_str)
        .ok_or_else(|| "missing tool in ANVIL_TOOL block".to_string())?;
    let tool_call_id = value
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("call_generated_001");

    // Try built-in tools first, then fall back to custom tools.
    let (resolved_name, input) = match ToolInput::from_json(tool_name, value) {
        Ok(input) => (tool_name.to_string(), input),
        Err(e) => {
            if let Some(tool_def) = registry.find_custom_tool(tool_name) {
                let input = ToolInput::from_custom_tool(tool_def, value)?;
                let display_name = crate::config::custom_tool_display_name(&tool_def.name);
                (display_name, input)
            } else {
                return Err(e);
            }
        }
    };

    Ok(ToolCallRequest::new(
        tool_call_id.to_string(),
        resolved_name,
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

/// Shared message selection within token budget.
/// Returns (selected messages in reverse order, used_tokens).
fn select_messages_within_budget<'a>(
    messages: impl DoubleEndedIterator<Item = &'a SessionMessage>,
    budget_for_messages: usize,
    calibration_ratio: f64,
) -> (Vec<&'a SessionMessage>, usize) {
    let mut selected = Vec::new();
    let mut used_tokens = 0usize;

    for message in messages.rev() {
        let kind = ContentKind::from_message_role(message.role);
        let estimated =
            estimate_tokens_calibrated(message.effective_content(), kind, calibration_ratio);
        if !selected.is_empty() && used_tokens + estimated > budget_for_messages {
            break;
        }
        used_tokens += estimated;
        selected.push(message);
    }

    (selected, used_tokens)
}

/// Internal helper: build a turn request with calibration ratio applied.
///
/// Both `build_turn_request_with_token_budget` (ratio=NO_CALIBRATION) and
/// `build_turn_request_calibrated` delegate to this function.
fn build_turn_request_with_calibration(
    model: impl Into<String>,
    session: &SessionRecord,
    stream: bool,
    token_budget: usize,
    system_prompt: &str,
    calibration_ratio: f64,
) -> (ProviderTurnRequest, usize) {
    let system_prompt_tokens =
        estimate_tokens_calibrated(system_prompt, ContentKind::Text, calibration_ratio);
    let budget_for_messages = token_budget
        .saturating_sub(system_prompt_tokens)
        .max(MINIMUM_MESSAGE_BUDGET);

    let (mut selected, used_tokens) = select_messages_within_budget(
        session.messages.iter(),
        budget_for_messages,
        calibration_ratio,
    );

    selected.reverse();

    let estimated_prompt_tokens = system_prompt_tokens + used_tokens;

    tracing::debug!(
        selected_messages = selected.len(),
        used_tokens = used_tokens,
        system_prompt_tokens = system_prompt_tokens,
        budget_for_messages = budget_for_messages,
        budget = token_budget,
        calibration_ratio = calibration_ratio,
        estimated_prompt_tokens = estimated_prompt_tokens,
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

    (
        ProviderTurnRequest::new(model.into(), messages, stream),
        estimated_prompt_tokens,
    )
}

fn derive_context_budget(context_window: u32, context_budget_override: Option<u32>) -> usize {
    if let Some(budget) = context_budget_override {
        tracing::debug!(
            budget = budget,
            context_window = context_window,
            "context budget from config override"
        );
        return budget as usize;
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
    "4. file.search — search for files by name or content (respects .gitignore):\n",
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

/// Data-driven definition of optional tools: (tool_name, tool_description, catalog_one_liner).
/// Note: web.fetch and web.search were moved to basic tools (always included)
/// because LLMs cannot discover them without prompt descriptions. See Issue #114.
const OPTIONAL_TOOLS: &[(&str, &str, &str)] = &[
    (
        "agent.explore",
        TOOL_DESC_AGENT_EXPLORE,
        "agent.explore: launch a read-only sub-agent to explore the codebase",
    ),
    (
        "agent.plan",
        TOOL_DESC_AGENT_PLAN,
        "agent.plan: launch a read-only sub-agent to create an implementation plan",
    ),
];

/// Names of all optional tools, derived from [`OPTIONAL_TOOLS`].
fn optional_tool_names() -> impl Iterator<Item = &'static str> {
    OPTIONAL_TOOLS.iter().map(|(name, _, _)| *name)
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

const PROMPT_OPTIONAL_CATALOG_HEADER: &str =
    "\nAdditional tools (use ANVIL_TOOL block format shown above):\n";

const PROMPT_TOOL_RULES: &str = concat!(
    "## ANVIL_PLAN — Change plan\n",
    "Before any file.write/file.edit, output one ANVIL_PLAN block using relative paths:\n",
    "```ANVIL_PLAN\n- [ ] src/foo.rs: description\n- [ ] src/bar.rs: description\n```\n",
    "Each item: `- [ ] <relative-path>: <description>`. Do NOT output ANVIL_FINAL until ALL items are done.\n",
    "To add items mid-task, output an ANVIL_PLAN_UPDATE block with the same format.\n",
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
    "- When the user's request requires file changes (implement, fix, create, modify, etc.), \
       you must complete the actual file modifications using file.write/file.edit, \
       not just output a plan or description.\n",
    "- For large existing files, file.write may be blocked. Use file.edit or file.edit_anchor for targeted modifications instead of rewriting entire files.\n",
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
    offline: bool,
    tier: PromptTier,
) -> String {
    match tier {
        PromptTier::Full => {
            // Full tier delegates to the protocol-aware builder (Json format).
            // TagBased protocol dispatch happens via tool_protocol_system_prompt_with_mode.
            build_json_protocol_prompt(languages, mcp_tool_descriptions, used_tools, offline)
        }
        PromptTier::Compact => {
            tool_protocol_system_prompt_compact(mcp_tool_descriptions, used_tools, offline)
        }
        PromptTier::Tiny => tool_protocol_system_prompt_tiny(),
    }
}

/// Generate the system prompt with dynamic tool selection and protocol mode.
pub(crate) fn tool_protocol_system_prompt_with_mode(
    languages: &[ProjectLanguage],
    mcp_tool_descriptions: Option<&str>,
    used_tools: &std::collections::HashSet<String>,
    offline: bool,
    protocol: ToolProtocolMode,
) -> String {
    match protocol {
        ToolProtocolMode::Json => {
            build_json_protocol_prompt(languages, mcp_tool_descriptions, used_tools, offline)
        }
        ToolProtocolMode::TagBased => {
            build_tag_protocol_prompt(languages, mcp_tool_descriptions, used_tools, offline)
        }
    }
}

/// JSON format system prompt (existing behavior).
fn build_json_protocol_prompt(
    languages: &[ProjectLanguage],
    mcp_tool_descriptions: Option<&str>,
    used_tools: &std::collections::HashSet<String>,
    offline: bool,
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
    prompt.push_str(TOOL_DESC_WEB_FETCH);
    prompt.push_str(TOOL_DESC_WEB_SEARCH);

    // Compact catalog: always show one-liner for each optional tool
    // (filtered by offline mode for web.* tools)
    let catalog_entries: Vec<&str> = OPTIONAL_TOOLS
        .iter()
        .filter(|(name, _, _)| !(offline && name.starts_with("web.")))
        .map(|(_, _, one_liner)| *one_liner)
        .collect();
    if !catalog_entries.is_empty() {
        prompt.push_str(PROMPT_OPTIONAL_CATALOG_HEADER);
        for entry in &catalog_entries {
            prompt.push_str("- ");
            prompt.push_str(entry);
            prompt.push('\n');
        }
        prompt.push('\n');
    }

    // Detailed descriptions for used optional tools
    for (tool_name, tool_desc, _) in OPTIONAL_TOOLS {
        if used_tools.contains(*tool_name) {
            if offline && tool_name.starts_with("web.") {
                continue;
            }
            prompt.push_str(tool_desc);
        }
    }

    // Tool rules and GitHub Insights (static)
    prompt.push_str(PROMPT_TOOL_RULES);

    // Confirm-class tool approval guidance (static)
    prompt.push_str(PROMPT_CONFIRM_CLASS_GUIDANCE);

    append_common_prompt_sections(&mut prompt, languages, mcp_tool_descriptions);

    prompt
}

/// Tag-based format system prompt (for smaller models).
fn build_tag_protocol_prompt(
    languages: &[ProjectLanguage],
    mcp_tool_descriptions: Option<&str>,
    _used_tools: &std::collections::HashSet<String>,
    _offline: bool,
) -> String {
    use crate::agent::tag_spec::TOOL_TAG_SPECS;

    let mut prompt = String::with_capacity(8192);

    prompt.push_str(PROMPT_WORK_APPROACH);

    // Generate tag-based tool descriptions from TOOL_TAG_SPECS
    for (i, spec) in TOOL_TAG_SPECS.iter().enumerate() {
        prompt.push_str(&format!(
            "{}. {} — use tag format:\n```ANVIL_TOOL\n{}\n```\n\n",
            i + 1,
            spec.name,
            spec.example
        ));
    }

    // Tool rules (same as JSON but with tag format note)
    prompt.push_str(PROMPT_TOOL_RULES);
    prompt.push_str(PROMPT_CONFIRM_CLASS_GUIDANCE);

    append_common_prompt_sections(&mut prompt, languages, mcp_tool_descriptions);

    prompt
}

/// Append sections common to both JSON and tag-based prompts.
fn append_common_prompt_sections(
    prompt: &mut String,
    languages: &[ProjectLanguage],
    mcp_tool_descriptions: Option<&str>,
) {
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
}

/// Compact tier: basic tools + rules, guides omitted.
fn tool_protocol_system_prompt_compact(
    mcp_tool_descriptions: Option<&str>,
    used_tools: &std::collections::HashSet<String>,
    offline: bool,
) -> String {
    let mut prompt = String::with_capacity(4096);

    // Work approach (static)
    prompt.push_str(PROMPT_WORK_APPROACH);

    // All basic tools
    prompt.push_str(TOOL_DESC_FILE_READ);
    prompt.push_str(TOOL_DESC_FILE_WRITE);
    prompt.push_str(TOOL_DESC_FILE_EDIT);
    prompt.push_str(TOOL_DESC_FILE_SEARCH);
    prompt.push_str(TOOL_DESC_SHELL_EXEC);
    prompt.push_str(TOOL_DESC_WEB_FETCH);
    prompt.push_str(TOOL_DESC_WEB_SEARCH);

    // Detailed descriptions for used optional tools (no catalog header)
    for (tool_name, tool_desc, _) in OPTIONAL_TOOLS {
        if used_tools.contains(*tool_name) {
            if offline && tool_name.starts_with("web.") {
                continue;
            }
            prompt.push_str(tool_desc);
        }
    }

    // Tool rules (static)
    prompt.push_str(PROMPT_TOOL_RULES);

    // MCP tool descriptions (with reduced limit)
    if let Some(mcp_desc) = mcp_tool_descriptions {
        let truncated = if mcp_desc.len() > 4000 {
            &mcp_desc[..4000]
        } else {
            mcp_desc
        };
        prompt.push_str("\n\n## MCP External Tools\n\n");
        prompt.push_str(truncated);
    }

    // Omitted: CONFIRM_CLASS_GUIDANCE, GIT_GUIDE, ENV_GUIDE, PROCESS_GUIDE, language guides

    prompt
}

/// Tiny tier: minimal tool syntax for very small models (<7B).
fn tool_protocol_system_prompt_tiny() -> String {
    let mut prompt = String::with_capacity(2048);

    prompt.push_str("You are Anvil, a coding agent.\n\n");
    prompt.push_str("Use ANVIL_TOOL blocks for tool calls. Available tools:\n\n");

    // Only core 4 tools + shell
    prompt.push_str(TOOL_DESC_FILE_READ);
    prompt.push_str(TOOL_DESC_FILE_WRITE);
    prompt.push_str(TOOL_DESC_FILE_EDIT);
    prompt.push_str(TOOL_DESC_SHELL_EXEC);

    prompt.push_str(concat!(
        "Rules:\n",
        "- All paths must be relative.\n",
        "- Include ANVIL_FINAL block after tool blocks.\n",
    ));

    prompt
}

/// Generate a system prompt with no optional tools (basic tools only).
///
/// Useful for testing that optional tools are excluded when unused.
/// Always uses [`PromptTier::Full`] for backward compatibility.
pub fn tool_protocol_system_prompt_basic_only(
    languages: &[ProjectLanguage],
    mcp_tool_descriptions: Option<&str>,
) -> String {
    let empty = std::collections::HashSet::new();
    tool_protocol_system_prompt(
        languages,
        mcp_tool_descriptions,
        &empty,
        false,
        PromptTier::Full,
    )
}

/// Generate a system prompt with all tools included (for test compatibility
/// and contexts where all tools should be available).
/// Always uses [`PromptTier::Full`] for backward compatibility.
pub fn tool_protocol_system_prompt_all_tools(
    languages: &[ProjectLanguage],
    mcp_tool_descriptions: Option<&str>,
) -> String {
    let all_tools: std::collections::HashSet<String> =
        optional_tool_names().map(|s| s.to_string()).collect();
    tool_protocol_system_prompt(
        languages,
        mcp_tool_descriptions,
        &all_tools,
        false,
        PromptTier::Full,
    )
}

/// Generate a system prompt with tag-based protocol (for small model testing).
pub fn tool_protocol_system_prompt_tag_based(
    languages: &[ProjectLanguage],
    mcp_tool_descriptions: Option<&str>,
) -> String {
    let empty = std::collections::HashSet::new();
    tool_protocol_system_prompt_with_mode(
        languages,
        mcp_tool_descriptions,
        &empty,
        false,
        ToolProtocolMode::TagBased,
    )
}

fn extract_fenced_blocks(content: &str, label: &str) -> Vec<(usize, String)> {
    let mut blocks = Vec::new();
    let start_marker = format!("```{label}\n");
    let end_marker = "\n```";
    let mut cursor = 0usize;

    while let Some(start) = content[cursor..].find(&start_marker) {
        let abs_start = cursor + start; // マーカーの絶対位置
        let block_start = abs_start + start_marker.len();
        if let Some(end) = content[block_start..].find(end_marker) {
            let block_end = block_start + end;
            blocks.push((abs_start, content[block_start..block_end].to_string()));
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

// ---------------------------------------------------------------------------
// ANVIL_PLAN / ANVIL_PLAN_UPDATE parsing (Issue #249)
// ---------------------------------------------------------------------------

/// Extract an ANVIL_PLAN or ANVIL_PLAN_UPDATE block from LLM output.
///
/// Returns the raw content of the first matching block, or `None`.
pub fn extract_plan_block(content: &str) -> Option<String> {
    extract_final_block(content, "ANVIL_PLAN")
        .or_else(|| extract_final_block_lenient(content, "ANVIL_PLAN"))
}

/// Extract an ANVIL_PLAN_UPDATE block from LLM output.
pub fn extract_plan_update_block(content: &str) -> Option<String> {
    extract_final_block(content, "ANVIL_PLAN_UPDATE")
        .or_else(|| extract_final_block_lenient(content, "ANVIL_PLAN_UPDATE"))
}

/// Parse a plan block into a list of `PlanItem`s.
///
/// Accepts markdown checkbox format:
/// ```text
/// - [ ] src/foo.rs: description of change
/// - [ ] src/bar.rs: another change
/// ```
///
/// Each line starting with `- [ ]` or `- [x]` is treated as a plan item.
/// The optional file path before `:` is extracted as a target file.
pub fn parse_plan_items(block: &str) -> Vec<crate::contracts::PlanItem> {
    let mut items = Vec::new();
    for line in block.lines() {
        let trimmed = line.trim();
        // Accept both "- [ ]" and "- [x]" prefixes; strip the checkbox
        let description = if let Some(rest) = trimmed
            .strip_prefix("- [ ] ")
            .or_else(|| trimmed.strip_prefix("- [x] "))
            .or_else(|| trimmed.strip_prefix("- [X] "))
        {
            rest.to_string()
        } else if trimmed.starts_with("- ") && !trimmed.is_empty() {
            // Also accept plain "- item" lines
            trimmed[2..].to_string()
        } else {
            continue;
        };

        if description.is_empty() {
            continue;
        }

        // Extract target file path(s): text before the first ":"
        let target_files = extract_target_files(&description);

        items.push(crate::contracts::PlanItem::new(description, target_files));
    }
    items
}

/// Extract file path(s) from a plan item description.
///
/// Supports both single file (`src/foo.rs: do stuff`) and comma-separated
/// multi-target (`src/a.rs, src/b.rs: update both`) formats.
///
/// Security: rejects paths containing `..` and absolute paths (starting with `/`).
/// Empty elements and whitespace-only paths are also filtered out.
fn extract_target_files(description: &str) -> Vec<String> {
    let colon_pos = match description.find(':') {
        Some(pos) => pos,
        None => return Vec::new(),
    };
    let candidate = description[..colon_pos].trim();

    // Split by comma for multi-target support
    let mut files = Vec::new();
    for part in candidate.split(',') {
        let path = part.trim();
        if path.is_empty() {
            continue;
        }
        // Security: reject paths with ".." (traversal)
        if path.contains("..") {
            continue;
        }
        // Security: reject absolute paths
        if path.starts_with('/') {
            continue;
        }
        // Heuristic: must contain a `/` or `.` to look like a file path
        if (path.contains('/') || path.contains('.')) && !files.contains(&path.to_string()) {
            files.push(path.to_string());
        }
    }
    files
}

// --- MCP tool description generation ---

use crate::mcp::McpToolInfo;

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn catalog_hides_web_tools_offline() {
        let prompt =
            tool_protocol_system_prompt(&[], None, &HashSet::new(), true, PromptTier::Full);
        assert!(
            !prompt.contains("- web.fetch:"),
            "offline prompt should not contain web.fetch catalog entry"
        );
        assert!(
            !prompt.contains("- web.search:"),
            "offline prompt should not contain web.search catalog entry"
        );
        assert!(
            prompt.contains("- agent.explore:"),
            "offline prompt should contain agent.explore catalog entry"
        );
        assert!(
            prompt.contains("- agent.plan:"),
            "offline prompt should contain agent.plan catalog entry"
        );
    }

    #[test]
    fn catalog_offline_with_restored_session() {
        let mut used_tools = HashSet::new();
        used_tools.insert("web.fetch".to_string());
        let prompt = tool_protocol_system_prompt(&[], None, &used_tools, true, PromptTier::Full);
        // web.fetch is now a basic tool (always included per Issue #114),
        // so the catalog entry should not exist but the basic description should.
        assert!(
            !prompt.contains("- web.fetch:"),
            "offline prompt with restored session should not show web.fetch catalog entry"
        );
        // web.fetch basic tool description IS present because it's always included
        assert!(
            prompt.contains("6. web.fetch"),
            "web.fetch should be present as a basic tool even in offline mode"
        );
    }

    #[test]
    fn catalog_strings_no_anvil_markers() {
        for (_, _, one_liner) in OPTIONAL_TOOLS {
            assert!(
                !one_liner.contains("ANVIL_TOOL"),
                "catalog one-liner should not contain ANVIL_TOOL: {}",
                one_liner
            );
            assert!(
                !one_liner.contains("ANVIL_FINAL"),
                "catalog one-liner should not contain ANVIL_FINAL: {}",
                one_liner
            );
        }
    }

    // ── Issue #157: estimate_pruned_message_count tests ───────────────

    #[test]
    fn estimate_pruned_zero_when_messages_fit() {
        let mut session = SessionRecord::new(std::path::PathBuf::from("/tmp/test"));
        for i in 0..3 {
            session.push_message(SessionMessage::new(
                MessageRole::User,
                "you",
                format!("msg {i}"),
            ));
        }
        // Large context window, small messages => no pruning
        let (pruned, _tokens) =
            BasicAgentLoop::estimate_pruned_message_count(&session, 128_000, 100, 1.0, None);
        assert_eq!(pruned, 0);
    }

    #[test]
    fn estimate_pruned_count_with_many_messages() {
        let mut session = SessionRecord::new(std::path::PathBuf::from("/tmp/test"));
        // Add many large messages to exceed a small budget
        for i in 0..50 {
            session.push_message(SessionMessage::new(
                MessageRole::User,
                "you",
                format!(
                    "message content that is somewhat long to consume tokens #{i} {}",
                    "x".repeat(200)
                ),
            ));
        }
        // Very small context window to force pruning
        let (pruned, _tokens) =
            BasicAgentLoop::estimate_pruned_message_count(&session, 512, 50, 1.0, None);
        assert!(pruned > 0, "should prune some messages with tiny budget");
        assert!(pruned < 50, "should keep at least one message");
    }

    #[test]
    fn estimate_pruned_zero_for_empty_session() {
        let session = SessionRecord::new(std::path::PathBuf::from("/tmp/test"));
        let (pruned, tokens) =
            BasicAgentLoop::estimate_pruned_message_count(&session, 128_000, 100, 1.0, None);
        assert_eq!(pruned, 0);
        assert_eq!(tokens, 0);
    }

    #[test]
    fn derive_context_budget_uses_override() {
        // When context_budget_override is Some, it should be used directly
        let budget = derive_context_budget(128_000, Some(4096));
        assert_eq!(budget, 4096);
    }

    #[test]
    fn derive_context_budget_falls_back_to_default() {
        // When context_budget_override is None, derive from context_window
        let budget = derive_context_budget(128_000, None);
        let expected = (128_000u32 / 4) as usize; // quarter of context_window
        assert_eq!(budget, expected);
    }

    #[test]
    fn derive_context_budget_default_clamps_minimum() {
        // context_window=2048: quarter=512, half=1024, clamp(256,1024) => 512
        let budget = derive_context_budget(2048, None);
        assert_eq!(budget, 512);

        // context_window=512: quarter=128, half=256, clamp(256,256) => 256
        let budget = derive_context_budget(512, None);
        assert_eq!(budget, 256);
    }

    // ============================================================
    // ANVIL_PLAN parser tests (Issue #249)
    // ============================================================

    #[test]
    fn extract_plan_block_basic() {
        let content = "Some text\n```ANVIL_PLAN\n- [ ] src/foo.rs: add function\n- [ ] src/bar.rs: fix bug\n```\nMore text";
        let block = extract_plan_block(content).expect("should extract");
        assert!(block.contains("src/foo.rs"));
        assert!(block.contains("src/bar.rs"));
    }

    #[test]
    fn extract_plan_block_lenient() {
        let content = "```ANVIL_PLAN\n- [ ] src/foo.rs: add function\n";
        let block = extract_plan_block(content).expect("should extract lenient");
        assert!(block.contains("src/foo.rs"));
    }

    #[test]
    fn extract_plan_block_none_when_absent() {
        let content = "Just regular text without any plan blocks.";
        assert!(extract_plan_block(content).is_none());
    }

    #[test]
    fn extract_plan_update_block_basic() {
        let content = "```ANVIL_PLAN_UPDATE\n- [ ] tests/new_test.rs: add test\n```";
        let block = extract_plan_update_block(content).expect("should extract");
        assert!(block.contains("tests/new_test.rs"));
    }

    #[test]
    fn parse_plan_items_checkbox_format() {
        let block = "- [ ] src/lib.rs: add module declaration\n- [ ] src/app/mod.rs: add field\n- [ ] tests/test.rs: add integration test";
        let items = parse_plan_items(block);
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].description, "src/lib.rs: add module declaration");
        assert_eq!(items[0].target_files, vec!["src/lib.rs"]);
        assert_eq!(items[1].target_files, vec!["src/app/mod.rs"]);
    }

    #[test]
    fn parse_plan_items_plain_dash_format() {
        let block = "- src/main.rs: entry point change\n- src/config.rs: add setting";
        let items = parse_plan_items(block);
        assert_eq!(items.len(), 2);
    }

    #[test]
    fn parse_plan_items_no_file_path() {
        let block = "- [ ] Add integration tests\n- [ ] Update documentation";
        let items = parse_plan_items(block);
        assert_eq!(items.len(), 2);
        assert!(items[0].target_files.is_empty());
    }

    #[test]
    fn parse_plan_items_ignores_non_items() {
        let block =
            "Plan:\n\n- [ ] src/foo.rs: change\n\nSome explanation\n\n- [ ] src/bar.rs: update";
        let items = parse_plan_items(block);
        assert_eq!(items.len(), 2);
    }

    #[test]
    fn parse_plan_items_empty_block() {
        let items = parse_plan_items("");
        assert!(items.is_empty());
    }

    #[test]
    fn parse_plan_items_with_checked_items() {
        let block = "- [x] src/done.rs: already done\n- [ ] src/todo.rs: still todo";
        let items = parse_plan_items(block);
        assert_eq!(items.len(), 2);
        // All parsed items start as Pending regardless of [x] in the source
        assert_eq!(items[0].status, crate::contracts::PlanItemStatus::Pending);
    }

    #[test]
    fn multi_target_parser_parses_comma_separated() {
        let block = "- [ ] src/a.rs, src/b.rs: update both";
        let items = parse_plan_items(block);
        assert_eq!(items.len(), 1);
        assert_eq!(
            items[0].target_files,
            vec!["src/a.rs".to_string(), "src/b.rs".to_string()]
        );
    }

    #[test]
    fn multi_target_parser_backward_compatible() {
        // Single file format must still work
        let block = "- [ ] src/main.rs: entry point change";
        let items = parse_plan_items(block);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].target_files, vec!["src/main.rs".to_string()]);
    }

    #[test]
    fn multi_target_parser_rejects_dotdot() {
        let block = "- [ ] src/../etc/passwd, src/a.rs: malicious path";
        let items = parse_plan_items(block);
        assert_eq!(items.len(), 1);
        // The .. path should be excluded, only src/a.rs should remain
        assert_eq!(items[0].target_files, vec!["src/a.rs".to_string()]);
    }

    #[test]
    fn multi_target_parser_rejects_absolute_path() {
        let block = "- [ ] /etc/passwd, src/a.rs: absolute path test";
        let items = parse_plan_items(block);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].target_files, vec!["src/a.rs".to_string()]);
    }

    #[test]
    fn multi_target_parser_trims_whitespace() {
        let block = "- [ ] src/a.rs , src/b.rs : update both";
        let items = parse_plan_items(block);
        assert_eq!(items.len(), 1);
        assert_eq!(
            items[0].target_files,
            vec!["src/a.rs".to_string(), "src/b.rs".to_string()]
        );
    }
}
