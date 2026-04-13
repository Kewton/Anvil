//! OpenAI-compatible provider client.
//!
//! Works with the standard `/v1/chat/completions` endpoint used by
//! OpenAI, Azure OpenAI, LM Studio, and other compatible servers.

use super::transport::{
    HttpTransport, ReqwestHttpTransport, RetryTransport, sanitize_error_message,
};
use super::{
    AgentEvent, AssistantToolCallRecord, ImageContent, ProviderClient, ProviderEvent,
    ProviderTurnError, ProviderTurnRequest, build_provider_done_event,
    build_provider_done_event_with_tool_calls,
};
use crate::config::EffectiveConfig;
use crate::contracts::InferencePerformanceView;
use crate::tooling::{NativeToolDef, ToolCallRequest, ToolInput};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// Client for OpenAI-compatible chat completion APIs.
///
/// Generic over [`HttpTransport`] for testability.
pub struct OpenAiCompatibleProviderClient<T = RetryTransport<ReqwestHttpTransport>> {
    base_url: String,
    api_key: Option<String>,
    transport: T,
}

#[derive(Debug, Clone, Serialize)]
struct OpenAiChatRequest {
    model: String,
    messages: Vec<OpenAiChatMessage>,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream_options: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<OpenAiToolDef>>,
}

/// OpenAI tool definition for function calling.
#[derive(Debug, Clone, Serialize)]
struct OpenAiToolDef {
    #[serde(rename = "type")]
    tool_type: String,
    function: OpenAiToolFunctionDef,
}

/// Function definition within an OpenAI tool definition.
#[derive(Debug, Clone, Serialize)]
struct OpenAiToolFunctionDef {
    name: String,
    description: String,
    parameters: serde_json::Value,
}

/// Request message: content is `Value` to support both plain text and
/// multimodal (text + image_url) arrays.
///
/// Extended for native tool calling (Issue #373):
/// - `tool_calls`: present on assistant messages that invoked tools
/// - `tool_call_id`: present on tool-result messages
#[derive(Debug, Clone, Serialize)]
struct OpenAiChatMessage {
    role: String,
    content: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<OpenAiToolCallMsg>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
}

/// Tool call record for assistant messages in the request payload.
#[derive(Debug, Clone, Serialize)]
struct OpenAiToolCallMsg {
    id: String,
    #[serde(rename = "type")]
    call_type: String,
    function: OpenAiToolCallMsgFunction,
}

/// Function details within a tool call message.
#[derive(Debug, Clone, Serialize)]
struct OpenAiToolCallMsgFunction {
    name: String,
    arguments: String,
}

/// Response message: content is always a plain string from the API.
#[derive(Debug, Clone, Deserialize)]
struct OpenAiResponseMessage {
    #[allow(dead_code)]
    role: String,
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_calls: Vec<OpenAiToolCall>,
}

