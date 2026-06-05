//! Repair-job dispatch flow extracted from `turn.rs` (parent #680).
//!
//! Hosts the production entry points for the verifier-repair state
//! machine and their internal step handlers. Originally `impl Agent`
//! methods; converted to free functions taking `&mut Agent`, matching
//! `actor_loop_flow` / `reply_retry` / earlier vertical-slice patterns.
//!
//! Entry points (pub(super)):
//! - `dispatch_repair_job_step` — RepairStep state-machine dispatch.
//! - `dispatch_missing_verifier_job_step` — missing-verifier
//!   bootstrap-action dispatch.
//! - `repair_rejection_next_action` — pure-fn error → next-action note.
//!
//! Internal helpers (private):
//! - `handle_repair_job_verifier_pass` / `_failure` / `_no_verifier` /
//!   `_safe_stop` — TaskContractVerifierOutcome branches.
//! - `handle_repair_job_diagnostic_step` / `_patch_provider_step` —
//!   RepairStep branches.
//! - `write_repair_job_step_status` — iteration-status renderer.
//! - `repair_job_diagnostic_skipped_error` /
//!   `repair_job_safe_stop_outcome` — terminal outcome builders.
//! - `drive_repair_job_verifier` — verifier rerun + outcome routing.
//! - `dispatch_after_repair_patch_rejection` — rejected-patch
//!   next-action routing.
//!
//! `pub(super)` limited / no facade re-export (DR3-001).

use super::Agent;
use super::actor_loop_flow::{
    TaskContractVerifierFlowOutcome, format_iteration_status, repair_job_done_outcome,
};
use super::controller_policy::ControllerRecoveryStrategy;
use super::repair_driver::VerifierRepairPassOutcome;
use super::repair_job;
use super::repair_job::verifier_repair_context_from_failure;
use super::summary::ExitReason;
use super::task_contract::{RecoveryTargetHint, SafeStopReason};
use super::turn_helpers::write_stdout_rendered;
use super::verifier_driver::TaskContractVerifierOutcome;
use super::verifier_orchestration::{
    TaskContractVerifierFlowArgs, VerifierDiagnosticPassOutcome,
    emit_repair_progress_classified_event, repair_terminal_exit_reason,
    task_contract_needs_verification, task_contract_verifier_failure_attempt_limit,
    task_contract_verifier_repair_note,
};
use super::verifier_repair_targeting::changed_files_for_verifier;
use crate::agent::orchestration::verify_repo_progress;
use crate::logging::log_llm_event;

fn handle_repair_job_verifier_pass(
    agent: &mut Agent,
    args: TaskContractVerifierFlowArgs<'_, '_>,
    previous_repair_context: repair_job::RepairJob,
    command: String,
) -> TaskContractVerifierFlowOutcome {
    if task_contract_needs_verification(
        agent.session.mode_state.mode,
        args.task_contract,
        &agent.task_contract_evidence_set_this_turn,
    ) {
        return TaskContractVerifierFlowOutcome::Exit {
            reason: ExitReason::MissingVerification,
            error_text: "verifier passed but verifier evidence could not be recorded".to_string(),
        };
    }
    let safe_command = crate::session::feedback::mask_secrets(&command).replace('`', "\\`");
    if let Some(sanitized) =
        super::verifier_skill::sanitize_verify_command_for_case_record(&command)
    {
        args.task_contract_verify_commands_collected.push(sanitized);
    }
    if let Some(job) = agent.repair_job.as_mut() {
        job.apply_event(repair_job::RepairJobEvent::VerifierObserved {
            delta: repair_job::VerifierDelta::Passed,
        });
    }
    emit_repair_progress_classified_event(
        agent.session_store.session_id(),
        Some(&previous_repair_context),
        agent.repair_job.as_ref(),
        true,
    );
    agent.missing_verifier_job = None;
    *args.verifier_repair_retries = 0;
    agent.repair_job_artifact_attempts = 0;
    *args.task_contract_verifier_passed_in_loop = true;
    agent.task_contract_verifier_passed_this_actor_loop = true;
    TaskContractVerifierFlowOutcome::Done {
        final_prose: format!(
            "Completed requested repository changes and verified them with `{safe_command}`."
        ),
    }
}

