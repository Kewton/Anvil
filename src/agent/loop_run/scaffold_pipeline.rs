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
