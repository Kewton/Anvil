//! Effective-tool-policy + arbiter candidate selection extracted from
//! `turn.rs` (parent #680).
//!
//! Hosts the `EffectiveToolPolicy` decision pipeline:
//!
//! - `prepare_effective_tool_policy` (pub(super) mutable entry point) —
//!   realigns turn-local artifact recovery projection with the authoritative
//!   contract before policy selection.
//! - `effective_tool_policy` (pub(super) read-only entry point) — pre-arbitration
//!   gates (AnswerOnly / Plan mode) + the arbiter-based shell over
//!   `build_arbiter_candidates` + `select_active_job` + `project_policy`.
//! - `build_arbiter_candidates` (pub(super)) — composes the priority-1
//!   through priority-6 candidate list per #660 design.
//! - `priority_one_arbiter_candidates` (private) — VerifierRepair /
//!   MissingVerifierCreate branches via `determine_loop_control_action`.
//! - `setup_bootstrap_candidate` (private) — Issue #664 (AD22)
//!   SetupBootstrap policy decision tree (`TaskContract` +
//!   `BehaviorContractProjection` + `VerifierPrerequisiteSignal`).
//! - `push_focused_edit_candidate` (private) — shared focused-edit
//!   candidate builder.
//! - `verifier_repair_policy_for_next_action` (private) — RepairJob
//!   next-action → EffectiveToolPolicy branching.
//! - `focused_edit_policy_for_target` (private) — Write/Edit/Read+Edit
//!   policy by target existence + already_read state.
//!
//! Originally `impl Agent` methods; converted to free functions taking
//! `&Agent`, matching `actor_loop_flow` / `reply_retry` / earlier
//! vertical-slice patterns. `pub(super)` limited / no facade re-export
//! (DR3-001).

use std::path::PathBuf;

use super::Agent;
use super::active_job_arbiter::{
    ActiveJobKind, Budget, DesiredAction, JobCandidate, LoopControlAction, LoopControlInputs,
    determine_loop_control_action, project_policy, select_active_job,
    should_install_setup_bootstrap,
};
use super::tool_history::focused_edit_target_already_read;
use super::tool_policy::{EffectiveToolPolicy, EffectiveToolPolicyReason};
use super::verifier_orchestration::verifier_repair_policy_for_target_hint;
use crate::modes::plan_act::{ExecutionMode, WorkMode};

pub(super) fn prepare_effective_tool_policy(agent: &mut Agent) -> EffectiveToolPolicy {
    super::set_artifact_recovery_target::realign_current_artifact_recovery_target_to_contract(
        agent,
    );
    effective_tool_policy(agent)
}

pub(super) fn effective_tool_policy(agent: &Agent) -> EffectiveToolPolicy {
    if let Some(policy) = objective_evidence_action_policy(agent) {
        return policy;
    }

    // Issue #660: `AnswerOnlyMode` is a pre-arbitration gate (priority 0
    // in §4 of the design policy). The arbiter never sees it; we early-
    // return before constructing any selectable `JobCandidate`.
    if super::tool_policy_decisions::answer_only_mode_active(agent) {
        if super::workspace_access::workspace_appears_empty(agent) {
            return EffectiveToolPolicy::restricted(
                EffectiveToolPolicyReason::AnswerOnly,
                Vec::new(),
            );
        }
        if super::tool_policy_decisions::script_execution_requested(agent) {
            return EffectiveToolPolicy::restricted(
                EffectiveToolPolicyReason::AnswerOnly,
                vec!["Read", "Glob", "Grep", "Bash"],
            );
        } else {
            return EffectiveToolPolicy::restricted(
                EffectiveToolPolicyReason::AnswerOnly,
                vec!["Read", "Glob", "Grep"],
            );
        }
    }

    // Issue #660 (Codex CB-001 / §4 design table): `PlanModeGate` is also
    // a pre-arbitration gate, not a selectable arbitration kind. The
    // Plan-file Write/Edit exception is the authority of
    // `src/tools/registry.rs::resolve_plan_mode_write_target` /
    // `enforce_plan_stage_scope`; the arbiter must not pre-empt that
    // decision with stale `task_contract_verifier_repair_pending` or
    // other selectable state.
    if agent.session.mode_state.mode == ExecutionMode::Plan {
        return EffectiveToolPolicy::unrestricted();
    }

    // Issue #660 (Phase E): arbiter is the **sole authority** for write
    // owner selection. `effective_tool_policy` is now a thin shell over
    // `build_arbiter_candidates` + `select_active_job` + `project_policy`.
    let candidates = build_arbiter_candidates(agent);
    let selection = select_active_job(&candidates);
    project_policy(&selection)
}

