/// Provider integration layer.
///
/// Abstracts LLM provider communication behind the [`ProviderClient`] trait,
/// allowing both Ollama and OpenAI-compatible backends to share the same
/// application-level flow.  HTTP transport is pluggable via [`HttpTransport`].
pub mod ollama;
pub mod openai;
pub mod transport;

use crate::agent::AgentEvent;
use crate::config::EffectiveConfig;
use crate::contracts::InferencePerformanceView;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

// Re-export key types so existing `use crate::provider::*` continues to work.
pub use ollama::{
    OllamaChatMessage, OllamaChatRequest, OllamaModelEntry, OllamaModelInfo, OllamaProviderClient,
    fetch_context_length_from_ollama, fetch_model_info_from_ollama, fetch_model_list_from_ollama,
    parse_context_length_from_show_response, parse_model_info_from_show_response,
    parse_model_list_from_tags_response, resolve_ollama_model_alias,
};
pub use transport::{
    HttpResponse, HttpTransport, ReqwestHttpTransport, RetryConfig, RetryTransport,
    classify_http_error, classify_reqwest_error, http_timeout, redact_secrets,
    sanitize_error_message,
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
}

impl ProviderMessage {
    pub fn new(role: ProviderMessageRole, content: impl Into<String>) -> Self {
        Self {
            role,
            content: content.into(),
            images: None,
        }
    }

    pub fn with_images(mut self, images: Vec<ImageContent>) -> Self {
        self.images = Some(images);
        self
    }
}

/// Request payload sent to a provider for one turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderTurnRequest {
    pub model: String,
    pub messages: Vec<ProviderMessage>,
    pub stream: bool,
}

impl ProviderTurnRequest {
    pub fn new(model: String, messages: Vec<ProviderMessage>, stream: bool) -> Self {
        Self {
            model,
            messages,
            stream,
        }
    }
}

/// Events emitted by a provider during a turn.
#[derive(Debug, Clone, PartialEq, Eq)]
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
            "openai" => ProviderBackend::OpenAi,
            other => {
                return Err(ProviderBootstrapError::UnsupportedBackend(
                    other.to_string(),
                ));
            }
        };

        let capabilities = match backend {
            ProviderBackend::Ollama => ProviderCapabilities {
                streaming: true,
                tool_calling: true,
            },
            ProviderBackend::OpenAi => ProviderCapabilities {
                streaming: true,
                tool_calling: true,
            },
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

/// Build a concrete provider client from the effective config.
///
/// The returned client wraps its HTTP transport in [`RetryTransport`] so that
/// transient network/server errors are automatically retried with exponential
/// backoff.
pub fn build_local_provider_client(
    config: &EffectiveConfig,
    shutdown_flag: Arc<AtomicBool>,
) -> Result<LocalProviderClient, ProviderBootstrapError> {
    let reqwest_transport = ReqwestHttpTransport::with_shutdown_flag(Arc::clone(&shutdown_flag));
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
