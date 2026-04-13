/// Provider integration layer.
///
/// Abstracts LLM provider communication behind the [`ProviderClient`] trait,
/// allowing both Ollama and OpenAI-compatible backends to share the same
/// application-level flow.  HTTP transport is pluggable via [`HttpTransport`].
pub mod ollama;
pub mod openai;
pub mod transport;

use crate::agent::AgentEvent;
use crate::agent::model_classifier::{ToolProtocolMode, determine_protocol_mode};
use crate::config::EffectiveConfig;
use crate::contracts::InferencePerformanceView;
use crate::tooling::NativeToolDef;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

// Re-export key types so existing `use crate::provider::*` continues to work.
pub use ollama::{
    OllamaChatMessage, OllamaChatRequest, OllamaModelEntry, OllamaModelInfo, OllamaProviderClient,
    OllamaRequestOptions, fetch_context_length_from_ollama, fetch_model_info_from_ollama,
    fetch_model_list_from_ollama, is_low_quality_sidecar_summary,
    parse_context_length_from_show_response, parse_model_info_from_show_response,
    parse_model_list_from_tags_response, resolve_ollama_model_alias,
};
pub use transport::{
    DEFAULT_HTTP_TIMEOUT_SECS, HttpResponse, HttpTransport, ReqwestHttpTransport, RetryConfig,
    RetryTransport, classify_http_error, classify_reqwest_error, http_timeout,
    normalize_http_timeout, redact_secrets, sanitize_error_message,
};

/// Default transport used by provider clients: reqwest with retry wrapper.
pub type DefaultTransport = RetryTransport<ReqwestHttpTransport>;

/// Supported LLM provider backends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderBackend {
    Ollama,
    OpenAi,
}

/// Dispatch enum for concrete provider clients.
pub enum LocalProviderClient {
    Ollama(OllamaProviderClient),
    OpenAi(openai::OpenAiCompatibleProviderClient),
}

/// Feature flags discovered (or assumed) for a provider.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ProviderCapabilities {
    pub streaming: bool,
    pub tool_calling: bool,
    pub tool_protocol: ToolProtocolMode,
    /// Whether this provider supports native tool calling (Issue #373).
    /// When `true`, tool definitions are sent via the provider API's `tools`
    /// parameter and tool results are sent as `tool` role messages.
    pub native_tool_calling: bool,
}

/// Bootstrapped provider context available for the lifetime of a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderRuntimeContext {
    pub backend: ProviderBackend,
    pub capabilities: ProviderCapabilities,
}

/// Base64-encoded image data for multimodal provider requests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageContent {
    pub base64: String,
    pub mime_type: String,
}

/// Record of a native tool call issued by the assistant.
///
/// Used to replay assistant tool_calls in follow-up turns and for session
/// persistence.  The `arguments` field holds the raw JSON string as received
/// from the provider.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssistantToolCallRecord {
    pub id: String,
    pub function_name: String,
    pub arguments: String,
}

/// Message role used in provider requests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderMessageRole {
    System,
    User,
    Assistant,
    Tool,
}

/// A single message in a provider request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderMessage {
    pub role: ProviderMessageRole,
    pub content: String,
    pub images: Option<Vec<ImageContent>>,
    /// Native tool calling: tool_call_id for tool-result messages.
    pub tool_call_id: Option<String>,
    /// Native tool calling: tool_calls record for assistant messages.
    pub assistant_tool_calls: Option<Vec<AssistantToolCallRecord>>,
}

impl ProviderMessage {
    pub fn new(role: ProviderMessageRole, content: impl Into<String>) -> Self {
        Self {
            role,
            content: content.into(),
            images: None,
            tool_call_id: None,
            assistant_tool_calls: None,
        }
    }

    pub fn with_images(mut self, images: Vec<ImageContent>) -> Self {
        self.images = Some(images);
        self
    }

    /// Construct a tool-result message for native tool calling.
    ///
    /// Role is automatically set to `Tool`.
    pub fn new_tool_result(tool_call_id: String, content: String) -> Self {
        Self {
            role: ProviderMessageRole::Tool,
            content,
            images: None,
            tool_call_id: Some(tool_call_id),
            assistant_tool_calls: None,
        }
    }

