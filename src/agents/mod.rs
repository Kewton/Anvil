use std::path::PathBuf;

use crate::state::session::PendingConfirmation;

pub mod editor;
pub mod executor;
pub mod planning;
pub mod pm;
pub mod prompt_loader;
pub mod reader;
pub mod reviewer;
pub mod tester;

#[derive(Debug, Clone)]
pub struct AgentTask {
    pub description: String,
    pub user_request: String,
    pub context: String,
    pub workspace_root: PathBuf,
}

#[derive(Debug, Clone)]
pub struct AgentResult {
    pub role: String,
    pub summary: String,
    pub status: ExecutionStatus,
    pub facts: Vec<AgentFact>,
    pub next_recommendation: Option<String>,
    pub commands_run: Vec<String>,
    pub changed_files: Vec<String>,
    pub evidence: Vec<(String, String)>,
    pub pending_confirmation: Option<PendingConfirmation>,
}

#[derive(Debug, Clone)]
pub struct AgentFact {
    pub key: String,
    pub value: String,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum ExecutionStatus {
    Completed,
    Blocked,
    NeedsConfirmation,
}

impl AgentResult {
    pub fn new(role: impl Into<String>, summary: impl Into<String>) -> Self {
        Self {
            role: role.into(),
            summary: summary.into(),
            status: ExecutionStatus::Completed,
            facts: Vec::new(),
            next_recommendation: None,
            commands_run: Vec::new(),
            changed_files: Vec::new(),
            evidence: Vec::new(),
            pending_confirmation: None,
        }
    }

    pub fn blocked(role: impl Into<String>, summary: impl Into<String>) -> Self {
        Self::new(role, summary).with_status(ExecutionStatus::Blocked)
    }

    pub fn awaiting_confirmation(role: impl Into<String>, summary: impl Into<String>) -> Self {
        Self::new(role, summary).with_status(ExecutionStatus::NeedsConfirmation)
    }

    pub fn with_status(mut self, value: ExecutionStatus) -> Self {
        self.status = value;
        self
    }

    pub fn with_next_recommendation(mut self, value: impl Into<String>) -> Self {
        self.next_recommendation = Some(value.into());
        self
    }

    pub fn with_facts(mut self, values: Vec<AgentFact>) -> Self {
        self.facts = values;
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

    pub fn with_pending_confirmation(mut self, value: PendingConfirmation) -> Self {
        self.pending_confirmation = Some(value);
        self.status = ExecutionStatus::NeedsConfirmation;
        self
    }

    pub fn is_blocked(&self) -> bool {
        self.status == ExecutionStatus::Blocked
    }

    pub fn needs_confirmation(&self) -> bool {
        self.status == ExecutionStatus::NeedsConfirmation
    }
}
