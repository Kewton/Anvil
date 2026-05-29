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

use std::io::{self, IsTerminal};
use std::path::{Path, PathBuf};

use super::Agent;
use super::actor_loop_flow::{
    build_feedback_for_deterministic_content_fallback, format_iteration_status,
};
use super::completion_evidence::{RepoEditCategory, classify_repo_edit_path};
use super::deterministic;
use super::interrupt::InterruptFlag;
use super::lifecycle;
use super::quality::{first_existing_impl_target, implementation_quality_issue_for_request};
use super::task_contract::{ArtifactRole, CompletionDecision};
use super::tool_history::{
    focused_edit_target_already_read, has_successful_non_plan_repo_edit, latest_user_turn_slice,
};
use super::turn::{
    WrittenScaffoldArtifacts, current_file_hash_for_relative_path,
    deterministic_timeout_fallback_plan, extract_filename_with_suffix, last_read_tool_path,
    latest_turn_preferred_read_edit_target, meaningful_workspace_files, normalize_memory_path,
    progress_path_display, sha256_hex, tool_result_failed, workspace_appears_empty,
    write_stdout_rendered,
};
use crate::agent::prompting;
use crate::agent::recovery;
use crate::logging::log_llm_event;
use crate::modes::plan_act::{ExecutionMode, ModePolicy, PlanStage};
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

impl ScaffoldFramework {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Next => "Next.js",
            Self::React => "React.js",
            Self::Nuxt => "Nuxt.js",
        }
    }

    pub(super) fn scaffold_hint(self) -> &'static str {
        match self {
            Self::Next => "Use create-next-app for the scaffold.",
            Self::React => {
                "Use a Vite React scaffold, for example: npm create vite@latest . -- --template react-ts."
            }
            Self::Nuxt => {
                "Use a Nuxt scaffold, for example: npx nuxi@latest init . --packageManager npm."
            }
        }
    }
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

pub(super) fn post_scaffold_edit_recovery_message(agent: &Agent) -> Option<String> {
    let path = post_scaffold_edit_recovery_target(agent)?;
    let attempt = recent_post_scaffold_edit_attempt(&agent.session.messages).max(1);
    let already_read =
        focused_edit_target_already_read(&agent.session.messages, &path, &agent.work_root);
    Some(recovery::post_scaffold_edit_recovery_note(
        &progress_path_display(
            &path.display().to_string(),
            &agent.work_root,
            agent.session.mode_state.active_plan_path.as_deref(),
            120,
        ),
        already_read,
        attempt,
    ))
}

pub(super) fn post_scaffold_continuation_recovery_message(agent: &Agent) -> Option<String> {
    let path = post_scaffold_continuation_recovery_target(agent)?;
    let attempt = recent_post_scaffold_continue_attempt(&agent.session.messages).max(1);
    Some(recovery::post_scaffold_continuation_note(
        &progress_path_display(
            &path.display().to_string(),
            &agent.work_root,
            agent.session.mode_state.active_plan_path.as_deref(),
            120,
        ),
        attempt,
    ))
}

pub(super) fn post_scaffold_edit_recovery_target(agent: &Agent) -> Option<PathBuf> {
    if agent.session.mode_state.mode != ExecutionMode::Act {
        return None;
    }
    if has_successful_non_plan_repo_edit(
        &agent.session.messages,
        &agent.work_root,
        agent.session.mode_state.active_plan_path.as_deref(),
    ) || !post_scaffold_recovery_active(
        &agent.session.messages,
        agent.session.active_root.as_deref(),
        &agent.config.cwd,
    ) {
        return None;
    }
    if let Some(candidate) = first_existing_impl_target(&agent.work_root) {
        return Some(candidate);
    }
    if let Some(candidate) =
        latest_turn_preferred_read_edit_target(&agent.session.messages, &agent.work_root)
    {
        return Some(candidate);
    }
    if let Some(path) = last_read_tool_path(&agent.session.messages)
        && let Ok(candidate) = resolve_user_path(&agent.work_root, &path)
        && candidate.is_file()
    {
        return Some(candidate);
    }
    first_existing_impl_target(&agent.work_root)
}

