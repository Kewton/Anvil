use std::path::PathBuf;

use crate::policy::permissions::{ExecutionContext, PermissionMode};

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub cwd: PathBuf,
    pub model: String,
    pub ollama_host: String,
    pub permission_mode: PermissionMode,
    pub execution_context: ExecutionContext,
    pub state_dir: PathBuf,
}