#[derive(Debug, Clone, Deserialize)]
struct OpenAiUsage {
    #[serde(default)]
    prompt_tokens: Option<u64>,
    #[serde(default)]
    completion_tokens: Option<u64>,
    #[serde(default)]
    #[allow(dead_code)]
    total_tokens: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
struct OpenAiChatResponse {
    choices: Vec<OpenAiChoice>,
    #[serde(default)]
    usage: Option<OpenAiUsage>,
}

#[derive(Debug, Clone, Deserialize)]
struct OpenAiStreamChunk {
    choices: Vec<OpenAiDeltaChoice>,
    #[serde(default)]
    usage: Option<OpenAiUsage>,
}

#[derive(Debug, Clone, Deserialize)]
struct OpenAiDeltaChoice {
    #[serde(default)]
    delta: OpenAiDeltaMessage,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct OpenAiDeltaMessage {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_calls: Vec<OpenAiDeltaToolCall>,
}

#[derive(Debug, Clone, Deserialize)]
struct OpenAiToolCall {
    #[serde(default)]
    id: String,
    function: OpenAiToolFunction,
}

#[derive(Debug, Clone, Deserialize)]
struct OpenAiToolFunction {
    name: String,
    arguments: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct OpenAiDeltaToolCall {
    #[serde(default)]
    index: usize,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    function: Option<OpenAiDeltaToolFunction>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct OpenAiDeltaToolFunction {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    arguments: Option<String>,
}

#[derive(Debug, Clone, Default)]
struct StreamingToolCallAccumulator {
    id: String,
    name: String,
    arguments: String,
}

#[derive(Debug, Clone, Deserialize)]
struct OpenAiChoice {
    message: OpenAiResponseMessage,
}

#[derive(Debug, Clone, Deserialize)]
struct OpenAiErrorEnvelope {
    error: OpenAiErrorBody,
}

#[derive(Debug, Clone, Deserialize)]
struct OpenAiErrorBody {
    message: String,
}

/// Build the `content` field for an OpenAI chat message.
///
/// When images are present, returns a JSON array containing a text part
/// followed by `image_url` parts (base64 data URIs).  Otherwise returns
/// a plain JSON string.
fn build_openai_content(text: &str, images: Option<&[ImageContent]>) -> Value {
    match images {
        Some(imgs) if !imgs.is_empty() => {
            let mut parts = vec![serde_json::json!({
                "type": "text",
                "text": text,
            })];
            for img in imgs {
                parts.push(serde_json::json!({
                    "type": "image_url",
                    "image_url": {
                        "url": format!("data:{};base64,{}", img.mime_type, img.base64),
                    },
                }));
            }
            Value::Array(parts)
        }
        _ => Value::String(text.to_string()),
    }
}

fn openai_message_role_and_content(message: &super::ProviderMessage) -> (String, Value) {
    match message.role {
        super::ProviderMessageRole::System => (
            "system".to_string(),
            build_openai_content(&message.content, message.images.as_deref()),
        ),
        super::ProviderMessageRole::User => (
            "user".to_string(),
            build_openai_content(&message.content, message.images.as_deref()),
        ),
        super::ProviderMessageRole::Assistant => (
            "assistant".to_string(),
            build_openai_content(&message.content, message.images.as_deref()),
        ),
        super::ProviderMessageRole::Tool => (
            // We do not send native tool_call/tool_call_id pairs on follow-up
            // turns, so OpenAI-compatible backends can underweight orphaned
            // `tool` role messages. Flatten tool outputs into explicit user
            // context instead.
            "user".to_string(),
            build_openai_content(
                &format!("Tool result:\n{}", message.content),
                message.images.as_deref(),
            ),
        ),
    }
}

/// Build a full `OpenAiChatMessage` from a `ProviderMessage`, handling native
/// tool calling fields when present.
///
/// In native mode (`native_tools` is `true`):
/// - Assistant messages with `assistant_tool_calls` → `role: "assistant"` + `tool_calls` array
/// - Tool messages with `tool_call_id` → `role: "tool"` + `tool_call_id`
/// - Otherwise → fallback to standard role/content mapping
fn build_openai_chat_message(
    message: &super::ProviderMessage,
    native_tools: bool,
) -> OpenAiChatMessage {
    // Native mode: assistant with tool_calls
    if native_tools {
        if let Some(ref tc_records) = message.assistant_tool_calls {
            let tool_calls: Vec<OpenAiToolCallMsg> = tc_records
                .iter()
                .map(|record| OpenAiToolCallMsg {
                    id: record.id.clone(),
                    call_type: "function".to_string(),
                    function: OpenAiToolCallMsgFunction {
                        name: record.function_name.clone(),
                        arguments: record.arguments.clone(),
                    },
                })
                .collect();
            return OpenAiChatMessage {
                role: "assistant".to_string(),
                content: build_openai_content(&message.content, message.images.as_deref()),
                tool_calls: Some(tool_calls),
                tool_call_id: None,
            };
        }

        // Native mode: tool result with tool_call_id
        if let Some(ref tc_id) = message.tool_call_id {
            return OpenAiChatMessage {
                role: "tool".to_string(),
                content: Value::String(message.content.clone()),
                tool_calls: None,
                tool_call_id: Some(tc_id.clone()),
            };
        }
    }

    // Fallback: standard role/content mapping
    let (role, content) = openai_message_role_and_content(message);
    OpenAiChatMessage {
        role,
        content,
        tool_calls: None,
        tool_call_id: None,
    }
}

/// Convert `NativeToolDef` to `OpenAiToolDef` for the request payload.
fn native_to_openai_tool_def(native: &NativeToolDef) -> OpenAiToolDef {
    OpenAiToolDef {
        tool_type: "function".to_string(),
        function: OpenAiToolFunctionDef {
            name: native.name.clone(),
            description: native.description.clone(),
            parameters: native.parameters.clone(),
        },
    }
}

/// Extract InferencePerformanceView from OpenAI usage.
fn extract_openai_performance(usage: &Option<OpenAiUsage>) -> Option<InferencePerformanceView> {
    let usage = usage.as_ref()?;
    Some(InferencePerformanceView {
        eval_tokens: usage.completion_tokens,
        prompt_tokens: usage.prompt_tokens,
        ..Default::default()
    })
}

fn normalize_openai_tool_name(name: &str) -> String {
    match name {
        "file_read" => "file.read".to_string(),
        "file_write" => "file.write".to_string(),
        "file_edit" => "file.edit".to_string(),
        "file_search" => "file.search".to_string(),
        "file_edit_anchor" => "file.edit_anchor".to_string(),
        "file_rewrite" => "file.rewrite".to_string(),
        "shell_exec" => "shell.exec".to_string(),
        "web_fetch" => "web.fetch".to_string(),
        "web_search" => "web.search".to_string(),
        "agent_explore" => "agent.explore".to_string(),
        "agent_plan" => "agent.plan".to_string(),
        "agent_fix_slice" => "agent.fix_slice".to_string(),
        "git_status" => "git.status".to_string(),
        "git_diff" => "git.diff".to_string(),
        "git_log" => "git.log".to_string(),
        _ => name.to_string(),
    }
}

fn default_tool_call_id(tool_name: &str, index: usize) -> String {
    format!("call_{}_{}", tool_name.replace('.', "_"), index)
}

/// Intermediate parsed representation of an OpenAI tool call.
///
/// Shared between the fallback path (`openai_tool_call_to_anvil_block`) and
/// the native path (`native_tool_call_to_request`).
#[derive(Debug, Clone)]
struct ParsedToolCall {
    id: String,
    tool_name: String,
    arguments: serde_json::Value,
}

/// Parse an OpenAI tool call into a `ParsedToolCall`.
///
/// Normalizes the function name (e.g. `file_read` -> `file.read`),
/// assigns a default id when the provider omits one, and validates
/// that arguments are a JSON object.
fn parse_openai_tool_call(
    tool_call: &OpenAiToolCall,
    index: usize,
) -> Result<ParsedToolCall, ProviderTurnError> {
    let raw_name = tool_call.function.name.trim();
    if raw_name.is_empty() {
        return Err(ProviderTurnError::Backend(format!(
            "openai tool_call at index {index} is missing function.name"
        )));
    }

    let tool_name = normalize_openai_tool_name(raw_name);
    let args_value: Value = serde_json::from_str(&tool_call.function.arguments).map_err(|err| {
        ProviderTurnError::Backend(format!(
            "invalid openai tool_call arguments for '{tool_name}': {err}"
        ))
    })?;

    if !args_value.is_object() {
        return Err(ProviderTurnError::Backend(format!(
            "openai tool_call arguments for '{tool_name}' must be a JSON object"
        )));
    }

    let id = if tool_call.id.trim().is_empty() {
        default_tool_call_id(&tool_name, index)
    } else {
        tool_call.id.clone()
    };

    Ok(ParsedToolCall {
        id,
        tool_name,
        arguments: args_value,
    })
}

/// Convert a `ParsedToolCall` into a `ToolCallRequest` for native execution.
fn native_tool_call_to_request(
    parsed: &ParsedToolCall,
) -> Result<ToolCallRequest, ProviderTurnError> {
    let input = ToolInput::from_json(&parsed.tool_name, &parsed.arguments).map_err(|err| {
        ProviderTurnError::Backend(format!(
            "failed to parse native tool_call '{}': {err}",
            parsed.tool_name
        ))
    })?;
    Ok(ToolCallRequest::new(
        parsed.id.clone(),
        parsed.tool_name.clone(),
        input,
    ))
}

/// Convert a `ParsedToolCall` into an `AssistantToolCallRecord` for replay.
fn parsed_to_assistant_record(parsed: &ParsedToolCall) -> AssistantToolCallRecord {
    // Reverse-normalize: tool_name uses dots (file.read), but OpenAI API uses underscores
    let function_name = parsed.tool_name.replace('.', "_");
    AssistantToolCallRecord {
        id: parsed.id.clone(),
        function_name,
        arguments: parsed.arguments.to_string(),
    }
}

fn openai_tool_call_to_anvil_block(
    tool_call: &OpenAiToolCall,
    index: usize,
) -> Result<String, ProviderTurnError> {
    let parsed = parse_openai_tool_call(tool_call, index)?;

    let Some(args_object) = parsed.arguments.as_object() else {
        return Err(ProviderTurnError::Backend(format!(
            "openai tool_call arguments for '{}' must be a JSON object",
            parsed.tool_name
        )));
    };

    let mut payload = serde_json::Map::new();
    payload.insert("id".to_string(), Value::String(parsed.id));
    payload.insert("tool".to_string(), Value::String(parsed.tool_name));
    for (key, value) in args_object {
        payload.insert(key.clone(), value.clone());
    }

    let json = serde_json::to_string(&Value::Object(payload)).map_err(|err| {
        ProviderTurnError::Backend(format!(
            "failed to encode synthetic ANVIL_TOOL block: {err}"
        ))
    })?;

    Ok(format!("```ANVIL_TOOL\n{json}\n```"))
}

fn build_native_tool_calls_content(
    content: Option<&str>,
    tool_calls: &[OpenAiToolCall],
) -> Result<String, ProviderTurnError> {
    let mut parts = Vec::new();
    if let Some(content) = content
        && !content.trim().is_empty()
    {
        parts.push(content.to_string());
    }

    for (index, tool_call) in tool_calls.iter().enumerate() {
        parts.push(openai_tool_call_to_anvil_block(tool_call, index)?);
    }

    Ok(parts.join("\n"))
}

/// Convert finalized tool calls into native `ToolCallRequest` objects and
/// `AssistantToolCallRecord`s for the Done event.
fn finalize_native_tool_calls(
    tool_calls: &[OpenAiToolCall],
) -> Result<(Vec<ToolCallRequest>, Vec<AssistantToolCallRecord>), ProviderTurnError> {
    let mut requests = Vec::with_capacity(tool_calls.len());
    let mut records = Vec::with_capacity(tool_calls.len());
    for (index, tc) in tool_calls.iter().enumerate() {
        let parsed = parse_openai_tool_call(tc, index)?;
        requests.push(native_tool_call_to_request(&parsed)?);
        records.push(parsed_to_assistant_record(&parsed));
    }
    Ok((requests, records))
}

fn merge_delta_tool_calls(
    accumulators: &mut BTreeMap<usize, StreamingToolCallAccumulator>,
    delta_tool_calls: &[OpenAiDeltaToolCall],
) {
    for delta_tool_call in delta_tool_calls {
        let accumulator = accumulators.entry(delta_tool_call.index).or_default();
        if let Some(id) = &delta_tool_call.id {
            accumulator.id.push_str(id);
        }
        if let Some(function) = &delta_tool_call.function {
            if let Some(name) = &function.name {
                accumulator.name.push_str(name);
            }
            if let Some(arguments) = &function.arguments {
                accumulator.arguments.push_str(arguments);
            }
        }
    }
}

fn finalize_streaming_tool_calls(
    accumulators: BTreeMap<usize, StreamingToolCallAccumulator>,
) -> Result<Vec<OpenAiToolCall>, ProviderTurnError> {
    let mut finalized = Vec::with_capacity(accumulators.len());
    for (index, accumulator) in accumulators {
        if accumulator.name.trim().is_empty() {
            return Err(ProviderTurnError::Backend(format!(
                "openai streaming tool_call at index {index} is missing function.name"
            )));
        }
        finalized.push(OpenAiToolCall {
            id: accumulator.id,
            function: OpenAiToolFunction {
                name: accumulator.name,
                arguments: accumulator.arguments,
            },
        });
    }
    Ok(finalized)
}

fn build_openai_api_url(base_url: &str, endpoint: &str) -> String {
    let normalized_base = base_url.trim_end_matches('/');
    let normalized_endpoint = endpoint.trim_start_matches('/');
    if normalized_base.ends_with("/v1") {
        format!("{normalized_base}/{normalized_endpoint}")
    } else {
        format!("{normalized_base}/v1/{normalized_endpoint}")
    }
}

impl OpenAiCompatibleProviderClient {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            api_key: None,
            transport: RetryTransport::new(ReqwestHttpTransport::new()),
        }
    }