pub(super) fn post_scaffold_continuation_recovery_target(agent: &Agent) -> Option<PathBuf> {
    if agent.session.mode_state.mode != ExecutionMode::Act {
        return None;
    }
    if !post_scaffold_continuation_active(
        &agent.session.messages,
        agent.session.active_root.as_deref(),
        &agent.config.cwd,
        &agent.work_root,
        agent.session.mode_state.active_plan_path.as_deref(),
    ) {
        return None;
    }
    if let Some(candidate) = first_existing_impl_target(&agent.work_root) {
        return Some(candidate);
    }
    if let Some(path) = last_read_tool_path(&agent.session.messages)
        && let Ok(candidate) = resolve_user_path(&agent.work_root, &path)
        && candidate.is_file()
    {
        return Some(candidate);
    }
    first_existing_impl_target(&agent.work_root)
}

pub(super) fn active_task_requires_nextjs_scaffold(agent: &Agent) -> bool {
    if agent.session.mode_state.mode != ExecutionMode::Act {
        return false;
    }
    let plan_contents = agent.current_plan_contents().ok().flatten();
    task_or_plan_requires_nextjs_scaffold(
        agent.active_request_text().as_deref(),
        plan_contents.as_deref(),
    )
}

pub(super) fn active_task_requested_scaffold_framework(agent: &Agent) -> Option<ScaffoldFramework> {
    if agent.session.mode_state.mode != ExecutionMode::Act {
        return None;
    }
    agent
        .active_request_text()
        .as_deref()
        .and_then(requested_scaffold_framework)
}

pub(super) fn scaffold_candidate_for_missing_role(
    agent: &Agent,
    role: ArtifactRole,
) -> Option<String> {
    scaffold_candidate_for_missing_role_from_snapshots(
        &agent.session.scaffold_artifact_snapshots,
        &agent.work_root,
        role,
    )
}

pub(super) fn repo_edit_has_post_scaffold_delta(agent: &Agent, relative_path: &str) -> bool {
    match scaffold_diff_status(
        &agent.session.scaffold_artifact_snapshots,
        relative_path,
        current_file_hash_for_relative_path(&agent.work_root, relative_path).as_deref(),
    ) {
        ScaffoldDiffStatus::NotScaffold => true,
        ScaffoldDiffStatus::Changed => true,
        ScaffoldDiffStatus::UnchangedOrMissing => false,
    }
}

pub(super) fn deterministic_nextjs_scaffold_skip_reason(agent: &Agent) -> Option<&'static str> {
    if agent.config.offline {
        return Some("offline mode blocks network scaffolding");
    }
    if !agent.config.yes_mode && !io::stdin().is_terminal() {
        return Some("network scaffolding requires yes mode or an interactive approval prompt");
    }
    None
}

pub(super) fn empty_workspace_scaffold_policy_error(
    agent: &Agent,
    name: &str,
    arguments: &serde_json::Value,
) -> Option<String> {
    let requested_framework = active_task_requested_scaffold_framework(agent)?;
    if !workspace_appears_empty(&agent.work_root)
        || has_successful_non_plan_repo_edit(
            &agent.session.messages,
            &agent.work_root,
            agent.session.mode_state.active_plan_path.as_deref(),
        )
    {
        return None;
    }
    let label = requested_framework.label();
    if name != "Bash" {
        return Some(format!(
            "Error: empty workspace {label} tasks require one scaffold Bash command first. Do not write package.json or placeholder files by hand."
        ));
    }
    let command = arguments
        .get("command")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    #[allow(deprecated)]
    let scaffold_ok = recovery::is_scaffold_command(command);
    if !scaffold_ok {
        return Some(format!(
            "Error: empty workspace {label} tasks require one scaffold Bash command first. Do not use cd, ls, manual bootstrap commands, or deprecated scaffolds. {}",
            requested_framework.scaffold_hint()
        ));
    }
    if !scaffold_command_matches_framework(requested_framework, command) {
        return Some(format!(
            "Error: the user requested {label}. Use a {label} scaffold command, not a different framework scaffold. {}",
            requested_framework.scaffold_hint()
        ));
    }
    None
}

