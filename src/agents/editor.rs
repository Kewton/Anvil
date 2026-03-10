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
        let changed_files: Vec<String> = target
            .iter()
            .map(|path| path.display().to_string())
            .collect();

        if should_apply_mutation(&task.description) {
            if let Some(path) = target.as_ref() {
                return apply_mutation(task, runtime, path, preview, changed_files);
            }
        }

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
        .with_next_recommendation("Apply the smallest viable patch to the selected target file")
        .with_changed_files(changed_files)
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

fn read_contents(runtime: &RuntimeEngine, path: &PathBuf) -> anyhow::Result<String> {
    match runtime.checked_execute(ToolRequest::ReadFile { path: path.clone() })? {
        RuntimeToolOutcome::Allowed(ToolResponse::FileContents(result)) => Ok(result.contents),
        RuntimeToolOutcome::Allowed(_) => Ok(String::new()),
        RuntimeToolOutcome::Blocked(reason) => Ok(format!("blocked: {reason}")),
        RuntimeToolOutcome::NeedsConfirmation(reason) => Ok(format!("confirm: {reason}")),
    }
}

fn should_apply_mutation(description: &str) -> bool {
    let normalized = description.to_ascii_lowercase();
    normalized.contains("apply")
        || normalized.contains("append")
        || normalized.contains("write")
        || normalized.contains("update file")
}

fn apply_mutation(
    task: &AgentTask,
    runtime: &RuntimeEngine,
    path: &PathBuf,
    preview: String,
    changed_files: Vec<String>,
) -> AgentResult {
    let existing = match read_contents(runtime, path) {
        Ok(contents) => contents,
        Err(error) => {
            return AgentResult::new(
                "editor",
                format!("Editor failed to read target file: {error}"),
            )
        }
    };

    let note = build_note(path, &task.description);
    let updated = if existing.ends_with('\n') {
        format!("{existing}{note}\n")
    } else {
        format!("{existing}\n{note}\n")
    };

    match runtime.checked_execute(ToolRequest::WriteFile {
        path: path.clone(),
        contents: updated,
    }) {
        Ok(RuntimeToolOutcome::Allowed(ToolResponse::WriteResult(result))) => AgentResult::new(
            "editor",
            format!(
                "Editor applied a bounded mutation for {} to {} ({} bytes) with preview: {}",
                task.description,
                result.path.display(),
                result.bytes_written,
                preview
            ),
        )
        .with_next_recommendation("Run a focused tester pass against the mutated file")
        .with_changed_files(changed_files)
        .with_evidence(vec![(
            "repo-file".to_string(),
            format!("mutated {}", result.path.display()),
        )]),
        Ok(RuntimeToolOutcome::Blocked(reason)) => {
            AgentResult::new("editor", format!("Editor blocked: {reason}"))
        }
        Ok(RuntimeToolOutcome::NeedsConfirmation(reason)) => {
            AgentResult::new("editor", format!("Editor awaiting confirmation: {reason}"))
        }
        Ok(RuntimeToolOutcome::Allowed(_)) | Err(_) => AgentResult::new(
            "editor",
            format!(
                "Editor could not apply a bounded mutation for: {}",
                task.description
            ),
        ),
    }
}

fn build_note(path: &PathBuf, description: &str) -> String {
    let prefix = match path.extension().and_then(|ext| ext.to_str()) {
        Some("rs" | "js" | "ts" | "tsx" | "jsx" | "java" | "c" | "cc" | "cpp" | "go" | "swift") => {
            "//"
        }
        Some("py" | "sh" | "rb" | "yml" | "yaml" | "toml") => "#",
        Some("md" | "txt") => "-",
        _ => "//",
    };

    let summary: String = description.chars().take(120).collect();
    format!("{prefix} anvil-mvp: {summary}")
}
