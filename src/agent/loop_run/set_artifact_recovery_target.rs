//! Artifact-recovery target installer helpers extracted from `turn.rs`
//! (parent #680).
//!
//! Hosts the projection-write half of the artifact-recovery target
//! lifecycle (the projection-read + clear half lives in
//! `artifact_recovery_flow`). Contract-aware production entry points +
//! private helpers:
//!
//! - `set_artifact_recovery_target_for_decision_with_contract` — decision
//!   → hint → atomic-install via `set_artifact_recovery_target_from_hint`.
//! - `set_artifact_recovery_target_for_action_with_contract` — action →
//!   hint → atomic-install.
//! - `set_artifact_recovery_target_from_hint_with_contract` — the SSOT
//!   atomic installer. Orders: align → install/refresh job → commit
//!   projection or atomic-clear on validation failure.
//!   Test-role path alignment is delegated to `artifact_target_alignment`.
//!
//! Originally `impl Agent` methods; converted to free functions taking
//! `&mut Agent` / `&Agent`, matching `actor_loop_flow` / `reply_retry`
//! / earlier vertical-slice patterns. `pub(super)` limited / no facade
//! re-export (DR3-001).

use super::Agent;
use super::task_contract::{
    ArtifactRecoveryAction, CompletionDecision, RecoveryTarget, RecoveryTargetHint, TaskContract,
};
use super::verifier_orchestration::JobInstallOutcome;
use crate::logging::log_llm_event;

pub(super) fn set_artifact_recovery_target_for_decision_with_contract(
    agent: &mut Agent,
    decision: &CompletionDecision,
    attempt: usize,
    contract: Option<&TaskContract>,
) -> Option<RecoveryTargetHint> {
    let hint = recovery_target_hint_for_decision(agent, decision, contract)?;
    set_artifact_recovery_target_from_hint_with_contract(agent, hint, attempt, contract)
}

fn recovery_target_hint_for_decision(
    agent: &mut Agent,
    decision: &CompletionDecision,
    contract: Option<&TaskContract>,
) -> Option<RecoveryTargetHint> {
    let CompletionDecision::Continue { missing } = decision else {
        return None;
    };
    if let Some(contract) = contract {
        let artifacts =
            super::artifact_state_projection::task_contract_artifact_states(agent, contract);
        let excerpts = super::task_contract::ArtifactExcerpts::new();
        if let Some(hint) =
            super::task_contract_recovery_planning::recovery_target_hint_for_missing_with_contract(
                contract, &artifacts, &excerpts, missing,
            )
        {
            return Some(hint);
        }
    }
    super::task_contract_recovery::task_contract_recovery_target(agent, decision)
}

pub(super) fn set_artifact_recovery_target_for_action_with_contract(
    agent: &mut Agent,
    action: &ArtifactRecoveryAction,
    attempt: usize,
    contract: Option<&TaskContract>,
) -> Option<RecoveryTargetHint> {
    let hint = match action {
        ArtifactRecoveryAction::Continue { target_hint, .. }
        | ArtifactRecoveryAction::RepairArtifact { target_hint } => target_hint.clone()?,
        _ => return None,
    };
    set_artifact_recovery_target_from_hint_with_contract(agent, hint, attempt, contract)
}

#[cfg(test)]
pub(super) fn set_artifact_recovery_target_from_hint(
    agent: &mut Agent,
    hint: RecoveryTargetHint,
    attempt: usize,
) -> Option<RecoveryTargetHint> {
    set_artifact_recovery_target_from_hint_with_contract(agent, hint, attempt, None)
}