    /// Construct an assistant message that includes native tool_calls.
    ///
    /// Role is automatically set to `Assistant`.
    pub fn new_assistant_with_tool_calls(
        content: String,
        tool_calls: Vec<AssistantToolCallRecord>,
    ) -> Self {
        Self {
            role: ProviderMessageRole::Assistant,
            content,
            images: None,
            tool_call_id: None,
            assistant_tool_calls: Some(tool_calls),
        }
    }

    /// Access the tool_call_id (for tool-result messages).
    pub fn tool_call_id(&self) -> Option<&str> {
        self.tool_call_id.as_deref()
    }

    /// Access the assistant tool_calls record.
    pub fn assistant_tool_calls(&self) -> Option<&[AssistantToolCallRecord]> {
        self.assistant_tool_calls.as_deref()
    }
}

/// Request payload sent to a provider for one turn.
#[derive(Debug, Clone, PartialEq)]
pub struct ProviderTurnRequest {
    pub model: String,
    pub messages: Vec<ProviderMessage>,
    pub stream: bool,
    /// Maximum number of tokens the LLM may generate in this turn.
    /// When set, providers translate this to their native limit parameter
    /// (e.g. Ollama `num_predict`, OpenAI `max_tokens`).
    pub max_output_tokens: Option<u32>,
    /// LLM sampling temperature for this turn.
    /// `Some(v)` sets explicit temperature; `None` uses the model default.
    /// Agentic turns default to 0.3 for tool-output stability (Issue #369).
    pub temperature: Option<f64>,
    /// Context window size to pass to the provider (e.g. Ollama `num_ctx`).
    /// Only set when the user explicitly specifies `--context-window` so that
    /// the default model context length is not overridden unintentionally.
    pub context_window: Option<u32>,
    /// Native tool definitions to send with the request (Issue #373).
    /// `None` means no native tool calling for this turn (fallback to text protocol).
    pub tools: Option<Vec<NativeToolDef>>,
}

impl ProviderTurnRequest {
    pub fn new(model: String, messages: Vec<ProviderMessage>, stream: bool) -> Self {
        Self {
            model,
            messages,
            stream,
            max_output_tokens: None,
            temperature: None,
            context_window: None,
            tools: None,
        }
    }

    /// Attach native tool definitions to this request.
    pub fn with_tools(mut self, tools: Vec<NativeToolDef>) -> Self {
        self.tools = Some(tools);
        self
    }
}

/// Events emitted by a provider during a turn.
///
/// `Agent` carries a full `AgentEvent` which can be large (256+ bytes for Done
/// with native tool call records).  Boxing is not worthwhile here because
/// `ProviderEvent` is never stored in long-lived collections — it is emitted
/// and immediately consumed by the `emit` callback.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(clippy::large_enum_variant)]
pub enum ProviderEvent {
    Agent(AgentEvent),
    TokenDelta(String),
}

/// Errors that can occur during a provider turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderTurnError {
    Cancelled,
    Network(String),
    ConnectionRefused(String),
    DnsFailure(String),
    ServerError { status_code: u16, message: String },
    ClientError { status_code: u16, message: String },
    Timeout(String),
    Parse(String),
    Backend(String),
    ModelNotFound { model: String, message: String },
    AuthenticationFailed { status_code: u16, message: String },
}

impl ProviderTurnError {
    /// Returns `true` if this error is eligible for automatic retry.
    ///
    /// Network errors, server errors (5xx), and timeouts are retryable.
    /// Client errors (4xx), parse errors, cancellations, and unclassified
    /// backend errors are not.
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::Network(_) | Self::ServerError { .. } | Self::Timeout(_)
        )
    }

    /// Returns `true` if this error represents a connection refused condition.
    pub fn is_connection_refused(&self) -> bool {
        matches!(self, Self::ConnectionRefused(_))
    }

    /// Returns `true` if this error represents a DNS failure condition.
    pub fn is_dns_failure(&self) -> bool {
        matches!(self, Self::DnsFailure(_))
    }
}

/// Classification of provider errors for persistence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProviderErrorKind {
    Cancelled,
    Network,
    ServerError,
    ClientError,
    Timeout,
    Parse,
    Backend,
    ConnectionRefused,
    DnsFailure,
    ModelNotFound,
    AuthenticationFailed,
    #[serde(other)]
    Unknown,
}

