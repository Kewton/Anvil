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
    let authority_contract = super::task_classification::task_contract_authority(agent);
    let contract_for_alignment =
        alignment_contract_for_hint(contract, authority_contract.as_deref(), hint.role);
    let hint = if let Some(contract) = contract_for_alignment {
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

pub(super) fn realign_current_artifact_recovery_target_to_contract(agent: &mut Agent) -> bool {
    let Some(contract) =
        super::task_classification::task_contract_authority(agent).map(|rc| (*rc).clone())
    else {
        return false;
    };
    realign_current_artifact_recovery_target_with_contract(agent, &contract)
}

pub(super) fn realign_current_artifact_recovery_target_with_contract(
    agent: &mut Agent,
    contract: &TaskContract,
) -> bool {
    let Some(current) = active_artifact_recovery_target_projection(agent) else {
        return false;
    };
    let identities = contract.required_identities_for_role(current.role);
    let Some(identity) = identities.first() else {
        return false;
    };
    if identity.path == current.path {
        if agent.current_artifact_recovery_target.is_none() {
            agent.current_artifact_recovery_target = Some(current);
            return true;
        }
        return false;
    }
    let hint = RecoveryTargetHint {
        role: current.role,
        path: identity.path.clone(),
        reason: format!(
            "required deliverable obligation is still missing: {}",
            super::task_contract::obligation_report_label(identity)
        ),
    };
    set_artifact_recovery_target_from_hint_with_contract(
        agent,
        hint,
        current.attempt,
        Some(contract),
    )
    .is_some()
}

fn active_artifact_recovery_target_projection(agent: &Agent) -> Option<RecoveryTarget> {
    agent.current_artifact_recovery_target.clone().or_else(|| {
        agent
            .artifact_completion_job
            .as_ref()
            .map(|job| job.projection_recovery_target())
    })
}

fn alignment_contract_for_hint<'a>(
    provided: Option<&'a TaskContract>,
    authority: Option<&'a TaskContract>,
    role: super::task_contract::ArtifactRole,
) -> Option<&'a TaskContract> {
    authority
        .filter(|contract| contract_has_identity_for_role(contract, role))
        .or_else(|| provided.filter(|contract| contract_has_identity_for_role(contract, role)))
        .or(provided)
        .or(authority)
}

