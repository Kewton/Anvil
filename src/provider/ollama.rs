//! Ollama provider client.
//!
//! Implements the [`ProviderClient`] trait for the Ollama local inference
//! server via its `/api/chat` endpoint.

use super::transport::{
    HttpTransport, ReqwestHttpTransport, RetryTransport, sanitize_error_message,
};
use super::{
    ProviderClient, ProviderEvent, ProviderMessageRole, ProviderTurnError, ProviderTurnRequest,
    build_provider_done_event,
};

/// Patterns in Ollama error messages that indicate a model is not found.
///
/// Ollama error response examples (v0.5.x verified):
/// {"error":"model 'nonexistent' not found, try pulling it first"}
/// {"error":"no such model 'invalid-name'"}
const MODEL_NOT_FOUND_PATTERNS: &[&str] = &["not found", "no such model"];
use crate::config::EffectiveConfig;
use crate::contracts::InferencePerformanceView;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::Duration;

/// Wire format for Ollama chat messages.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OllamaChatMessage {
    pub role: String,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub images: Option<Vec<String>>,
}

/// Ollama request options (e.g. `num_predict` for output token limit).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct OllamaRequestOptions {
    /// Maximum number of tokens to generate (maps to Ollama `num_predict`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub num_predict: Option<u32>,
}

/// Wire format for an Ollama `/api/chat` request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OllamaChatRequest {
    pub model: String,
    pub messages: Vec<OllamaChatMessage>,
    pub stream: bool,
    #[serde(default)]
    pub think: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub options: Option<OllamaRequestOptions>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct OllamaChatChunk {
    #[serde(default)]
    message: Option<OllamaChatMessage>,
    #[serde(default)]
    done: bool,
    #[serde(default)]
    eval_count: Option<u64>,
    #[serde(default)]
    eval_duration: Option<u64>,
    #[serde(default)]
    prompt_eval_count: Option<u64>,
    #[serde(default)]
    prompt_eval_duration: Option<u64>,
}

/// Extract InferencePerformanceView from Ollama eval metrics.
fn extract_inference_performance(
    eval_count: Option<u64>,
    eval_duration: Option<u64>,
    prompt_eval_count: Option<u64>,
) -> Option<InferencePerformanceView> {
    let eval_count = eval_count?;
    let eval_duration = eval_duration?;
    let eval_duration_ms = eval_duration / 1_000_000;
    let tokens_per_sec_tenths = if eval_duration > 0 {
        eval_count
            .checked_mul(10_000_000_000)
            .map(|v| v / eval_duration)
    } else {
        None
    };
    Some(InferencePerformanceView {
        tokens_per_sec_tenths,
        eval_tokens: Some(eval_count),
        eval_duration_ms: Some(eval_duration_ms),
        prompt_tokens: prompt_eval_count,
    })
}

/// Client for the Ollama local inference server.
///
/// Generic over [`HttpTransport`] so tests can inject a mock.
pub struct OllamaProviderClient<T = RetryTransport<ReqwestHttpTransport>> {
    base_url: String,
    transport: T,
}

impl OllamaProviderClient {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            transport: RetryTransport::new(ReqwestHttpTransport::new()),
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
            options: request.max_output_tokens.map(|n| OllamaRequestOptions {
                num_predict: Some(n),
            }),
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

            let eval_count = parsed.eval_count;
            let eval_duration = parsed.eval_duration;
            let prompt_eval_count = parsed.prompt_eval_count;

            if let Some(message) = parsed.message
                && !message.content.is_empty()
            {
                assistant_output.push_str(&message.content);
                events.push(ProviderEvent::TokenDelta(message.content));
            }

            if parsed.done {
                let perf =
                    extract_inference_performance(eval_count, eval_duration, prompt_eval_count);
                events.push(ProviderEvent::Agent(build_provider_done_event(
                    &assistant_output,
                    perf,
                )));
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
            base_url: crate::config::DEFAULT_OLLAMA_URL.to_string(),
            transport: RetryTransport::new(ReqwestHttpTransport::new()),
        }
    }
}

