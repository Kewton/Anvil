use crate::agent::AgentEvent;
use crate::config::EffectiveConfig;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderBackend {
    Ollama,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderCapabilities {
    pub streaming: bool,
    pub tool_calling: bool,
}

impl Default for ProviderCapabilities {
    fn default() -> Self {
        Self {
            streaming: false,
            tool_calling: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderRuntimeContext {
    pub backend: ProviderBackend,
    pub capabilities: ProviderCapabilities,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderMessageRole {
    System,
    User,
    Assistant,
    Tool,
}

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderEvent {
    Agent(AgentEvent),
    TokenDelta(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderTurnError {
    Cancelled,
    Backend(String),
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

pub trait ProviderClient {
    fn stream_turn(
        &self,
        request: &ProviderTurnRequest,
        emit: &mut dyn FnMut(ProviderEvent),
    ) -> Result<(), ProviderTurnError>;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OllamaChatMessage {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OllamaChatRequest {
    pub model: String,
    pub messages: Vec<OllamaChatMessage>,
    pub stream: bool,
}

pub struct OllamaProviderClient;

impl OllamaProviderClient {
    pub fn build_chat_request(request: &ProviderTurnRequest) -> OllamaChatRequest {
        OllamaChatRequest {
            model: request.model.clone(),
            messages: request
                .messages
                .iter()
                .map(|message| OllamaChatMessage {
                    role: match message.role {
                        ProviderMessageRole::System => "system".to_string(),
                        ProviderMessageRole::User => "user".to_string(),
                        ProviderMessageRole::Assistant => "assistant".to_string(),
                        ProviderMessageRole::Tool => "tool".to_string(),
                    },
                    content: message.content.clone(),
                })
                .collect(),
            stream: request.stream,
        }
    }
}

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
        // Phase 2 bootstrap uses backend-level preset capabilities.
        // Later phases can replace or refine this with live capability discovery.
        let backend = match config.runtime.provider.as_str() {
            "ollama" => ProviderBackend::Ollama,
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
        };

        Ok(Self {
            backend,
            capabilities,
        })
    }
}