fn objective_evidence_action_policy(agent: &Agent) -> Option<EffectiveToolPolicy> {
    let contract = super::task_classification::task_contract_authority(agent)?;
    let objective = contract.objective_contract();
    if objective.evidence_kind
        != super::task_contract::ObjectiveEvidenceKind::SafetyBoundaryEvidence
        || !objective.requires_evidence()
        || super::objective_evidence::objective_evidence_satisfied_for_contract(
            &agent.task_contract_evidence_set_this_turn,
            &contract,
        )
        || !objective_required_deliverables_ready(agent, &contract)
    {
        return None;
    }

    if super::objective_evidence::command_observation_evidence_collected_for_contract(
        &agent.task_contract_evidence_set_this_turn,
        &contract,
    ) && let Some(target) = objective_evidence_artifact_binding_target(agent, &contract)
    {
        return Some(EffectiveToolPolicy::evidence_action_artifact_binding(
            target,
        ));
    }

    Some(EffectiveToolPolicy::evidence_action_bash_only())
}

fn objective_evidence_artifact_binding_target(
    agent: &Agent,
    contract: &super::task_contract::TaskContract,
) -> Option<PathBuf> {
    let objective = contract.objective_contract();
    for role in objective.required_deliverables() {
        if let Some(identity) = contract.required_identities_for_role(*role).first() {
            return Some(agent.work_root.join(&identity.path));
        }
    }
    agent
        .current_artifact_recovery_target
        .as_ref()
        .map(|target| agent.work_root.join(&target.path))
}

fn objective_required_deliverables_ready(
    agent: &Agent,
    contract: &super::task_contract::TaskContract,
) -> bool {
    let objective = contract.objective_contract();
    if !objective.has_required_deliverables() {
        return true;
    }
    let projection = agent
        .artifact_ledger
        .required_artifacts_completed_projection(contract);
    if projection.overflowed() {
        return false;
    }
    objective
        .required_deliverables()
        .iter()
        .all(|role| projection.is_satisfied(*role))
}

/// Issue #660: build the arbiter candidate list from the same source
/// signals the legacy chain reads. Per DR1-004 only candidates with a
/// determined `desired_action` are pushed — there is no
/// `RejectionReason::NoDesiredAction`.
pub(super) fn build_arbiter_candidates(agent: &Agent) -> Vec<JobCandidate> {
    if let Some(candidates) = priority_one_arbiter_candidates(agent) {
        return candidates;
    }

    let mut candidates: Vec<JobCandidate> = Vec::new();

    // Priority 2: ArtifactRecovery. When a role-scoped artifact-completion
    // job is active, the ObjectiveContract target must own the turn; a
    // read/truncation-derived focused edit target is only advisory.
    if let (Some(target), Some(job)) = (
        super::artifact_recovery_flow::artifact_recovery_target_path(agent),
        agent.artifact_completion_job.as_ref(),
    ) {
        let target_already_read =
            focused_edit_target_already_read(&agent.session.messages, &target, &agent.work_root);
        let policy = EffectiveToolPolicy::artifact_directed_from_job(
            target.clone(),
            target_already_read,
            job.role(),
            job.target_path(),
            job.allowed_write_actions(),
            job.allowed_read_scope(),
        );
        let write_actions = job.allowed_write_actions().clone();
        let read_scope = job.allowed_read_scope().clone();
        candidates.push(JobCandidate {
            kind: ActiveJobKind::ArtifactRecovery,
            desired_action: DesiredAction::ArtifactDirected {
                target,
                already_read: target_already_read,
                write_actions,
                read_scope,
            },
            policy,
            budget: Budget::Unbounded,
        });
    }

    // Priority 3: ForcedSmallEditRecovery.
    if let Some(target) = super::forced_small_edit::forced_small_edit_recovery_target(agent) {
        push_focused_edit_candidate(
            agent,
            &mut candidates,
            target,
            ActiveJobKind::ForcedSmallEditRecovery,
            EffectiveToolPolicyReason::FocusedEditRecovery,
        );
    }

    // Issue #664 (Priority 4 / AD22): SetupBootstrap candidate.
    if let Some(candidate) = setup_bootstrap_candidate(agent) {
        candidates.push(candidate);
    }

    // Priority 5: FocusedEditRecovery.
    if let Some(target) = super::recovery_targets::focused_edit_recovery_target(agent) {
        push_focused_edit_candidate(
            agent,
            &mut candidates,
            target,
            ActiveJobKind::FocusedEditRecovery,
            EffectiveToolPolicyReason::FocusedEditRecovery,
        );
    }

    // Priority 6: LocalLlmSmallEditAfterRead.
    if let Some(target) = super::tool_prep::local_llm_small_edit_target(agent) {
        push_focused_edit_candidate(
            agent,
            &mut candidates,
            target,
            ActiveJobKind::LocalLlmSmallEditAfterRead,
            EffectiveToolPolicyReason::LocalLlmSmallEditAfterRead,
        );
    }

    candidates
}