    pub fn from_config(config: &EffectiveConfig) -> Self {
        Self {
            base_url: config.runtime.provider_url.clone(),
            api_key: config.runtime.api_key.clone(),
            transport: RetryTransport::new(ReqwestHttpTransport::new()),
        }
    }
}

impl<T> OpenAiCompatibleProviderClient<T> {
    pub fn with_transport(base_url: impl Into<String>, transport: T) -> Self {
        Self {
            base_url: base_url.into(),
            api_key: None,
            transport,
        }
    }

    pub fn with_api_key(mut self, key: impl Into<String>) -> Self {
        self.api_key = Some(key.into());
        self
    }

    /// Build an OpenAI chat request from a provider turn request.
    ///
    /// Note: `request.context_window` (Ollama `num_ctx`) is intentionally not
    /// mapped here. The OpenAI chat completions API does not support a
    /// per-request context window override.
    ///
    /// When `request.tools` is `Some`, native tool definitions are included
    /// in the request and messages use native tool calling roles/fields.
    fn build_chat_request(
        request: &ProviderTurnRequest,
        stream_options: Option<serde_json::Value>,
    ) -> OpenAiChatRequest {
        let native_tools = request.tools.is_some();
        OpenAiChatRequest {
            model: request.model.clone(),
            messages: request
                .messages
                .iter()
                .map(|m| build_openai_chat_message(m, native_tools))
                .collect(),
            stream: request.stream,
            stream_options,
            max_tokens: request.max_output_tokens,
            temperature: request.temperature,
            tools: request
                .tools
                .as_ref()
                .map(|tools| tools.iter().map(native_to_openai_tool_def).collect()),
        }
    }
}

impl<T: HttpTransport> OpenAiCompatibleProviderClient<T> {
    /// Check connectivity to the OpenAI-compatible server by requesting `/v1/models`.
    ///
    /// If an API key is configured, it is sent as an `Authorization` header
    /// (without `Bearer` prefix, matching the existing code pattern in
    /// `send_chat_request`).
    pub fn health_check(&self) -> Result<(), ProviderTurnError> {
        let url = build_openai_api_url(&self.base_url, "models");
        let headers: Vec<(&str, &str)> = self
            .api_key
            .as_deref()
            .map(|key| vec![("Authorization", key)])
            .unwrap_or_default();
        match self.transport.get_with_headers(&url, &headers) {
            Ok(response) => {
                let status_code = response.status_code;
                match status_code {
                    401 | 403 => Err(ProviderTurnError::AuthenticationFailed {
                        status_code,
                        message: sanitize_error_message(&format!(
                            "HTTP {} from {}",
                            status_code, self.base_url
                        )),
                    }),
                    s if s >= 500 => Err(ProviderTurnError::ServerError {
                        status_code: s,
                        message: sanitize_error_message(&format!(
                            "HTTP {} from {}",
                            s, self.base_url
                        )),
                    }),
                    _ => Ok(()),
                }
            }
            Err(e) => Err(e),
        }
    }

