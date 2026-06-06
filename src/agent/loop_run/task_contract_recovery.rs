//! TaskContract recovery action / target planners extracted from
//! `turn.rs` (parent #680).
//!
//! Hosts the per-turn `TaskContract` recovery planner:
//!
//! - `task_contract_recovery_action` — production chokepoint. Decides
//!   between `RunVerifier` (verifier-repair ready or completion-probe
//!   says run), `Continue { missing }` (artifact-state derived), or
//!   `Done`. Calls the `artifact_state_projection` SSOT for ledger /
//!   legacy alignment, computes verifier-repair readiness, and
//!   consults `project_probe::probe_completion` to short-circuit
//!   speculative `Continue` decisions when the workspace already
//!   demonstrates done-ness.
//! - `task_contract_recovery_target` — projection from
//!   `CompletionDecision::Continue { missing }` to a
//!   `RecoveryTargetHint` for the first missing role. Tries: scaffold
//!   candidate → scope-internal `Owned` workspace artifact →
//!   synthesised conventional implementation file.
//! - `task_contract_repair_state` (private) — adapter to
//!   `super::repair_job::task_contract_repair_state_from_job` carrying
//!   the per-turn `task_contract_verifier_repair_pending` flag and the
//!   active `repair_job` snapshot.
//!
//! Originally `impl Agent` methods; converted to free functions taking
//! `&mut Agent` / `&Agent`, matching the `actor_loop_flow` /
//! `reply_retry` / earlier vertical-slice precedent. `pub(super)`
//! limited / no facade re-export (DR3-001).

use super::Agent;
use super::verifier_orchestration::synthesized_missing_implementation_target_path_for_request;
use super::workspace_candidates::existing_workspace_candidate_for_role_in_scope;
use crate::logging::log_llm_event;

fn task_contract_repair_state(
    agent: &Agent,
    repair_edit_count: Option<usize>,
    repo_edit_calls_made_this_turn: usize,
) -> super::task_contract::VerifierRepairState {
    super::repair_job::task_contract_repair_state_from_job(
        agent.task_contract_verifier_repair_pending,
        agent.repair_job.as_ref(),
        repair_edit_count,
        repo_edit_calls_made_this_turn,
    )
}

pub(super) fn task_contract_recovery_action(
    agent: &mut Agent,
    contract: &super::task_contract::TaskContract,
    repair_edit_count: Option<usize>,
    repo_edit_calls_made_this_turn: usize,
) -> super::task_contract::ArtifactRecoveryAction {
    super::agent_misc::refresh_artifact_completion_satisfied(agent);
    if agent.artifact_completion_job.as_ref().is_some_and(|job| {
        matches!(
            job.status(),
            super::artifact_completion_job::ArtifactCompletionStatus::Satisfied
        )
    }) {
        return if contract.verification_required {
            super::task_contract::ArtifactRecoveryAction::RunVerifier
        } else {
            super::task_contract::ArtifactRecoveryAction::Done
        };
    }

    let verifier_repair_ready_to_verify = agent.task_contract_verifier_repair_pending
        && (repair_edit_count
            .is_some_and(|edit_count| repo_edit_calls_made_this_turn > edit_count)
            || agent.repair_job.as_ref().is_some_and(|job| {
                matches!(
                    job.next_action(),
                    super::repair_job::RepairNextAction::RerunVerifier
                        | super::repair_job::RepairNextAction::VerifiedDone
                )
            }));
    if verifier_repair_ready_to_verify {
        return super::task_contract::ArtifactRecoveryAction::RunVerifier;
    }
    let artifacts =
        super::artifact_state_projection::task_contract_artifact_states(agent, contract);
    let repair_state =
        task_contract_repair_state(agent, repair_edit_count, repo_edit_calls_made_this_turn);
    let missing_verifier_suppress_retry = agent
        .missing_verifier_job
        .as_ref()
        .is_some_and(|job| job.should_suppress_verifier_retry());
    // Issue #651 Phase 5: feed the SSOT `owned_test_artifacts` slice
    // into the planner so the SafeStop gate (test_execution_required
    // && owned_test_artifacts.is_empty()) can fire.
    let owned_test_artifacts =
        super::owned_test_projection::owned_test_artifacts_for_verifier(agent, contract);
    let action = super::task_contract::plan_artifact_recovery(
        super::task_contract::ArtifactRecoveryInputs {
            contract,
            evidence: &agent.task_contract_evidence_set_this_turn,
            artifacts: &artifacts,
            repair_state: &repair_state,
            artifact_excerpts: &agent.task_contract_excerpts,
            missing_verifier_suppress_retry,
            owned_test_artifacts: &owned_test_artifacts,
        },
    );
    if matches!(
        action,
        super::task_contract::ArtifactRecoveryAction::Continue { .. }
    ) {
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
                return super::task_contract::ArtifactRecoveryAction::RunVerifier;
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
            }
            super::project_probe::CompletionProbeDecision::KeepArtifactFlow => {}
        }
    }
    action
}

