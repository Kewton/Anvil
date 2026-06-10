//! Tool-prep helpers extracted from `turn.rs` (parent #680).
//!
//! Hosts three small Agent-level "prep before tool call" helpers that
//! the request-builder, the reply-retry loop, and the policy flow
//! consult before / during a tool dispatch:
//!
//! - `tool_specs_for_policy` — filters the registered tool specs to
//!   the `EffectiveToolPolicy`'s allowed-name list (when set).
//! - `local_llm_small_edit_target` — picks an Edit target for the
//!   small-edit protocol that local LLMs prefer (Act mode + edit required by
//!   WorkMode or ObjectiveContract artifact + task expects repo change + no
//!   successful non-plan repo edit yet + target already read).
//! - `mode_policy_message` — renders the per-`WorkMode` `[Mode Policy]`
//!   system note (Auto returns None).
//!
//! Originally `impl Agent` methods; converted to free functions taking
//! `&Agent`, matching the `actor_loop_flow` / `reply_retry` / earlier
//! vertical-slice precedent. `pub(super)` limited / no facade
//! re-export (DR3-001).

use std::path::PathBuf;

use super::Agent;
use super::read_target_helpers::latest_turn_preferred_read_edit_target;
use super::task_contract::TaskKind;
use super::tool_history::{focused_edit_target_already_read, has_successful_non_plan_repo_edit};
use super::tool_policy::EffectiveToolPolicy;
use crate::modes::plan_act::{ExecutionMode, WorkMode};
use crate::session::store::ConversationMessage;
use crate::tools::registry::ToolSpec;

pub(super) fn tool_specs_for_policy(agent: &Agent, policy: &EffectiveToolPolicy) -> Vec<ToolSpec> {
    let mut specs = agent.tool_registry.specs().to_vec();
    if let Some(allowed_tools) = policy.allowed_tool_names_for_prompt() {
        specs.retain(|spec| allowed_tools.contains(&spec.function.name.as_str()));
    }
    specs
}

pub(super) fn local_llm_small_edit_target(agent: &Agent) -> Option<PathBuf> {
    if !crate::model_capabilities::model_capabilities(&super::agent_misc::current_assistant_model(
        agent,
    ))
    .read_after_small_edit_protocol
    {
        return None;
    }
    if agent.session.mode_state.mode != ExecutionMode::Act
        || !super::workspace_access::repo_edit_required_by_mode_or_objective(agent)
        || !super::workspace_access::active_task_expects_repo_change(agent)
    {
        return None;
    }
    if has_successful_non_plan_repo_edit(
        &agent.session.messages,
        &agent.work_root,
        agent.session.mode_state.active_plan_path.as_deref(),
    ) {
        return None;
    }
    if let Some(target) = super::artifact_recovery_flow::artifact_recovery_target_path(agent) {
        return focused_edit_target_already_read(
            &agent.session.messages,
            &target,
            &agent.work_root,
        )
        .then_some(target);
    }
    let target = latest_turn_preferred_read_edit_target(&agent.session.messages, &agent.work_root)?;
    focused_edit_target_already_read(&agent.session.messages, &target, &agent.work_root)
        .then_some(target)
}

pub(super) fn mode_policy_message(agent: &Agent) -> Option<ConversationMessage> {
    let work_mode = agent.session.mode_state.work_mode;
    let contract = super::task_classification::task_contract_authority(agent);
    let task_kind = contract.as_ref().map(|contract| contract.task_kind);
    let objective_requires_artifact = contract
        .as_ref()
        .is_some_and(|contract| !contract.required_artifacts.is_empty());
    mode_policy_text_for(work_mode, task_kind, objective_requires_artifact)
        .map(str::to_string)
        .map(ConversationMessage::system)
}

fn mode_policy_text_for(
    work_mode: WorkMode,
    task_kind: Option<TaskKind>,
    objective_requires_artifact: bool,
) -> Option<&'static str> {
    if (work_mode != WorkMode::AnswerOnly || objective_requires_artifact)
        && task_kind.is_some_and(|kind| kind != TaskKind::Coding)
    {
        return Some(
            "[Mode Policy] Task kind is non-coding. Follow the objective contract and required artifacts/evidence. Do not create code scaffolds or coding-specific verification unless explicitly requested.",
        );
    }

    let text = match work_mode {
        WorkMode::Auto => return None,
        WorkMode::TypeScriptUi => {
            "[Mode Policy] Work mode is TypeScript UI. Prefer the existing JavaScript or TypeScript framework when present. Do not switch to Python or documentation-only output unless the user asks."
        }
        WorkMode::Python => {
            "[Mode Policy] Work mode is Python. Use Python-oriented files and verification. Do not create TypeScript, React, Next.js, Nuxt, or browser UI scaffolds unless the user asks."
        }
        WorkMode::Docs => {
            "[Mode Policy] Work mode is documentation. Edit or create documentation files only unless code changes are explicitly requested."
        }
        WorkMode::AnswerOnly => {
            "[Mode Policy] Work mode is answer-only/read-only. You may inspect files if needed, and may run an explicitly requested local script or read-only command, but do not require or perform repository edits."
        }
        WorkMode::GenericCode | WorkMode::Unknown => {
            "[Mode Policy] Work mode is generic code. Follow the repository stack and avoid TypeScript UI deterministic fallback unless the request explicitly asks for a browser UI."
        }
    };
    Some(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_coding_task_kind_suppresses_python_mode_policy() {
        let text =
            mode_policy_text_for(WorkMode::Python, Some(TaskKind::Data), true).expect("policy");
        assert!(text.contains("Task kind is non-coding"), "got: {text}");
        assert!(!text.contains("Python-oriented"), "got: {text}");
    }

    #[test]
    fn non_coding_task_kind_suppresses_typescript_ui_mode_policy() {
        let text = mode_policy_text_for(WorkMode::TypeScriptUi, Some(TaskKind::Research), true)
            .expect("policy");
        assert!(text.contains("Task kind is non-coding"), "got: {text}");
        assert!(!text.contains("TypeScript UI"), "got: {text}");
    }

    #[test]
    fn coding_task_kind_keeps_coding_mode_policy() {
        let text =
            mode_policy_text_for(WorkMode::Python, Some(TaskKind::Coding), true).expect("policy");
        assert!(text.contains("Python-oriented"), "got: {text}");
    }

    #[test]
    fn answer_only_policy_remains_stronger_without_artifact_requirement() {
        let text = mode_policy_text_for(WorkMode::AnswerOnly, Some(TaskKind::Data), false)
            .expect("policy");
        assert!(text.contains("answer-only/read-only"), "got: {text}");
        assert!(!text.contains("Task kind is non-coding"), "got: {text}");
    }

    #[test]
    fn artifact_requirement_overrides_answer_only_mode_policy() {
        let text =
            mode_policy_text_for(WorkMode::AnswerOnly, Some(TaskKind::Data), true).expect("policy");
        assert!(text.contains("Task kind is non-coding"), "got: {text}");
        assert!(!text.contains("answer-only/read-only"), "got: {text}");
    }
}
