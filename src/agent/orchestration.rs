use std::collections::{BTreeMap, hash_map::DefaultHasher};
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use ignore::WalkBuilder;

use crate::model_registry::RuntimeModels;
use crate::ollama::client::OllamaClient;
use crate::session::store::ConversationMessage;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureClass {
    Transport,
    Parser,
    Incomplete,
    MaxIterations,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActorPlan {
    pub summary: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoSnapshot {
    files: BTreeMap<PathBuf, u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoVerification {
    pub changed_files: Vec<String>,
    pub implementation_files_changed: usize,
    pub test_files_changed: usize,
    pub setup_files_changed: usize,
}

impl RepoVerification {
    pub fn summary_note(&self) -> String {
        let changed = if self.changed_files.is_empty() {
            "none".to_string()
        } else {
            self.changed_files.join(", ")
        };
        format!(
            "Verifier: changed=[{changed}] impl={} test={} setup={}.",
            self.implementation_files_changed, self.test_files_changed, self.setup_files_changed
        )
    }

    pub fn made_any_progress(&self) -> bool {
        self.implementation_files_changed > 0
            || self.test_files_changed > 0
            || self.setup_files_changed > 0
    }

    pub fn restart_note(&self) -> String {
        if self.made_any_progress() {
            "Continue from the changed files above. Do not restart setup.".to_string()
        } else {
            "No repository progress was detected. Continue without restarting setup, and make a concrete change next.".to_string()
        }
    }

    pub fn prefers_converged_actor(self) -> bool {
        self.made_any_progress()
    }
}

pub fn classify_failure(error: &str) -> FailureClass {
    let lower = error.to_ascii_lowercase();
    if lower.contains("failed to contact ollama chat api")
        || lower.contains("error sending request for url")
        || lower.contains("connection reset")
        || lower.contains("connection refused")
        || lower.contains("broken pipe")
    {
        FailureClass::Transport
    } else if lower.contains("native tool parser failed")
        || lower.contains("unexpected end element")
        || lower.contains("unexpected eof")
        || lower.contains("</function>")
    {
        FailureClass::Parser
    } else if lower.contains("kept stopping before making the requested repository edits")
        || lower.contains("kept describing actions without using tools")
        || lower.contains("returned empty responses repeatedly")
    {
        FailureClass::Incomplete
    } else if lower.contains("did not finish within max iterations") {
        FailureClass::MaxIterations
    } else {
        FailureClass::Other
    }
}

pub fn should_restart_actor(failure: FailureClass) -> bool {
    matches!(
        failure,
        FailureClass::Transport
            | FailureClass::Parser
            | FailureClass::Incomplete
            | FailureClass::MaxIterations
    )
}

pub fn planner_system_prompt() -> String {
    "You are the Planner for a local coding agent. Reply only with a short execution plan for the Actor. Keep it under 6 bullets and under 140 words. Include: goal, likely files or areas to touch, validation, and a finish condition. No markdown code fences. No tool syntax.".to_string()
}

pub fn planner_user_prompt(user_prompt: &str, work_root: &Path) -> String {
    format!(
        "Project root: {}\nUser request: {}\nReturn a short execution plan for the Actor only.",
        work_root.display(),
        user_prompt
    )
}

pub fn build_actor_plan(
    client: &OllamaClient,
    models: &RuntimeModels,
    work_root: &Path,
    user_prompt: &str,
) -> Result<ActorPlan, String> {
    let planner_model = models.sidecar.as_deref().unwrap_or(models.main.as_str());
    let messages = vec![
        ConversationMessage::system(planner_system_prompt()),
        ConversationMessage::user(planner_user_prompt(user_prompt, work_root)),
    ];
    let reply = client.chat_text(planner_model, &messages)?;
    if !reply.tool_calls.is_empty() {
        return Err("planner unexpectedly requested tools".to_string());
    }
    let summary = reply.content.trim();
    if summary.is_empty() {
        return Err("planner returned an empty plan".to_string());
    }
    Ok(ActorPlan {
        summary: summary.to_string(),
    })
}

pub fn capture_repo_snapshot(root: &Path) -> RepoSnapshot {
    let mut files = BTreeMap::new();
    let walker = WalkBuilder::new(root)
        .hidden(false)
        .git_ignore(false)
        .git_exclude(false)
        .git_global(false)
        .build();

    for entry in walker.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        if should_skip_path(root, path) {
            continue;
        }
        let Some(relative) = path.strip_prefix(root).ok().map(Path::to_path_buf) else {
            continue;
        };
        if let Ok(bytes) = fs::read(path) {
            files.insert(relative, stable_hash(&bytes));
        }
    }

    RepoSnapshot { files }
}

pub fn verify_repo_progress(before: &RepoSnapshot, root: &Path) -> RepoVerification {
    let after = capture_repo_snapshot(root);
    let mut changed_files = Vec::new();
    let mut implementation_files_changed = 0usize;
    let mut test_files_changed = 0usize;
    let mut setup_files_changed = 0usize;

    for (path, after_hash) in after.files {
        let changed = before.files.get(&path) != Some(&after_hash);
        if !changed {
            continue;
        }
        let display = path.display().to_string();
        if changed_files.len() < 4 {
            changed_files.push(display.clone());
        }
        if is_test_file(&path) {
            test_files_changed += 1;
        } else if is_setup_file(&path) {
            setup_files_changed += 1;
        } else if is_implementation_file(&path) {
            implementation_files_changed += 1;
        }
    }

    RepoVerification {
        changed_files,
        implementation_files_changed,
        test_files_changed,
        setup_files_changed,
    }
}

pub fn strip_restart_heavy_messages(messages: &mut Vec<ConversationMessage>, keep_from: usize) {
    let mut trimmed = messages.iter().take(keep_from).cloned().collect::<Vec<_>>();
    trimmed.retain(|message| {
        !(message.role == "system"
            && (message.content.starts_with("[compact-summary]")
                || message.content.starts_with("[Actor Restart]")
                || message.content.starts_with("Verifier state:")))
    });
    *messages = trimmed;
}

fn stable_hash(bytes: &[u8]) -> u64 {
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish()
}

fn should_skip_path(root: &Path, path: &Path) -> bool {
    path.strip_prefix(root)
        .ok()
        .map(|relative| {
            relative.components().any(|component| {
                matches!(
                    component.as_os_str().to_str(),
                    Some(".git" | ".anvil" | "node_modules" | "target")
                )
            })
        })
        .unwrap_or(false)
}

fn is_test_file(path: &Path) -> bool {
    let display = path.display().to_string();
    display.contains("__tests__") || display.contains(".test.") || display.contains(".spec.")
}

fn is_setup_file(path: &Path) -> bool {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    matches!(
        file_name,
        "package.json"
            | "package-lock.json"
            | "pnpm-lock.yaml"
            | "yarn.lock"
            | "tsconfig.json"
            | "jest.config.js"
            | "jest.config.ts"
            | "vitest.config.ts"
            | "vitest.config.js"
            | "next.config.ts"
            | "next.config.js"
            | "eslint.config.js"
            | "eslint.config.mjs"
    )
}

fn is_implementation_file(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|ext| ext.to_str()),
        Some(
            "rs" | "ts"
                | "tsx"
                | "js"
                | "jsx"
                | "py"
                | "go"
                | "java"
                | "kt"
                | "swift"
                | "c"
                | "cc"
                | "cpp"
                | "h"
                | "hpp"
                | "css"
                | "scss"
                | "html"
                | "mdx"
                | "vue"
                | "svelte"
        )
    )
}
