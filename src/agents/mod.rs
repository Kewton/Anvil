use std::path::PathBuf;

pub mod editor;
pub mod pm;
pub mod prompt_loader;
pub mod reader;
pub mod reviewer;
pub mod tester;

#[derive(Debug, Clone)]
pub struct AgentTask {
    pub description: String,
    pub context: String,
    pub workspace_root: PathBuf,
}

#[derive(Debug, Clone)]
pub struct AgentResult {
    pub role: String,
    pub summary: String,
}

impl AgentResult {
    pub fn new(role: impl Into<String>, summary: impl Into<String>) -> Self {
        Self {
            role: role.into(),
            summary: summary.into(),
        }
    }
}
