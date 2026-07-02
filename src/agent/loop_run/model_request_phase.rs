//! Actor-loop model request phase.
//!
//! This is a behavior-preserving wrapper around the existing pre-reply driver.
//! It owns phase telemetry and the typed input bundle for this phase; terminal
//! authority remains with the caller that handles `ActorLoopPreReplyOutcome`.

use crate::agent::orchestration::{RepoSnapshot, RepoVerification};
use crate::agent::recovery;

use super::Agent;
use super::active_job_arbiter::RecoveryOwner;
use super::actor_loop_flow::{
    ActorLoopPreReplyArgs, ActorLoopPreReplyOutcome, drive_actor_loop_pre_reply_phase,
};
use super::interrupt::InterruptFlag;
use super::loop_phase::{LoopPhase, LoopPhaseTransition, emit_loop_phase};
use super::loop_state::LoopState;
use super::task_contract::TaskContract;

pub(super) struct ModelRequestPhaseArgs<'a> {
    pub(super) before_snapshot: &'a RepoSnapshot,
    pub(super) accumulated: &'a [RepoVerification],
    pub(super) task_contract: Option<&'a TaskContract>,
    pub(super) loop_state: &'a mut LoopState,
    pub(super) framework_app_fallback_materialized: &'a mut bool,
    pub(super) action_expectation: recovery::ActionExpectation,
    pub(super) stream_output: bool,
    pub(super) last_iter: usize,
    pub(super) interrupt_flag: &'a InterruptFlag,
}

pub(super) fn run_model_request_phase(
    agent: &mut Agent,
    args: ModelRequestPhaseArgs<'_>,
) -> ActorLoopPreReplyOutcome {
    emit_loop_phase(
        agent,
        LoopPhase::ModelRequestPrepared,
        LoopPhaseTransition::Enter,
        args.last_iter,
        serde_json::json!({
            "contract_present": args.task_contract.is_some(),
        }),
    );

    let outcome = drive_actor_loop_pre_reply_phase(
        agent,
        ActorLoopPreReplyArgs {
            before_snapshot: args.before_snapshot,
            accumulated: args.accumulated,
            task_contract: args.task_contract,
            repo_edit_calls_made_this_turn: &mut args.loop_state.repo_edit_calls_made_this_turn,
            contract_verification_retries: &mut args.loop_state.contract_verification_retries,
            contract_verifier_repair_edit_count: &mut args
                .loop_state
                .contract_verifier_repair_edit_count,
            repo_change_retries: &mut args.loop_state.repo_change_retries,
            verifier_repair_retries: &mut args.loop_state.verifier_repair_retries,
            task_contract_verify_commands_collected: &mut args
                .loop_state
                .task_contract_verify_commands_collected,
            task_contract_verifier_passed_in_loop: &mut args
                .loop_state
                .task_contract_verifier_passed_in_loop,
            framework_app_fallback_materialized: args.framework_app_fallback_materialized,
            action_expectation: args.action_expectation,
            stream_output: args.stream_output,
            last_iter: args.last_iter,
            interrupt_flag: args.interrupt_flag,
        },
    );

    emit_loop_phase(
        agent,
        LoopPhase::ModelRequestPrepared,
        LoopPhaseTransition::Exit,
        args.last_iter,
        model_request_exit_detail(&outcome),
    );

    outcome
}

fn model_request_exit_detail(outcome: &ActorLoopPreReplyOutcome) -> serde_json::Value {
    match outcome {
        ActorLoopPreReplyOutcome::ReplyPrepared {
            missing_verifier_setup_turn,
            recovery_owner,
            ..
        } => serde_json::json!({
            "outcome": "reply_prepared",
            "missing_verifier_setup_turn": missing_verifier_setup_turn,
            "recovery_job_kind": recovery_owner_label(*recovery_owner),
        }),
        ActorLoopPreReplyOutcome::Continue => serde_json::json!({
            "outcome": "continue",
        }),
        ActorLoopPreReplyOutcome::Done { .. } => serde_json::json!({
            "outcome": "done",
        }),
        ActorLoopPreReplyOutcome::Exit { reason, .. } => serde_json::json!({
            "outcome": "exit",
            "exit_reason": reason.label(),
        }),
    }
}

fn recovery_owner_label(owner: RecoveryOwner) -> &'static str {
    owner
        .recovery_job_kind()
        .map(|kind| kind.as_str())
        .unwrap_or("None")
}

#[cfg(test)]
mod tests {
    use super::super::summary::ExitReason;
    use super::*;

    #[test]
    fn model_request_exit_detail_labels_non_reply_outcomes() {
        assert_eq!(
            model_request_exit_detail(&ActorLoopPreReplyOutcome::Continue)["outcome"],
            "continue"
        );
        assert_eq!(
            model_request_exit_detail(&ActorLoopPreReplyOutcome::Done {
                final_prose: String::new(),
            })["outcome"],
            "done"
        );
        let exit = model_request_exit_detail(&ActorLoopPreReplyOutcome::Exit {
            reason: ExitReason::MissingRepoEdits,
            error_text: String::new(),
        });
        assert_eq!(exit["outcome"], "exit");
        assert_eq!(exit["exit_reason"], "missing_repo_edits");
    }

    #[test]
    fn recovery_owner_label_uses_existing_recovery_job_projection() {
        assert_eq!(recovery_owner_label(RecoveryOwner::None), "None");
        assert_eq!(
            recovery_owner_label(RecoveryOwner::ArtifactCompletion),
            "MissingDeliverableJob"
        );
        assert_eq!(
            recovery_owner_label(RecoveryOwner::RepairJob),
            "EvidenceFailedJob"
        );
        assert_eq!(
            recovery_owner_label(RecoveryOwner::MissingVerifierJob),
            "MissingEvidenceJob"
        );
    }
}
