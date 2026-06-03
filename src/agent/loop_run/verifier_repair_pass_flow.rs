//! Verifier-repair pass flow extracted from `turn.rs` (parent #680).
//!
//! Hosts the per-attempt verifier-repair pass lifecycle (the
//! repair-plan-driven targeted-edit loop):
//!
//! - `prepare_verifier_repair_pass` — admission gate + accepted plan
//!   build + behavior-projection emit + prompt-message render.
//! - `handle_verifier_repair_pass_attempt` — per-attempt dispatcher
//!   (reply success / chat error / unexpected tool calls / validation
//!   outcomes → `VerifierRepairAttemptProgress`).
//! - `verifier_repair_pass_wall_clock_timeout_error` — wall-clock
//!   timeout error + log.
//! - `handle_verifier_repair_pass_reply` (private) — patch-proposal
//!   parse + shadow validation + intent admission + legacy validation
//!   comparison + final apply via
//!   `verifier_orchestration::apply_verifier_repair_pass_edit`.
//! - `log_verifier_repair_pass_timeout` (private) — bounded
//!   `agent.verifier_repair_pass.timeout` event.
//!
//! Originally `impl Agent` methods; converted to free functions taking
//! `&mut Agent` / `&Agent`, matching the `actor_loop_flow` /
//! `reply_retry` / earlier vertical-slice precedent. `pub(super)`
//! limited / no facade re-export (DR3-001).

use std::time::Duration;

use super::Agent;
use super::repair_driver::{
    VERIFIER_REPAIR_PASS_WALL_CLOCK_LIMIT_SECS, VerifierRepairPassOutcome,
    verifier_repair_pass_timeout_error,
};
use super::repair_patch_validation::{
    CheapCheckOutcome, RepairRejectionSignal, ValidationFailure,
    build_verifier_repair_pass_ledger_outcome, validate_accepted_repair_plan_authorizes_target,
};
use super::verifier_orchestration::{
    PreparedVerifierRepairPass, VerifierRepairAttemptProgress,
    emit_patch_proposal_legacy_validation_comparison_event,
    emit_patch_proposal_shadow_validation_event,
    validate_verifier_repair_intents_with_accepted_plan, verifier_repair_intent_limits,
    verifier_repair_pass_messages, verifier_repair_pass_request_error_message,
};
use super::verifier_repair_shadow::legacy_repair_brief_input_from_assessment;
use crate::logging::log_llm_event;
use crate::ollama::client::AssistantReply;

pub(super) fn verifier_repair_pass_wall_clock_timeout_error(
    agent: &Agent,
    prepared: &PreparedVerifierRepairPass,
    target_hint: &super::task_contract::RecoveryTargetHint,
    attempt: usize,
    elapsed: Duration,
) -> String {
    let error = verifier_repair_pass_timeout_error(elapsed);
    log_verifier_repair_pass_timeout(agent, prepared, target_hint, attempt, elapsed, None);
    error
}

fn log_verifier_repair_pass_timeout(
    agent: &Agent,
    prepared: &PreparedVerifierRepairPass,
    target_hint: &super::task_contract::RecoveryTargetHint,
    attempt: usize,
    elapsed: Duration,
    attempt_timeout_secs: Option<u64>,
) {
    log_llm_event(
        "agent.verifier_repair_pass.timeout",
        serde_json::json!({
            "session_id": agent.session_store.session_id(),
            "model": &prepared.model,
            "path": target_hint.path,
            "attempt": attempt,
            "elapsed_secs": elapsed.as_secs(),
            "limit_secs": VERIFIER_REPAIR_PASS_WALL_CLOCK_LIMIT_SECS,
            "attempt_timeout_secs": attempt_timeout_secs,
        }),
    );
}

pub(super) fn handle_verifier_repair_pass_attempt(
    agent: &mut Agent,
    prepared: &mut PreparedVerifierRepairPass,
    target_hint: &super::task_contract::RecoveryTargetHint,
    attempt: usize,
    attempt_timeout_secs: u64,
    elapsed: Duration,
    reply: Result<AssistantReply, String>,
) -> VerifierRepairAttemptProgress {
    let reply = match reply {
        Ok(reply) => reply,
        Err(err) => {
            let last_error = verifier_repair_pass_request_error_message(&err, attempt_timeout_secs);
            if last_error.contains("verifier_repair_pass_timeout") {
                log_verifier_repair_pass_timeout(
                    agent,
                    prepared,
                    target_hint,
                    attempt,
                    elapsed,
                    Some(attempt_timeout_secs),
                );
            }
            return VerifierRepairAttemptProgress::Break { last_error };
        }
    };
    if !reply.tool_calls.is_empty() {
        return VerifierRepairAttemptProgress::Continue {
            last_error: "repair reply contained unexpected tool calls".to_string(),
            last_invalid_outcome: None,
        };
    }
    match handle_verifier_repair_pass_reply(agent, prepared, target_hint, attempt, &reply.content) {
        Ok(outcome) => VerifierRepairAttemptProgress::Return(outcome),
        Err(ValidationFailure {
            outcome: CheapCheckOutcome::Failed(message),
            weakening,
            rejection_signal,
        }) => VerifierRepairAttemptProgress::Continue {
            last_error: message,
            last_invalid_outcome: build_verifier_repair_pass_ledger_outcome(
                weakening,
                rejection_signal,
                prepared.context.semantic_plan.as_ref(),
            ),
        },
        Err(ValidationFailure {
            outcome: CheapCheckOutcome::Unavailable,
            ..
        }) => {
            log_llm_event(
                "agent.verifier_repair_pass.unavailable",
                serde_json::json!({
                    "session_id": agent.session_store.session_id(),
                    "model": &prepared.model,
                    "target_path": target_hint.path,
                    "attempt": attempt,
                }),
            );
            VerifierRepairAttemptProgress::Return(VerifierRepairPassOutcome::Unavailable {
                relative_path: target_hint.path.clone(),
            })
        }
    }
}