fn handle_repair_job_verifier_failure(
    agent: &mut Agent,
    args: TaskContractVerifierFlowArgs<'_, '_>,
    previous_repair_context: repair_job::RepairJob,
    changed_files: &[String],
    command: String,
    output: String,
) -> TaskContractVerifierFlowOutcome {
    *args.contract_verification_retries += 1;
    let mut repair_context = verifier_repair_context_from_failure(
        &agent.work_root,
        &command,
        &output,
        changed_files,
        *args.contract_verification_retries,
        Some(&previous_repair_context),
    );
    let attempt_limit =
        task_contract_verifier_failure_attempt_limit(Some(&previous_repair_context));
    if *args.contract_verification_retries >= attempt_limit {
        agent.repair_job = Some(repair_context);
        super::verifier_orchestration::emit_safe_stop_report_for_repair_exhausted(agent);
        return TaskContractVerifierFlowOutcome::Exit {
            reason: ExitReason::RepairExhausted,
            error_text: format!(
                "verifier repair budget exhausted: {}\n{}",
                crate::session::feedback::mask_secrets(&command),
                crate::session::feedback::mask_secrets(&output)
            ),
        };
    }

    let applied_outcome_promotion = repair_job::apply_verifier_rerun_observation(
        &mut repair_context,
        Some(&previous_repair_context),
    );
    *args.contract_verifier_repair_edit_count = Some(args.repo_edit_calls_made_this_turn);
    agent.task_contract_verifier_repair_pending = true;
    if let Some(outcome) = repair_context.rerun_outcome {
        log_llm_event(
            "agent.verifier_repair.rerun_classified",
            serde_json::json!({
                "session_id": agent.session_store.session_id(),
                "outcome": outcome.as_str(),
                "previous_failure_signature": repair_context.previous_failure_signature.as_deref(),
                "current_failure_signature": repair_context.failure_signature.as_str(),
                "previous_failure_count": repair_context.previous_failure_count,
                "current_failure_count": repair_context.failure_count,
                "job_preserving": true,
            }),
        );
    }
    emit_repair_progress_classified_event(
        agent.session_store.session_id(),
        Some(&previous_repair_context),
        Some(&repair_context),
        false,
    );
    agent.repair_job = Some(repair_context);
    super::verifier_orchestration::maybe_emit_repair_exhausted_from_promotion(
        agent,
        applied_outcome_promotion,
    );
    *args.repo_change_retries = 0;
    *args.verifier_repair_retries = 0;
    agent.repair_job_artifact_attempts = 0;
    write_stdout_rendered(
        &format_iteration_status(
            args.last_iter,
            agent.config.max_iterations,
            "Verification failed",
            "Repair job observed verifier failure and updated its state.",
            agent.footer.current_cols(),
        ),
        true,
    );
    super::message_push::push_system_note(
        agent,
        task_contract_verifier_repair_note(
            &command,
            &output,
            *args.contract_verification_retries,
            attempt_limit,
            agent.repair_job.as_ref(),
        ),
    );
    TaskContractVerifierFlowOutcome::Continue
}

fn handle_repair_job_verifier_no_verifier(
    agent: &mut Agent,
    args: TaskContractVerifierFlowArgs<'_, '_>,
) -> TaskContractVerifierFlowOutcome {
    if let Some(job) = agent.repair_job.as_mut() {
        job.apply_event(repair_job::RepairJobEvent::VerifierObserved {
            delta: repair_job::VerifierDelta::VerifierUnavailable,
        });
    }
    agent.task_contract_verifier_repair_pending = true;
    *args.repo_change_retries = 0;
    *args.verifier_repair_retries = 0;
    write_stdout_rendered(
        &format_iteration_status(
            args.last_iter,
            agent.config.max_iterations,
            "Verification missing",
            "Repair job observed that no runnable verifier is available.",
            agent.footer.current_cols(),
        ),
        true,
    );
    TaskContractVerifierFlowOutcome::Continue
}

