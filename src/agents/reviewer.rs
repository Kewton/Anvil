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
                let additions = diff
                    .lines()
                    .filter(|line| line.starts_with('+') && !line.starts_with("+++"))
                    .count();
                let deletions = diff
                    .lines()
                    .filter(|line| line.starts_with('-') && !line.starts_with("---"))
                    .count();
                AgentResult::new(
                    "reviewer",
                    format!(
                        "Reviewer summarized the current diff for {} across {} changed files with {} additions and {} deletions",
                        task.user_request, changed_files, additions, deletions
                    ),
                )
                .with_next_recommendation(
                    "Review the highest-risk files first and decide whether another tester pass is needed",
                )
                .with_evidence(vec![(
                    "tool-output".to_string(),
                    format!(
                        "diff summary: {} files, +{}, -{}",
                        changed_files, additions, deletions
                    ),
                )])
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
                    task.user_request
                ),
            ),
        }
    }
}