pub(super) fn record_scaffold_artifact_snapshot(
    agent: &mut Agent,
    request: &str,
    files: Vec<ScaffoldArtifactFileSnapshot>,
) {
    if files.is_empty() {
        return;
    }
    agent
        .session
        .scaffold_artifact_snapshots
        .push(ScaffoldArtifactSnapshot {
            created_turn_index: agent.current_turn_index,
            request_hash: sha256_hex(request.as_bytes()),
            files,
        });
    const MAX_SCAFFOLD_SNAPSHOTS: usize = 4;
    while agent.session.scaffold_artifact_snapshots.len() > MAX_SCAFFOLD_SNAPSHOTS {
        agent.session.scaffold_artifact_snapshots.remove(0);
    }
}

pub(super) fn python_csv_names_from_request_and_anvil(
    agent: &Agent,
    request: &str,
) -> (Option<String>, Option<String>) {
    let request_script = extract_filename_with_suffix(request, ".py");
    let request_sample = extract_filename_with_suffix(request, ".csv");
    let instructions = prompting::load_project_instructions(&agent.config.cwd, &agent.work_root);
    let instruction_text = instructions
        .as_ref()
        .map(|value| value.global_content.as_str());
    let instruction_script =
        instruction_text.and_then(|text| extract_filename_with_suffix(text, ".py"));
    let instruction_sample =
        instruction_text.and_then(|text| extract_filename_with_suffix(text, ".csv"));
    (
        request_script.or(instruction_script),
        request_sample.or(instruction_sample),
    )
}

pub(super) fn mode_deterministic_scaffold_spec(
    agent: &Agent,
    request: &str,
    policy: &ModePolicy,
) -> Option<DeterministicScaffoldSpec> {
    if super::policy_allows_python_specialized_fallback(policy, &agent.config) {
        if let Some(files) = deterministic::fastapi_scaffold_files(request) {
            return Some(DeterministicScaffoldSpec {
                label: "FastAPI scaffold",
                event: EVENT_DETERMINISTIC_FASTAPI_SCAFFOLD,
                scaffold_kind: "FastAPI",
                files,
            });
        }
        let (script_name, sample_name) = python_csv_names_from_request_and_anvil(agent, request);
        let files = deterministic::empty_python_cli_files_with_names(
            request,
            script_name.as_deref(),
            sample_name.as_deref(),
        )?;
        return Some(DeterministicScaffoldSpec {
            label: "Python scaffold",
            event: EVENT_DETERMINISTIC_PYTHON_CLI,
            scaffold_kind: "Python",
            files,
        });
    }
    if policy.allow_docs_deterministic_fallback {
        return deterministic::empty_docs_files(request).map(|files| DeterministicScaffoldSpec {
            label: "Docs scaffold",
            event: "agent.empty_workspace.deterministic_docs",
            scaffold_kind: "Docs",
            files,
        });
    }
    None
}

pub(super) fn write_deterministic_scaffold_files(
    agent: &mut Agent,
    files: Vec<(PathBuf, String)>,
    error_prefix: &str,
    skip_existing: bool,
) -> Option<WrittenScaffoldArtifacts> {
    let mut written = Vec::<PathBuf>::new();
    let mut snapshot_files = Vec::<ScaffoldArtifactFileSnapshot>::new();
    for (relative, content) in files {
        let target = agent.work_root.join(&relative);
        if skip_existing && target.exists() {
            continue;
        }
        if let Some(parent) = target.parent()
            && let Err(err) = std::fs::create_dir_all(parent)
        {
            agent.session.working_memory.note_error(format!(
                "{error_prefix}: failed to create {}: {err}",
                parent.display()
            ));
            return None;
        }
        if let Err(err) = std::fs::write(&target, content.as_bytes()) {
            agent.session.working_memory.note_error(format!(
                "{error_prefix}: failed to write {}: {err}",
                target.display()
            ));
            return None;
        }
        let relative_display = relative.to_string_lossy().to_string();
        snapshot_files.push(scaffold_file_snapshot(
            &relative_display.replace('\\', "/"),
            content.as_bytes(),
        ));
        agent
            .session
            .working_memory
            .note_touched_file(normalize_memory_path(&relative_display, &agent.work_root));
        written.push(relative);
    }
    Some((written, snapshot_files))
}