fn handle_repair_job_verifier_safe_stop(
    agent: &mut Agent,
    last_iter: usize,
    reason: SafeStopReason,
) -> TaskContractVerifierFlowOutcome {
    let outcome = super::verifier_orchestration::handle_task_contract_verifier_safe_stop(
        agent,
        last_iter,
        reason,
        "repair_job_verifier",
    );
    if let Some(job) = agent.repair_job.as_mut() {
        job.apply_event(repair_job::RepairJobEvent::VerifierObserved {
            delta: repair_job::VerifierDelta::VerifierUnavailable,
        });
    }
    outcome
}

pub(super) fn dispatch_repair_job_step(
    agent: &mut Agent,
    args: TaskContractVerifierFlowArgs<'_, '_>,
    repo_edit_calls_made_this_turn: &mut usize,
) -> TaskContractVerifierFlowOutcome {
    let Some(step) = agent
        .repair_job
        .as_mut()
        .map(|job| job.begin_next_repair_step())
    else {
        return TaskContractVerifierFlowOutcome::Continue;
    };

    match step {
        repair_job::RepairStep::RunDiagnostic => handle_repair_job_diagnostic_step(agent, args),
        repair_job::RepairStep::RunPatchProvider { target_hint } => {
            handle_repair_job_patch_provider_step(
                agent,
                args,
                repo_edit_calls_made_this_turn,
                target_hint,
            )
        }
        repair_job::RepairStep::RunVerifier => drive_repair_job_verifier(agent, args),
        repair_job::RepairStep::SafeStop { reason } => repair_job_safe_stop_outcome(agent, reason),
        repair_job::RepairStep::Done => repair_job_done_outcome(),
    }
}

fn handle_repair_job_diagnostic_step(
    agent: &mut Agent,
    args: TaskContractVerifierFlowArgs<'_, '_>,
) -> TaskContractVerifierFlowOutcome {
    write_repair_job_step_status(
        agent,
        args.last_iter,
        "Verifier diagnostic",
        "Running short-lived diagnostic LLM pass outside the main session.",
    );
    match super::verifier_orchestration::run_verifier_diagnostic_pass(agent) {
        VerifierDiagnosticPassOutcome::Accepted => {
            write_repair_job_step_status(
                agent,
                args.last_iter,
                "Verifier diagnostic",
                "Accepted validated diagnostic result; continuing verifier repair.",
            );
            TaskContractVerifierFlowOutcome::Continue
        }
        VerifierDiagnosticPassOutcome::RetryPending { error } => {
            write_repair_job_step_status(
                agent,
                args.last_iter,
                "Verifier diagnostic",
                &format!("Diagnostic pass failed ({error}); retrying with fallback model."),
            );
            TaskContractVerifierFlowOutcome::Continue
        }
        VerifierDiagnosticPassOutcome::Unavailable { error } => {
            super::safe_stop_emit::emit_safe_stop_report_for_repair_terminal(
                agent,
                repair_job::RepairTerminalReason::DiagnosticUnavailable,
            );
            TaskContractVerifierFlowOutcome::Exit {
                reason: ExitReason::VerifierFailed,
                error_text: format!("verifier repair diagnostic_unavailable: {error}"),
            }
        }
        VerifierDiagnosticPassOutcome::Skipped => TaskContractVerifierFlowOutcome::Exit {
            reason: ExitReason::VerifierFailed,
            error_text: repair_job_diagnostic_skipped_error(agent),
        },
    }
}