pub(super) fn task_contract_recovery_target(
    agent: &Agent,
    decision: &super::task_contract::CompletionDecision,
) -> Option<super::task_contract::RecoveryTargetHint> {
    let super::task_contract::CompletionDecision::Continue { missing } = decision else {
        return None;
    };
    let role = missing.first().copied()?;
    if let Some(identity) = super::workspace_access::active_request_text(agent)
        .as_deref()
        .and_then(|request| {
            super::task_contract::explicit_artifact_obligations_from_request(request)
                .into_iter()
                .find(|identity| identity.role == role)
        })
    {
        return Some(super::task_contract::RecoveryTargetHint {
            role,
            path: identity.path,
            reason: "required artifact identity is still missing".to_string(),
        });
    }
    if let Some(path) = super::scaffold_pipeline::scaffold_candidate_for_missing_role(agent, role) {
        return Some(super::task_contract::RecoveryTargetHint {
            role,
            path,
            reason: "bootstrap scaffold artifact for the missing role is still unchanged"
                .to_string(),
        });
    }
    // Issue #646 (D1): two-step gate — first scope, then full ownership
    // classifier. A scope-internal but non-`Owned` artifact (e.g. an
    // unchanged scaffold body, a CandidateOnly README the user never
    // mentioned) MUST NOT be surfaced as a recovery target either. The
    // ownership signal — edit / scaffold delta / explicit scope mention
    // — is the same one the planner uses upstream.
    let scope = super::workspace_access::current_workspace_scope(agent);
    if let Some(path) =
        existing_workspace_candidate_for_role_in_scope(&agent.work_root, role, &scope)
    {
        let scaffold_changed =
            super::scaffold_pipeline::repo_edit_has_post_scaffold_delta(agent, &path);
        let edited_this_session = agent.turn_edited_relative_paths.contains(&path);
        let ownership = super::artifact_ownership::classify_ownership(
            super::artifact_ownership::OwnershipInputs {
                work_root: &agent.work_root,
                relative_path: &path,
                scope: &scope,
                edited_this_session,
                scaffold_changed,
                verifier_passed_in_scope: false,
                // Keep target selection conservative. The ledger/verifier
                // projection is the only place that broadens nested-test
                // admission for current-task evidence; generic recovery must
                // not claim pre-existing nested tests without evidence.
                nested_test_admission: super::artifact_ownership::NestedTestAdmission::default(),
            },
        );
        if matches!(
            ownership,
            super::artifact_ownership::ArtifactOwnership::Owned
        ) {
            return Some(super::task_contract::RecoveryTargetHint {
                role,
                path,
                reason: "existing workspace artifact matches the missing role".to_string(),
            });
        }
    }
    if let Some(path) = super::workspace_access::active_request_text(agent)
        .as_deref()
        .and_then(|req| synthesized_missing_implementation_target_path_for_request(role, req))
    {
        return Some(super::task_contract::RecoveryTargetHint {
            role,
            path,
            reason: "no existing implementation artifact for the active request; create a conventional implementation file"
                .to_string(),
        });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::loop_run::artifact_completion_job::{
        ArtifactCompletionJob, ArtifactCompletionStatus,
    };
    use crate::agent::loop_run::artifact_ledger::LedgerAdmissionContext;
    use crate::agent::loop_run::commands::test_agent_with_config;
    use crate::agent::loop_run::task_contract::{
        ArtifactRecoveryAction, ArtifactRole, RecoveryTargetHint, TaskContract, TaskKind,
    };
    use crate::config::Config;
    use crate::session::store::ConversationMessage;

    #[test]
    fn docs_artifact_satisfied_without_verification_returns_done() {
        let (mut agent, _temp) = test_agent_with_config(Config::default());
        let request = concat!(
            "STATE_CONTROL_PACKET\n",
            r#"{"objective":"Create README.md documentation with Setup and Usage sections.","next_required_action":"artifact","required_artifacts":[{"path":"README.md","role":"docs"}]}"#
        );
        agent
            .session
            .messages
            .push(ConversationMessage::user(request.to_string()));
        agent
            .session
            .working_memory
            .set_active_task(Some(request.to_string()));
        super::super::task_classification::populate_task_contract_authority(&mut agent);

        let contract = TaskContract::from_request(request);
        assert_eq!(contract.task_kind, TaskKind::Docs);
        assert!(
            contract
                .required_artifacts
                .contains(&ArtifactRole::UsageDocs)
        );
        assert!(
            !contract.verification_required,
            "plain docs artifact completion should not require a coding verifier"
        );

        let scope = super::super::workspace_access::current_workspace_scope(&agent);
        agent.artifact_completion_job = Some(
            ArtifactCompletionJob::new(
                &agent.work_root,
                &scope,
                RecoveryTargetHint {
                    role: ArtifactRole::UsageDocs,
                    path: "README.md".to_string(),
                    reason: "missing docs artifact".to_string(),
                },
                true,
                false,
            )
            .expect("docs artifact completion job"),
        );
        std::fs::write(
            agent.work_root.join("README.md"),
            "# Local Notes CLI\n\n## Setup\n\nInstall the binary.\n\n## Usage\n\nRun notes from the shell.\n",
        )
        .unwrap();
        let recorded = agent.artifact_ledger.record_repo_edit_event(
            &LedgerAdmissionContext::new(&agent.work_root, &scope),
            "README.md".to_string(),
            ArtifactRole::UsageDocs,
            true,
        );
        assert!(
            recorded.is_some(),
            "README.md repo edit must be admitted as UsageDocs evidence"
        );

        let action = task_contract_recovery_action(&mut agent, &contract, None, 1);
        assert_eq!(action, ArtifactRecoveryAction::Done);
        assert!(
            agent
                .artifact_completion_job
                .as_ref()
                .is_some_and(|job| { matches!(job.status(), ArtifactCompletionStatus::Satisfied) })
        );
    }
}