    fn send_chat_request(
        &self,
        request: &ProviderTurnRequest,
    ) -> Result<Vec<ProviderEvent>, ProviderTurnError> {
        let chat_request = Self::build_chat_request(request, None);

        let request_body = serde_json::to_vec(&chat_request).map_err(|err| {
            ProviderTurnError::Backend(format!("failed to encode openai request: {err}"))
        })?;
        let url = build_openai_api_url(&self.base_url, "chat/completions");

        let mut headers = Vec::new();
        if let Some(api_key) = &self.api_key {
            headers.push(("Authorization", api_key.as_str()));
        }
        let response = self
            .transport
            .post_json_with_headers(&url, &request_body, &headers)?;
        if response.status_code != 200 {
            let body_text = normalize_openai_error(&response.body);
            let message = sanitize_error_message(&format!(
                "openai request failed with status {}: {}",
                response.status_code,
                body_text.trim()
            ));
            return Err(match response.status_code {
                401 | 403 => ProviderTurnError::AuthenticationFailed {
                    status_code: response.status_code,
                    message,
                },
                _ => ProviderTurnError::Backend(message),
            });
        }

        let is_native_mode = request.tools.is_some();

        if request.stream && looks_like_sse_stream(&response.body) {
            return parse_openai_sse_response(&response.body, is_native_mode);
        }

        let parsed: OpenAiChatResponse = serde_json::from_slice(&response.body)
            .map_err(|err| ProviderTurnError::Backend(format!("invalid openai response: {err}")))?;

        let perf = extract_openai_performance(&parsed.usage);
        let choice = parsed.choices.first().ok_or_else(|| {
            ProviderTurnError::Backend("openai response contained no choices".to_string())
        })?;
        let content = choice.message.content.clone().unwrap_or_default();
        if !choice.message.tool_calls.is_empty() {
            if is_native_mode {
                // Native mode: emit ToolCallRequest objects via Done.tool_calls
                let (requests, records) = finalize_native_tool_calls(&choice.message.tool_calls)?;
                return Ok(vec![ProviderEvent::Agent(
                    build_provider_done_event_with_tool_calls(&content, perf, requests, records),
                )]);
            }
            // Fallback mode: convert to ANVIL_TOOL text blocks
            let assistant_message =
                build_native_tool_calls_content(Some(&content), &choice.message.tool_calls)?;
            return Ok(vec![ProviderEvent::Agent(build_provider_done_event(
                &assistant_message,
                perf,
            ))]);
        }

        Ok(vec![
            ProviderEvent::TokenDelta(content.clone()),
            ProviderEvent::Agent(build_provider_done_event(&content, perf)),
        ])
    }
}

impl<T: HttpTransport> ProviderClient for OpenAiCompatibleProviderClient<T> {
    fn stream_turn(
        &self,
        request: &ProviderTurnRequest,
        emit: &mut dyn FnMut(ProviderEvent),
    ) -> Result<(), ProviderTurnError> {
        if request.stream {
            return self.stream_turn_sse(request, emit);
        }
        for event in self.send_chat_request(request)? {
            emit(event);
        }
        Ok(())
    }
}

/// Build and emit a Done event from finalized streaming tool calls.
///
/// In native mode (`is_native`), emits `Done.tool_calls` with parsed requests.
/// In fallback mode, emits ANVIL_TOOL text blocks in `assistant_message`.
fn emit_done_with_tool_calls(
    content: &str,
    tool_calls: Vec<OpenAiToolCall>,
    perf: Option<InferencePerformanceView>,
    is_native: bool,
    emit: &mut dyn FnMut(ProviderEvent),
) -> Result<(), ProviderTurnError> {
    if is_native {
        let (requests, records) = finalize_native_tool_calls(&tool_calls)?;
        emit(ProviderEvent::Agent(
            build_provider_done_event_with_tool_calls(content, perf, requests, records),
        ));
    } else {
        let assistant_message = build_native_tool_calls_content(Some(content), &tool_calls)?;
        emit(ProviderEvent::Agent(build_provider_done_event(
            &assistant_message,
            perf,
        )));
    }
    Ok(())
}

impl<T: HttpTransport> OpenAiCompatibleProviderClient<T> {
    fn stream_turn_sse(
        &self,
        request: &ProviderTurnRequest,
        emit: &mut dyn FnMut(ProviderEvent),
    ) -> Result<(), ProviderTurnError> {
        let chat_request =
            Self::build_chat_request(request, Some(serde_json::json!({ "include_usage": true })));

        let request_body = serde_json::to_vec(&chat_request).map_err(|err| {
            ProviderTurnError::Backend(format!("failed to encode openai request: {err}"))
        })?;
        let url = build_openai_api_url(&self.base_url, "chat/completions");
        tracing::debug!(
            model = %request.model,
            messages = request.messages.len(),
            stream = request.stream,
            "sending openai chat request"
        );

        let mut headers = Vec::new();
        if let Some(api_key) = &self.api_key {
            headers.push(("Authorization", api_key.as_str()));
        }

        let is_native_mode = request.tools.is_some();
        let mut content = String::new();
        let mut emitted_done = false;
        let mut had_error: Option<ProviderTurnError> = None;
        let mut stream_usage: Option<OpenAiUsage> = None;
        let mut streaming_tool_calls = BTreeMap::new();
        let mut saw_native_tool_calls = false;

        self.transport
            .stream_lines(&url, &request_body, &headers, &mut |line| {
                if had_error.is_some() || emitted_done {
                    return;
                }
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    return;
                }
                if !trimmed.starts_with("data: ") {
                    // Not SSE — try as a regular OpenAI JSON response (fallback)
                    if let Ok(parsed) = serde_json::from_str::<OpenAiChatResponse>(trimmed) {
                        if let Some(choice) = parsed.choices.first() {
                            let perf = extract_openai_performance(&parsed.usage);
                            let msg_content = choice.message.content.clone().unwrap_or_default();
                            if !choice.message.tool_calls.is_empty() {
                                saw_native_tool_calls = true;
                                if let Err(err) = emit_done_with_tool_calls(
                                    &msg_content,
                                    choice.message.tool_calls.clone(),
                                    perf,
                                    is_native_mode,
                                    emit,
                                ) {
                                    had_error = Some(err);
                                    return;
                                }
                            } else {
                                content.push_str(&msg_content);
                                emit(ProviderEvent::TokenDelta(msg_content));
                                emit(ProviderEvent::Agent(build_provider_done_event(
                                    &content, perf,
                                )));
                            }
                            emitted_done = true;
                        }
                        return;
                    }
                    // Check if error envelope
                    if let Ok(parsed) = serde_json::from_str::<OpenAiErrorEnvelope>(trimmed) {
                        had_error = Some(ProviderTurnError::Backend(parsed.error.message));
                    }
                    return;
                }
                let payload = &trimmed[6..];
                if payload == "[DONE]" {
                    if !emitted_done {
                        let perf = extract_openai_performance(&stream_usage);
                        if saw_native_tool_calls {
                            match finalize_streaming_tool_calls(std::mem::take(
                                &mut streaming_tool_calls,
                            )) {
                                Ok(finalized) => {
                                    if let Err(err) = emit_done_with_tool_calls(
                                        &content,
                                        finalized,
                                        perf,
                                        is_native_mode,
                                        emit,
                                    ) {
                                        had_error = Some(err);
                                        return;
                                    }
                                }
                                Err(err) => {
                                    had_error = Some(err);
                                    return;
                                }
                            }
                        } else {
                            emit(ProviderEvent::Agent(build_provider_done_event(
                                &content, perf,
                            )));
                        }
                        emitted_done = true;
                    }
                    return;
                }

                match serde_json::from_str::<OpenAiStreamChunk>(payload) {
                    Ok(chunk) => {
                        // Capture usage from final SSE chunk (stream_options: include_usage)
                        if let Some(usage) = chunk.usage {
                            stream_usage = Some(usage);
                        }
                        for choice in chunk.choices {
                            if let Some(delta) = choice.delta.content {
                                content.push_str(&delta);
                                emit(ProviderEvent::TokenDelta(delta));
                            }
                            if !choice.delta.tool_calls.is_empty() {
                                saw_native_tool_calls = true;
                                merge_delta_tool_calls(
                                    &mut streaming_tool_calls,
                                    &choice.delta.tool_calls,
                                );
                            }
                            if choice.finish_reason.is_some() && !emitted_done {
                                let perf = extract_openai_performance(&stream_usage);
                                if saw_native_tool_calls {
                                    match finalize_streaming_tool_calls(std::mem::take(
                                        &mut streaming_tool_calls,
                                    )) {
                                        Ok(finalized) => {
                                            if let Err(err) = emit_done_with_tool_calls(
                                                &content,
                                                finalized,
                                                perf,
                                                is_native_mode,
                                                emit,
                                            ) {
                                                had_error = Some(err);
                                                return;
                                            }
                                        }
                                        Err(err) => {
                                            had_error = Some(err);
                                            return;
                                        }
                                    }
                                } else {
                                    emit(ProviderEvent::Agent(build_provider_done_event(
                                        &content, perf,
                                    )));
                                }
                                emitted_done = true;
                            }
                        }
                    }
                    Err(err) => {
                        had_error = Some(ProviderTurnError::Backend(format!(
                            "invalid openai stream chunk: {err}"
                        )));
                    }
                }
            })?;

