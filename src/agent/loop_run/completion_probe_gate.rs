//! Completion probe gating for task-contract recovery.
//!
//! This module owns the narrow question of whether an artifact-flow
//! `Continue` action can be safely upgraded to `RunVerifier` based on project
//! completion probing. It deliberately does not plan missing deliverables.

use super::Agent;
use crate::logging::log_llm_event;

pub(super) fn completion_probe_override(
    agent: &mut Agent,
    contract: &super::task_contract::TaskContract,
    action: &super::task_contract::ArtifactRecoveryAction,
    owned_test_artifacts: &[String],
) -> Option<super::task_contract::ArtifactRecoveryAction> {
    if !matches!(
        action,
        super::task_contract::ArtifactRecoveryAction::Continue { .. }
    ) || missing_owned_test_repair_action(contract, owned_test_artifacts, action)
    {
        return None;
    }

    let request = super::workspace_access::active_request_text(agent).unwrap_or_default();
    let scope = super::workspace_access::current_workspace_scope(agent);
    let probe = super::project_probe::probe_completion(
        &agent.work_root,
        &request,
        contract,
        &scope,
        &agent.turn_edited_relative_paths,
    );
    match probe {
        super::project_probe::CompletionProbeDecision::RunVerifier {
            reason,
            project_unit,
        } => {
            log_llm_event(
                "agent.completion_probe.decision",
                serde_json::json!({
                    "session_id": agent.session_store.session_id(),
                    "turn_index": agent.current_turn_index,
                    "decision": "run_verifier",
                    "reason": reason,
                    "project_unit": project_unit.summary(),
                }),
            );
            Some(super::task_contract::ArtifactRecoveryAction::RunVerifier)
        }
        super::project_probe::CompletionProbeDecision::RejectStackMismatch { reason } => {
            log_llm_event(
                "agent.completion_probe.decision",
                serde_json::json!({
                    "session_id": agent.session_store.session_id(),
                    "turn_index": agent.current_turn_index,
                    "decision": "reject_stack_mismatch",
                    "reason": reason,
                }),
            );
            None
        }
        super::project_probe::CompletionProbeDecision::KeepArtifactFlow => None,
    }
}

fn missing_owned_test_repair_action(
    contract: &super::task_contract::TaskContract,
    owned_test_artifacts: &[String],
    action: &super::task_contract::ArtifactRecoveryAction,
) -> bool {
    if !contract.completion_policy.test_execution_required() || !owned_test_artifacts.is_empty() {
        return false;
    }
    matches!(
        action,
        super::task_contract::ArtifactRecoveryAction::Continue { missing, .. }
            if missing.contains(&super::task_contract::ArtifactRole::Test)
    )
}