pub(super) fn prepare_verifier_repair_pass(
    agent: &mut Agent,
    target_hint: &super::task_contract::RecoveryTargetHint,
) -> Result<PreparedVerifierRepairPass, VerifierRepairPassOutcome> {
    let Some(context) = agent.repair_job.clone() else {
        return Err(VerifierRepairPassOutcome::Skipped);
    };
    let active_request = super::workspace_access::active_request_text(agent).unwrap_or_default();
    // Issue #917: per-turn classification authority (None → empty-input
    // contract, matching the legacy `unwrap_or_default`).
    let task_contract =
        super::task_classification::task_contract_authority(agent).unwrap_or_else(|| {
            std::rc::Rc::new(super::task_contract::TaskContract::from_request(
                &active_request,
            ))
        });
    let behavior_projection = super::required_behavior::project_behavior_contract(&task_contract);
    super::active_job_emit::emit_behavior_contract_projected_if_changed(
        agent,
        behavior_projection.as_ref(),
        super::required_behavior::BEHAVIOR_CONTRACT_CONSUMER_VERIFIER_REPAIR,
    );
    let behavior_contract_has_repair_authority =
        super::required_behavior::behavior_contract_has_repair_authority(&task_contract);
    let legacy_input = context
        .assessment
        .as_ref()
        .map(|assessment| legacy_repair_brief_input_from_assessment(assessment, 0.6));
    let accepted_plan = match super::repair_plan_admission::validate_repair_plan_admission(
        super::repair_plan_admission::RepairPlanAdmissionInput {
            context: &context,
            active_request: &active_request,
            behavior_contract_present: behavior_contract_has_repair_authority,
            legacy_input,
        },
    ) {
        Ok(plan) => plan,
        Err(err) => {
            let reason = err.message();
            let error = format!("verifier_repair_pass_invalid: {reason}");
            if let Some(job) = agent.repair_job.as_mut() {
                job.apply_event(super::repair_plan_admission::admission_error_event(&err));
            }
            log_llm_event(
                "agent.verifier_repair_plan.rejected",
                serde_json::json!({
                    "session_id": agent.session_store.session_id(),
                    "target_path": target_hint.path,
                    "reason": reason,
                }),
            );
            return Err(VerifierRepairPassOutcome::Invalid {
                error,
                repair_attempt_outcome: None,
            });
        }
    };
    if let Err(err) = validate_accepted_repair_plan_authorizes_target(
        &accepted_plan,
        target_hint,
        &target_hint.path,
    ) {
        return Err(VerifierRepairPassOutcome::Invalid {
            error: format!("verifier_repair_pass_invalid: {}", err.reason_label()),
            repair_attempt_outcome: build_verifier_repair_pass_ledger_outcome(
                err.weakening,
                err.rejection_signal,
                context.semantic_plan.as_ref(),
            ),
        });
    }
    let messages = match verifier_repair_pass_messages(
        &agent.work_root,
        &context,
        target_hint,
        &active_request,
        behavior_projection.as_ref(),
    ) {
        Ok(messages) => messages,
        Err(err) => {
            return Err(VerifierRepairPassOutcome::Invalid {
                error: format!("verifier_repair_pass_invalid: {err}"),
                repair_attempt_outcome: None,
            });
        }
    };
    Ok(PreparedVerifierRepairPass {
        context,
        accepted_plan,
        messages,
        model: agent.models.main.clone(),
    })
}

fn handle_verifier_repair_pass_reply(
    agent: &mut Agent,
    prepared: &mut PreparedVerifierRepairPass,
    target_hint: &super::task_contract::RecoveryTargetHint,
    attempt: usize,
    reply_content: &str,
) -> Result<VerifierRepairPassOutcome, ValidationFailure> {
    let validation = super::repair_patch_validation::parse_verifier_repair_patch_proposal_reply(
        reply_content,
        verifier_repair_intent_limits(),
    )
    .map_err(|message| {
        ValidationFailure::failed_with_signal(message, RepairRejectionSignal::Malformed)
    })
    .and_then(|proposal| {
        let shadow_validation = emit_patch_proposal_shadow_validation_event(
            agent.session_store.session_id(),
            &prepared.model,
            attempt,
            &agent.work_root,
            &prepared.accepted_plan,
            &proposal,
        );
        if shadow_validation.is_decisive() && !shadow_validation.accepted() {
            return Err(ValidationFailure::failed_with_signal(
                format!(
                    "patch provider admission rejected: {}",
                    shadow_validation.reason
                ),
                RepairRejectionSignal::Malformed,
            ));
        }
        let intents = super::repair_patch_validation::patch_proposal_to_verifier_repair_intents(
            proposal.clone(),
            verifier_repair_intent_limits(),
        )
        .map_err(|message| {
            ValidationFailure::failed_with_signal(message, RepairRejectionSignal::Malformed)
        })?;
        let validation = validate_verifier_repair_intents_with_accepted_plan(
            &agent.work_root,
            &prepared.context,
            target_hint,
            &prepared.accepted_plan,
            intents,
        );
        emit_patch_proposal_legacy_validation_comparison_event(
            agent.session_store.session_id(),
            &prepared.model,
            attempt,
            &proposal,
            shadow_validation,
            &validation,
        );
        validation
    });
    match validation {
        Ok(edit) => super::verifier_orchestration::apply_verifier_repair_pass_edit(
            agent,
            prepared,
            target_hint,
            attempt,
            edit,
        ),
        Err(error) => Err(error),
    }
}
