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
    pub next_recommendation: Option<String>,
    pub commands_run: Vec<String>,
    pub changed_files: Vec<String>,
    pub evidence: Vec<(String, String)>,
}

impl AgentResult {
    pub fn new(role: impl Into<String>, summary: impl Into<String>) -> Self {
        Self {
            role: role.into(),
            summary: summary.into(),
            next_recommendation: None,
            commands_run: Vec::new(),
            changed_files: Vec::new(),
            evidence: Vec::new(),
        }
    }

    pub fn with_next_recommendation(mut self, value: impl Into<String>) -> Self {
        self.next_recommendation = Some(value.into());
        self
    }

    pub fn with_commands_run(mut self, values: Vec<String>) -> Self {
        self.commands_run = values;
        self
    }

    pub fn with_changed_files(mut self, values: Vec<String>) -> Self {
        self.changed_files = values;
        self
    }

    pub fn with_evidence(mut self, values: Vec<(String, String)>) -> Self {
        self.evidence = values;
        self
    }
}