        if let Some(err) = had_error {
            tracing::error!(error = %err, "openai provider request failed");
            return Err(err);
        }
        if !emitted_done {
            let perf = extract_openai_performance(&stream_usage);
            if saw_native_tool_calls {
                let finalized = finalize_streaming_tool_calls(streaming_tool_calls)?;
                emit_done_with_tool_calls(&content, finalized, perf, is_native_mode, emit)?;
            } else {
                emit(ProviderEvent::Agent(build_provider_done_event(
                    &content, perf,
                )));
            }
        }
        Ok(())
    }
}

fn parse_openai_sse_response(
    body: &[u8],
    is_native_mode: bool,
) -> Result<Vec<ProviderEvent>, ProviderTurnError> {
    let text = String::from_utf8_lossy(body);
    let mut content = String::new();
    let mut events = Vec::new();
    let mut streaming_tool_calls = BTreeMap::new();
    let mut saw_native_tool_calls = false;

    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || !trimmed.starts_with("data: ") {
            continue;
        }
        let payload = &trimmed[6..];
        if payload == "[DONE]" {
            break;
        }

        let chunk: OpenAiStreamChunk = serde_json::from_str(payload).map_err(|err| {
            ProviderTurnError::Backend(format!("invalid openai stream chunk: {err}"))
        })?;

        for choice in chunk.choices {
            if let Some(delta) = choice.delta.content {
                content.push_str(&delta);
                events.push(ProviderEvent::TokenDelta(delta));
            }
            if !choice.delta.tool_calls.is_empty() {
                saw_native_tool_calls = true;
                merge_delta_tool_calls(&mut streaming_tool_calls, &choice.delta.tool_calls);
            }
            if choice.finish_reason.is_some() {
                if saw_native_tool_calls {
                    let finalized =
                        finalize_streaming_tool_calls(std::mem::take(&mut streaming_tool_calls))?;
                    if is_native_mode {
                        let (requests, records) = finalize_native_tool_calls(&finalized)?;
                        events.push(ProviderEvent::Agent(
                            build_provider_done_event_with_tool_calls(
                                &content, None, requests, records,
                            ),
                        ));
                    } else {
                        let assistant_message =
                            build_native_tool_calls_content(Some(&content), &finalized)?;
                        events.push(ProviderEvent::Agent(build_provider_done_event(
                            &assistant_message,
                            None,
                        )));
                    }
                } else {
                    events.push(ProviderEvent::Agent(build_provider_done_event(
                        &content, None,
                    )));
                }
            }
        }
    }

