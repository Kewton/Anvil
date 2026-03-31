//! OpenAI-compatible provider client.
//!
//! Works with the standard `/v1/chat/completions` endpoint used by
//! OpenAI, Azure OpenAI, LM Studio, and other compatible servers.

use super::transport::{
    HttpTransport, ReqwestHttpTransport, RetryTransport, sanitize_error_message,
};
use super::{
    AgentEvent, ImageContent, ProviderClient, ProviderEvent, ProviderTurnError,
    ProviderTurnRequest, build_provider_done_event,
};
use crate::config::EffectiveConfig;
use crate::contracts::InferencePerformanceView;
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
}

/// Request message: content is `Value` to support both plain text and
/// multimodal (text + image_url) arrays.
#[derive(Debug, Clone, Serialize)]
struct OpenAiChatMessage {
    role: String,
    content: Value,
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
        "shell_exec" => "shell.exec".to_string(),
        "web_fetch" => "web.fetch".to_string(),
        "web_search" => "web.search".to_string(),
        "agent_explore" => "agent.explore".to_string(),
        "agent_plan" => "agent.plan".to_string(),
        "git_status" => "git.status".to_string(),
        "git_diff" => "git.diff".to_string(),
        "git_log" => "git.log".to_string(),
        _ => name.to_string(),
    }
}

fn default_tool_call_id(tool_name: &str, index: usize) -> String {
    format!("call_{}_{}", tool_name.replace('.', "_"), index)
}

fn openai_tool_call_to_anvil_block(
    tool_call: &OpenAiToolCall,
    index: usize,
) -> Result<String, ProviderTurnError> {
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

    let Some(args_object) = args_value.as_object() else {
        return Err(ProviderTurnError::Backend(format!(
            "openai tool_call arguments for '{tool_name}' must be a JSON object"
        )));
    };

    let mut payload = serde_json::Map::new();
    let tool_call_id = if tool_call.id.trim().is_empty() {
        default_tool_call_id(&tool_name, index)
    } else {
        tool_call.id.clone()
    };

    payload.insert("id".to_string(), Value::String(tool_call_id));
    payload.insert("tool".to_string(), Value::String(tool_name));
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
    fn build_chat_request(
        request: &ProviderTurnRequest,
        stream_options: Option<serde_json::Value>,
    ) -> OpenAiChatRequest {
        OpenAiChatRequest {
            model: request.model.clone(),
            messages: request
                .messages
                .iter()
                .map(|m| {
                    let (role, content) = openai_message_role_and_content(m);
                    OpenAiChatMessage { role, content }
                })
                .collect(),
            stream: request.stream,
            stream_options,
            max_tokens: request.max_output_tokens,
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

        if request.stream && looks_like_sse_stream(&response.body) {
            return parse_openai_sse_response(&response.body);
        }

        let parsed: OpenAiChatResponse = serde_json::from_slice(&response.body)
            .map_err(|err| ProviderTurnError::Backend(format!("invalid openai response: {err}")))?;

        let perf = extract_openai_performance(&parsed.usage);
        let choice = parsed.choices.first().ok_or_else(|| {
            ProviderTurnError::Backend("openai response contained no choices".to_string())
        })?;
        let content = choice.message.content.clone().unwrap_or_default();
        if !choice.message.tool_calls.is_empty() {
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
                            let assistant_message = if !choice.message.tool_calls.is_empty() {
                                match build_native_tool_calls_content(
                                    Some(&msg_content),
                                    &choice.message.tool_calls,
                                ) {
                                    Ok(message) => {
                                        saw_native_tool_calls = true;
                                        message
                                    }
                                    Err(err) => {
                                        had_error = Some(err);
                                        return;
                                    }
                                }
                            } else {
                                content.push_str(&msg_content);
                                emit(ProviderEvent::TokenDelta(msg_content));
                                content.clone()
                            };
                            emit(ProviderEvent::Agent(build_provider_done_event(
                                &assistant_message,
                                perf,
                            )));
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
                        let assistant_message = if saw_native_tool_calls {
                            match finalize_streaming_tool_calls(std::mem::take(
                                &mut streaming_tool_calls,
                            ))
                            .and_then(|finalized| {
                                build_native_tool_calls_content(Some(&content), &finalized)
                            }) {
                                Ok(message) => message,
                                Err(err) => {
                                    had_error = Some(err);
                                    return;
                                }
                            }
                        } else {
                            content.clone()
                        };
                        emit(ProviderEvent::Agent(build_provider_done_event(
                            &assistant_message,
                            perf,
                        )));
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
                                let assistant_message = if saw_native_tool_calls {
                                    match finalize_streaming_tool_calls(std::mem::take(
                                        &mut streaming_tool_calls,
                                    ))
                                    .and_then(|finalized| {
                                        build_native_tool_calls_content(Some(&content), &finalized)
                                    }) {
                                        Ok(message) => message,
                                        Err(err) => {
                                            had_error = Some(err);
                                            return;
                                        }
                                    }
                                } else {
                                    content.clone()
                                };
                                emit(ProviderEvent::Agent(build_provider_done_event(
                                    &assistant_message,
                                    perf,
                                )));
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
            let assistant_message = if saw_native_tool_calls {
                let finalized = finalize_streaming_tool_calls(streaming_tool_calls)?;
                build_native_tool_calls_content(Some(&content), &finalized)?
            } else {
                content.clone()
            };
            emit(ProviderEvent::Agent(build_provider_done_event(
                &assistant_message,
                perf,
            )));
        }
        Ok(())
    }
}

fn parse_openai_sse_response(body: &[u8]) -> Result<Vec<ProviderEvent>, ProviderTurnError> {
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
                let assistant_message = if saw_native_tool_calls {
                    let finalized =
                        finalize_streaming_tool_calls(std::mem::take(&mut streaming_tool_calls))?;
                    build_native_tool_calls_content(Some(&content), &finalized)?
                } else {
                    content.clone()
                };
                events.push(ProviderEvent::Agent(build_provider_done_event(
                    &assistant_message,
                    None,
                )));
            }
        }
    }

    if events
        .iter()
        .all(|event| !matches!(event, ProviderEvent::Agent(AgentEvent::Done { .. })))
    {
        let assistant_message = if saw_native_tool_calls {
            let finalized = finalize_streaming_tool_calls(streaming_tool_calls)?;
            build_native_tool_calls_content(Some(&content), &finalized)?
        } else {
            content
        };
        events.push(ProviderEvent::Agent(build_provider_done_event(
            &assistant_message,
            None,
        )));
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
}
