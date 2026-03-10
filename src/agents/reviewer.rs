use crate::agents::{AgentResult, AgentTask};
use crate::runtime::engine::{RuntimeEngine, RuntimeToolOutcome};
use crate::tools::registry::{ToolRequest, ToolResponse};

#[derive(Debug, Default)]
pub struct ReviewerAgent;

impl ReviewerAgent {
    pub fn run(&self, task: &AgentTask, runtime: &RuntimeEngine) -> AgentResult {
        match runtime.checked_execute(ToolRequest::Diff {
            root: task.workspace_root.clone(),
        }) {
            Ok(RuntimeToolOutcome::Allowed(ToolResponse::Diff(diff))) => {
                let changed_files = diff
                    .lines()
                    .filter(|line| line.starts_with("diff --git "))
                    .count();
                AgentResult::new(
                    "reviewer",
                    format!(
                        "Reviewer prepared a risk pass for {} across {} changed files",
                        task.description, changed_files
                    ),
                )
                .with_next_recommendation(
                    "Review the flagged files and decide whether a tester pass is needed",
                )
            }
            Ok(RuntimeToolOutcome::Blocked(reason)) => {
                AgentResult::new("reviewer", format!("Reviewer blocked: {reason}"))
            }
            Ok(RuntimeToolOutcome::NeedsConfirmation(reason)) => AgentResult::new(
                "reviewer",
                format!("Reviewer awaiting confirmation: {reason}"),
            ),
            Ok(RuntimeToolOutcome::Allowed(_)) | Err(_) => AgentResult::new(
                "reviewer",
                format!(
                    "Reviewer could not inspect the diff for: {}",
                    task.description
                ),
            ),
        }
    }
}
