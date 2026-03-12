use crate::config::EffectiveConfig;

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
            other => return Err(ProviderBootstrapError::UnsupportedBackend(other.to_string())),
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
