//! OpenAI-compatible provider client.
//!
//! Works with the standard `/v1/chat/completions` endpoint used by
//! OpenAI, Azure OpenAI, LM Studio, and other compatible servers.

use super::transport::{
    HttpTransport, ReqwestHttpTransport, RetryTransport, sanitize_error_message,
};
use super::{
    AgentEvent, ImageContent, ProviderClient, ProviderEvent, ProviderTurnError, ProviderTurnRequest,
};
use crate::config::EffectiveConfig;
use crate::contracts::InferencePerformanceView;
use serde::{Deserialize, Serialize};
use serde_json::Value;

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
    content: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct OpenAiUsage {
    #[serde(default)]
    #[allow(dead_code)]
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

/// Extract InferencePerformanceView from OpenAI usage.
fn extract_openai_performance(usage: &Option<OpenAiUsage>) -> Option<InferencePerformanceView> {
    let usage = usage.as_ref()?;
    Some(InferencePerformanceView {
        tokens_per_sec_tenths: None,
        eval_tokens: usage.completion_tokens,
        eval_duration_ms: None,
    })
}

/// Build a Done AgentEvent for OpenAI (shared helper).
fn build_openai_done_event(
    content: &str,
    inference_performance: Option<InferencePerformanceView>,
) -> AgentEvent {
    AgentEvent::Done {
        status: "Done. session saved".to_string(),
        assistant_message: content.to_string(),
        completion_summary: "Provider turn finished successfully.".to_string(),
        saved_status: "session saved".to_string(),
        tool_logs: Vec::new(),
        elapsed_ms: 0,
        inference_performance,
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
}

impl<T: HttpTransport> OpenAiCompatibleProviderClient<T> {
    /// Check connectivity to the OpenAI-compatible server by requesting `/v1/models`.
    ///
    /// If an API key is configured, it is sent as an `Authorization` header
    /// (without `Bearer` prefix, matching the existing code pattern in
    /// `send_chat_request`).
    pub fn health_check(&self) -> Result<(), ProviderTurnError> {
        let url = format!("{}/v1/models", self.base_url.trim_end_matches('/'));
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
        let chat_request = OpenAiChatRequest {
            model: request.model.clone(),
            messages: request
                .messages
                .iter()
                .map(|m| OpenAiChatMessage {
                    role: match m.role {
                        super::ProviderMessageRole::System => "system".to_string(),
                        super::ProviderMessageRole::User => "user".to_string(),
                        super::ProviderMessageRole::Assistant => "assistant".to_string(),
                        super::ProviderMessageRole::Tool => "tool".to_string(),
                    },
                    content: build_openai_content(&m.content, m.images.as_deref()),
                })
                .collect(),
            stream: request.stream,
        };

        let request_body = serde_json::to_vec(&chat_request).map_err(|err| {
            ProviderTurnError::Backend(format!("failed to encode openai request: {err}"))
        })?;
        let url = format!(
            "{}/v1/chat/completions",
            self.base_url.trim_end_matches('/')
        );

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
        let content = parsed
            .choices
            .first()
            .and_then(|choice| choice.message.content.clone())
            .ok_or_else(|| {
                ProviderTurnError::Backend("openai response contained no choices".to_string())
            })?;

        Ok(vec![
            ProviderEvent::TokenDelta(content.clone()),
            ProviderEvent::Agent(build_openai_done_event(&content, perf)),
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
        let chat_request = OpenAiChatRequest {
            model: request.model.clone(),
            messages: request
                .messages
                .iter()
                .map(|m| OpenAiChatMessage {
                    role: match m.role {
                        super::ProviderMessageRole::System => "system".to_string(),
                        super::ProviderMessageRole::User => "user".to_string(),
                        super::ProviderMessageRole::Assistant => "assistant".to_string(),
                        super::ProviderMessageRole::Tool => "tool".to_string(),
                    },
                    content: build_openai_content(&m.content, m.images.as_deref()),
                })
                .collect(),
            stream: true,
        };

        let request_body = serde_json::to_vec(&chat_request).map_err(|err| {
            ProviderTurnError::Backend(format!("failed to encode openai request: {err}"))
        })?;
        let url = format!(
            "{}/v1/chat/completions",
            self.base_url.trim_end_matches('/')
        );
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
                            content.push_str(&msg_content);
                            emit(ProviderEvent::TokenDelta(msg_content));
                            emit(ProviderEvent::Agent(build_openai_done_event(
                                &content, perf,
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
                        emit(ProviderEvent::Agent(build_openai_done_event(
                            &content, None,
                        )));
                        emitted_done = true;
                    }
                    return;
                }

                match serde_json::from_str::<OpenAiStreamChunk>(payload) {
                    Ok(chunk) => {
                        for choice in chunk.choices {
                            if let Some(delta) = choice.delta.content {
                                content.push_str(&delta);
                                emit(ProviderEvent::TokenDelta(delta));
                            }
                            if choice.finish_reason.is_some() && !emitted_done {
                                emit(ProviderEvent::Agent(build_openai_done_event(
                                    &content, None,
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
            emit(ProviderEvent::Agent(build_openai_done_event(
                &content, None,
            )));
        }
        Ok(())
    }
}

fn parse_openai_sse_response(body: &[u8]) -> Result<Vec<ProviderEvent>, ProviderTurnError> {
    let text = String::from_utf8_lossy(body);
    let mut content = String::new();
    let mut events = Vec::new();

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
            if choice.finish_reason.is_some() {
                events.push(ProviderEvent::Agent(build_openai_done_event(
                    &content, None,
                )));
            }
        }
    }

    if events
        .iter()
        .all(|event| !matches!(event, ProviderEvent::Agent(AgentEvent::Done { .. })))
    {
        events.push(ProviderEvent::Agent(build_openai_done_event(
            &content, None,
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
