//! Ollama provider client.
//!
//! Implements the [`ProviderClient`] trait for the Ollama local inference
//! server via its `/api/chat` endpoint.

use super::transport::{CurlHttpTransport, HttpTransport, RetryTransport};
use super::{
    AgentEvent, ProviderClient, ProviderEvent, ProviderMessageRole, ProviderTurnError,
    ProviderTurnRequest,
};
use crate::config::EffectiveConfig;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::process::Command;

/// Wire format for Ollama chat messages.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OllamaChatMessage {
    pub role: String,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub images: Option<Vec<String>>,
}

/// Wire format for an Ollama `/api/chat` request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OllamaChatRequest {
    pub model: String,
    pub messages: Vec<OllamaChatMessage>,
    pub stream: bool,
    #[serde(default)]
    pub think: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct OllamaChatChunk {
    #[serde(default)]
    message: Option<OllamaChatMessage>,
    #[serde(default)]
    done: bool,
}

/// Client for the Ollama local inference server.
///
/// Generic over [`HttpTransport`] so tests can inject a mock.
pub struct OllamaProviderClient<T = RetryTransport<CurlHttpTransport>> {
    base_url: String,
    transport: T,
}

impl OllamaProviderClient {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            transport: RetryTransport::new(CurlHttpTransport::new()),
        }
    }

    pub fn from_config(config: &EffectiveConfig) -> Self {
        Self::new(config.runtime.provider_url.clone())
    }
}

impl<T> OllamaProviderClient<T> {
    pub fn with_transport(base_url: impl Into<String>, transport: T) -> Self {
        Self {
            base_url: base_url.into(),
            transport,
        }
    }

    pub fn build_chat_request(request: &ProviderTurnRequest) -> OllamaChatRequest {
        OllamaChatRequest {
            model: request.model.clone(),
            messages: request
                .messages
                .iter()
                .map(|message| {
                    let images = message
                        .images
                        .as_ref()
                        .map(|imgs| imgs.iter().map(|img| img.base64.clone()).collect());
                    OllamaChatMessage {
                        role: match message.role {
                            ProviderMessageRole::System => "system".to_string(),
                            ProviderMessageRole::User => "user".to_string(),
                            ProviderMessageRole::Assistant => "assistant".to_string(),
                            ProviderMessageRole::Tool => "tool".to_string(),
                        },
                        content: message.content.clone(),
                        images,
                    }
                })
                .collect(),
            stream: request.stream,
            think: false,
        }
    }

    pub fn normalize_stream_chunks(
        chunks: &[String],
    ) -> Result<Vec<ProviderEvent>, ProviderTurnError> {
        let mut events = Vec::new();
        let mut assistant_output = String::new();

        for chunk in chunks {
            let parsed: OllamaChatChunk = serde_json::from_str(chunk).map_err(|err| {
                ProviderTurnError::Backend(format!("invalid ollama response: {err}"))
            })?;

            if let Some(message) = parsed.message
                && !message.content.is_empty()
            {
                assistant_output.push_str(&message.content);
                events.push(ProviderEvent::TokenDelta(message.content));
            }

            if parsed.done {
                events.push(ProviderEvent::Agent(AgentEvent::Done {
                    status: "Done. session saved".to_string(),
                    assistant_message: assistant_output.clone(),
                    completion_summary: "Provider turn finished successfully.".to_string(),
                    saved_status: "session saved".to_string(),
                    tool_logs: Vec::new(),
                    elapsed_ms: 0,
                }));
            }
        }

        Ok(events)
    }
}

pub fn resolve_ollama_model_alias(requested: &str, available: &[String]) -> String {
    if available.iter().any(|name| name == requested) {
        return requested.to_string();
    }

    let mut prefix_matches = available
        .iter()
        .filter(|name| name.starts_with(requested))
        .cloned()
        .collect::<Vec<_>>();
    prefix_matches.sort();
    prefix_matches.dedup();

    if prefix_matches.len() == 1 {
        prefix_matches.remove(0)
    } else {
        requested.to_string()
    }
}

impl Default for OllamaProviderClient {
    fn default() -> Self {
        Self {
            base_url: "http://127.0.0.1:11434".to_string(),
            transport: RetryTransport::new(CurlHttpTransport::new()),
        }
    }
}

impl<T: HttpTransport> OllamaProviderClient<T> {
    /// Check connectivity to the Ollama server by requesting `/api/tags`.
    ///
    /// Returns `Ok(())` if the server responds, or an error message string
    /// on failure.  The health check uses the client's configured transport
    /// (which includes [`RetryTransport`] for automatic retry).
    pub fn health_check(&self) -> Result<(), String> {
        let url = format!("{}/api/tags", self.base_url.trim_end_matches('/'));
        match self.transport.get(&url) {
            Ok(_) => Ok(()),
            Err(e) => Err(format!("Ollamaに接続できません ({}): {}", self.base_url, e)),
        }
    }
}

