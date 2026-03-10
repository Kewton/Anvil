use std::path::PathBuf;

use crate::agents::{AgentResult, AgentTask};
use crate::runtime::engine::{RuntimeEngine, RuntimeToolOutcome};
use crate::tools::registry::{ToolRequest, ToolResponse};

#[derive(Debug, Default)]
pub struct EditorAgent;

impl EditorAgent {
    pub fn run(&self, task: &AgentTask, runtime: &RuntimeEngine) -> AgentResult {
        let needle = select_search_term(&task.description);
        let first_match = match runtime.checked_execute(ToolRequest::Search {
            root: task.workspace_root.clone(),
            needle: needle.clone(),
        }) {
            Ok(RuntimeToolOutcome::Allowed(ToolResponse::SearchMatches(matches))) => {
                matches.into_iter().next()
            }
            Ok(RuntimeToolOutcome::Blocked(reason)) => {
                return AgentResult::new("editor", format!("Editor blocked: {reason}"));
            }
            Ok(RuntimeToolOutcome::NeedsConfirmation(reason)) => {
                return AgentResult::new(
                    "editor",
                    format!("Editor awaiting confirmation: {reason}"),
                );
            }
            _ => None,
        };

        let target = first_match
            .as_ref()
            .map(|item| item.path.clone())
            .or_else(|| first_repo_file(&task.workspace_root));
        let preview = target
            .as_ref()
            .and_then(|path| read_preview(runtime, path).ok())
            .unwrap_or_else(|| "no preview available".to_string());

        AgentResult::new(
            "editor",
            format!(
                "Editor prepared a bounded edit plan for {} using target {} with preview: {}",
                task.description,
                target
                    .as_ref()
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|| "<none>".to_string()),
                preview
            ),
        )
    }
}

fn select_search_term(description: &str) -> String {
    description
        .split(|char: char| !char.is_alphanumeric())
        .find(|token| token.len() >= 4)
        .unwrap_or("mod")
        .to_ascii_lowercase()
}

fn first_repo_file(root: &PathBuf) -> Option<PathBuf> {
    std::fs::read_dir(root)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .find(|path| path.is_file())
}

fn read_preview(runtime: &RuntimeEngine, path: &PathBuf) -> anyhow::Result<String> {
    match runtime.checked_execute(ToolRequest::ReadFile { path: path.clone() })? {
        RuntimeToolOutcome::Allowed(ToolResponse::FileContents(result)) => Ok(result
            .contents
            .lines()
            .next()
            .unwrap_or_default()
            .chars()
            .take(80)
            .collect()),
        RuntimeToolOutcome::Allowed(_) => Ok(String::new()),
        RuntimeToolOutcome::Blocked(reason) => Ok(format!("blocked: {reason}")),
        RuntimeToolOutcome::NeedsConfirmation(reason) => Ok(format!("confirm: {reason}")),
    }
}