/// A provider error record stored in the session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderErrorRecord {
    pub kind: ProviderErrorKind,
    pub message: String,
}

impl std::fmt::Display for ProviderTurnError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cancelled => write!(f, "provider turn cancelled"),
            Self::Network(msg) => write!(f, "network error: {msg}"),
            Self::ConnectionRefused(msg) => {
                let redacted = redact_secrets(msg);
                write!(f, "connection refused: {redacted}")
            }
            Self::DnsFailure(msg) => {
                let redacted = redact_secrets(msg);
                write!(f, "DNS resolution failed: {redacted}")
            }
            Self::ServerError {
                status_code,
                message,
            } => write!(f, "server error ({status_code}): {message}"),
            Self::ClientError {
                status_code,
                message,
            } => write!(f, "client error ({status_code}): {message}"),
            Self::Timeout(msg) => write!(f, "timeout: {msg}"),
            Self::Parse(msg) => write!(f, "parse error: {msg}"),
            Self::Backend(message) => write!(f, "provider backend error: {message}"),
            Self::ModelNotFound { model, message } => {
                let redacted = redact_secrets(message);
                write!(f, "model '{model}' not found: {redacted}")
            }
            Self::AuthenticationFailed {
                status_code,
                message,
            } => {
                let redacted = redact_secrets(message);
                write!(f, "authentication failed ({status_code}): {redacted}")
            }
        }
    }
}

impl std::error::Error for ProviderTurnError {}

/// Extract a model name from a `ProviderTurnError::ModelNotFound` Display string.
/// Expected format: `"model '<name>' not found: <detail>"`.
fn extract_model_from_message(message: &str) -> String {
    if let Some(start) = message.find("model '") {
        let rest = &message[start + 7..];
        if let Some(end) = rest.find('\'') {
            return rest[..end].to_string();
        }
    }
    "unknown".to_string()
}

impl ProviderTurnError {
    /// Reconstruct a `ProviderTurnError` from a persisted `ProviderErrorRecord`.
    pub fn from_error_record(record: &ProviderErrorRecord) -> Self {
        match record.kind {
            ProviderErrorKind::Cancelled => Self::Cancelled,
            ProviderErrorKind::Network => Self::Network(record.message.clone()),
            ProviderErrorKind::ConnectionRefused => Self::ConnectionRefused(record.message.clone()),
            ProviderErrorKind::DnsFailure => Self::DnsFailure(record.message.clone()),
            ProviderErrorKind::ServerError => Self::ServerError {
                status_code: 500,
                message: record.message.clone(),
            },
            ProviderErrorKind::ClientError => Self::ClientError {
                status_code: 400,
                message: record.message.clone(),
            },
            ProviderErrorKind::Timeout => Self::Timeout(record.message.clone()),
            ProviderErrorKind::Parse => Self::Parse(record.message.clone()),
            ProviderErrorKind::Backend => Self::Backend(record.message.clone()),
            ProviderErrorKind::ModelNotFound => Self::ModelNotFound {
                model: extract_model_from_message(&record.message),
                message: record.message.clone(),
            },
            ProviderErrorKind::AuthenticationFailed => Self::AuthenticationFailed {
                status_code: 401,
                message: record.message.clone(),
            },
            ProviderErrorKind::Unknown => Self::Backend(record.message.clone()),
        }
    }
}

impl From<&ProviderTurnError> for ProviderErrorKind {
    fn from(err: &ProviderTurnError) -> Self {
        match err {
            ProviderTurnError::Cancelled => Self::Cancelled,
            ProviderTurnError::Network(_) => Self::Network,
            ProviderTurnError::ConnectionRefused(_) => Self::ConnectionRefused,
            ProviderTurnError::DnsFailure(_) => Self::DnsFailure,
            ProviderTurnError::ServerError { .. } => Self::ServerError,
            ProviderTurnError::ClientError { .. } => Self::ClientError,
            ProviderTurnError::Timeout(_) => Self::Timeout,
            ProviderTurnError::Parse(_) => Self::Parse,
            ProviderTurnError::Backend(_) => Self::Backend,
            ProviderTurnError::ModelNotFound { .. } => Self::ModelNotFound,
            ProviderTurnError::AuthenticationFailed { .. } => Self::AuthenticationFailed,
        }
    }
}

