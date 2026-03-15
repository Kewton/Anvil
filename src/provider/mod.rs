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
use serde::{Deserialize, Serialize};

// Re-export key types so existing `use crate::provider::*` continues to work.
pub use ollama::{
    OllamaChatMessage, OllamaChatRequest, OllamaProviderClient, resolve_ollama_model_alias,
};
pub use transport::{CurlHttpTransport, HttpResponse, HttpTransport, TcpHttpTransport};

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
}

impl ProviderMessage {
    pub fn new(role: ProviderMessageRole, content: impl Into<String>) -> Self {
        Self {
            role,
            content: content.into(),
        }
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
    Backend(String),
}

/// Classification of provider errors for persistence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProviderErrorKind {
    Cancelled,
    Backend,
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
            Self::Backend(message) => write!(f, "provider backend error: {message}"),
        }
    }
}

impl std::error::Error for ProviderTurnError {}

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
pub fn build_local_provider_client(
    config: &EffectiveConfig,
) -> Result<LocalProviderClient, ProviderBootstrapError> {
    match config.runtime.provider.as_str() {
        "ollama" => Ok(LocalProviderClient::Ollama(
            OllamaProviderClient::from_config(config),
        )),
        "openai" => Ok(LocalProviderClient::OpenAi(
            openai::OpenAiCompatibleProviderClient::from_config(config),
        )),
        other => Err(ProviderBootstrapError::UnsupportedBackend(
            other.to_string(),
        )),
    }
}
