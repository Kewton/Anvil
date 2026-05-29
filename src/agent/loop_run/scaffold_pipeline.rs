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

use std::path::PathBuf;

use crate::ollama::client::AssistantReply;
use crate::ollama::xml_fallback::ToolCall;

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
