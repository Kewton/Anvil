use std::path::PathBuf;

use crate::policy::permissions::{ExecutionContext, PermissionMode};

#[path = "config/model_profiles.rs"]
pub mod model_profiles;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderKind {
    Ollama,
    LmStudio,
}

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub cwd: PathBuf,
    pub provider: ProviderKind,
    pub model: String,
    pub host: String,
    pub permission_mode: PermissionMode,
    pub execution_context: ExecutionContext,
    pub state_dir: PathBuf,
}
