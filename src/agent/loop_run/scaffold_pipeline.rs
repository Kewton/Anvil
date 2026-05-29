//! Issue #683 (parent #680, Phase 3): scaffold / deterministic-scaffold
//! pipeline data types + telemetry-event-name constants extracted from
//! `turn.rs`.
//!
//! Hosts:
//!
//! * `ScaffoldFramework` — Next / React / Nuxt framework discriminator
//!   used by the empty-workspace scaffold helpers.
//! * `PlanExplorationKey` — `(stage, tool_name, normalized_args)` triple
//!   used by the actor-loop plan-mode repeated-exploration detector.
//! * `ScaffoldFallbackResult` — NotApplicable / Applied / Failed /
//!   Skipped enum returned by the scaffold fallback dispatch.
//! * `DeterministicScaffoldSpec` — bounded `(label, event,
//!   scaffold_kind, files)` tuple that the deterministic install helpers
//!   feed to the file writer.
//!
//! Telemetry-event-name constants (kept as `pub(super) const` so the
//! string values stay shared between the emit site and the tests that
//! pin them):
//!
//! * `EVENT_DETERMINISTIC_FASTAPI_SCAFFOLD`
//! * `EVENT_DETERMINISTIC_PYTHON_CLI`
//! * `EVENT_DETERMINISTIC_FORMAT_ERROR_SMALL_EDIT`
//! * `EVENT_DETERMINISTIC_PYTHON_TEST_FALLBACK`
//! * `CREATE_NEXT_APP_PACKAGE_VERSION` — `create-next-app@<v>` pin so
//!   the deterministic Next.js scaffold path stays reproducible.
//!
//! Phase 3 scope (Issue #683): this PR migrates the **type + const
//! definitions only**. Scaffold install / framework detection / fallback
//! dispatch methods on `impl Agent` stay in `turn.rs` for now and will
//! be migrated in follow-up PRs, mirroring the Phase 1 /
//! actor_loop_flow pattern.
//!
//! `pub(super)` limited / no facade re-export (DR3-001).
//! `turn.rs` is the only in-crate consumer.

use std::path::{Path, PathBuf};