fn handle_repair_job_patch_provider_step(
    agent: &mut Agent,
    args: TaskContractVerifierFlowArgs<'_, '_>,
    repo_edit_calls_made_this_turn: &mut usize,
    target_hint: RecoveryTargetHint,
) -> TaskContractVerifierFlowOutcome {
    write_repair_job_step_status(
        agent,
        args.last_iter,
        "Verifier repair",
        "Running controller-applied repair pass for the selected target.",
    );
    // Issue #978 (parent #974, Issue D): deterministic EvidenceFailed operator
    // for missing serde-family Cargo dependencies runs before the LLM repair
    // pass. Keyed on the failure diagnostic (not `target_hint`) so it stays
    // correct even when target selection misroutes; edits `Cargo.toml`.
    if let super::cargo_dependency_repair::CargoDependencyRepairOutcome::Applied { relative_path } =
        super::cargo_dependency_repair::try_apply_cargo_dependency_repair(agent)
    {
        *repo_edit_calls_made_this_turn = repo_edit_calls_made_this_turn.saturating_add(1);
        *args.repo_change_retries = 0;
        *args.verifier_repair_retries = 0;
        write_repair_job_step_status(
            agent,
            args.last_iter,
            "Verifier repair",
            &format!(
                "Added missing Cargo dependencies to {relative_path} deterministically; verifier will rerun."
            ),
        );
        return TaskContractVerifierFlowOutcome::Continue;
    }
    // Issue #991 (parent #988, Issue C): deterministic Rust binding-mismatch
    // operators (lib name / CARGO_BIN_EXE) run before the LLM repair pass. Keyed
    // on the failure diagnostic + workspace inspection (not `target_hint`).
    // Recording the recovery strategy surfaces the operator in the eval/report
    // deterministic-operator hit rate.
    if let super::rust_binding_repair::RustBindingRepairOutcome::Applied {
        operator,
        relative_path,
    } = super::rust_binding_repair::try_apply_rust_binding_repair(agent)
    {
        agent
            .controller_policy_ledger
            .record(ControllerRecoveryStrategy::DeterministicBindingRepair);
        *repo_edit_calls_made_this_turn = repo_edit_calls_made_this_turn.saturating_add(1);
        *args.repo_change_retries = 0;
        *args.verifier_repair_retries = 0;
        write_repair_job_step_status(
            agent,
            args.last_iter,
            "Verifier repair",
            &format!(
                "Applied deterministic binding repair ({operator}) to {relative_path}; verifier will rerun."
            ),
        );
        return TaskContractVerifierFlowOutcome::Continue;
    }
    if let super::mechanical_compile_repair::MechanicalCompileRepairOutcome::Applied {
        relative_path,
    } =
        super::mechanical_compile_repair::try_apply_mechanical_compile_repair(agent, &target_hint)
    {
        *repo_edit_calls_made_this_turn = repo_edit_calls_made_this_turn.saturating_add(1);
        *args.repo_change_retries = 0;
        *args.verifier_repair_retries = 0;
        write_repair_job_step_status(
            agent,
            args.last_iter,
            "Verifier repair",
            &format!(
                "Applied deterministic compile repair to {relative_path}; verifier will rerun."
            ),
        );
        return TaskContractVerifierFlowOutcome::Continue;
    }
    match super::verifier_orchestration::run_verifier_repair_pass_and_apply(agent, &target_hint) {
        VerifierRepairPassOutcome::Applied { relative_path } => {
            *repo_edit_calls_made_this_turn = repo_edit_calls_made_this_turn.saturating_add(1);
            *args.repo_change_retries = 0;
            *args.verifier_repair_retries = 0;
            write_repair_job_step_status(
                agent,
                args.last_iter,
                "Verifier repair",
                &format!("Applied controller repair edit to {relative_path}; verifier will rerun."),
            );
            TaskContractVerifierFlowOutcome::Continue
        }
        VerifierRepairPassOutcome::Invalid {
            error,
            repair_attempt_outcome,
        } => {
            super::verifier_orchestration::record_controller_verifier_repair_invalid(
                agent,
                &error,
                repair_attempt_outcome,
            );
            dispatch_after_repair_patch_rejection(agent, args.last_iter, error, Some(target_hint))
        }
        VerifierRepairPassOutcome::Unavailable { relative_path } => {
            super::verifier_orchestration::record_controller_verifier_repair_invalid(
                agent,
                &format!(
                    "verifier repair unavailable: no safe cheap check available for {relative_path}"
                ),
                None,
            );
            write_repair_job_step_status(
                agent,
                args.last_iter,
                "Verifier repair",
                &format!(
                    "No safe cheap check available for {relative_path}; continuing through repair job state."
                ),
            );
            TaskContractVerifierFlowOutcome::Continue
        }
        VerifierRepairPassOutcome::Skipped => TaskContractVerifierFlowOutcome::Exit {
            reason: ExitReason::VerifierFailed,
            error_text: "verifier repair patch provider skipped after committed dispatch"
                .to_string(),
        },
    }
}