/// Build a Done AgentEvent from provider response (shared by ollama and openai).
pub(crate) fn build_provider_done_event(
    assistant_output: &str,
    inference_performance: Option<InferencePerformanceView>,
) -> AgentEvent {
    AgentEvent::Done {
        status: "Done. session saved".to_string(),
        assistant_message: assistant_output.to_string(),
        completion_summary: "Provider turn finished successfully.".to_string(),
        saved_status: "session saved".to_string(),
        tool_logs: Vec::new(),
        elapsed_ms: 0,
        inference_performance,
        tool_calls: None,
        assistant_tool_call_records: None,
    }
}

/// Build a Done AgentEvent with native tool calls (Issue #373).
pub(crate) fn build_provider_done_event_with_tool_calls(
    assistant_output: &str,
    inference_performance: Option<InferencePerformanceView>,
    tool_calls: Vec<crate::tooling::ToolCallRequest>,
    records: Vec<AssistantToolCallRecord>,
) -> AgentEvent {
    AgentEvent::Done {
        status: "Done. session saved".to_string(),
        assistant_message: assistant_output.to_string(),
        completion_summary: "Provider turn finished successfully.".to_string(),
        saved_status: "session saved".to_string(),
        tool_logs: Vec::new(),
        elapsed_ms: 0,
        inference_performance,
        tool_calls: Some(tool_calls),
        assistant_tool_call_records: Some(records),
    }
}

/// Abstraction over LLM provider communication.
///
/// Implementors receive a request and emit [`ProviderEvent`]s via the
/// provided callback.
pub trait ProviderClient {
    fn stream_turn(
        &self,
        request: &ProviderTurnRequest,
        emit: &mut dyn FnMut(ProviderEvent),
    ) -> Result<(), ProviderTurnError>;
}

// ---------------------------------------------------------------------------
// Bootstrap / dispatch
// ---------------------------------------------------------------------------

/// Errors during provider bootstrap.
#[derive(Debug)]
pub enum ProviderBootstrapError {
    UnsupportedBackend(String),
}

impl std::fmt::Display for ProviderBootstrapError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedBackend(backend) => {
                write!(f, "unsupported provider backend: {backend}")
            }
        }
    }
}

impl std::error::Error for ProviderBootstrapError {}

impl ProviderRuntimeContext {
    pub fn bootstrap(config: &EffectiveConfig) -> Result<Self, ProviderBootstrapError> {
        let backend = match config.runtime.provider.as_str() {
            "ollama" => ProviderBackend::Ollama,
            "openai" | "lmstudio" => ProviderBackend::OpenAi,
            other => {
                return Err(ProviderBootstrapError::UnsupportedBackend(
                    other.to_string(),
                ));
            }
        };

        let tool_protocol =
            determine_protocol_mode(&config.runtime.model, config.runtime.tag_protocol);

        // Determine native tool calling capability (Issue #373).
        let native_tool_calling = resolve_native_tool_calling(
            backend,
            &config.runtime.model,
            config.runtime.native_tool_calling,
        );

        // Both backends currently share identical capability defaults.
        let capabilities = ProviderCapabilities {
            streaming: true,
            tool_calling: true,
            tool_protocol,
            native_tool_calling,
        };

        Ok(Self {
            backend,
            capabilities,
        })
    }
}

impl LocalProviderClient {
    /// Check connectivity to the configured provider.
    ///
    /// Dispatches to the inner client's `health_check` method.
    /// Returns `Ok(())` on success or a human-readable error message on failure.
    pub fn health_check(&self) -> Result<(), ProviderTurnError> {
        match self {
            Self::Ollama(client) => client.health_check(),
            Self::OpenAi(client) => client.health_check(),
        }
    }
}

impl ProviderClient for LocalProviderClient {
    fn stream_turn(
        &self,
        request: &ProviderTurnRequest,
        emit: &mut dyn FnMut(ProviderEvent),
    ) -> Result<(), ProviderTurnError> {
        match self {
            Self::Ollama(client) => client.stream_turn(request, emit),
            Self::OpenAi(client) => client.stream_turn(request, emit),
        }
    }
}

// ---------------------------------------------------------------------------
// Native tool calling resolution (Issue #373)
// ---------------------------------------------------------------------------