pub(super) fn finalize_deterministic_scaffold_materialization(
    agent: &mut Agent,
    request: &str,
    last_iter: usize,
    spec: &DeterministicScaffoldSpec,
    assistant_context: &str,
    written: Vec<PathBuf>,
    snapshot_files: Vec<ScaffoldArtifactFileSnapshot>,
) {
    let written_paths = written
        .iter()
        .map(|path| path.to_string_lossy().to_string())
        .collect::<Vec<_>>();
    write_stdout_rendered(
        &format_iteration_status(
            last_iter,
            agent.config.max_iterations,
            spec.label,
            &format!(
                "Materialized deterministic scaffold files: {}.",
                written_paths.join(", ")
            ),
            agent.footer.current_cols(),
        ),
        true,
    );
    log_llm_event(
        spec.event,
        serde_json::json!({
            "session_id": agent.session_store.session_id(),
            "work_root": agent.work_root.display().to_string(),
            "work_mode": agent.session.mode_state.work_mode.as_str(),
            "fallback_level": agent.config.deterministic_fallback.fallback_level(),
            "fallback_action": "bootstrap_scaffold",
            "completion_evidence": false,
            "files": written_paths,
        }),
    );
    record_scaffold_artifact_snapshot(agent, request, snapshot_files);
    agent.session.messages.push(ConversationMessage::assistant(
        format!(
            "Created deterministic {} scaffold files as {assistant_context}: {}. This is not task completion.",
            spec.scaffold_kind,
            written_paths.join(", ")
        ),
        Vec::new(),
    ));
    agent.push_system_note(render_deterministic_scaffold_continuation_note(
        request,
        &written_paths,
    ));
}

pub(super) fn maybe_apply_deterministic_nextjs_scaffold(
    agent: &mut Agent,
    last_iter: usize,
    interrupt_flag: &InterruptFlag,
) -> ScaffoldFallbackResult {
    if !agent
        .config
        .deterministic_fallback
        .allows_support_recovery()
    {
        return ScaffoldFallbackResult::NotApplicable;
    }
    if !active_task_requires_nextjs_scaffold(agent)
        || !workspace_appears_empty(&agent.work_root)
        || recent_scaffold_command_seen(&agent.session.messages)
    {
        return ScaffoldFallbackResult::NotApplicable;
    }

    if let Some(reason) = deterministic_nextjs_scaffold_skip_reason(agent) {
        write_stdout_rendered(
            &format_iteration_status(
                last_iter,
                agent.config.max_iterations,
                "Scaffold fallback skipped",
                reason,
                agent.footer.current_cols(),
            ),
            true,
        );
        log_llm_event(
            "agent.empty_workspace.deterministic_nextjs_scaffold_skipped",
            serde_json::json!({
                "session_id": agent.session_store.session_id(),
                "work_root": agent.work_root.display().to_string(),
                "fallback_level": agent.config.deterministic_fallback.fallback_level(),
                "fallback_action": "minimal_patch",
                "reason": reason,
            }),
        );
        return ScaffoldFallbackResult::Skipped;
    }

    let fallback_reply = deterministic_nextjs_scaffold_reply();
    let fallback_tool_calls = fallback_reply
        .tool_calls
        .iter()
        .cloned()
        .map(|tool_call| agent.prepare_tool_call(tool_call))
        .collect::<Vec<_>>();
    agent.session.messages.push(ConversationMessage::assistant(
        fallback_reply.content,
        fallback_tool_calls.clone(),
    ));
    write_stdout_rendered(
        &format_iteration_status(
            last_iter,
            agent.config.max_iterations,
            "Scaffold fallback",
            "Empty Next.js workspace stalled on exploration; running pinned deterministic scaffold command.",
            agent.footer.current_cols(),
        ),
        true,
    );

    let mut fallback_failed = false;
    for tool_call in fallback_tool_calls {
        let raw_result = agent.execute_tool_call(
            &tool_call.name,
            &tool_call.arguments,
            None,
            Some(interrupt_flag.flag.clone()),
        );
        if tool_result_failed(&raw_result) {
            fallback_failed = true;
        }
        let compact_result = prompting::compact_tool_result(&tool_call.name, raw_result);
        agent.session.messages.push(ConversationMessage::tool(
            tool_call.name.clone(),
            compact_result,
        ));
    }

    let event = if fallback_failed {
        "agent.empty_workspace.deterministic_nextjs_scaffold_failed"
    } else {
        "agent.empty_workspace.deterministic_nextjs_scaffold"
    };
    log_llm_event(
        event,
        serde_json::json!({
            "session_id": agent.session_store.session_id(),
            "work_root": agent.work_root.display().to_string(),
            "fallback_level": agent.config.deterministic_fallback.fallback_level(),
            "fallback_action": "minimal_patch",
            "create_next_app_version": CREATE_NEXT_APP_PACKAGE_VERSION,
        }),
    );

    if fallback_failed {
        ScaffoldFallbackResult::Failed
    } else {
        ScaffoldFallbackResult::Applied
    }
}