fn write_repair_job_step_status(agent: &Agent, last_iter: usize, title: &str, message: &str) {
    write_stdout_rendered(
        &format_iteration_status(
            last_iter,
            agent.config.max_iterations,
            title,
            message,
            agent.footer.current_cols(),
        ),
        true,
    );
}

fn repair_job_diagnostic_skipped_error(agent: &mut Agent) -> String {
    super::safe_stop_emit::emit_safe_stop_report_for_repair_terminal(
        agent,
        repair_job::RepairTerminalReason::DiagnosticUnavailable,
    );
    "verifier repair diagnostic skipped after committed dispatch".to_string()
}

fn repair_job_safe_stop_outcome(
    agent: &mut Agent,
    reason: repair_job::RepairTerminalReason,
) -> TaskContractVerifierFlowOutcome {
    super::safe_stop_emit::emit_safe_stop_report_for_repair_terminal(agent, reason);
    TaskContractVerifierFlowOutcome::Exit {
        reason: repair_terminal_exit_reason(reason),
        error_text: format!("verifier repair safe stop: {}", reason.as_str()),
    }
}

fn drive_repair_job_verifier(
    agent: &mut Agent,
    args: TaskContractVerifierFlowArgs<'_, '_>,
) -> TaskContractVerifierFlowOutcome {
    let Some(previous_repair_context) = agent.repair_job.clone() else {
        return TaskContractVerifierFlowOutcome::Continue;
    };
    let current_verif = verify_repo_progress(args.before_snapshot, &agent.work_root);
    let changed_files = changed_files_for_verifier(args.accumulated, &current_verif);
    write_stdout_rendered(
        &format_iteration_status(
            args.last_iter,
            agent.config.max_iterations,
            "Task contract",
            "Running verifier for repair job result.",
            agent.footer.current_cols(),
        ),
        true,
    );

    match super::verifier_orchestration::run_task_contract_verifier_once(agent, &changed_files) {
        TaskContractVerifierOutcome::Passed { command } => {
            handle_repair_job_verifier_pass(agent, args, previous_repair_context, command)
        }
        TaskContractVerifierOutcome::Failed { command, output } => {
            handle_repair_job_verifier_failure(
                agent,
                args,
                previous_repair_context,
                &changed_files,
                command,
                output,
            )
        }
        TaskContractVerifierOutcome::NoVerifier => {
            handle_repair_job_verifier_no_verifier(agent, args)
        }
        TaskContractVerifierOutcome::Disabled => TaskContractVerifierFlowOutcome::Exit {
            reason: ExitReason::MissingVerification,
            error_text: "task contract requires verification, but ANVIL_NO_AUTO_TEST is set"
                .to_string(),
        },
        TaskContractVerifierOutcome::TransportError { error } => {
            TaskContractVerifierFlowOutcome::Exit {
                reason: ExitReason::TransportError,
                error_text: error,
            }
        }
        TaskContractVerifierOutcome::SafeStop { reason } => {
            handle_repair_job_verifier_safe_stop(agent, args.last_iter, reason)
        }
    }
}

