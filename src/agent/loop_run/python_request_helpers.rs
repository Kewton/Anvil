//! Python request inspection helpers extracted from `turn.rs` (parent
//! #680).
//!
//! Hosts the Python-specific signals that drive completion / scaffold
//! gating decisions for `WorkMode::Python`:
//!
//! - `active_python_request_requires_tests` — Python work mode +
//!   request explicitly requires tests.
//! - `python_verifier_available_for_requested_tests` — `AutoTestRunner`
//!   can detect a runnable Python test plan in the workspace.
//! - `python_test_artifact_exists` — top-level `test_*.py` /
//!   `*_test.py` / `tests.py` exists in `work_root`.
//!
//! Originally `impl Agent` methods; converted to free functions taking
//! `&Agent`, matching the `actor_loop_flow` / `reply_retry` / earlier
//! vertical-slice precedent. `pub(super)` limited / no facade
//! re-export (DR3-001).

use super::Agent;
use super::auto_test::{AutoTestKind, AutoTestRunner};
use super::quality::request_explicitly_requires_tests;
use crate::modes::plan_act::WorkMode;

pub(super) fn active_python_request_requires_tests(agent: &Agent) -> bool {
    agent.session.mode_state.work_mode == WorkMode::Python
        && super::workspace_access::active_request_text(agent)
            .as_deref()
            .is_some_and(request_explicitly_requires_tests)
}

pub(super) fn python_verifier_available_for_requested_tests(agent: &Agent) -> bool {
    AutoTestRunner::detect(
        &agent.work_root,
        &agent.session.working_memory.touched_files,
    )
    .is_some_and(|plan| plan.auto_test_kind() == AutoTestKind::Test)
}

pub(super) fn python_test_artifact_exists(agent: &Agent) -> bool {
    let Ok(entries) = std::fs::read_dir(&agent.work_root) else {
        return false;
    };
    entries.flatten().any(|entry| {
        let path = entry.path();
        if !path.is_file() {
            return false;
        }
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            return false;
        };
        (name.starts_with("test_") && name.ends_with(".py"))
            || name.ends_with("_test.py")
            || name == "tests.py"
    })
}