/// Timeout in seconds for sidecar LLM summarization requests.
const SIDECAR_TIMEOUT_SECS: u64 = 30;
/// Maximum response body size for sidecar summarization (64 KiB).
const MAX_SIDECAR_RESPONSE_SIZE: usize = 65_536;

/// Summarization prompt sent to the sidecar model.
const SIDECAR_SUMMARIZE_PROMPT: &str = "\
You are a concise summarizer. Respond ONLY with bullet points.
Summarize this conversation so far in 3-5 bullet points, focusing on:
what was discussed, what files were modified, what decisions were made.";

impl<T: HttpTransport> OllamaProviderClient<T> {
    /// Check connectivity to the Ollama server by requesting `/api/tags`.
    ///
    /// Returns `Ok(())` if the server responds, or an error message string
    /// on failure.  The health check uses the client's configured transport
    /// (which includes [`RetryTransport`] for automatic retry).
    pub fn health_check(&self) -> Result<(), ProviderTurnError> {
        let url = format!("{}/api/tags", self.base_url.trim_end_matches('/'));
        self.transport.get(&url).map(|_| ())
    }

    /// Generate a conversation summary using a sidecar model.
    ///
    /// Sends the conversation text to the Ollama `/api/chat` endpoint with
    /// a dedicated 30-second timeout. Returns `None` on any failure (timeout,
    /// network error, parse error) so callers can fall back to rule-based
    /// summarization.
    pub fn sidecar_summarize(&self, model: &str, conversation_text: &str) -> Option<String> {
        let url = format!("{}/api/chat", self.base_url.trim_end_matches('/'));
        let request = OllamaChatRequest {
            model: model.to_string(),
            messages: vec![
                OllamaChatMessage {
                    role: "system".to_string(),
                    content: SIDECAR_SUMMARIZE_PROMPT.to_string(),
                    images: None,
                },
                OllamaChatMessage {
                    role: "user".to_string(),
                    content: conversation_text.to_string(),
                    images: None,
                },
            ],
            stream: false,
            think: false,
            options: None,
        };

        let body = serde_json::to_vec(&request).ok()?;

        // Use a dedicated HTTP client with 30s timeout (not self.transport).
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(SIDECAR_TIMEOUT_SECS))
            .build()
            .ok()?;

        let response = match client
            .post(&url)
            .header("Content-Type", "application/json")
            .body(body)
            .send()
        {
            Ok(resp) => resp,
            Err(err) => {
                tracing::warn!(
                    "sidecar_summarize request failed: {}",
                    sanitize_error_message(&err.to_string())
                );
                return None;
            }
        };

        if !response.status().is_success() {
            tracing::warn!(
                status = %response.status(),
                "sidecar_summarize received non-success status"
            );
            return None;
        }

        // Read response body with size limit using take() to avoid
        // allocating memory for oversized responses (CB-001).
        let body_bytes = {
            use std::io::Read;
            let limit = MAX_SIDECAR_RESPONSE_SIZE as u64;
            // Read up to limit + 1 bytes so we can detect overflow.
            let mut buf = Vec::new();
            match response.take(limit + 1).read_to_end(&mut buf) {
                Ok(_) => {
                    if buf.len() > MAX_SIDECAR_RESPONSE_SIZE {
                        tracing::warn!(
                            limit = MAX_SIDECAR_RESPONSE_SIZE,
                            "sidecar_summarize response exceeds size limit"
                        );
                        return None;
                    }
                    buf
                }
                Err(err) => {
                    tracing::warn!(
                        "sidecar_summarize failed to read response: {}",
                        sanitize_error_message(&err.to_string())
                    );
                    return None;
                }
            }
        };

        let parsed: Value = match serde_json::from_slice(&body_bytes) {
            Ok(v) => v,
            Err(err) => {
                tracing::warn!("sidecar_summarize failed to parse response JSON: {err}");
                return None;
            }
        };

