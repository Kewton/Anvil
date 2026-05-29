//! Effective-tool-policy + arbiter candidate selection extracted from
//! `turn.rs` (parent #680).
//!
//! Hosts the `EffectiveToolPolicy` decision pipeline:
//!
//! - `effective_tool_policy` (pub(super) entry point) — pre-arbitration
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

pub(super) fn effective_tool_policy(agent: &Agent) -> EffectiveToolPolicy {
    // Issue #660: `AnswerOnlyMode` is a pre-arbitration gate (priority 0
    // in §4 of the design policy). The arbiter never sees it; we early-
    // return before constructing any selectable `JobCandidate`.
    if agent.answer_only_mode_active() {
        if agent.workspace_appears_empty() {
            return EffectiveToolPolicy::restricted(
                EffectiveToolPolicyReason::AnswerOnly,
                Vec::new(),
            );
        }
        if agent.script_execution_requested() {
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

/// Issue #660: build the arbiter candidate list from the same source
/// signals the legacy chain reads. Per DR1-004 only candidates with a
/// determined `desired_action` are pushed — there is no
/// `RejectionReason::NoDesiredAction`.
pub(super) fn build_arbiter_candidates(agent: &Agent) -> Vec<JobCandidate> {
    if let Some(candidates) = priority_one_arbiter_candidates(agent) {
        return candidates;
    }

    let mut candidates: Vec<JobCandidate> = Vec::new();

    // Priority 2: ForcedSmallEditRecovery.
    if let Some(target) = agent.forced_small_edit_recovery_target() {
        push_focused_edit_candidate(
            agent,
            &mut candidates,
            target,
            ActiveJobKind::ForcedSmallEditRecovery,
            EffectiveToolPolicyReason::FocusedEditRecovery,
        );
    }

    // Priority 3: ArtifactRecovery.
    if let (Some(target), Some(job)) = (
        agent.artifact_recovery_target_path(),
        agent.artifact_completion_job.as_ref(),
    ) {
        let target_already_read =
            focused_edit_target_already_read(&agent.session.messages, &target, &agent.work_root);
        let policy = EffectiveToolPolicy::artifact_directed_from_job(
            target.clone(),
            target_already_read,
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

    // Issue #664 (Priority 4 / AD22): SetupBootstrap candidate.
    if let Some(candidate) = setup_bootstrap_candidate(agent) {
        candidates.push(candidate);
    }

    // Priority 5: FocusedEditRecovery.
    if let Some(target) = agent.focused_edit_recovery_target() {
        push_focused_edit_candidate(
            agent,
            &mut candidates,
            target,
            ActiveJobKind::FocusedEditRecovery,
            EffectiveToolPolicyReason::FocusedEditRecovery,
        );
    }

    // Priority 6: LocalLlmSmallEditAfterRead.
    if let Some(target) = agent.local_llm_small_edit_target() {
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
                    target_hint,
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
                return Some(vec![JobCandidate {
                    kind: ActiveJobKind::VerifierRepair,
                    desired_action: DesiredAction::MissingVerifierCreate,
                    policy: EffectiveToolPolicy::restricted(
                        EffectiveToolPolicyReason::VerifierRepair,
                        job.allowed_tool_names().to_vec(),
                    ),
                    budget: Budget::Unbounded,
                }]);
            }
            Some(Vec::new())
        }
        LoopControlAction::RunVerifier | LoopControlAction::RequestModelTurn => None,
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
    let request = agent.active_request_text()?;
    let task_contract = super::task_contract::TaskContract::from_request(&request);
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