pub(super) fn dispatch_missing_verifier_job_step(
    agent: &mut Agent,
    args: TaskContractVerifierFlowArgs<'_, '_>,
    next_action: repair_job::VerifierBootstrapNextAction,
) -> Option<TaskContractVerifierFlowOutcome> {
    match next_action {
        repair_job::VerifierBootstrapNextAction::RequestSetupEdit => None,
        repair_job::VerifierBootstrapNextAction::RerunVerifier => Some(
            super::verifier_orchestration::drive_task_contract_verifier(agent, args),
        ),
        repair_job::VerifierBootstrapNextAction::SafeStop { reason } => {
            super::safe_stop_emit::emit_safe_stop_report_for_verifier_missing(agent);
            Some(TaskContractVerifierFlowOutcome::Exit {
                reason: ExitReason::MissingVerification,
                error_text: reason.to_string(),
            })
        }
    }
}

fn dispatch_after_repair_patch_rejection(
    agent: &mut Agent,
    last_iter: usize,
    error: String,
    attempted_target_hint: Option<RecoveryTargetHint>,
) -> TaskContractVerifierFlowOutcome {
    let Some(action) = agent.repair_job.as_ref().map(|job| job.next_action()) else {
        return TaskContractVerifierFlowOutcome::Exit {
            reason: ExitReason::VerifierFailed,
            error_text: error,
        };
    };
    match action {
        repair_job::RepairNextAction::SafeStop { reason } => {
            super::safe_stop_emit::emit_safe_stop_report_for_repair_terminal(agent, reason);
            TaskContractVerifierFlowOutcome::Exit {
                reason: repair_terminal_exit_reason(reason),
                error_text: format!(
                    "verifier repair safe stop after rejected patch: {error}. next_action: {}",
                    repair_rejection_next_action(&error)
                ),
            }
        }
        repair_job::RepairNextAction::RequestDiagnostic | repair_job::RepairNextAction::Replan => {
            write_stdout_rendered(
                &format_iteration_status(
                    last_iter,
                    agent.config.max_iterations,
                    "Verifier repair",
                    "Rejected invalid controller repair proposal; repair job will re-run diagnostics.",
                    agent.footer.current_cols(),
                ),
                true,
            );
            TaskContractVerifierFlowOutcome::Continue
        }
        repair_job::RepairNextAction::RequestPatch { target_hint } => {
            let target_changed = attempted_target_hint
                .as_ref()
                .map(|attempted| attempted.path != target_hint.path)
                .unwrap_or(false);
            let note = if target_changed {
                "Rejected invalid controller repair proposal; repair job selected another target."
            } else {
                "Rejected invalid controller repair proposal; repair job will retry within its budget."
            };
            write_stdout_rendered(
                &format_iteration_status(
                    last_iter,
                    agent.config.max_iterations,
                    "Verifier repair",
                    note,
                    agent.footer.current_cols(),
                ),
                true,
            );
            TaskContractVerifierFlowOutcome::Continue
        }
        repair_job::RepairNextAction::RerunVerifier => TaskContractVerifierFlowOutcome::Continue,
        repair_job::RepairNextAction::VerifiedDone => TaskContractVerifierFlowOutcome::Done {
            final_prose:
                "Completed requested repository changes and verified them with the required verifier."
                    .to_string(),
        },
    }
}

pub(super) fn repair_rejection_next_action(error: &str) -> &'static str {
    let normalized = error.to_ascii_lowercase();
    if normalized.contains("role_mismatch") {
        return "typed correction target and patch role disagreed; retry the active correction kind's admitted role/path or fall back to a fresh diagnostic before editing another role";
    }
    if normalized.contains("ambiguous")
        || normalized.contains("authority")
        || normalized.contains("expectation")
    {
        return "clarify the expected behavior or provide authoritative examples before retrying";
    }
    if normalized.contains("malformed") {
        return "retry with a narrower task or simpler verifier output so the patch proposal can be structured safely";
    }
    if normalized.contains("duplicate") || normalized.contains("noop") {
        return "inspect the verifier failure and retry with a different repair target";
    }
    "inspect the verifier diagnostics and retry with a narrower repair target"
}