        let content = parsed
            .get("message")
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_str())
            .map(|s| s.to_string());

        if content.is_none() {
            tracing::warn!("sidecar_summarize response missing message.content");
        }

        // Log sidecar success with model and summary length (Issue #206 D-1)
        if let Some(ref text) = content {
            tracing::info!(
                model = %model,
                summary_len = text.len(),
                "sidecar_summarize success"
            );
        }

        content
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
                        let eval_count = chunk.eval_count;
                        let eval_duration = chunk.eval_duration;
                        let prompt_eval_count = chunk.prompt_eval_count;
                        if let Some(message) = chunk.message
                            && !message.content.is_empty()
                        {
                            assistant_output.push_str(&message.content);
                            emit(ProviderEvent::TokenDelta(message.content));
                        }
                        if chunk.done {
                            let perf = extract_inference_performance(
                                eval_count,
                                eval_duration,
                                prompt_eval_count,
                            );
                            emit(ProviderEvent::Agent(build_provider_done_event(
                                &assistant_output,
                                perf,
                            )));
                        }
                    }
                    Err(err) => {
                        if let Ok(value) = serde_json::from_str::<serde_json::Value>(line)
                            && let Some(error) = value.get("error").and_then(|v| v.as_str())
                        {
                            if MODEL_NOT_FOUND_PATTERNS.iter().any(|p| error.contains(p)) {
                                let model = resolved_request.model.clone();
                                had_error = Some(ProviderTurnError::ModelNotFound {
                                    model,
                                    message: sanitize_error_message(error),
                                });
                            } else {
                                had_error = Some(ProviderTurnError::Backend(error.to_string()));
                            }
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

/// Maximum allowed response size from `/api/show` (1 MiB).
const MAX_SHOW_RESPONSE_SIZE: usize = 1_048_576;

/// Upper bound for a sane `context_length` value (10 million tokens).
const MAX_CONTEXT_LENGTH: u32 = 10_000_000;

/// Shared HTTP client for Ollama metadata requests (timeout 5s).
fn ollama_metadata_client() -> &'static reqwest::blocking::Client {
    static CLIENT: std::sync::OnceLock<reqwest::blocking::Client> = std::sync::OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .expect("Failed to build Ollama metadata HTTP client")
    })
}