fn contract_has_identity_for_role(
    contract: &TaskContract,
    role: super::task_contract::ArtifactRole,
) -> bool {
    contract
        .required_artifact_identities
        .iter()
        .any(|identity| identity.role == role)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::loop_run::commands::test_agent_with_config;
    use crate::agent::loop_run::project_profile::parse_project_profile_confirmation;
    use crate::agent::loop_run::task_contract::{ArtifactRole, TaskContract};
    use crate::config::Config;
    use crate::session::store::ConversationMessage;
    use std::rc::Rc;

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
    fn installer_prefers_per_turn_authority_over_stale_provided_contract() {
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
        agent.project_profile_confirm_called_this_turn = true;
        let profile = parse_project_profile_confirmation(
            r#"{"language":"python","shape":"library","deliverable_kind":"code","primary_artifacts":["math_utils.py","tests/test_math_utils.py"],"forbidden_artifacts":["setup","docs"],"evidence_kind":"test_run","needs_environment_setup":false,"preferred_runner":null,"confidence":1.0,"reason":"explicit Python test identity"}"#,
        )
        .expect("profile");
        let authority =
            TaskContract::from_request_with_kind_and_project_profile(request, None, Some(&profile));
        agent
            .task_contract_this_turn
            .set(Rc::new(authority))
            .expect("unset contract cell");
        let stale_contract = TaskContract::from_request("Create a Rust CLI and add tests");

        let installed = set_artifact_recovery_target_from_hint_with_contract(
            &mut agent,
            RecoveryTargetHint {
                role: ArtifactRole::Test,
                path: "tests/cli.rs".to_string(),
                reason: "synthesized test artifact aligned with requested rust project family"
                    .to_string(),
            },
            1,
            Some(&stale_contract),
        )
        .expect("authority identity should produce an installable target");

        assert_eq!(installed.path, "tests/test_math_utils.py");
        assert_eq!(
            installed.reason,
            "contract required test artifact identity is still missing"
        );
        let job = agent
            .artifact_completion_job
            .as_ref()
            .expect("artifact completion job should be installed");
        assert_eq!(job.target_path(), "tests/test_math_utils.py");
    }

    #[test]
    fn current_target_realigns_to_per_turn_contract_identity_before_retry_note() {
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
        agent.project_profile_confirm_called_this_turn = true;
        std::fs::create_dir_all(agent.work_root.join("tests")).unwrap();
        let profile = parse_project_profile_confirmation(
            r#"{"language":"python","shape":"library","deliverable_kind":"code","primary_artifacts":["math_utils.py","tests/test_math_utils.py"],"forbidden_artifacts":["setup","docs"],"evidence_kind":"test_run","needs_environment_setup":false,"preferred_runner":null,"confidence":1.0,"reason":"explicit Python test identity"}"#,
        )
        .expect("profile");
        let authority =
            TaskContract::from_request_with_kind_and_project_profile(request, None, Some(&profile));
        agent
            .task_contract_this_turn
            .set(Rc::new(authority))
            .expect("unset contract cell");
        agent.current_artifact_recovery_target = Some(RecoveryTarget {
            role: ArtifactRole::Test,
            path: "tests/cli.rs".to_string(),
            reason: "synthesized test artifact aligned with requested rust project family"
                .to_string(),
            attempt: 2,
        });

        assert!(realign_current_artifact_recovery_target_to_contract(
            &mut agent
        ));

        let target = agent
            .current_artifact_recovery_target
            .as_ref()
            .expect("target");
        assert_eq!(target.path, "tests/test_math_utils.py");
        assert_eq!(
            target.reason,
            "required deliverable obligation is still missing: role=test, kind=file, path=tests/test_math_utils.py"
        );
        let job = agent
            .artifact_completion_job
            .as_ref()
            .expect("artifact completion job should be refreshed");
        assert_eq!(job.target_path(), "tests/test_math_utils.py");
    }

    #[test]
    fn stale_job_without_current_target_realigns_to_contract_identity_before_policy() {
        let (mut agent, _temp) = test_agent_with_config(Config::default());
        let request = "Coding TDD task: create math_utils.py and tests/test_math_utils.py only. Implement clamp(value, minimum, maximum). Use Python unittest.";
        let stale = set_artifact_recovery_target_from_hint(
            &mut agent,
            RecoveryTargetHint {
                role: ArtifactRole::Test,
                path: "tests/cli.rs".to_string(),
                reason: "synthesized test artifact aligned with requested rust project family"
                    .to_string(),
            },
            4,
        )
        .expect("stale test target should install");
        assert_eq!(stale.path, "tests/cli.rs");
        agent
            .session
            .messages
            .push(ConversationMessage::user(request.to_string()));
        agent
            .session
            .working_memory
            .set_active_task(Some(request.to_string()));
        agent.project_profile_confirm_called_this_turn = true;
        let profile = parse_project_profile_confirmation(
            r#"{"language":"python","shape":"library","deliverable_kind":"code","primary_artifacts":["math_utils.py","tests/test_math_utils.py"],"forbidden_artifacts":["setup","docs"],"evidence_kind":"test_run","needs_environment_setup":false,"preferred_runner":null,"confidence":1.0,"reason":"explicit Python test identity"}"#,
        )
        .expect("profile");
        let authority =
            TaskContract::from_request_with_kind_and_project_profile(request, None, Some(&profile));
        agent
            .task_contract_this_turn
            .set(Rc::new(authority))
            .expect("unset contract cell");
        agent.current_artifact_recovery_target = None;

        assert!(realign_current_artifact_recovery_target_to_contract(
            &mut agent
        ));

        let target = agent
            .current_artifact_recovery_target
            .as_ref()
            .expect("projection should be restored from job and realigned");
        assert_eq!(target.path, "tests/test_math_utils.py");
        assert_eq!(target.attempt, 0);
        let job = agent
            .artifact_completion_job
            .as_ref()
            .expect("artifact completion job should be reinstalled");
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