/// Resolve whether native tool calling should be enabled.
///
/// Priority: explicit env/config override > backend default > model heuristic.
fn resolve_native_tool_calling(
    backend: ProviderBackend,
    model_name: &str,
    config_override: Option<bool>,
) -> bool {
    // Explicit override takes priority
    if let Some(flag) = config_override {
        return flag;
    }

    match backend {
        // OpenAI / LmStudio backends: enabled by default
        ProviderBackend::OpenAi => true,
        // Ollama: depends on model capabilities
        ProviderBackend::Ollama => is_native_tool_capable_model(model_name),
    }
}

/// Heuristic to determine if an Ollama model supports native tool calling.
///
/// Models known to support function calling via Ollama's native API:
/// - llama3.1+, llama3.2+, llama3.3+
/// - qwen2.5+, qwen3+
/// - mistral (latest), mistral-nemo
/// - command-r, command-r-plus
/// - gemma4 (with tool support)
///
/// Conservative: returns `false` for unknown models.
pub fn is_native_tool_capable_model(model_name: &str) -> bool {
    let lower = model_name.to_lowercase();

    // llama3.1, llama3.2, llama3.3 (but not llama3 base which lacks tool support)
    if lower.starts_with("llama3.1")
        || lower.starts_with("llama3.2")
        || lower.starts_with("llama3.3")
    {
        return true;
    }

    // qwen2.5, qwen3
    if lower.starts_with("qwen2.5") || lower.starts_with("qwen3") {
        return true;
    }

    // mistral variants with tool support
    if lower.starts_with("mistral") && (lower.contains("nemo") || lower.contains("latest")) {
        return true;
    }

    // command-r family
    if lower.starts_with("command-r") {
        return true;
    }

    // gemma4
    if lower.starts_with("gemma4") {
        return true;
    }

    false
}

/// Build a concrete provider client from the effective config.
///
/// The returned client wraps its HTTP transport in [`RetryTransport`] so that
/// transient network/server errors are automatically retried with exponential
/// backoff.
pub fn build_local_provider_client(
    config: &EffectiveConfig,
    shutdown_flag: Arc<AtomicBool>,
) -> Result<LocalProviderClient, ProviderBootstrapError> {
    let reqwest_transport = ReqwestHttpTransport::with_timeout_and_shutdown_flag(
        config.runtime.http_timeout_secs,
        Arc::clone(&shutdown_flag),
    );
    let transport = RetryTransport::with_shutdown_flag(
        reqwest_transport,
        RetryConfig::default(),
        shutdown_flag,
    );
    match config.runtime.provider.as_str() {
        "ollama" => {
            let client = OllamaProviderClient::with_transport(
                config.runtime.provider_url.clone(),
                transport,
            );
            Ok(LocalProviderClient::Ollama(client))
        }
        "openai" => {
            let mut client = openai::OpenAiCompatibleProviderClient::with_transport(
                config.runtime.provider_url.clone(),
                transport,
            );
            if let Some(ref key) = config.runtime.api_key {
                client = client.with_api_key(key.clone());
            }
            Ok(LocalProviderClient::OpenAi(client))
        }
        "lmstudio" => Ok(LocalProviderClient::OpenAi(
            openai::OpenAiCompatibleProviderClient::with_transport(
                config.runtime.provider_url.clone(),
                transport,
            ),
        )),
        other => Err(ProviderBootstrapError::UnsupportedBackend(
            other.to_string(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_message_new_has_images_none() {
        let msg = ProviderMessage::new(ProviderMessageRole::User, "hello");
        assert_eq!(msg.images, None);
    }

    #[test]
    fn provider_message_with_images_sets_images() {
        let images = vec![ImageContent {
            base64: "abc123".to_string(),
            mime_type: "image/png".to_string(),
        }];
        let msg =
            ProviderMessage::new(ProviderMessageRole::User, "hello").with_images(images.clone());
        assert_eq!(msg.images, Some(images));
    }

    #[test]
    fn image_content_clone_and_eq() {
        let img = ImageContent {
            base64: "data".to_string(),
            mime_type: "image/jpeg".to_string(),
        };
        let img2 = img.clone();
        assert_eq!(img, img2);
    }
}