pub(super) fn maybe_materialize_mode_deterministic_fallback(
    agent: &mut Agent,
    last_iter: usize,
) -> bool {
    if !agent
        .config
        .deterministic_fallback
        .allows_template_completion()
    {
        return false;
    }
    let policy = agent.session.mode_state.policy();
    let Some(request) = agent.active_request_text() else {
        return false;
    };
    let Some(mut spec) = mode_deterministic_scaffold_spec(agent, &request, &policy) else {
        return false;
    };
    if !workspace_appears_empty(&agent.work_root) {
        return false;
    }
    let Some((written, snapshot_files)) = write_deterministic_scaffold_files(
        agent,
        std::mem::take(&mut spec.files),
        "deterministic fallback",
        false,
    ) else {
        return false;
    };
    finalize_deterministic_scaffold_materialization(
        agent,
        &request,
        last_iter,
        &spec,
        "bootstrap only",
        written,
        snapshot_files,
    );
    true
}

pub(super) fn materialize_deterministic_fallback_plan(
    agent: &mut Agent,
    event_name: &str,
) -> Result<bool, String> {
    let Some(plan_path) = agent.session.mode_state.active_plan_path.clone() else {
        return Ok(false);
    };

    let current_contents = agent.current_plan_contents()?.unwrap_or_default();
    if lifecycle::plan_is_substantive(&current_contents) {
        return Ok(false);
    }

    let task = agent
        .session
        .working_memory
        .active_task
        .clone()
        .or_else(|| {
            agent
                .session
                .messages
                .iter()
                .rev()
                .find(|message| message.role == "user")
                .map(|message| message.content.clone())
        })
        .unwrap_or_else(|| "Complete the requested task.".to_string());

    let fallback_plan = deterministic_timeout_fallback_plan(
        &task,
        agent.session.mode_state.task_profile,
        &agent.work_root,
    );
    agent.ensure_plan_file(&plan_path)?;
    std::fs::write(&plan_path, fallback_plan).map_err(|err| {
        format!(
            "failed to write deterministic fallback plan {}: {err}",
            plan_path.display()
        )
    })?;
    agent.session.mode_state.plan_stage = PlanStage::Ready;
    log_llm_event(
        event_name,
        serde_json::json!({
            "session_id": agent.session_store.session_id(),
            "plan_path": plan_path.display().to_string(),
            "task_profile": agent.session.mode_state.task_profile.as_str(),
            "model_override": agent.plan_model_override,
            "fallback_level": agent.config.deterministic_fallback.fallback_level(),
            "fallback_action": "minimal_patch",
        }),
    );
    Ok(true)
}

