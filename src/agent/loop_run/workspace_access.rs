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
//!   policy or ObjectiveContract artifact obligation + request classified as
//!   `ActionExpectation::RepoChange`.
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
        && repo_edit_required_by_mode_or_objective(agent)
        && active_request_text(agent).as_deref().is_some_and(|task| {
            recovery::classify_action_expectation(task, agent.session.mode_state.mode)
                == recovery::ActionExpectation::RepoChange
        })
}

pub(super) fn repo_edit_required_by_mode_or_objective(agent: &Agent) -> bool {
    repo_edit_required_by_mode_or_objective_for(
        agent.session.mode_state.policy().repo_edit_required,
        objective_requires_artifact(agent),
    )
}

fn repo_edit_required_by_mode_or_objective_for(
    mode_requires_repo_edit: bool,
    objective_requires_artifact: bool,
) -> bool {
    mode_requires_repo_edit || objective_requires_artifact
}

fn objective_requires_artifact(agent: &Agent) -> bool {
    super::task_classification::task_contract_authority(agent)
        .is_some_and(|contract| !contract.required_artifacts.is_empty())
}

pub(super) fn active_request_text(agent: &Agent) -> Option<String> {
    repo_change_request_text(
        agent.session.working_memory.active_task.as_deref(),
        &agent.session.messages,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::loop_run::commands::test_agent_with_config;
    use crate::config::Config;
    use crate::modes::plan_act::WorkMode;
    use crate::session::store::ConversationMessage;

    fn agent_with_answer_only_request(request: &str) -> (Agent, tempfile::TempDir) {
        let (mut agent, temp) = test_agent_with_config(Config::default());
        agent.session.mode_state.work_mode = WorkMode::AnswerOnly;
        agent
            .session
            .messages
            .push(ConversationMessage::user(request.to_string()));
        super::super::task_classification::populate_task_contract_authority(&mut agent);
        (agent, temp)
    }

    #[test]
    fn repo_edit_policy_is_enabled_by_objective_artifact() {
        assert!(repo_edit_required_by_mode_or_objective_for(false, true));
    }

    #[test]
    fn repo_edit_policy_remains_disabled_for_read_only_objective() {
        assert!(!repo_edit_required_by_mode_or_objective_for(false, false));
    }

    #[test]
    fn repo_edit_policy_keeps_mode_required_edits() {
        assert!(repo_edit_required_by_mode_or_objective_for(true, false));
    }

    #[test]
    fn research_artifact_overrides_answer_only_repo_edit_policy() {
        let (agent, _temp) = agent_with_answer_only_request(
            "Investigate deployment options and write report.md. Do not modify code.",
        );

        assert!(
            repo_edit_required_by_mode_or_objective(&agent),
            "required research artifact must override WorkMode::AnswerOnly repo-edit policy"
        );
        assert!(
            active_task_expects_repo_change(&agent),
            "research artifact creation must still count as repo-change work"
        );
    }

    #[test]
    fn ops_artifact_overrides_answer_only_repo_edit_policy() {
        let (agent, _temp) = agent_with_answer_only_request(
            "Run pwd and write ops-observation.md containing the exact observed directory. Do not modify code.",
        );

        assert!(
            repo_edit_required_by_mode_or_objective(&agent),
            "required ops observation artifact must override WorkMode::AnswerOnly repo-edit policy"
        );
        assert!(
            active_task_expects_repo_change(&agent),
            "ops observation artifact creation must still count as repo-change work"
        );
    }

    #[test]
    fn answer_only_without_artifact_remains_read_only() {
        let (agent, _temp) =
            agent_with_answer_only_request("Investigate notes.txt for me. Do not modify files.");

        assert!(
            !repo_edit_required_by_mode_or_objective(&agent),
            "read-only research without a deliverable artifact must stay read-only"
        );
        assert!(
            !active_task_expects_repo_change(&agent),
            "read-only research without a deliverable artifact must not request repo changes"
        );
    }
}