    if events
        .iter()
        .all(|event| !matches!(event, ProviderEvent::Agent(AgentEvent::Done { .. })))
    {
        if saw_native_tool_calls {
            let finalized = finalize_streaming_tool_calls(streaming_tool_calls)?;
            if is_native_mode {
                let (requests, records) = finalize_native_tool_calls(&finalized)?;
                events.push(ProviderEvent::Agent(
                    build_provider_done_event_with_tool_calls(&content, None, requests, records),
                ));
            } else {
                let assistant_message =
                    build_native_tool_calls_content(Some(&content), &finalized)?;
                events.push(ProviderEvent::Agent(build_provider_done_event(
                    &assistant_message,
                    None,
                )));
            }
        } else {
            events.push(ProviderEvent::Agent(build_provider_done_event(
                &content, None,
            )));
        }
    }

    Ok(events)
}

fn normalize_openai_error(body: &[u8]) -> String {
    serde_json::from_slice::<OpenAiErrorEnvelope>(body)
        .map(|parsed| parsed.error.message)
        .unwrap_or_else(|_| String::from_utf8_lossy(body).to_string())
}

fn looks_like_sse_stream(body: &[u8]) -> bool {
    String::from_utf8_lossy(body)
        .lines()
        .any(|line| line.trim_start().starts_with("data: "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::{ProviderMessage, ProviderMessageRole};

    #[test]
    fn build_chat_request_flattens_tool_messages_into_user_context() {
        let request = ProviderTurnRequest::new(
            "test-model".to_string(),
            vec![
                ProviderMessage::new(ProviderMessageRole::System, "system prompt"),
                ProviderMessage::new(
                    ProviderMessageRole::Tool,
                    "[tool result: file.read] read ok",
                ),
            ],
            false,
        );

        let chat_request = OpenAiCompatibleProviderClient::<()>::build_chat_request(&request, None);

        assert_eq!(chat_request.messages[1].role, "user");
        assert_eq!(
            chat_request.messages[1].content,
            Value::String("Tool result:\n[tool result: file.read] read ok".to_string())
        );
    }

    // -----------------------------------------------------------------------
    // Phase 2: Native tool calling tests (Issue #373)
    // -----------------------------------------------------------------------

    #[test]
    fn build_chat_request_includes_tools_when_present() {
        let tools = vec![NativeToolDef {
            name: "file_read".to_string(),
            description: "Read a file".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" }
                },
                "required": ["path"]
            }),
        }];

        let request = ProviderTurnRequest::new(
            "test-model".to_string(),
            vec![ProviderMessage::new(ProviderMessageRole::User, "hello")],
            false,
        )
        .with_tools(tools);

        let chat_request = OpenAiCompatibleProviderClient::<()>::build_chat_request(&request, None);

        let openai_tools = chat_request.tools.expect("tools should be present");
        assert_eq!(openai_tools.len(), 1);
        assert_eq!(openai_tools[0].tool_type, "function");
        assert_eq!(openai_tools[0].function.name, "file_read");
        assert_eq!(openai_tools[0].function.description, "Read a file");
    }

    #[test]
    fn build_chat_request_omits_tools_when_none() {
        let request = ProviderTurnRequest::new(
            "test-model".to_string(),
            vec![ProviderMessage::new(ProviderMessageRole::User, "hello")],
            false,
        );

        let chat_request = OpenAiCompatibleProviderClient::<()>::build_chat_request(&request, None);
        assert!(chat_request.tools.is_none());
    }

    #[test]
    fn build_chat_request_serializes_tools_field_correctly() {
        let tools = vec![NativeToolDef {
            name: "file_read".to_string(),
            description: "Read a file".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" }
                },
                "required": ["path"]
            }),
        }];

        let request = ProviderTurnRequest::new(
            "test-model".to_string(),
            vec![ProviderMessage::new(ProviderMessageRole::User, "hello")],
            false,
        )
        .with_tools(tools);

        let chat_request = OpenAiCompatibleProviderClient::<()>::build_chat_request(&request, None);
        let json = serde_json::to_value(&chat_request).expect("should serialize");

        // Verify tools array is correctly shaped for OpenAI API
        let tools_array = json["tools"].as_array().expect("tools should be array");
        assert_eq!(tools_array.len(), 1);
        assert_eq!(tools_array[0]["type"], "function");
        assert_eq!(tools_array[0]["function"]["name"], "file_read");
        assert_eq!(tools_array[0]["function"]["description"], "Read a file");
        assert!(tools_array[0]["function"]["parameters"].is_object());
    }

    #[test]
    fn build_chat_request_without_tools_omits_tools_in_json() {
        let request = ProviderTurnRequest::new(
            "test-model".to_string(),
            vec![ProviderMessage::new(ProviderMessageRole::User, "hello")],
            false,
        );

        let chat_request = OpenAiCompatibleProviderClient::<()>::build_chat_request(&request, None);
        let json = serde_json::to_value(&chat_request).expect("should serialize");
        assert!(
            json.get("tools").is_none(),
            "tools field should be omitted when None"
        );
    }

    #[test]
    fn build_chat_request_native_mode_sends_tool_role_messages() {
        let tools = vec![NativeToolDef {
            name: "file_read".to_string(),
            description: "Read a file".to_string(),
            parameters: serde_json::json!({"type": "object", "properties": {}}),
        }];

        let request = ProviderTurnRequest::new(
            "test-model".to_string(),
            vec![
                ProviderMessage::new(ProviderMessageRole::System, "system prompt"),
                ProviderMessage::new_tool_result(
                    "call_001".to_string(),
                    "file content here".to_string(),
                ),
            ],
            false,
        )
        .with_tools(tools);

        let chat_request = OpenAiCompatibleProviderClient::<()>::build_chat_request(&request, None);

        // Tool-result message should use native "tool" role + tool_call_id
        let tool_msg = &chat_request.messages[1];
        assert_eq!(tool_msg.role, "tool");
        assert_eq!(tool_msg.tool_call_id.as_deref(), Some("call_001"));
        assert_eq!(
            tool_msg.content,
            Value::String("file content here".to_string())
        );
    }

    #[test]
    fn build_chat_request_native_mode_sends_assistant_tool_calls() {
        use crate::provider::AssistantToolCallRecord;

        let tools = vec![NativeToolDef {
            name: "file_read".to_string(),
            description: "Read a file".to_string(),
            parameters: serde_json::json!({"type": "object", "properties": {}}),
        }];

        let assistant_msg = ProviderMessage::new_assistant_with_tool_calls(
            "Let me read that file.".to_string(),
            vec![AssistantToolCallRecord {
                id: "call_abc".to_string(),
                function_name: "file_read".to_string(),
                arguments: r#"{"path":"src/main.rs"}"#.to_string(),
            }],
        );

        let request = ProviderTurnRequest::new(
            "test-model".to_string(),
            vec![
                ProviderMessage::new(ProviderMessageRole::User, "read main.rs"),
                assistant_msg,
            ],
            false,
        )
        .with_tools(tools);

        let chat_request = OpenAiCompatibleProviderClient::<()>::build_chat_request(&request, None);

        let asst_msg = &chat_request.messages[1];
        assert_eq!(asst_msg.role, "assistant");
        let tc = asst_msg
            .tool_calls
            .as_ref()
            .expect("tool_calls should be present");
        assert_eq!(tc.len(), 1);
        assert_eq!(tc[0].id, "call_abc");
        assert_eq!(tc[0].call_type, "function");
        assert_eq!(tc[0].function.name, "file_read");
        assert_eq!(tc[0].function.arguments, r#"{"path":"src/main.rs"}"#);
    }

    #[test]
    fn build_chat_request_fallback_flattens_tool_results_even_with_tool_call_id() {
        // Without tools in the request (fallback mode), tool-result messages
        // should be flattened to user role even if they have tool_call_id
        let request = ProviderTurnRequest::new(
            "test-model".to_string(),
            vec![
                ProviderMessage::new(ProviderMessageRole::System, "system prompt"),
                ProviderMessage::new_tool_result(
                    "call_001".to_string(),
                    "file content here".to_string(),
                ),
            ],
            false,
        );

        let chat_request = OpenAiCompatibleProviderClient::<()>::build_chat_request(&request, None);

        // In fallback mode, tool-result should be flattened to user
        let tool_msg = &chat_request.messages[1];
        assert_eq!(tool_msg.role, "user");
        assert!(tool_msg.tool_call_id.is_none());
    }

    #[test]
    fn parse_openai_tool_call_normalizes_name_and_assigns_default_id() {
        let tc = OpenAiToolCall {
            id: "".to_string(),
            function: OpenAiToolFunction {
                name: "file_read".to_string(),
                arguments: r#"{"path":"foo.rs"}"#.to_string(),
            },
        };

        let parsed = parse_openai_tool_call(&tc, 0).expect("should parse");
        assert_eq!(parsed.tool_name, "file.read");
        assert_eq!(parsed.id, "call_file_read_0");
    }

    #[test]
    fn parse_openai_tool_call_preserves_provided_id() {
        let tc = OpenAiToolCall {
            id: "call_abc123".to_string(),
            function: OpenAiToolFunction {
                name: "shell_exec".to_string(),
                arguments: r#"{"command":"ls"}"#.to_string(),
            },
        };

        let parsed = parse_openai_tool_call(&tc, 5).expect("should parse");
        assert_eq!(parsed.id, "call_abc123");
        assert_eq!(parsed.tool_name, "shell.exec");
    }

    #[test]
    fn parse_openai_tool_call_rejects_empty_name() {
        let tc = OpenAiToolCall {
            id: "call_001".to_string(),
            function: OpenAiToolFunction {
                name: "  ".to_string(),
                arguments: "{}".to_string(),
            },
        };

        let err = parse_openai_tool_call(&tc, 0).expect_err("should fail");
        assert!(err.to_string().contains("missing function.name"));
    }

    #[test]
    fn parse_openai_tool_call_rejects_non_object_arguments() {
        let tc = OpenAiToolCall {
            id: "call_001".to_string(),
            function: OpenAiToolFunction {
                name: "file_read".to_string(),
                arguments: r#""just a string""#.to_string(),
            },
        };

        let err = parse_openai_tool_call(&tc, 0).expect_err("should fail");
        assert!(err.to_string().contains("must be a JSON object"));
    }

    #[test]
    fn native_tool_call_to_request_produces_correct_tool_input() {
        let parsed = ParsedToolCall {
            id: "call_001".to_string(),
            tool_name: "file.read".to_string(),
            arguments: serde_json::json!({"path": "src/main.rs"}),
        };

        let request = native_tool_call_to_request(&parsed).expect("should convert");
        assert_eq!(request.tool_call_id, "call_001");
        assert_eq!(request.tool_name, "file.read");
        match &request.input {
            ToolInput::FileRead { path } => assert_eq!(path, "src/main.rs"),
            other => panic!("expected FileRead, got {:?}", other),
        }
    }

    #[test]
    fn native_tool_call_to_request_handles_file_write() {
        let parsed = ParsedToolCall {
            id: "call_002".to_string(),
            tool_name: "file.write".to_string(),
            arguments: serde_json::json!({"path": "test.txt", "content": "hello"}),
        };

        let request = native_tool_call_to_request(&parsed).expect("should convert");
        assert_eq!(request.tool_name, "file.write");
        match &request.input {
            ToolInput::FileWrite { path, content } => {
                assert_eq!(path, "test.txt");
                assert_eq!(content, "hello");
            }
            other => panic!("expected FileWrite, got {:?}", other),
        }
    }

    #[test]
    fn parsed_to_assistant_record_reverses_dot_to_underscore() {
        let parsed = ParsedToolCall {
            id: "call_xyz".to_string(),
            tool_name: "file.read".to_string(),
            arguments: serde_json::json!({"path": "foo.rs"}),
        };

        let record = parsed_to_assistant_record(&parsed);
        assert_eq!(record.id, "call_xyz");
        assert_eq!(record.function_name, "file_read");
        assert_eq!(record.arguments, r#"{"path":"foo.rs"}"#);
    }

    #[test]
    fn finalize_native_tool_calls_produces_requests_and_records() {
        let tool_calls = vec![
            OpenAiToolCall {
                id: "call_001".to_string(),
                function: OpenAiToolFunction {
                    name: "file_read".to_string(),
                    arguments: r#"{"path":"src/lib.rs"}"#.to_string(),
                },
            },
            OpenAiToolCall {
                id: "call_002".to_string(),
                function: OpenAiToolFunction {
                    name: "shell_exec".to_string(),
                    arguments: r#"{"command":"cargo build"}"#.to_string(),
                },
            },
        ];

        let (requests, records) = finalize_native_tool_calls(&tool_calls).expect("should succeed");
        assert_eq!(requests.len(), 2);
        assert_eq!(records.len(), 2);

        assert_eq!(requests[0].tool_name, "file.read");
        assert_eq!(requests[0].tool_call_id, "call_001");
        assert_eq!(requests[1].tool_name, "shell.exec");
        assert_eq!(requests[1].tool_call_id, "call_002");

        assert_eq!(records[0].function_name, "file_read");
        assert_eq!(records[1].function_name, "shell_exec");
    }

    #[test]
    fn openai_chat_message_serializes_without_optional_fields() {
        let msg = OpenAiChatMessage {
            role: "user".to_string(),
            content: Value::String("hello".to_string()),
            tool_calls: None,
            tool_call_id: None,
        };

        let json = serde_json::to_value(&msg).expect("should serialize");
        assert_eq!(json["role"], "user");
        assert_eq!(json["content"], "hello");
        // Optional fields should be omitted
        assert!(json.get("tool_calls").is_none());
        assert!(json.get("tool_call_id").is_none());
    }

    #[test]
    fn openai_chat_message_serializes_with_tool_calls() {
        let msg = OpenAiChatMessage {
            role: "assistant".to_string(),
            content: Value::String("thinking".to_string()),
            tool_calls: Some(vec![OpenAiToolCallMsg {
                id: "call_001".to_string(),
                call_type: "function".to_string(),
                function: OpenAiToolCallMsgFunction {
                    name: "file_read".to_string(),
                    arguments: r#"{"path":"foo"}"#.to_string(),
                },
            }]),
            tool_call_id: None,
        };

        let json = serde_json::to_value(&msg).expect("should serialize");
        assert_eq!(json["role"], "assistant");
        let tc = json["tool_calls"].as_array().expect("should be array");
        assert_eq!(tc.len(), 1);
        assert_eq!(tc[0]["id"], "call_001");
        assert_eq!(tc[0]["type"], "function");
        assert_eq!(tc[0]["function"]["name"], "file_read");
    }

    #[test]
    fn openai_chat_message_serializes_with_tool_call_id() {
        let msg = OpenAiChatMessage {
            role: "tool".to_string(),
            content: Value::String("file content".to_string()),
            tool_calls: None,
            tool_call_id: Some("call_001".to_string()),
        };

        let json = serde_json::to_value(&msg).expect("should serialize");
        assert_eq!(json["role"], "tool");
        assert_eq!(json["tool_call_id"], "call_001");
        assert!(json.get("tool_calls").is_none());
    }
}
