use crate::agents::{AgentResult, AgentTask};
use crate::runtime::engine::{RuntimeEngine, RuntimeToolOutcome};
use crate::tools::registry::{ToolRequest, ToolResponse};

#[derive(Debug, Default)]
pub struct ReaderAgent;

impl ReaderAgent {
    pub fn run(&self, task: &AgentTask, runtime: &RuntimeEngine) -> AgentResult {
        let cwd = match runtime.checked_execute(ToolRequest::InspectEnv) {
            Ok(RuntimeToolOutcome::Allowed(ToolResponse::EnvSnapshot(snapshot))) => {
                snapshot.cwd.display().to_string()
            }
            _ => task.workspace_root.display().to_string(),
        };

        let needle = select_search_term(&task.description);
        let matches = match runtime.checked_execute(ToolRequest::Search {
            root: task.workspace_root.clone(),
            needle: needle.clone(),
        }) {
            Ok(RuntimeToolOutcome::Allowed(ToolResponse::SearchMatches(matches))) => matches.len(),
            Ok(RuntimeToolOutcome::Blocked(reason)) => {
                return AgentResult::new("reader", format!("Reader blocked: {reason}"));
            }
            Ok(RuntimeToolOutcome::NeedsConfirmation(reason)) => {
                return AgentResult::new(
                    "reader",
                    format!("Reader awaiting confirmation: {reason}"),
                );
            }
            _ => 0,
        };

        AgentResult::new(
            "reader",
            format!(
                "Reader inspected {} and found {} matches for \"{}\" while handling: {}",
                cwd, matches, needle, task.description
            ),
        )
        .with_next_recommendation(
            "Use the matched files to decide whether editing or review is needed",
        )
    }
}

fn select_search_term(description: &str) -> String {
    description
        .split(|char: char| !char.is_alphanumeric())
        .find(|token| token.len() >= 4)
        .unwrap_or("fn")
        .to_ascii_lowercase()
}