use super::completion_evidence::{RepoEditCategory, classify_repo_edit_path};
use super::deterministic;
use super::quality::{first_existing_impl_target, implementation_quality_issue_for_request};
use super::task_contract::ArtifactRole;
use super::tool_history::latest_user_turn_slice;
use super::turn::{current_file_hash_for_relative_path, meaningful_workspace_files, sha256_hex};
use crate::agent::recovery;
use crate::ollama::client::AssistantReply;
use crate::ollama::xml_fallback::ToolCall;
use crate::safety::path_guard::resolve_user_path;
use crate::session::store::{
    ConversationMessage, ScaffoldArtifactFileSnapshot, ScaffoldArtifactRole,
    ScaffoldArtifactSnapshot,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ScaffoldFramework {
    Next,
    React,
    Nuxt,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) struct PlanExplorationKey {
    pub(super) stage: String,
    pub(super) tool_name: String,
    pub(super) normalized_args: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ScaffoldFallbackResult {
    NotApplicable,
    Applied,
    Failed,
    Skipped,
}

pub(super) struct DeterministicScaffoldSpec {
    pub(super) label: &'static str,
    pub(super) event: &'static str,
    pub(super) scaffold_kind: &'static str,
    pub(super) files: Vec<(PathBuf, String)>,
}

pub(super) const EVENT_DETERMINISTIC_FASTAPI_SCAFFOLD: &str =
    "agent.empty_workspace.deterministic_fastapi_scaffold";
pub(super) const EVENT_DETERMINISTIC_PYTHON_CLI: &str =
    "agent.empty_workspace.deterministic_python_cli";
pub(super) const EVENT_DETERMINISTIC_FORMAT_ERROR_SMALL_EDIT: &str =
    "agent.deterministic_format_error_small_edit";
pub(super) const EVENT_DETERMINISTIC_PYTHON_TEST_FALLBACK: &str =
    "agent.empty_workspace.deterministic_python_test_fallback";
pub(super) const CREATE_NEXT_APP_PACKAGE_VERSION: &str = "16.2.4";

pub(super) fn task_requires_nextjs_scaffold(task: &str) -> bool {
    requested_scaffold_framework(task) == Some(ScaffoldFramework::Next)
}

pub(super) fn requested_scaffold_framework(task: &str) -> Option<ScaffoldFramework> {
    let normalized = task.to_ascii_lowercase();
    if normalized.contains("next.js") || normalized.contains("nextjs") {
        Some(ScaffoldFramework::Next)
    } else if normalized.contains("nuxt.js") || normalized.contains("nuxt") {
        Some(ScaffoldFramework::Nuxt)
    } else if normalized.contains("react.js") || normalized.contains("react") {
        Some(ScaffoldFramework::React)
    } else {
        None
    }
}

pub(super) fn scaffold_command_matches_framework(
    framework: ScaffoldFramework,
    command: &str,
) -> bool {
    let normalized = command.to_ascii_lowercase();
    match framework {
        ScaffoldFramework::Next => normalized.contains("create-next-app"),
        ScaffoldFramework::React => {
            (normalized.contains("create vite")
                || normalized.contains("create-vite")
                || normalized.contains("vite@latest")
                || normalized.contains("vite@"))
                && normalized.contains("react")
        }
        ScaffoldFramework::Nuxt => {
            normalized.contains("nuxi")
                || normalized.contains("create-nuxt")
                || normalized.contains("create nuxt")
                || normalized.contains("nuxt@")
        }
    }
}

pub(super) fn task_or_plan_requires_nextjs_scaffold(
    active_task: Option<&str>,
    plan_contents: Option<&str>,
) -> bool {
    active_task.is_some_and(task_requires_nextjs_scaffold)
        || plan_contents.is_some_and(task_requires_nextjs_scaffold)
}

pub(super) fn deterministic_nextjs_scaffold_reply() -> AssistantReply {
    let command = format!(
        "npx --yes create-next-app@{CREATE_NEXT_APP_PACKAGE_VERSION} . --typescript --tailwind --eslint --app --no-src-dir --import-alias \"@/*\" --use-npm --yes"
    );
    AssistantReply {
        content: String::new(),
        tool_calls: vec![ToolCall {
            id: "deterministic-nextjs-scaffold-1".to_string(),
            name: "Bash".to_string(),
            arguments: serde_json::json!({
                "command": command
            }),
        }],
        prompt_tokens: None,
        completion_tokens: None,
    }
}

pub(super) const DETERMINISTIC_FRAMEWORK_APP_FALLBACK_MARKER: &str =
    "Materialized deterministic framework app fallback files";

pub(super) fn recent_post_scaffold_edit_attempt(messages: &[ConversationMessage]) -> usize {
    latest_user_turn_slice(messages)
        .iter()
        .rev()
        .find_map(|message| {
            if message.role != "system" {
                return None;
            }
            message
                .content
                .rsplit("post_scaffold_edit_attempt=")
                .next()
                .and_then(|suffix| suffix.trim().parse::<usize>().ok())
        })
        .unwrap_or(0)
}

pub(super) fn recent_post_scaffold_continue_attempt(messages: &[ConversationMessage]) -> usize {
    latest_user_turn_slice(messages)
        .iter()
        .rev()
        .find_map(|message| {
            if message.role != "system" {
                return None;
            }
            message
                .content
                .rsplit("post_scaffold_continue_attempt=")
                .next()
                .and_then(|suffix| suffix.trim().parse::<usize>().ok())
        })
        .unwrap_or(0)
}

pub(super) fn recent_scaffold_command_seen(messages: &[ConversationMessage]) -> bool {
    latest_user_turn_slice(messages)
        .iter()
        .rev()
        .any(|message| {
            if message.role != "assistant" {
                return false;
            }
            message.tool_calls.iter().rev().any(|tool_call| {
                // Issue #664: legacy recovery-side scaffold detector.
                #[allow(deprecated)]
                let is_scaffold = tool_call.name == "Bash"
                    && tool_call
                        .arguments
                        .get("command")
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(recovery::is_scaffold_command);
                is_scaffold
            })
        })
}

pub(super) fn recent_deterministic_framework_app_fallback_seen(
    messages: &[ConversationMessage],
) -> bool {
    latest_user_turn_slice(messages)
        .iter()
        .rev()
        .any(|message| {
            message.role == "assistant"
                && message
                    .content
                    .contains(DETERMINISTIC_FRAMEWORK_APP_FALLBACK_MARKER)
        })
}

pub(super) fn post_scaffold_recovery_active(
    messages: &[ConversationMessage],
    active_root: Option<&Path>,
    cwd: &Path,
) -> bool {
    recent_scaffold_command_seen(messages)
        || recent_deterministic_framework_app_fallback_seen(messages)
        || recent_post_scaffold_edit_attempt(messages) > 0
        || (active_root.is_some_and(|root| root != cwd)
            && latest_user_turn_slice(messages).iter().any(|message| {
                message.role == "system"
                    && message
                        .content
                        .trim_start()
                        .starts_with("[Workspace Root Updated]")
            }))
}

pub(super) fn post_scaffold_continuation_active(
    _messages: &[ConversationMessage],
    _active_root: Option<&Path>,
    _cwd: &Path,
    _work_root: &Path,
    _plan_path: Option<&Path>,
) -> bool {
    // A second forced microscopic edit tends to trap scaffolded apps in
    // placeholder-copy churn. After the first repo edit, let the normal
    // implementation loop and final quality gate drive the next action.
    false
}

pub(super) fn render_deterministic_scaffold_continuation_note(
    request: &str,
    written_paths: &[String],
) -> String {
    let request_json = serde_json::to_string(request).unwrap_or_else(|_| "\"<invalid>\"".into());
    format!(
        "[Deterministic Scaffold] The generated files are bootstrap scaffold only and do not satisfy the task by themselves. request_json={request_json}. Read and edit the scaffold to implement the user's specific requirements, including domain-specific implementation, tests, and usage documentation. Existing scaffold files: {}. Do not give a final answer until the implementation, tests, and docs match request_json and verification has run.",
        written_paths.join(", ")
    )
}

pub(super) fn deterministic_framework_game_files_needed(
    work_root: &Path,
    files: &[(PathBuf, String)],
) -> bool {
    let impl_paths = files
        .iter()
        .map(|(path, _)| path)
        .filter(|path| deterministic_framework_game_impl_path(path))
        .collect::<Vec<_>>();
    if impl_paths.is_empty() || impl_paths.iter().any(|path| work_root.join(path).is_file()) {
        return false;
    }

    let Some(existing_files) = meaningful_workspace_files(work_root, 32) else {
        return false;
    };
    if existing_files.is_empty() {
        return true;
    }

    existing_files.iter().all(|existing| {
        files
            .iter()
            .filter(|(path, _)| !deterministic_framework_game_impl_path(path))
            .any(|(path, _)| path == existing)
    })
}

pub(super) fn deterministic_framework_app_files_needed(
    work_root: &Path,
    files: &[(PathBuf, String)],
    request: &str,
) -> bool {
    if deterministic_framework_game_files_needed(work_root, files) {
        return true;
    }

    let Some(target) = first_existing_impl_target(work_root) else {
        return false;
    };
    let Ok(current) = std::fs::read_to_string(&target) else {
        return false;
    };
    if implementation_quality_issue_for_request(request, &current).is_none() {
        return false;
    }
    deterministic::playable_ui_repair(request, &target, &current).is_some()
}

pub(super) fn scaffold_file_snapshot(path: &str, content: &[u8]) -> ScaffoldArtifactFileSnapshot {
    ScaffoldArtifactFileSnapshot {
        path: path.to_string(),
        content_hash: sha256_hex(content),
        roles: vec![scaffold_role_for_path(Path::new(path))],
        bootstrap_only: true,
    }
}

pub(super) fn scaffold_role_for_path(path: &Path) -> ScaffoldArtifactRole {
    match classify_repo_edit_path(path) {
        RepoEditCategory::Impl => ScaffoldArtifactRole::Implementation,
        RepoEditCategory::Test => ScaffoldArtifactRole::Test,
        RepoEditCategory::Docs => ScaffoldArtifactRole::UsageDocs,
        RepoEditCategory::Setup => ScaffoldArtifactRole::Setup,
        RepoEditCategory::Other => ScaffoldArtifactRole::Other,
    }
}

pub(super) fn scaffold_role_matches_artifact_role(
    candidate: ScaffoldArtifactRole,
    role: ArtifactRole,
) -> bool {
    matches!(
        (candidate, role),
        (
            ScaffoldArtifactRole::Implementation,
            ArtifactRole::Implementation
        ) | (ScaffoldArtifactRole::Test, ArtifactRole::Test)
            | (ScaffoldArtifactRole::UsageDocs, ArtifactRole::UsageDocs)
            | (ScaffoldArtifactRole::Setup, ArtifactRole::Setup)
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ScaffoldDiffStatus {
    NotScaffold,
    Changed,
    UnchangedOrMissing,
}

pub(super) fn scaffold_diff_status(
    snapshots: &[ScaffoldArtifactSnapshot],
    relative_path: &str,
    current_hash: Option<&str>,
) -> ScaffoldDiffStatus {
    let Some(file) = snapshots
        .iter()
        .rev()
        .flat_map(|snapshot| snapshot.files.iter())
        .find(|file| file.bootstrap_only && file.path == relative_path)
    else {
        return ScaffoldDiffStatus::NotScaffold;
    };
    match current_hash {
        Some(hash) if hash != file.content_hash => ScaffoldDiffStatus::Changed,
        _ => ScaffoldDiffStatus::UnchangedOrMissing,
    }
}

pub(super) fn scaffold_candidate_for_missing_role_from_snapshots(
    snapshots: &[ScaffoldArtifactSnapshot],
    work_root: &Path,
    role: ArtifactRole,
) -> Option<String> {
    let mut candidates = snapshots
        .iter()
        .rev()
        .flat_map(|snapshot| snapshot.files.iter())
        .filter(|file| {
            file.bootstrap_only
                && file
                    .roles
                    .iter()
                    .any(|candidate| scaffold_role_matches_artifact_role(*candidate, role))
                && matches!(
                    scaffold_diff_status(
                        snapshots,
                        &file.path,
                        current_file_hash_for_relative_path(work_root, &file.path).as_deref(),
                    ),
                    ScaffoldDiffStatus::UnchangedOrMissing
                )
        })
        .collect::<Vec<_>>();
    candidates.sort_by_key(|file| scaffold_candidate_priority(work_root, role, &file.path));
    candidates.first().map(|file| file.path.clone())
}

pub(super) fn scaffold_candidate_priority(
    work_root: &Path,
    role: ArtifactRole,
    relative_path: &str,
) -> u8 {
    if role != ArtifactRole::Implementation {
        return 0;
    }
    let path = Path::new(relative_path);
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    let len = resolve_user_path(work_root, relative_path)
        .ok()
        .and_then(|path| std::fs::metadata(path).ok())
        .map(|meta| meta.len())
        .unwrap_or(0);
    if len == 0 {
        return 40;
    }
    if matches!(file_name, "__init__.py" | "mod.rs") && len <= 128 {
        return 30;
    }
    if matches!(
        file_name,
        "main.py"
            | "main.rs"
            | "main.ts"
            | "main.tsx"
            | "app.py"
            | "server.py"
            | "server.ts"
            | "lib.rs"
            | "index.ts"
            | "index.tsx"
    ) {
        return 0;
    }
    10
}

pub(super) fn deterministic_framework_game_impl_path(path: &Path) -> bool {
    matches!(
        path.to_string_lossy().as_ref(),
        "app.vue" | "src/App.tsx" | "src/app/page.tsx" | "app/page.tsx" | "src/routes/+page.svelte"
    )
}

pub(super) fn deterministic_support_target_relative(work_root: &Path, relative: &Path) -> PathBuf {
    if let Ok(rest) = relative.strip_prefix("src/app")
        && work_root.join("app").is_dir()
    {
        return PathBuf::from("app").join(rest);
    }
    if let Ok(rest) = relative.strip_prefix("app")
        && work_root.join("src/app").is_dir()
    {
        return PathBuf::from("src/app").join(rest);
    }
    relative.to_path_buf()
}
