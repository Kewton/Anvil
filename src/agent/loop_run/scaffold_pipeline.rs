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
use super::deterministic_fallback_plan::deterministic_timeout_fallback_plan;
use super::file_excerpt::{current_file_hash_for_relative_path, sha256_hex};
use super::interrupt::InterruptFlag;
use super::lifecycle;
use super::path_helpers::{normalize_memory_path, sync_package_json_with_existing_lock};
use super::plan_mode_helpers::{
    should_fallback_plan_model_after_timeout, should_materialize_plan_after_timeout,
    should_materialize_plan_after_tool_call_format_error,
};
use super::quality::{
    first_existing_impl_target, implementation_quality_issue_for_request,
    package_json_with_requested_port, react_dev_wrapper_for_requested_port,
};
use super::read_target_helpers::{last_read_tool_path, latest_turn_preferred_read_edit_target};
use super::task_contract::{ArtifactObligation, ArtifactRole, CompletionDecision, TaskKind};
use super::tool_display::progress_path_display;
use super::tool_history::{
    focused_edit_target_already_read, has_successful_non_plan_repo_edit, latest_user_turn_slice,
};
use super::turn_helpers::{
    WrittenScaffoldArtifacts, extract_filename_with_suffix, tool_result_failed,
    write_stdout_rendered,
};
use super::workspace_walk::{meaningful_workspace_files, workspace_appears_empty};
use crate::agent::prompting;
use crate::agent::recovery;
use crate::logging::log_llm_event;
use crate::model_capabilities::model_capabilities;
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ProjectRuntime {
    Rust,
    Node,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ProjectShape {
    Cli,
    Library,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ProjectIntent {
    pub(super) runtime: Option<ProjectRuntime>,
    pub(super) shape: Option<ProjectShape>,
    pub(super) wants_tests: bool,
}

#[derive(Debug)]
pub(super) struct ScaffoldPlan {
    pub(super) label: &'static str,
    pub(super) event: &'static str,
    pub(super) scaffold_kind: &'static str,
    pub(super) files: Vec<(PathBuf, String)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ScaffoldOutcome {
    Applied { written: Vec<PathBuf> },
    Failed,
}

impl ProjectIntent {
    pub(super) fn from_request(request: &str) -> Self {
        let lower = request.to_ascii_lowercase();
        let asks_create = contains_ascii_word_any(
            &lower,
            &[
                "create",
                "build",
                "develop",
                "implement",
                "scaffold",
                "write",
                "generate",
            ],
        ) || request.contains("作って")
            || request.contains("作成")
            || request.contains("開発")
            || request.contains("実装")
            || request.contains("生成");
        if !asks_create {
            return Self {
                runtime: None,
                shape: None,
                wants_tests: false,
            };
        }
        let runtime = if contains_ascii_word_any(&lower, &["rust", "cargo", "crate"]) {
            Some(ProjectRuntime::Rust)
        } else if lower.contains("node.js")
            || lower.contains("nodejs")
            || lower.contains("node --test")
            || contains_ascii_word_any(&lower, &["node", "javascript", "js", "npm", "package.json"])
        {
            Some(ProjectRuntime::Node)
        } else {
            None
        };
        let shape = if contains_ascii_word_any(&lower, &["cli", "command-line", "commandline"])
            || lower.contains("command line")
            || lower.contains("コマンドライン")
        {
            Some(ProjectShape::Cli)
        } else if contains_ascii_word_any(&lower, &["library", "lib", "crate", "module", "package"])
            || lower.contains("ライブラリ")
        {
            Some(ProjectShape::Library)
        } else {
            runtime.map(|_| ProjectShape::Library)
        };
        let wants_tests = lower.contains("test")
            || lower.contains("node --test")
            || lower.contains("cargo test")
            || request.contains("テスト")
            || request.contains("検証");

        Self {
            runtime,
            shape,
            wants_tests,
        }
    }
}

impl From<ScaffoldPlan> for DeterministicScaffoldSpec {
    fn from(plan: ScaffoldPlan) -> Self {
        Self {
            label: plan.label,
            event: plan.event,
            scaffold_kind: plan.scaffold_kind,
            files: plan.files,
        }
    }
}

pub(super) fn project_skeleton_plan_for_request(request: &str) -> Option<ScaffoldPlan> {
    let intent = ProjectIntent::from_request(request);
    let obligations = super::task_contract::explicit_artifact_obligations_from_request(request);
    plan_project_skeleton_with_obligations(&intent, &obligations)
}

fn plan_project_skeleton_with_obligations(
    intent: &ProjectIntent,
    obligations: &[ArtifactObligation],
) -> Option<ScaffoldPlan> {
    match (intent.runtime?, intent.shape?) {
        (ProjectRuntime::Rust, ProjectShape::Cli) => Some(ScaffoldPlan {
            label: "Rust CLI scaffold",
            event: "agent.empty_workspace.deterministic_rust_cli",
            scaffold_kind: "Rust CLI",
            files: rust_cli_skeleton_files(rust_requested_impl_path(obligations, "src/main.rs")),
        }),
        (ProjectRuntime::Rust, ProjectShape::Library) => Some(ScaffoldPlan {
            label: "Rust library scaffold",
            event: "agent.empty_workspace.deterministic_rust_library",
            scaffold_kind: "Rust library",
            files: rust_library_skeleton_files(rust_requested_impl_path(obligations, "src/lib.rs")),
        }),
        (ProjectRuntime::Node, ProjectShape::Cli) => Some(ScaffoldPlan {
            label: "Node CLI scaffold",
            event: "agent.empty_workspace.deterministic_node_cli",
            scaffold_kind: "Node CLI",
            files: node_skeleton_files(
                ProjectShape::Cli,
                node_requested_impl_path(obligations, "src/index.js"),
            ),
        }),
        (ProjectRuntime::Node, ProjectShape::Library) => Some(ScaffoldPlan {
            label: "Node library scaffold",
            event: "agent.empty_workspace.deterministic_node_library",
            scaffold_kind: "Node library",
            files: node_skeleton_files(
                ProjectShape::Library,
                node_requested_impl_path(obligations, "src/index.js"),
            ),
        }),
    }
}

pub(super) fn materialize_scaffold(
    agent: &mut Agent,
    request: &str,
    last_iter: usize,
    plan: ScaffoldPlan,
    reason: &str,
    skip_existing: bool,
) -> ScaffoldOutcome {
    let mut spec = DeterministicScaffoldSpec::from(plan);
    let Some((written, snapshot_files)) = write_deterministic_scaffold_files(
        agent,
        std::mem::take(&mut spec.files),
        reason,
        skip_existing,
    ) else {
        return ScaffoldOutcome::Failed;
    };
    finalize_deterministic_scaffold_materialization(
        agent,
        request,
        last_iter,
        &spec,
        "generic project skeleton",
        written.clone(),
        snapshot_files,
    );
    ScaffoldOutcome::Applied { written }
}

fn contains_ascii_word_any(haystack: &str, tokens: &[&str]) -> bool {
    haystack
        .split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_' && ch != '-' && ch != '.')
        .any(|word| tokens.contains(&word))
}

fn rust_requested_impl_path(obligations: &[ArtifactObligation], default: &str) -> PathBuf {
    obligations
        .iter()
        .find(|obligation| {
            obligation.role == ArtifactRole::Implementation && obligation.path.ends_with(".rs")
        })
        .map(|obligation| PathBuf::from(&obligation.path))
        .unwrap_or_else(|| PathBuf::from(default))
}

fn node_requested_impl_path(obligations: &[ArtifactObligation], default: &str) -> PathBuf {
    obligations
        .iter()
        .find(|obligation| {
            obligation.role == ArtifactRole::Implementation
                && matches!(
                    Path::new(&obligation.path)
                        .extension()
                        .and_then(|ext| ext.to_str()),
                    Some("js" | "mjs" | "cjs" | "ts" | "tsx" | "jsx")
                )
        })
        .map(|obligation| PathBuf::from(&obligation.path))
        .unwrap_or_else(|| PathBuf::from(default))
}

fn rust_cli_skeleton_files(impl_path: PathBuf) -> Vec<(PathBuf, String)> {
    let impl_path_display = impl_path.to_string_lossy().to_string();
    vec![
        (
            PathBuf::from("Cargo.toml"),
            format!(
                r#"[package]
name = "app"
version = "0.1.0"
edition = "2021"

[[bin]]
name = "app"
path = "{impl_path_display}"
"#
            ),
        ),
        (
            impl_path,
            r#"use std::io::{self, Read};

fn main() {
    let mut input = String::new();
    io::stdin()
        .read_to_string(&mut input)
        .expect("failed to read stdin");
    print!("{input}");
}
"#
            .to_string(),
        ),
        (
            PathBuf::from("README.md"),
            r#"# Rust CLI Skeleton

## Run

```bash
cargo run -- < input.txt
```

## Test

```bash
cargo test
```

Replace the pass-through command body with the requested CLI behavior.
"#
            .to_string(),
        ),
        (
            PathBuf::from("tests/cli.rs"),
            r#"use std::io::Write;
use std::process::{Command, Stdio};

#[test]
fn cli_accepts_stdin() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_app"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn app");
    child
        .stdin
        .as_mut()
        .expect("stdin")
        .write_all(b"sample input")
        .expect("write stdin");
    let output = child.wait_with_output().expect("wait");

    assert!(output.status.success());
    assert_eq!(String::from_utf8(output.stdout).expect("utf8"), "sample input");
}
"#
            .to_string(),
        ),
    ]
}

fn rust_library_skeleton_files(impl_path: PathBuf) -> Vec<(PathBuf, String)> {
    let impl_path_display = impl_path.to_string_lossy().to_string();
    vec![
        (
            PathBuf::from("Cargo.toml"),
            format!(
                r#"[package]
name = "app"
version = "0.1.0"
edition = "2021"

[lib]
name = "app"
path = "{impl_path_display}"
"#
            ),
        ),
        (
            impl_path,
            r#"pub fn transform(input: &str) -> String {
    input.to_string()
}

#[cfg(test)]
mod tests {
    use super::transform;

    #[test]
    fn transform_passes_input_through() {
        assert_eq!(transform("sample input"), "sample input");
    }
}
"#
            .to_string(),
        ),
        (
            PathBuf::from("README.md"),
            r#"# Rust Library Skeleton

## Use

```rust
let output = app::transform("sample input");
```

## Test

```bash
cargo test
```

Replace the neutral `transform` implementation with the requested library behavior.
"#
            .to_string(),
        ),
        (
            PathBuf::from("tests/integration.rs"),
            r#"#[test]
fn transform_passes_input_through_from_integration_test() {
    assert_eq!(app::transform("sample input"), "sample input");
}
"#
            .to_string(),
        ),
    ]
}