fn priority_one_arbiter_candidates(agent: &Agent) -> Option<Vec<JobCandidate>> {
    match determine_loop_control_action(LoopControlInputs {
        mode: agent.session.mode_state.mode,
        task_contract_verifier_repair_pending: agent.task_contract_verifier_repair_pending,
        repair_next_action: agent.repair_job.as_ref().map(|job| job.next_action()),
        missing_verifier_next_action: agent
            .missing_verifier_job
            .as_ref()
            .map(|job| job.next_action()),
        task_contract_action: None,
    }) {
        LoopControlAction::ContinueRepairJob { next_action } => {
            let target_hint = match &next_action {
                super::repair_job::RepairNextAction::RequestPatch { target_hint } => {
                    Some(target_hint.clone())
                }
                _ => None,
            };
            Some(vec![JobCandidate {
                kind: ActiveJobKind::VerifierRepair,
                desired_action: DesiredAction::VerifierRepair {
                    command: String::new(),
                    target_hint: target_hint.clone(),
                    worker_request: diagnostic_repair_worker_request_for_evidence_failed(
                        agent,
                        target_hint.as_ref(),
                    ),
                },
                policy: verifier_repair_policy_for_next_action(agent, &next_action),
                budget: Budget::Unbounded,
            }])
        }
        LoopControlAction::ContinueMissingVerifierJob { next_action } => {
            if matches!(
                next_action,
                super::repair_job::VerifierBootstrapNextAction::RequestSetupEdit
            ) && let Some(job) = agent.missing_verifier_job.as_ref()
            {
                let worker_request = test_author_worker_request_for_missing_evidence(agent);
                return Some(vec![JobCandidate {
                    kind: ActiveJobKind::VerifierRepair,
                    desired_action: DesiredAction::MissingVerifierCreate { worker_request },
                    policy: EffectiveToolPolicy::restricted(
                        EffectiveToolPolicyReason::VerifierRepair,
                        job.allowed_tool_names().to_vec(),
                    ),
                    budget: Budget::Unbounded,
                }]);
            }
            Some(Vec::new())
        }
        LoopControlAction::RunVerifier
        | LoopControlAction::Done
        | LoopControlAction::RequestModelTurn => None,
    }
}

fn test_author_worker_request_for_missing_evidence(
    agent: &Agent,
) -> Option<super::worker_contract::TestAuthorWorkerRequest> {
    let contract = super::task_classification::task_contract_authority(agent)?;
    let active_request = super::workspace_access::active_request_text(agent).unwrap_or_default();
    let implementation_context = agent
        .task_contract_excerpts
        .get(&super::task_contract::ArtifactRole::Implementation)
        .map(String::as_str);
    super::worker_contract::test_author_worker_request_for_missing_evidence(
        contract.as_ref(),
        super::verifier_orchestration::synthesized_missing_test_target_path_for_request(
            &active_request,
        ),
        implementation_context,
    )
}