impl<T: HttpTransport> ProviderClient for OllamaProviderClient<T> {
    fn stream_turn(
        &self,
        request: &ProviderTurnRequest,
        emit: &mut dyn FnMut(ProviderEvent),
    ) -> Result<(), ProviderTurnError> {
        let resolved_model = resolve_model_with_ollama_tags(&self.base_url, &request.model);
        let mut resolved_request = request.clone();
        resolved_request.model = resolved_model;
        let chat_request = Self::build_chat_request(&resolved_request);
        let request_body = serde_json::to_vec(&chat_request).map_err(|err| {
            ProviderTurnError::Backend(format!("failed to encode ollama request: {err}"))
        })?;
        let url = format!("{}/api/chat", self.base_url.trim_end_matches('/'));

        tracing::debug!(
            model = %resolved_request.model,
            messages = resolved_request.messages.len(),
            stream = resolved_request.stream,
            "sending ollama chat request"
        );

        let mut assistant_output = String::new();
        let mut had_error: Option<ProviderTurnError> = None;

        self.transport
            .stream_lines(&url, &request_body, &[], &mut |line| {
                if had_error.is_some() {
                    return;
                }
                match serde_json::from_str::<OllamaChatChunk>(line) {
                    Ok(chunk) => {
                        if let Some(message) = chunk.message
                            && !message.content.is_empty()
                        {
                            assistant_output.push_str(&message.content);
                            emit(ProviderEvent::TokenDelta(message.content));
                        }
                        if chunk.done {
                            emit(ProviderEvent::Agent(AgentEvent::Done {
                                status: "Done. session saved".to_string(),
                                assistant_message: assistant_output.clone(),
                                completion_summary: "Provider turn finished successfully."
                                    .to_string(),
                                saved_status: "session saved".to_string(),
                                tool_logs: Vec::new(),
                                elapsed_ms: 0,
                            }));
                        }
                    }
                    Err(err) => {
                        if let Ok(value) = serde_json::from_str::<serde_json::Value>(line)
                            && let Some(error) = value.get("error").and_then(|v| v.as_str())
                        {
                            had_error = Some(ProviderTurnError::Backend(error.to_string()));
                            return;
                        }
                        had_error = Some(ProviderTurnError::Backend(format!(
                            "invalid ollama response: {err}"
                        )));
                    }
                }
            })?;

        if let Some(err) = had_error {
            tracing::error!(error = %err, "ollama provider request failed");
            return Err(err);
        }
        Ok(())
    }
}

/// Request body for the Ollama `/api/show` endpoint.
#[derive(Serialize)]
struct ShowRequest {
    model: String,
}

/// Maximum allowed response size from `/api/show` (1 MiB).
const MAX_SHOW_RESPONSE_SIZE: usize = 1_048_576;

/// Upper bound for a sane `context_length` value (10 million tokens).
const MAX_CONTEXT_LENGTH: u32 = 10_000_000;

/// Query the Ollama `/api/show` endpoint and extract the model's
/// `context_length` from `model_info`.
///
/// Returns `None` on any failure (network, parse, missing key).
pub fn fetch_context_length_from_ollama(provider_url: &str, model: &str) -> Option<u32> {
    let url = format!("{}/api/show", provider_url.trim_end_matches('/'));
    let body = serde_json::to_vec(&ShowRequest {
        model: model.to_string(),
    })
    .ok()?;

    let output = Command::new("curl")
        .arg("-sS")
        .arg("--max-time")
        .arg("5")
        .arg("--proto")
        .arg("=http,https")
        .arg("--max-filesize")
        .arg(MAX_SHOW_RESPONSE_SIZE.to_string())
        .arg("--data-binary")
        .arg("@-")
        .arg("--")
        .arg(&url)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .ok()
        .and_then(|mut child| {
            use std::io::Write;
            if let Some(ref mut stdin) = child.stdin {
                let _ = stdin.write_all(&body);
            }
            child.wait_with_output().ok()
        })?;

    if !output.status.success() {
        return None;
    }

    parse_context_length_from_show_response(&output.stdout)
}

/// Parse an Ollama `/api/show` JSON response body and extract context_length.
///
/// This is the pure-logic core of [`fetch_context_length_from_ollama`],
/// extracted for unit testing without network access.
pub fn parse_context_length_from_show_response(json_bytes: &[u8]) -> Option<u32> {
    let value: Value = serde_json::from_slice(json_bytes).ok()?;
    let model_info = value.get("model_info")?.as_object()?;

    for (key, val) in model_info {
        if key.ends_with(".context_length") {
            let ctx_len = val.as_u64()?;
            let clamped = u32::try_from(ctx_len.min(u64::from(MAX_CONTEXT_LENGTH))).ok()?;
            if clamped > 0 {
                return Some(clamped);
            }
        }
    }
    None
}

fn resolve_model_with_ollama_tags(base_url: &str, requested: &str) -> String {
    let url = format!("{}/api/tags", base_url.trim_end_matches('/'));
    let output = Command::new("curl")
        .arg("-sS")
        .arg("--max-time")
        .arg("5")
        .arg(url)
        .output();

    let Ok(output) = output else {
        return requested.to_string();
    };
    if !output.status.success() {
        return requested.to_string();
    }

    let Ok(value) = serde_json::from_slice::<Value>(&output.stdout) else {
        return requested.to_string();
    };
    let Some(models) = value.get("models").and_then(Value::as_array) else {
        return requested.to_string();
    };
    let names = models
        .iter()
        .filter_map(|model| model.get("name").and_then(Value::as_str))
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    resolve_ollama_model_alias(requested, &names)
}