pub(super) fn set_artifact_recovery_target_from_hint_with_contract(
    agent: &mut Agent,
    hint: RecoveryTargetHint,
    attempt: usize,
    contract: Option<&TaskContract>,
) -> Option<RecoveryTargetHint> {
    let request = super::workspace_access::active_request_text(agent);
    let fallback_contract = if contract.is_none() {
        super::task_classification::task_contract_authority(agent)
    } else {
        None
    };
    let contract = contract.or_else(|| fallback_contract.as_deref());
    let hint = if let Some(contract) = contract {
        super::artifact_target_alignment::align_recovery_target_hint_to_request(
            request.as_deref(),
            &contract.required_artifact_identities,
            hint,
        )
    } else {
        super::artifact_target_alignment::align_recovery_target_hint_to_request(
            request.as_deref(),
            &[],
            hint,
        )
    };
    // Issue #652 PR-001: the `ArtifactCompletionJob` is the SSOT for
    // target + role-specific retry budget. We must NOT update
    // `current_artifact_recovery_target` before the job has been
    // validated and installed — otherwise a validation failure would
    // leave the projection set with no job attached, and
    // `EffectiveToolPolicy::artifact_directed` would grant write
    // access for a target with no role-specific budget. Ordering:
    //   1. attempt to install / refresh the job for the hint
    //   2. on success → commit the projection (atomic SWAP from any
    //      prior state)
    //   3. on failure (only possible for Test role) → clear BOTH
    //      the projection and the job so no stale slot remains.
    let install =
        super::artifact_recovery_flow::maybe_install_artifact_completion_job_for_hint(agent, &hint);
    match install {
        JobInstallOutcome::InstalledOrSkipped => {
            let target = RecoveryTarget::from_hint(hint.clone(), attempt);
            let changed = agent.current_artifact_recovery_target.as_ref() != Some(&target);
            if changed {
                log_llm_event(
                    "agent.artifact_recovery_target.selected",
                    serde_json::json!({
                        "session_id": agent.session_store.session_id(),
                        "turn_index": agent.current_turn_index,
                        "role": target.role.label(),
                        "path": target.path,
                        "reason": target.reason,
                        "attempt": target.attempt,
                    }),
                );
            }
            agent.current_artifact_recovery_target = Some(target);
            Some(hint)
        }
        JobInstallOutcome::ValidationFailed => {
            // PR-001 atomic clear: the new Test hint failed
            // `ArtifactCompletionJob::new` validation. Drop the prior
            // projection too — otherwise the artifact-directed
            // policy would keep granting write permission for a
            // target with no attached role-specific budget.
            if agent.current_artifact_recovery_target.take().is_some() {
                log_llm_event(
                    "agent.artifact_recovery_target.cleared",
                    serde_json::json!({
                        "session_id": agent.session_store.session_id(),
                        "turn_index": agent.current_turn_index,
                        "reason": "artifact_completion_job_validation_failed",
                    }),
                );
            }
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::loop_run::commands::test_agent_with_config;
    use crate::agent::loop_run::task_contract::{ArtifactRole, TaskContract};
    use crate::config::Config;
    use crate::session::store::ConversationMessage;

    #[test]
    fn installer_uses_contract_identity_before_request_family_default() {
        let (mut agent, _temp) = test_agent_with_config(Config::default());
        let request = "Coding TDD task: create math_utils.py and tests/test_math_utils.py only. Implement clamp(value, minimum, maximum). Use Python unittest.";
        agent
            .session
            .messages
            .push(ConversationMessage::user(request.to_string()));
        agent
            .session
            .working_memory
            .set_active_task(Some(request.to_string()));
        let contract = TaskContract::from_request(request);

        let installed = set_artifact_recovery_target_from_hint_with_contract(
            &mut agent,
            RecoveryTargetHint {
                role: ArtifactRole::Test,
                path: "tests/cli.rs".to_string(),
                reason: "synthesized test artifact aligned with requested rust project family"
                    .to_string(),
            },
            1,
            Some(&contract),
        )
        .expect("contract identity should produce an installable target");

        assert_eq!(installed.path, "tests/test_math_utils.py");
        assert_eq!(
            installed.reason,
            "contract required test artifact identity is still missing"
        );
        let target = agent
            .current_artifact_recovery_target
            .as_ref()
            .expect("target projection should be installed");
        assert_eq!(target.path, "tests/test_math_utils.py");
        let job = agent
            .artifact_completion_job
            .as_ref()
            .expect("artifact completion job should be installed");
        assert_eq!(job.target_path(), "tests/test_math_utils.py");
    }

    #[test]
    fn decision_installer_uses_contract_identity_without_request_context() {
        let (mut agent, _temp) = test_agent_with_config(Config::default());
        let request = "Coding TDD task: create math_utils.py and tests/test_math_utils.py only. Implement clamp(value, minimum, maximum). Use Python unittest.";
        let contract = TaskContract::from_request(request);
        let decision = CompletionDecision::Continue {
            missing: vec![ArtifactRole::Test],
        };

        let installed = set_artifact_recovery_target_for_decision_with_contract(
            &mut agent,
            &decision,
            1,
            Some(&contract),
        )
        .expect("contract identity should produce a test target");

        assert_eq!(installed.path, "tests/test_math_utils.py");
        assert_eq!(
            installed.reason,
            "required deliverable obligation is still missing: role=test, kind=file, path=tests/test_math_utils.py"
        );
        let target = agent
            .current_artifact_recovery_target
            .as_ref()
            .expect("target projection should be installed");
        assert_eq!(target.path, "tests/test_math_utils.py");
    }
}