fn diagnostic_repair_worker_request_for_evidence_failed(
    agent: &Agent,
    next_action_target_hint: Option<&super::task_contract::RecoveryTargetHint>,
) -> Option<super::worker_contract::DiagnosticRepairWorkerRequest> {
    let contract = super::task_classification::task_contract_authority(agent)?;
    let job = agent.repair_job.as_ref()?;
    let target_hint = next_action_target_hint
        .or(job.repair_target_hint.as_ref())
        .or(job.target_hint.as_ref())?;
    let allowed_change_kind = job
        .correction_job
        .as_ref()
        .map(|correction| correction.kind.as_str())
        .unwrap_or("bounded_target_repair");
    let diagnostic = diagnostic_repair_diagnostic_for_job(job);
    Some(
        super::worker_contract::diagnostic_repair_worker_request_for_evidence_failed(
            contract.as_ref(),
            &diagnostic,
            target_hint,
            allowed_change_kind,
            Some(job.command.as_str()),
        ),
    )
}

fn diagnostic_repair_diagnostic_for_job(job: &super::repair_job::RepairJob) -> String {
    let error_kind = job
        .error_kind
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(job.failure_signature.as_str());
    if job.output_excerpt.trim().is_empty() {
        error_kind.to_string()
    } else {
        format!("{error_kind}\n{}", job.output_excerpt)
    }
}

fn push_focused_edit_candidate(
    agent: &Agent,
    candidates: &mut Vec<JobCandidate>,
    target: PathBuf,
    kind: ActiveJobKind,
    reason: EffectiveToolPolicyReason,
) {
    let already_read =
        focused_edit_target_already_read(&agent.session.messages, &target, &agent.work_root);
    candidates.push(JobCandidate {
        kind,
        desired_action: DesiredAction::FocusedEdit {
            target: target.clone(),
            already_read,
        },
        policy: focused_edit_policy_for_target(agent, target, reason),
        budget: Budget::Unbounded,
    });
}

fn setup_bootstrap_candidate(agent: &Agent) -> Option<JobCandidate> {
    if matches!(
        agent.session.mode_state.work_mode,
        WorkMode::Docs | WorkMode::AnswerOnly
    ) {
        return None;
    }
    // Issue #917: per-turn classification authority (`None` keeps the legacy
    // early return via `?`). `&Rc<TaskContract>` deref-coerces to `&TaskContract`.
    let task_contract = super::task_classification::task_contract_authority(agent)?;
    let behavior_projection = super::required_behavior::project_behavior_contract(&task_contract);
    let verifier_signal = super::task_contract::VerifierPrerequisiteSignal::from_sources(
        agent.owned_test_verifier_missing_observed_this_turn,
        behavior_projection.as_ref(),
    );
    if !should_install_setup_bootstrap(
        &task_contract,
        behavior_projection.as_ref(),
        &verifier_signal,
        agent.artifact_ledger.overflowed(),
    ) {
        return None;
    }
    Some(JobCandidate {
        kind: ActiveJobKind::SetupBootstrap,
        desired_action: DesiredAction::SetupBash,
        policy: EffectiveToolPolicy::setup_bootstrap(),
        budget: Budget::Unbounded,
    })
}

fn verifier_repair_policy_for_next_action(
    agent: &Agent,
    action: &super::repair_job::RepairNextAction,
) -> EffectiveToolPolicy {
    match action {
        super::repair_job::RepairNextAction::RequestPatch { target_hint } => {
            verifier_repair_policy_for_target_hint(
                target_hint,
                &agent.session.messages,
                &agent.work_root,
            )
        }
        super::repair_job::RepairNextAction::RequestDiagnostic
        | super::repair_job::RepairNextAction::Replan
        | super::repair_job::RepairNextAction::RerunVerifier
        | super::repair_job::RepairNextAction::SafeStop { .. }
        | super::repair_job::RepairNextAction::VerifiedDone => {
            EffectiveToolPolicy::restricted(EffectiveToolPolicyReason::VerifierRepair, Vec::new())
        }
    }
}