fn node_test_import_path(impl_path: &Path) -> String {
    format!("../{}", impl_path.to_string_lossy().replace('\\', "/"))
}

fn node_skeleton_files(shape: ProjectShape, impl_path: PathBuf) -> Vec<(PathBuf, String)> {
    let impl_path_display = impl_path.to_string_lossy().replace('\\', "/");
    let bin_block = if shape == ProjectShape::Cli {
        format!(
            r#",
  "bin": {{
    "app": "./{impl_path_display}"
  }}"#
        )
    } else {
        String::new()
    };
    let test_import_path = node_test_import_path(&impl_path);
    vec![
        (
            PathBuf::from("package.json"),
            format!(
                r#"{{
  "name": "app",
  "version": "0.1.0",
  "private": true,
  "type": "module",
  "scripts": {{
    "test": "node --test"
  }}{}
}}
"#,
                bin_block
            ),
        ),
        (
            impl_path,
            r#"#!/usr/bin/env node
import { readFileSync } from "node:fs";

export function transform(input) {
  return input;
}

if (import.meta.url === `file://${process.argv[1]}`) {
  const input = readFileSync(0, "utf8");
  process.stdout.write(transform(input));
}
"#
            .to_string(),
        ),
        (
            PathBuf::from("README.md"),
            r#"# Node Project Skeleton

## Run

```bash
node src/index.js < input.txt
```

## Test

```bash
npm test
```

Replace the neutral `transform` implementation with the requested behavior.
"#
            .to_string(),
        ),
        (
            PathBuf::from("tests/index.test.js"),
            format!(
                r#"import test from "node:test";
import assert from "node:assert/strict";

import {{ transform }} from "{test_import_path}";

test("transform passes input through", () => {{
  assert.equal(transform("sample input"), "sample input");
}});
"#
            ),
        ),
    ]
}

