//! Per-Agent workspace + active-request accessors extracted from
//! `turn.rs` (parent #680).
//!
//! Hosts the four foundational `&self` accessors that the rest of the
//! `loop_run` module consults on every turn:
//!
//! - `current_workspace_scope` — Issue #646: build the active
//!   `TaskWorkspaceScope` (pure projection of `work_root` + the
//!   active user request; no filesystem mutation).
//! - `workspace_appears_empty` — thin wrapper over
//!   `workspace_walk::workspace_appears_empty(&work_root)`.
//! - `active_task_expects_repo_change` — Act mode + repo-edit-required
//!   policy + request classified as `ActionExpectation::RepoChange`.
//! - `active_request_text` — surface the current user request via
//!   `repo_change_request_text(active_task, messages)`.
//!
//! Originally `impl Agent` methods; converted to free functions taking
//! `&Agent`, matching the `actor_loop_flow` / `reply_retry` / earlier
//! vertical-slice precedent. `pub(super)` limited / no facade
//! re-export (DR3-001).

use super::Agent;
use super::quality::repo_change_request_text;
use super::task_workspace_scope::TaskWorkspaceScope;
use super::workspace_walk::workspace_appears_empty as workspace_walk_appears_empty;
use crate::agent::recovery;
use crate::modes::plan_act::ExecutionMode;

pub(super) fn current_workspace_scope(agent: &Agent) -> TaskWorkspaceScope {
    let request = active_request_text(agent).unwrap_or_default();
    TaskWorkspaceScope::detect(&agent.work_root, &request)
}

pub(super) fn workspace_appears_empty(agent: &Agent) -> bool {
    workspace_walk_appears_empty(&agent.work_root)
}

pub(super) fn active_task_expects_repo_change(agent: &Agent) -> bool {
    agent.session.mode_state.mode == ExecutionMode::Act
        && agent.session.mode_state.policy().repo_edit_required
        && active_request_text(agent).as_deref().is_some_and(|task| {
            recovery::classify_action_expectation(task, agent.session.mode_state.mode)
                == recovery::ActionExpectation::RepoChange
        })
}

pub(super) fn active_request_text(agent: &Agent) -> Option<String> {
    repo_change_request_text(
        agent.session.working_memory.active_task.as_deref(),
        &agent.session.messages,
    )
}