fn focused_edit_policy_for_target(
    agent: &Agent,
    target: PathBuf,
    reason: EffectiveToolPolicyReason,
) -> EffectiveToolPolicy {
    let target_already_read =
        focused_edit_target_already_read(&agent.session.messages, &target, &agent.work_root);
    if !target.is_file() {
        EffectiveToolPolicy::focused_edit(reason, vec!["Write"], target, target_already_read)
    } else if target_already_read {
        EffectiveToolPolicy::focused_edit(reason, vec!["Edit"], target, target_already_read)
    } else {
        EffectiveToolPolicy::focused_edit(reason, vec!["Read", "Edit"], target, target_already_read)
    }
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use crate::agent::loop_run::artifact_completion_job::ArtifactCompletionJob;
    use crate::agent::loop_run::artifact_ledger::LedgerAdmissionContext;
    use crate::agent::loop_run::commands::test_agent_with_config;
    use crate::agent::loop_run::completion_evidence::CompletionEvidence;
    use crate::agent::loop_run::project_profile::parse_project_profile_confirmation;
    use crate::agent::loop_run::task_contract::{
        ArtifactRole, RecoveryTarget, RecoveryTargetHint, TaskContract,
    };
    use crate::agent::loop_run::tool_policy::EffectiveToolPolicyReason;
    use crate::config::Config;
    use crate::session::store::ConversationMessage;

    use super::*;

    fn command_observation_docs_contract(request: &str) -> TaskContract {
        let profile = parse_project_profile_confirmation(
            r#"{
                "language":"unknown",
                "shape":"cli",
                "deliverable_kind":"document",
                "primary_artifacts":["ops-observation.md"],
                "forbidden_artifacts":["source_code","tests","setup"],
                "evidence_kind":"command_observation",
                "needs_environment_setup":false,
                "preferred_runner":null,
                "confidence":1.0,
                "reason":"the document must be grounded in an observed local command"
            }"#,
        )
        .expect("profile");
        TaskContract::from_request_with_kind_and_project_profile(request, None, Some(&profile))
    }

    fn seed_active_contract(
        agent: &mut crate::agent::loop_run::Agent,
        request: &str,
        contract: TaskContract,
    ) -> Rc<TaskContract> {
        agent
            .session
            .messages
            .push(ConversationMessage::user(request.to_string()));
        agent
            .session
            .working_memory
            .set_active_task(Some(request.to_string()));
        agent.project_profile_confirm_called_this_turn = true;
        let contract = Rc::new(contract);
        agent
            .task_contract_this_turn
            .set(contract.clone())
            .expect("unset task contract cell");
        contract
    }

    fn python_tdd_contract(request: &str) -> TaskContract {
        let profile = parse_project_profile_confirmation(
            r#"{
                "language":"python",
                "shape":"library",
                "deliverable_kind":"code",
                "primary_artifacts":["math_utils.py","tests/test_math_utils.py"],
                "forbidden_artifacts":["docs","setup"],
                "evidence_kind":"test_run",
                "needs_environment_setup":false,
                "preferred_runner":"python -m unittest discover -s tests",
                "confidence":1.0,
                "reason":"explicit Python TDD request"
            }"#,
        )
        .expect("profile");
        TaskContract::from_request_with_kind_and_project_profile(request, None, Some(&profile))
    }

    #[test]
    fn command_observation_evidence_policy_waits_for_deliverable() {
        let (mut agent, _temp) = test_agent_with_config(Config::default());
        let request =
            "Run pwd and write ops-observation.md containing the exact observed directory.";
        let contract = command_observation_docs_contract(request);
        seed_active_contract(&mut agent, request, contract);

        let policy = effective_tool_policy(&agent);

        assert_ne!(policy.reason(), EffectiveToolPolicyReason::EvidenceAction);
    }

    #[test]
    fn prepared_policy_realigns_stale_artifact_job_to_contract_identity() {
        let (mut agent, _temp) = test_agent_with_config(Config::default());
        let request = "Coding TDD task: create math_utils.py and tests/test_math_utils.py only. Implement clamp(value, minimum, maximum). Use Python unittest.";
        let contract = seed_active_contract(&mut agent, request, python_tdd_contract(request));
        let scope = super::super::workspace_access::current_workspace_scope(&agent);
        std::fs::create_dir_all(agent.work_root.join("tests")).unwrap();
        std::fs::write(agent.work_root.join("tests/cli.rs"), "stale rust test\n").unwrap();
        agent.artifact_completion_job = Some(
            ArtifactCompletionJob::new(
                &agent.work_root,
                &scope,
                RecoveryTargetHint {
                    role: ArtifactRole::Test,
                    path: "tests/cli.rs".to_string(),
                    reason: "stale request-derived fallback".to_string(),
                },
                true,
                false,
            )
            .expect("stale artifact completion job"),
        );
        agent.current_artifact_recovery_target = Some(RecoveryTarget {
            role: ArtifactRole::Test,
            path: "tests/cli.rs".to_string(),
            reason: "stale request-derived fallback".to_string(),
            attempt: 2,
        });

        let policy = prepare_effective_tool_policy(&mut agent);

        assert_eq!(
            contract.required_identities_for_role(ArtifactRole::Test)[0].path,
            "tests/test_math_utils.py"
        );
        assert_eq!(
            policy.reason(),
            EffectiveToolPolicyReason::ArtifactDirectedRecovery
        );
        let artifact = policy
            .artifact_directed_policy()
            .expect("artifact-directed policy");
        assert!(artifact.target.ends_with("tests/test_math_utils.py"));
        let context = artifact.job_context.as_ref().expect("job context");
        assert_eq!(context.target_path, "tests/test_math_utils.py");
        assert_eq!(
            agent
                .current_artifact_recovery_target
                .as_ref()
                .expect("current target")
                .path,
            "tests/test_math_utils.py"
        );
        assert_eq!(
            agent
                .artifact_completion_job
                .as_ref()
                .expect("job")
                .target_path(),
            "tests/test_math_utils.py"
        );
    }

    #[test]
    fn command_observation_evidence_policy_allows_only_bash_after_deliverable() {
        let (mut agent, _temp) = test_agent_with_config(Config::default());
        let request =
            "Run pwd and write ops-observation.md containing the exact observed directory.";
        let contract = seed_active_contract(
            &mut agent,
            request,
            command_observation_docs_contract(request),
        );
        let scope = super::super::workspace_access::current_workspace_scope(&agent);
        std::fs::write(agent.work_root.join("ops-observation.md"), "pending\n").unwrap();
        agent.artifact_ledger.record_repo_edit_event(
            &LedgerAdmissionContext::new(&agent.work_root, &scope),
            "ops-observation.md".to_string(),
            ArtifactRole::UsageDocs,
            true,
        );

        let policy = effective_tool_policy(&agent);

        assert_eq!(
            contract.objective_contract().required_deliverables().len(),
            1
        );
        assert_eq!(policy.reason(), EffectiveToolPolicyReason::EvidenceAction);
        assert_eq!(policy.allowed_tool_names_for_prompt(), Some(&["Bash"][..]));
    }

    #[test]
    fn command_observation_evidence_policy_binds_artifact_after_observed_command() {
        let (mut agent, _temp) = test_agent_with_config(Config::default());
        let request =
            "Run pwd and write ops-observation.md containing the exact observed directory.";
        seed_active_contract(
            &mut agent,
            request,
            command_observation_docs_contract(request),
        );
        let scope = super::super::workspace_access::current_workspace_scope(&agent);
        std::fs::write(agent.work_root.join("ops-observation.md"), "pending\n").unwrap();
        agent.artifact_ledger.record_repo_edit_event(
            &LedgerAdmissionContext::new(&agent.work_root, &scope),
            "ops-observation.md".to_string(),
            ArtifactRole::UsageDocs,
            true,
        );
        agent
            .task_contract_evidence_set_this_turn
            .push(CompletionEvidence::CommandObservation {
                command: "pwd".to_string(),
                exit_status: 0,
                safety_boundary_passed: true,
            });

        let policy = effective_tool_policy(&agent);

        assert_eq!(policy.reason(), EffectiveToolPolicyReason::EvidenceAction);
        assert_eq!(
            policy.allowed_tool_names_for_prompt(),
            Some(&["Write", "Edit"][..])
        );
        let artifact = policy
            .artifact_directed_policy()
            .expect("artifact binding policy");
        assert_eq!(artifact.target, agent.work_root.join("ops-observation.md"));
        assert!(artifact.target_already_read);
    }
}