pub(super) fn maybe_materialize_task_contract_fallback(
    agent: &mut Agent,
    decision: &CompletionDecision,
    last_iter: usize,
) -> bool {
    if !super::policy_allows_python_specialized_fallback(
        &agent.session.mode_state.policy(),
        &agent.config,
    ) {
        return false;
    }
    if !matches!(decision, CompletionDecision::Continue { .. }) {
        return false;
    }
    if !workspace_appears_empty(&agent.work_root) {
        return false;
    }
    let Some(request) = agent.active_request_text() else {
        return false;
    };
    let Some(files) = deterministic::fastapi_scaffold_files(&request) else {
        return false;
    };
    let mut spec = DeterministicScaffoldSpec {
        label: "Task scaffold",
        event: "agent.task_contract.deterministic_fastapi_scaffold",
        scaffold_kind: "FastAPI",
        files,
    };
    let Some((written, snapshot_files)) = write_deterministic_scaffold_files(
        agent,
        std::mem::take(&mut spec.files),
        "task contract deterministic fallback",
        true,
    ) else {
        return false;
    };
    if written.is_empty() {
        return false;
    }
    finalize_deterministic_scaffold_materialization(
        agent,
        &request,
        last_iter,
        &spec,
        "contract recovery",
        written,
        snapshot_files,
    );
    true
}

pub(super) fn maybe_materialize_framework_game_fallback(
    agent: &mut Agent,
    last_iter: usize,
) -> bool {
    if !agent.config.deterministic_fallback.allows_hint_only() {
        return false;
    }
    if !agent
        .session
        .mode_state
        .policy()
        .allow_ui_deterministic_fallback
    {
        return false;
    }
    let Some(request) = agent.active_request_text() else {
        return false;
    };
    let Some(files) = deterministic::empty_framework_app_files(&request) else {
        return false;
    };
    if !workspace_appears_empty(&agent.work_root)
        && !deterministic_framework_app_files_needed(&agent.work_root, &files, &request)
    {
        return false;
    }
    if !agent
        .config
        .deterministic_fallback
        .allows_template_completion()
    {
        let level = agent.config.deterministic_fallback.fallback_level();
        write_stdout_rendered(
            &format_iteration_status(
                last_iter,
                agent.config.max_iterations,
                "App fallback hint",
                &format!(
                    "Deterministic full-template fallback is disabled at level {level}; asked the model to continue with a task-specific implementation."
                ),
                agent.footer.current_cols(),
            ),
            true,
        );
        log_llm_event(
            "agent.empty_workspace.deterministic_framework_app_hint",
            serde_json::json!({
                "session_id": agent.session_store.session_id(),
                "work_root": agent.work_root.display().to_string(),
                "fallback_level": level,
                "fallback_action": "hint_only",
            }),
        );
        return true;
    }

    let mut written = Vec::<PathBuf>::new();
    for (relative, content) in files {
        let target = agent.work_root.join(&relative);
        if let Some(parent) = target.parent()
            && let Err(err) = std::fs::create_dir_all(parent)
        {
            agent.session.working_memory.note_error(format!(
                "deterministic fallback: failed to create {}: {err}",
                parent.display()
            ));
            return false;
        }
        if let Err(err) = std::fs::write(&target, content) {
            agent.session.working_memory.note_error(format!(
                "deterministic fallback: failed to write {}: {err}",
                target.display()
            ));
            return false;
        }
        agent
            .session
            .working_memory
            .note_touched_file(normalize_memory_path(
                &relative.to_string_lossy(),
                &agent.work_root,
            ));
        written.push(relative);
    }

    let written_paths = written
        .iter()
        .map(|path| path.to_string_lossy().to_string())
        .collect::<Vec<_>>();
    write_stdout_rendered(
        &format_iteration_status(
            last_iter,
            agent.config.max_iterations,
            "App fallback",
            &format!(
                "Materialized deterministic framework app files: {}.",
                written_paths.join(", ")
            ),
            agent.footer.current_cols(),
        ),
        true,
    );
    log_llm_event(
        "agent.empty_workspace.deterministic_framework_app",
        serde_json::json!({
            "session_id": agent.session_store.session_id(),
            "work_root": agent.work_root.display().to_string(),
            "fallback_level": agent.config.deterministic_fallback.fallback_level(),
            "fallback_action": "full_template",
            "files": written_paths,
        }),
    );
    agent
        .session
        .record_feedback_if_unset(build_feedback_for_deterministic_content_fallback(
            &agent.work_root,
        ));
    agent.session.messages.push(ConversationMessage::assistant(
        format!(
            "Materialized deterministic framework app fallback files as a recovery scaffold: {}. Continue implementation and verification before treating the task as complete.",
            written_paths.join(", ")
        ),
        Vec::new(),
    ));
    true
}