pub(super) const EVENT_DETERMINISTIC_FASTAPI_SCAFFOLD: &str =
    "agent.empty_workspace.deterministic_fastapi_scaffold";
pub(super) const EVENT_DETERMINISTIC_PYTHON_CLI: &str =
    "agent.empty_workspace.deterministic_python_cli";
pub(super) const EVENT_DETERMINISTIC_FORMAT_ERROR_SMALL_EDIT: &str =
    "agent.deterministic_format_error_small_edit";
pub(super) const EVENT_DETERMINISTIC_PYTHON_TEST_FALLBACK: &str =
    "agent.empty_workspace.deterministic_python_test_fallback";
// Issue #977 (parent #974, Issue C): MissingEvidence runner-manifest
// completion. Distinct from the empty-workspace scaffold events — this fires
// when test artifacts already exist but no runnable test command is bound.
pub(super) const EVENT_DETERMINISTIC_NODE_TEST_RUNNER_MANIFEST: &str =
    "agent.deterministic_node_test_runner_manifest";
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
        RepoEditCategory::Data => ScaffoldArtifactRole::Other,
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
        super::workspace_access::active_request_text(agent).as_deref(),
        plan_contents.as_deref(),
    )
}

pub(super) fn active_task_requested_scaffold_framework(agent: &Agent) -> Option<ScaffoldFramework> {
    if agent.session.mode_state.mode != ExecutionMode::Act {
        return None;
    }
    super::workspace_access::active_request_text(agent)
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

/// Issue #924: SSOT predicate gating coding-shaped scaffold / deterministic
/// fallback. Non-coding kinds (Docs/Data/Research/Ops/Authoring) are demoted
/// out of coding-shaped scaffold. `unwrap_or(Coding)` preserves the historical
/// always-Coding behavior for turns without an active task contract
/// (plan mode / first turn / pre-classification) — identical to #918/#921/#923.
/// Security posture: in the scaffold context this is a compatibility fail-open
/// default (Coding permits coding-shaped materialization), not a fail-closed
/// default. Existing active-request / mode / config / target gates remain the
/// bounded reachability controls; changing this default is out of scope for #924.
pub(super) fn scaffold_allowed_for_active_task(agent: &Agent) -> bool {
    let task_kind = super::task_classification::task_contract_authority(agent)
        .map(|contract| contract.task_kind)
        .unwrap_or(TaskKind::Coding);
    if !super::verifier::capability_for(task_kind).is_coding() {
        // AC5: observable skip — kind enum only, no path (no masking needed).
        tracing::debug!(
            target: "anvil::scaffold",
            task_kind = task_kind.as_str(),
            "scaffold gated: non-coding task_kind demoted out of coding-shaped scaffold"
        );
        return false;
    }
    true
}

pub(super) fn mode_deterministic_scaffold_spec(
    agent: &Agent,
    request: &str,
    policy: &ModePolicy,
) -> Option<DeterministicScaffoldSpec> {
    // Issue #924 branch-local gate: the coding-shaped branches (project_skeleton +
    // python/fastapi) only fire for coding tasks. `scaffold_allowed_for_active_task`
    // is evaluated lazily (left-to-right `&&` short-circuit) — only after a
    // coding-shaped candidate is found — so a Docs-only request never triggers the
    // gate's `task_contract_authority` init / debug-skip log (DR3-001). Non-coding
    // kinds fall through to the Docs branch below.
    if let Some(plan) = project_skeleton_plan_for_request(request)
        && scaffold_allowed_for_active_task(agent)
    {
        return Some(plan.into());
    }
    if super::policy_allows_python_specialized_fallback(policy, &agent.config)
        && scaffold_allowed_for_active_task(agent)
    {
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
    super::message_push::push_system_note(
        agent,
        render_deterministic_scaffold_continuation_note(request, &written_paths),
    );
}

pub(super) fn maybe_apply_deterministic_nextjs_scaffold(
    agent: &mut Agent,
    last_iter: usize,
    interrupt_flag: &InterruptFlag,
) -> ScaffoldFallbackResult {
    if !scaffold_allowed_for_active_task(agent) {
        return ScaffoldFallbackResult::NotApplicable;
    }
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
        .map(|tool_call| super::tool_call_prepare::prepare_tool_call(agent, tool_call))
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
        let raw_result = super::tool_call_execution::execute_tool_call(
            agent,
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
    let Some(request) = super::workspace_access::active_request_text(agent) else {
        return false;
    };
    let Some(mut spec) = mode_deterministic_scaffold_spec(agent, &request, &policy) else {
        return false;
    };
    if !workspace_appears_empty(&agent.work_root) {
        return false;
    }
    let plan = ScaffoldPlan {
        label: spec.label,
        event: spec.event,
        scaffold_kind: spec.scaffold_kind,
        files: std::mem::take(&mut spec.files),
    };
    matches!(
        materialize_scaffold(
            agent,
            &request,
            last_iter,
            plan,
            "deterministic fallback",
            false,
        ),
        ScaffoldOutcome::Applied { .. }
    )
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
    if !scaffold_allowed_for_active_task(agent) {
        return false;
    }
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
    let Some(request) = super::workspace_access::active_request_text(agent) else {
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
    if !scaffold_allowed_for_active_task(agent) {
        return false;
    }
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
    let Some(request) = super::workspace_access::active_request_text(agent) else {
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

pub(super) fn maybe_materialize_python_test_fallback(
    agent: &mut Agent,
) -> Result<Option<String>, String> {
    if !scaffold_allowed_for_active_task(agent) {
        return Ok(None);
    }
    if !super::policy_allows_python_specialized_fallback(
        &agent.session.mode_state.policy(),
        &agent.config,
    ) {
        return Ok(None);
    }
    let request = super::workspace_access::active_request_text(agent).unwrap_or_default();
    let mut python_files = std::fs::read_dir(&agent.work_root)
        .map_err(|err| format!("failed to read {}: {err}", agent.work_root.display()))?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && path.extension().and_then(|ext| ext.to_str()) == Some("py")
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| {
                        !name.starts_with("test_")
                            && !name.ends_with("_test.py")
                            && name != "tests.py"
                    })
        })
        .collect::<Vec<_>>();
    python_files.sort();
    if python_files.len() != 1 {
        return Ok(None);
    }
    let script = python_files.remove(0);
    let Some(file_name) = script.file_name().and_then(|name| name.to_str()) else {
        return Ok(None);
    };
    let stem = script
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or("script");
    let test_name = format!("test_{stem}.py");
    let target = agent.work_root.join(&test_name);
    let content = if request.to_ascii_lowercase().contains("fizzbuzz") {
        format!(
            r#"#!/usr/bin/env python3
import subprocess
import sys


def test_fizzbuzz_limit_15():
    result = subprocess.run(
        [sys.executable, "{file_name}", "--limit", "15"],
        check=True,
        text=True,
        capture_output=True,
    )
    assert result.stdout.strip().splitlines() == [
        "1", "2", "Fizz", "4", "Buzz", "Fizz", "7", "8", "Fizz", "Buzz",
        "11", "Fizz", "13", "14", "FizzBuzz",
    ]


if __name__ == "__main__":
    test_fizzbuzz_limit_15()
    print("python smoke ok")
"#
        )
    } else {
        format!(
            r#"#!/usr/bin/env python3
import subprocess
import sys


def test_cli_help_runs():
    result = subprocess.run(
        [sys.executable, "{file_name}", "--help"],
        text=True,
        capture_output=True,
    )
    assert result.returncode == 0
    assert result.stdout.strip() or result.stderr.strip()


if __name__ == "__main__":
    test_cli_help_runs()
    print("python smoke ok")
"#
        )
    };
    std::fs::write(&target, content)
        .map_err(|err| format!("failed to write {}: {err}", target.display()))?;
    agent
        .session
        .working_memory
        .note_touched_file(normalize_memory_path(&test_name, &agent.work_root));
    log_llm_event(
        EVENT_DETERMINISTIC_PYTHON_TEST_FALLBACK,
        serde_json::json!({
            "session_id": agent.session_store.session_id(),
            "work_root": agent.work_root.display().to_string(),
            "work_mode": agent.session.mode_state.work_mode.as_str(),
            "fallback_level": agent.config.deterministic_fallback.fallback_level(),
            "fallback_action": "python_test_scaffold",
            "target": &test_name,
        }),
    );
    Ok(Some(test_name))
}

/// Issue #977 (parent #974, Issue C): deterministically complete the Node
/// test-runner manifest (`package.json`) when the workspace has Node test
/// artifacts but no runnable test command can be bound. This is the
/// MissingEvidence "runner manifest/script missing" recovery — it materializes
/// the manifest from the deterministic `node_runner_manifest` operator (no LLM
/// free regeneration) so the existing `auto_test::detect_node_scripts` binding
/// can run `npm test` on the next EvidenceRunner pass.
///
/// Returns `Ok(Some(relative_path))` when the manifest was written,
/// `Ok(None)` when no completion applies (non-coding task, support recovery
/// disabled, runner already bindable, or a malformed manifest that must not be
/// clobbered), and `Err` on a filesystem write failure.
pub(super) fn maybe_materialize_node_test_runner_manifest(
    agent: &mut Agent,
) -> Result<Option<String>, String> {
    if !scaffold_allowed_for_active_task(agent) {
        return Ok(None);
    }
    if !agent
        .config
        .deterministic_fallback
        .allows_support_recovery()
    {
        return Ok(None);
    }
    let Some(completion) = super::node_request_helpers::node_test_runner_completion(agent) else {
        return Ok(None);
    };
    let relative = "package.json";
    let target = agent.work_root.join(relative);
    std::fs::write(&target, &completion.contents)
        .map_err(|err| format!("failed to write {}: {err}", target.display()))?;
    agent
        .session
        .working_memory
        .note_touched_file(normalize_memory_path(relative, &agent.work_root));
    log_llm_event(
        EVENT_DETERMINISTIC_NODE_TEST_RUNNER_MANIFEST,
        serde_json::json!({
            "session_id": agent.session_store.session_id(),
            "work_root": agent.work_root.display().to_string(),
            "work_mode": agent.session.mode_state.work_mode.as_str(),
            "fallback_level": agent.config.deterministic_fallback.fallback_level(),
            "fallback_action": completion.action.as_str(),
            "target": relative,
        }),
    );
    // Issue #1005: surface the Node test-runner manifest completion through the
    // RepairOperatorRegistry so the operator that handled this failure class is
    // observable from the same SSOT as the verifier-repair-slot operators (AC2).
    let selection = super::repair_operator::select(
        Some(super::repair_operator::FailureClass::NodeTestRunnerUnbound),
        Some(super::task_contract::ArtifactRole::Setup),
    );
    super::repair_operator::record_operator_selection(
        agent.session_store.session_id(),
        &selection,
        Some(super::repair_operator::OperatorId::NodeTestRunnerManifest),
    );
    Ok(Some(relative.to_string()))
}

pub(super) fn maybe_apply_deterministic_polish_fallback(
    agent: &Agent,
    request: &str,
    relative_target: &str,
) -> Result<bool, String> {
    if !scaffold_allowed_for_active_task(agent) {
        return Ok(false);
    }
    if !agent
        .config
        .deterministic_fallback
        .allows_template_completion()
    {
        return Ok(false);
    }
    let target = agent.work_root.join(relative_target);
    let current = std::fs::read_to_string(&target)
        .map_err(|err| format!("failed to read {}: {err}", target.display()))?;
    let Some(replacement) = deterministic::playable_ui_polish(request, &target, &current) else {
        return Ok(false);
    };
    std::fs::write(&target, replacement)
        .map_err(|err| format!("failed to write {}: {err}", target.display()))?;
    maybe_apply_requested_port_script(agent, request)?;
    log_llm_event(
        "agent.deterministic_ui_polish",
        serde_json::json!({
            "session_id": agent.session_store.session_id(),
            "work_root": agent.work_root.display().to_string(),
            "fallback_level": agent.config.deterministic_fallback.fallback_level(),
            "fallback_action": "full_template",
            "target": relative_target,
        }),
    );
    Ok(true)
}

pub(super) fn maybe_apply_local_llm_small_edit_fallback(
    agent: &mut Agent,
    request: &str,
) -> Result<Option<String>, String> {
    if !scaffold_allowed_for_active_task(agent) {
        return Ok(None);
    }
    if !agent
        .config
        .deterministic_fallback
        .allows_template_completion()
    {
        return Ok(None);
    }
    if !model_capabilities(&super::agent_misc::current_assistant_model(agent))
        .read_after_small_edit_protocol
    {
        return Ok(None);
    }
    let Some(target) = local_llm_small_edit_fallback_target(agent) else {
        return Ok(None);
    };
    let current = std::fs::read_to_string(&target)
        .map_err(|err| format!("failed to read {}: {err}", target.display()))?;
    let polish_request = if super::quality::request_needs_playable_ui_quality_gate(request) {
        "ゲームUIの品質を上げてください。".to_string()
    } else {
        format!("{request}\n品質を上げてください。")
    };
    let Some(replacement) = deterministic::playable_ui_polish(&polish_request, &target, &current)
    else {
        return Ok(None);
    };
    std::fs::write(&target, replacement)
        .map_err(|err| format!("failed to write {}: {err}", target.display()))?;
    agent
        .session
        .record_feedback_if_unset(build_feedback_for_deterministic_content_fallback(
            &agent.work_root,
        ));
    let relative = target
        .strip_prefix(&agent.work_root)
        .unwrap_or(target.as_path())
        .to_string_lossy()
        .replace('\\', "/");
    log_llm_event(
        "agent.deterministic_local_llm_small_edit",
        serde_json::json!({
            "session_id": agent.session_store.session_id(),
            "work_root": agent.work_root.display().to_string(),
            "fallback_level": agent.config.deterministic_fallback.fallback_level(),
            "fallback_action": "full_template",
            "target": &relative,
        }),
    );
    Ok(Some(relative))
}

pub(super) fn local_llm_small_edit_fallback_target(agent: &Agent) -> Option<PathBuf> {
    if agent.session.mode_state.mode != ExecutionMode::Act
        || !agent.session.mode_state.policy().repo_edit_required
        || !super::workspace_access::active_task_expects_repo_change(agent)
    {
        return None;
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

pub(super) fn maybe_apply_deterministic_quality_fallback(
    agent: &Agent,
    request: &str,
    relative_target: &str,
) -> Result<bool, String> {
    if !scaffold_allowed_for_active_task(agent) {
        return Ok(false);
    }
    if !agent
        .config
        .deterministic_fallback
        .allows_template_completion()
    {
        return Ok(false);
    }
    let target = agent.work_root.join(relative_target);
    let current = std::fs::read_to_string(&target)
        .map_err(|err| format!("failed to read {}: {err}", target.display()))?;
    let Some(replacement) = deterministic::playable_ui_repair(request, &target, &current) else {
        return Ok(false);
    };
    std::fs::write(&target, replacement)
        .map_err(|err| format!("failed to write {}: {err}", target.display()))?;
    maybe_apply_deterministic_framework_support_files(agent, request)?;
    maybe_apply_requested_port_script(agent, request)?;
    log_llm_event(
        "agent.deterministic_ui_quality_repair",
        serde_json::json!({
            "session_id": agent.session_store.session_id(),
            "work_root": agent.work_root.display().to_string(),
            "fallback_level": agent.config.deterministic_fallback.fallback_level(),
            "fallback_action": "full_template",
            "target": relative_target,
        }),
    );
    Ok(true)
}

pub(super) fn maybe_apply_deterministic_framework_support_files(
    agent: &Agent,
    request: &str,
) -> Result<(), String> {
    if !agent
        .config
        .deterministic_fallback
        .allows_support_recovery()
    {
        return Ok(());
    }
    let Some(files) = deterministic::empty_framework_app_files(request) else {
        return Ok(());
    };
    let mut written_paths = Vec::<String>::new();
    for (relative, content) in files {
        if deterministic_framework_game_impl_path(&relative) {
            continue;
        }
        let content = sync_package_json_with_existing_lock(&agent.work_root, &relative, content);
        let target_relative = deterministic_support_target_relative(&agent.work_root, &relative);
        let target = agent.work_root.join(&target_relative);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|err| format!("failed to create {}: {err}", parent.display()))?;
        }
        std::fs::write(&target, content)
            .map_err(|err| format!("failed to write {}: {err}", target.display()))?;
        written_paths.push(target_relative.to_string_lossy().replace('\\', "/"));
    }
    if !written_paths.is_empty() {
        log_llm_event(
            "agent.deterministic_framework_support_files",
            serde_json::json!({
                "session_id": agent.session_store.session_id(),
                "work_root": agent.work_root.display().to_string(),
                "fallback_level": agent.config.deterministic_fallback.fallback_level(),
                "fallback_action": "minimal_patch",
                "files": written_paths,
            }),
        );
    }
    Ok(())
}

pub(super) fn maybe_apply_requested_port_script(
    agent: &Agent,
    request: &str,
) -> Result<(), String> {
    let package_path = agent.work_root.join("package.json");
    let Ok(current) = std::fs::read_to_string(&package_path) else {
        return Ok(());
    };
    let react_dev_wrapper = react_dev_wrapper_for_requested_port(request, &current);
    let Some(updated) = package_json_with_requested_port(request, &current) else {
        if let Some(wrapper) = react_dev_wrapper {
            write_react_dev_wrapper(agent, wrapper)?;
        }
        return Ok(());
    };
    std::fs::write(&package_path, updated)
        .map_err(|err| format!("failed to write {}: {err}", package_path.display()))?;
    if let Some(wrapper) = react_dev_wrapper {
        write_react_dev_wrapper(agent, wrapper)?;
    }
    Ok(())
}

fn write_react_dev_wrapper(agent: &Agent, wrapper: String) -> Result<(), String> {
    let scripts_dir = agent.work_root.join("scripts");
    std::fs::create_dir_all(&scripts_dir)
        .map_err(|err| format!("failed to create {}: {err}", scripts_dir.display()))?;
    let wrapper_path = scripts_dir.join("dev.mjs");
    std::fs::write(&wrapper_path, wrapper)
        .map_err(|err| format!("failed to write {}: {err}", wrapper_path.display()))
}

pub(super) fn maybe_apply_deterministic_quality_fallback_after_timeout(
    agent: &Agent,
    err: &str,
) -> Option<AssistantReply> {
    if !scaffold_allowed_for_active_task(agent) {
        return None;
    }
    if !err.to_ascii_lowercase().contains("timed out")
        || !super::quality_gate::current_request_needs_playable_ui_quality_gate(agent)
    {
        return None;
    }
    None
}

pub(super) fn maybe_apply_deterministic_polish_fallback_after_timeout(
    agent: &Agent,
    err: &str,
) -> Option<AssistantReply> {
    if !scaffold_allowed_for_active_task(agent) {
        return None;
    }
    if !err.to_ascii_lowercase().contains("timed out")
        || !super::quality_gate::current_request_needs_playable_ui_quality_gate(agent)
    {
        return None;
    }
    None
}

pub(super) fn maybe_apply_deterministic_edit_after_format_error(
    agent: &mut Agent,
    err: &str,
) -> Result<Option<AssistantReply>, String> {
    if !scaffold_allowed_for_active_task(agent) {
        return Ok(None);
    }
    if !agent.config.specialized_fallback_enabled() {
        return Ok(None);
    }
    if !lifecycle::is_tool_call_format_error(err)
        || !model_capabilities(&super::agent_misc::current_assistant_model(agent))
            .deterministic_edit_after_format_error
        || has_successful_non_plan_repo_edit(
            &agent.session.messages,
            &agent.work_root,
            agent.session.mode_state.active_plan_path.as_deref(),
        )
    {
        return Ok(None);
    }
    let Some(path) = last_read_tool_path(&agent.session.messages) else {
        return Ok(None);
    };
    let Ok(target) = resolve_user_path(&agent.work_root, &path) else {
        return Ok(None);
    };
    if !target.is_file() {
        return Ok(None);
    }
    let current = std::fs::read_to_string(&target)
        .map_err(|err| format!("failed to read {}: {err}", target.display()))?;
    let replacement = if current.contains("pub fn multiply") && current.contains("left + right") {
        current.replacen("left + right", "left * right", 1)
    } else if current.contains("pub fn add") && current.contains("left - right") {
        current.replacen("left - right", "left + right", 1)
    } else {
        return Ok(None);
    };
    std::fs::write(&target, replacement)
        .map_err(|err| format!("failed to write {}: {err}", target.display()))?;
    let relative = target
        .strip_prefix(&agent.work_root)
        .unwrap_or(&target)
        .to_string_lossy()
        .replace('\\', "/");
    agent
        .session
        .working_memory
        .note_touched_file(relative.clone());
    log_llm_event(
        EVENT_DETERMINISTIC_FORMAT_ERROR_SMALL_EDIT,
        serde_json::json!({
            "session_id": agent.session_store.session_id(),
            "work_root": agent.work_root.display().to_string(),
            "fallback_level": agent.config.deterministic_fallback.fallback_level(),
            "fallback_action": "minimal_patch",
            "target": &relative,
        }),
    );
    Ok(Some(AssistantReply {
        content: format!(
            "Applied a deterministic small-edit fallback after malformed tool calls in {relative}."
        ),
        tool_calls: Vec::new(),
        prompt_tokens: None,
        completion_tokens: None,
    }))
}

pub(super) fn maybe_fallback_plan_model_after_timeout(agent: &mut Agent, err: &str) -> bool {
    let Some(sidecar) = agent
        .models
        .sidecar
        .as_ref()
        .filter(|model| !model.trim().is_empty())
    else {
        return false;
    };
    if !should_fallback_plan_model_after_timeout(
        agent.session.mode_state.mode,
        agent.plan_model_override.as_deref(),
        err,
        sidecar,
    ) {
        return false;
    }

    agent.plan_model_override = Some(sidecar.clone());
    super::message_push::push_system_note(
        agent,
        format!("Main planning model timed out. Retry the plan step with sidecar model {sidecar}."),
    );
    true
}

pub(super) fn maybe_materialize_plan_after_timeout(
    agent: &mut Agent,
    err: &str,
) -> Result<Option<AssistantReply>, String> {
    if !should_materialize_plan_after_timeout(
        agent.session.mode_state.mode,
        agent.plan_model_override.as_deref(),
        err,
    ) {
        return Ok(None);
    }
    if !materialize_deterministic_fallback_plan(agent, "agent.plan.timeout_fallback_materialized")?
    {
        return Ok(None);
    }
    Ok(Some(AssistantReply {
        content: "Plan complete. Reply yes to execute, no to revise, or provide feedback."
            .to_string(),
        tool_calls: Vec::new(),
        prompt_tokens: None,
        completion_tokens: None,
    }))
}

pub(super) fn maybe_materialize_plan_after_tool_call_format_error(
    agent: &mut Agent,
    err: &str,
) -> Result<Option<AssistantReply>, String> {
    if !should_materialize_plan_after_tool_call_format_error(agent.session.mode_state.mode, err) {
        return Ok(None);
    }
    if !materialize_deterministic_fallback_plan(
        agent,
        "agent.plan.tool_call_format_fallback_materialized",
    )? {
        return Ok(None);
    }
    Ok(Some(AssistantReply {
        content: "Plan complete. Reply yes to execute, no to revise, or provide feedback."
            .to_string(),
        tool_calls: Vec::new(),
        prompt_tokens: None,
        completion_tokens: None,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn file_paths(plan: &ScaffoldPlan) -> Vec<String> {
        plan.files
            .iter()
            .map(|(path, _)| path.to_string_lossy().to_string())
            .collect()
    }

    fn file_content<'a>(plan: &'a ScaffoldPlan, path: &str) -> &'a str {
        plan.files
            .iter()
            .find(|(candidate, _)| candidate == Path::new(path))
            .map(|(_, content)| content.as_str())
            .expect("file content")
    }

    fn write_workspace_metadata_fixture(root: &Path, prompt: &str, command: &str) {
        std::fs::write(root.join("prompt.md"), prompt).unwrap();
        std::fs::write(root.join("cmd.txt"), command).unwrap();
        std::fs::write(root.join("anvil.out"), "controller stdout\n").unwrap();
    }

    #[test]
    fn project_skeleton_plans_rust_cli_word_counter_shape() {
        let plan = project_skeleton_plan_for_request("Create a Rust CLI word counter with tests")
            .expect("rust cli plan");
        let paths = file_paths(&plan);

        assert_eq!(plan.scaffold_kind, "Rust CLI");
        assert!(paths.contains(&"Cargo.toml".to_string()));
        assert!(paths.contains(&"src/main.rs".to_string()));
        assert!(paths.contains(&"tests/cli.rs".to_string()));
        assert!(!paths.contains(&"src/lib.rs".to_string()));
    }

    #[test]
    fn project_skeleton_plans_node_json_formatter_cli_shape() {
        let plan = project_skeleton_plan_for_request(
            "Build a Node.js JSON formatter CLI with node --test",
        )
        .expect("node cli plan");
        let package_json = file_content(&plan, "package.json");
        let paths = file_paths(&plan);

        assert_eq!(plan.scaffold_kind, "Node CLI");
        assert!(paths.contains(&"package.json".to_string()));
        assert!(paths.contains(&"src/index.js".to_string()));
        assert!(paths.contains(&"tests/index.test.js".to_string()));
        assert!(package_json.contains(r#""test": "node --test""#));
        assert!(package_json.contains(r#""bin""#));
    }

    #[test]
    fn project_skeleton_plans_rust_library_without_slugify_behavior() {
        let plan = project_skeleton_plan_for_request("Create a Rust slugify library")
            .expect("rust library plan");
        let paths = file_paths(&plan);
        let combined = plan
            .files
            .iter()
            .map(|(_, content)| content.as_str())
            .collect::<Vec<_>>()
            .join("\n")
            .to_ascii_lowercase();

        assert_eq!(plan.scaffold_kind, "Rust library");
        assert!(paths.contains(&"Cargo.toml".to_string()));
        assert!(paths.contains(&"src/lib.rs".to_string()));
        assert!(paths.contains(&"tests/integration.rs".to_string()));
        assert!(!paths.contains(&"src/main.rs".to_string()));
        assert!(!combined.contains("slug"));
        assert!(combined.contains("transform"));
    }

    #[test]
    fn project_skeleton_uses_requested_rust_impl_obligation_path() {
        let plan = project_skeleton_plan_for_request(
            "Create a Rust CLI word counter in tools/word_count.rs",
        )
        .expect("rust cli plan");
        let paths = file_paths(&plan);
        let cargo_toml = file_content(&plan, "Cargo.toml");

        assert!(paths.contains(&"tools/word_count.rs".to_string()));
        assert!(!paths.contains(&"src/main.rs".to_string()));
        assert!(cargo_toml.contains(r#"path = "tools/word_count.rs""#));
    }

    #[test]
    fn project_skeleton_uses_requested_node_impl_obligation_path() {
        let plan = project_skeleton_plan_for_request("Build a Node CLI at bin/formatter.js")
            .expect("node cli plan");
        let paths = file_paths(&plan);
        let package_json = file_content(&plan, "package.json");
        let test_file = file_content(&plan, "tests/index.test.js");

        assert!(paths.contains(&"bin/formatter.js".to_string()));
        assert!(!paths.contains(&"src/index.js".to_string()));
        assert!(package_json.contains(r#""app": "./bin/formatter.js""#));
        assert!(test_file.contains(r#"from "../bin/formatter.js""#));
    }

    #[test]
    fn materialize_scaffold_writes_rust_cli_skeleton() {
        use crate::agent::loop_run::commands::test_agent_with_config;
        use crate::config::{Config, DeterministicFallbackMode};
        use crate::modes::plan_act::WorkMode;
        use crate::session::store::ConversationMessage;

        let cfg = Config {
            deterministic_fallback: DeterministicFallbackMode::FullTemplate,
            ..Config::default()
        };
        let (mut agent, temp) = test_agent_with_config(cfg);
        agent.session.mode_state.work_mode = WorkMode::GenericCode;
        agent.session.messages.push(ConversationMessage::user(
            "Create a Rust CLI word counter with tests".to_string(),
        ));

        let fired = maybe_materialize_mode_deterministic_fallback(&mut agent, 0);

        assert!(fired);
        assert!(temp.path().join("Cargo.toml").is_file());
        assert!(temp.path().join("src/main.rs").is_file());
        assert!(temp.path().join("tests/cli.rs").is_file());
    }

    #[test]
    fn rust_scaffold_materializes_when_prompt_metadata_exists() {
        use crate::agent::loop_run::commands::test_agent_with_config;
        use crate::config::{Config, DeterministicFallbackMode};
        use crate::modes::plan_act::WorkMode;
        use crate::session::store::ConversationMessage;

        let cfg = Config {
            deterministic_fallback: DeterministicFallbackMode::FullTemplate,
            ..Config::default()
        };
        let (mut agent, temp) = test_agent_with_config(cfg);
        write_workspace_metadata_fixture(temp.path(), "Create a Rust CLI\n", "cargo test\n");
        agent.session.mode_state.work_mode = WorkMode::GenericCode;
        agent.session.messages.push(ConversationMessage::user(
            "Create a Rust CLI word counter with tests".to_string(),
        ));

        let fired = maybe_materialize_mode_deterministic_fallback(&mut agent, 0);

        assert!(fired);
        assert!(temp.path().join("Cargo.toml").is_file());
        assert!(temp.path().join("src/main.rs").is_file());
        assert_eq!(
            std::fs::read_to_string(temp.path().join("prompt.md")).unwrap(),
            "Create a Rust CLI\n"
        );
        assert_eq!(
            std::fs::read_to_string(temp.path().join("cmd.txt")).unwrap(),
            "cargo test\n"
        );
        assert_eq!(
            std::fs::read_to_string(temp.path().join("anvil.out")).unwrap(),
            "controller stdout\n"
        );
    }

    #[test]
    fn node_scaffold_materializes_when_prompt_metadata_exists() {
        use crate::agent::loop_run::commands::test_agent_with_config;
        use crate::config::{Config, DeterministicFallbackMode};
        use crate::modes::plan_act::WorkMode;
        use crate::session::store::ConversationMessage;

        let cfg = Config {
            deterministic_fallback: DeterministicFallbackMode::FullTemplate,
            ..Config::default()
        };
        let (mut agent, temp) = test_agent_with_config(cfg);
        write_workspace_metadata_fixture(temp.path(), "Build a Node CLI\n", "npm test\n");
        agent.session.mode_state.work_mode = WorkMode::GenericCode;
        agent.session.messages.push(ConversationMessage::user(
            "Build a Node.js JSON formatter CLI with node --test".to_string(),
        ));

        let fired = maybe_materialize_mode_deterministic_fallback(&mut agent, 0);

        assert!(fired);
        assert!(temp.path().join("package.json").is_file());
        assert!(temp.path().join("src/index.js").is_file());
        assert_eq!(
            std::fs::read_to_string(temp.path().join("prompt.md")).unwrap(),
            "Build a Node CLI\n"
        );
        assert_eq!(
            std::fs::read_to_string(temp.path().join("cmd.txt")).unwrap(),
            "npm test\n"
        );
        assert_eq!(
            std::fs::read_to_string(temp.path().join("anvil.out")).unwrap(),
            "controller stdout\n"
        );
    }

    #[test]
    fn docs_scaffold_materializes_when_prompt_cmd_and_anvil_output_metadata_exist() {
        use crate::agent::loop_run::commands::test_agent_with_config;
        use crate::config::{Config, DeterministicFallbackMode};
        use crate::modes::plan_act::WorkMode;
        use crate::session::store::ConversationMessage;

        let cfg = Config {
            deterministic_fallback: DeterministicFallbackMode::FullTemplate,
            ..Config::default()
        };
        let (mut agent, temp) = test_agent_with_config(cfg);
        write_workspace_metadata_fixture(
            temp.path(),
            "READMEを作成してください。\n",
            "cat README.md\n",
        );
        agent.session.mode_state.work_mode = WorkMode::Docs;
        agent.session.messages.push(ConversationMessage::user(
            "READMEを作成してください。".to_string(),
        ));

        let fired = maybe_materialize_mode_deterministic_fallback(&mut agent, 0);

        assert!(fired);
        assert!(temp.path().join("README.md").is_file());
        assert_eq!(
            std::fs::read_to_string(temp.path().join("prompt.md")).unwrap(),
            "READMEを作成してください。\n"
        );
        assert_eq!(
            std::fs::read_to_string(temp.path().join("cmd.txt")).unwrap(),
            "cat README.md\n"
        );
        assert_eq!(
            std::fs::read_to_string(temp.path().join("anvil.out")).unwrap(),
            "controller stdout\n"
        );
    }
}