/// Query the Ollama `/api/show` endpoint and extract the model's
/// `context_length` from `model_info`.
///
/// Returns `None` on any failure (network, parse, missing key).
pub fn fetch_context_length_from_ollama(provider_url: &str, model: &str) -> Option<u32> {
    let url = format!("{}/api/show", provider_url.trim_end_matches('/'));
    let body = serde_json::json!({"model": model});
    let client = ollama_metadata_client();
    let response = client.post(&url).json(&body).send().ok()?;
    if !response.status().is_success() {
        return None;
    }
    let bytes = {
        use std::io::Read;
        let mut buf = Vec::new();
        response
            .take(MAX_SHOW_RESPONSE_SIZE as u64)
            .read_to_end(&mut buf)
            .ok()?;
        buf
    };
    parse_context_length_from_show_response(&bytes)
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

// ---------------------------------------------------------------------------
// Model list / info types and parse functions (Issue #77)
// ---------------------------------------------------------------------------

/// An entry from the Ollama `/api/tags` model list.
#[derive(Debug)]
pub struct OllamaModelEntry {
    pub name: String,
    pub size: u64,
}

/// Detailed model information from the Ollama `/api/show` endpoint.
#[derive(Debug)]
pub struct OllamaModelInfo {
    pub parameter_size: Option<String>,
    pub quantization_level: Option<String>,
    pub context_length: Option<u32>,
}

/// Maximum allowed response size from `/api/tags` (5 MiB).
const MAX_TAGS_RESPONSE_SIZE: usize = 5_242_880;

/// Parse an Ollama `/api/tags` JSON response body and extract the model list.
///
/// Pure-logic function for unit testing without network access.
pub fn parse_model_list_from_tags_response(json_bytes: &[u8]) -> Option<Vec<OllamaModelEntry>> {
    let value: Value = serde_json::from_slice(json_bytes).ok()?;
    let models = value.get("models")?.as_array()?;
    let mut entries = Vec::new();
    for model in models {
        let name = model.get("name")?.as_str()?.to_string();
        let size = model.get("size").and_then(Value::as_u64).unwrap_or(0);
        entries.push(OllamaModelEntry { name, size });
    }
    Some(entries)
}

/// Parse an Ollama `/api/show` JSON response body and extract model info.
///
/// Pure-logic function for unit testing without network access.
/// Extends the existing `parse_context_length_from_show_response` pattern.
pub fn parse_model_info_from_show_response(json_bytes: &[u8]) -> Option<OllamaModelInfo> {
    let value: Value = serde_json::from_slice(json_bytes).ok()?;

    // Extract parameter_size and quantization_level from details
    let details = value.get("details");
    let parameter_size = details
        .and_then(|d| d.get("parameter_size"))
        .and_then(Value::as_str)
        .map(ToString::to_string);
    let quantization_level = details
        .and_then(|d| d.get("quantization_level"))
        .and_then(Value::as_str)
        .map(ToString::to_string);

    // Extract context_length from model_info (same logic as parse_context_length_from_show_response)
    let context_length =
        value
            .get("model_info")
            .and_then(Value::as_object)
            .and_then(|model_info| {
                for (key, val) in model_info {
                    if key.ends_with(".context_length")
                        && let Some(ctx_len) = val.as_u64()
                    {
                        let clamped =
                            u32::try_from(ctx_len.min(u64::from(MAX_CONTEXT_LENGTH))).ok()?;
                        if clamped > 0 {
                            return Some(clamped);
                        }
                    }
                }
                None
            });

    Some(OllamaModelInfo {
        parameter_size,
        quantization_level,
        context_length,
    })
}

/// Fetch the list of available models from Ollama `/api/tags`.
///
/// Returns `None` on any failure (network, parse, missing key).
pub fn fetch_model_list_from_ollama(provider_url: &str) -> Option<Vec<OllamaModelEntry>> {
    let url = format!("{}/api/tags", provider_url.trim_end_matches('/'));
    let client = ollama_metadata_client();
    let response = client.get(&url).send().ok()?;
    if !response.status().is_success() {
        return None;
    }
    let bytes = {
        use std::io::Read;
        let mut buf = Vec::new();
        response
            .take(MAX_TAGS_RESPONSE_SIZE as u64)
            .read_to_end(&mut buf)
            .ok()?;
        buf
    };
    parse_model_list_from_tags_response(&bytes)
}

/// Fetch detailed model information from Ollama `/api/show`.
///
/// Returns `None` on any failure (network, parse, missing key).
pub fn fetch_model_info_from_ollama(provider_url: &str, model: &str) -> Option<OllamaModelInfo> {
    let url = format!("{}/api/show", provider_url.trim_end_matches('/'));
    let body = serde_json::json!({"model": model});
    let client = ollama_metadata_client();
    let response = client.post(&url).json(&body).send().ok()?;
    if !response.status().is_success() {
        return None;
    }
    let bytes = {
        use std::io::Read;
        let mut buf = Vec::new();
        response
            .take(MAX_SHOW_RESPONSE_SIZE as u64)
            .read_to_end(&mut buf)
            .ok()?;
        buf
    };
    parse_model_info_from_show_response(&bytes)
}

// TODO: fetch_context_length_from_ollama, fetch_model_list_from_ollama,
// fetch_model_info_from_ollamaをOllamaProviderClient::メソッドに統合し、
// HttpTransport経由で実装するリファクタリング（別Issue）

fn resolve_model_with_ollama_tags(base_url: &str, requested: &str) -> String {
    let url = format!("{}/api/tags", base_url.trim_end_matches('/'));
    let client = ollama_metadata_client();
    let response = match client.get(&url).send() {
        Ok(r) => r,
        Err(_) => return requested.to_string(),
    };
    if !response.status().is_success() {
        return requested.to_string();
    }
    let bytes = match response.bytes() {
        Ok(b) => b,
        Err(_) => return requested.to_string(),
    };
    let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
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
