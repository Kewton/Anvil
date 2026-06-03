//! Per-Agent misc lifecycle helpers extracted from `turn.rs` (parent
//! #680).
//!
//! Hosts four small Agent-level helpers that live at distinct points
//! in the actor-loop lifecycle:
//!
//! - `refresh_artifact_completion_satisfied` — Issue #663 (AD2 / AD9
//!   / DR1-001) SSOT chokepoint for the ledger-driven Satisfied
//!   transition. Status guard lives inside
//!   `ArtifactCompletionJob::record_satisfied_from_ledger`; the
//!   helper computes the projection once and delegates.
//! - `tool_policy_violation_exit_reason` — static mapping from
//!   `RecoveryOwner` to `ExitReason`.
//! - `record_missing_verifier_setup_failure` — records an invalid
//!   `MissingVerifierJob` setup attempt; emits a
//!   `verifier_missing` SafeStop report when exhausted; otherwise
//!   prints an iteration status + pushes the
//!   `task_contract_no_verifier_note` system note.
//! - `current_assistant_model` — picks the assistant model name from
//!   the active mode + plan-model override.
//!
//! Originally `impl Agent` methods; converted to free functions taking
//! `&mut Agent` / `&Agent`, matching the `actor_loop_flow` /
//! `reply_retry` / earlier vertical-slice precedent. `pub(super)`
//! limited / no facade re-export (DR3-001).

use super::Agent;
use super::active_job_arbiter::RecoveryOwner;
use super::actor_loop_flow::format_iteration_status;
use super::plan_mode_helpers::assistant_model_for_mode;
use super::summary::ExitReason;
use super::turn_helpers::write_stdout_rendered;
use super::verifier_orchestration::task_contract_no_verifier_note;

pub(super) fn refresh_artifact_completion_satisfied(agent: &mut Agent) {
    if agent.artifact_completion_job.is_none() {
        return;
    }
    // Issue #917: read the per-turn classification authority instead of
    // recomputing. `None` (no current-turn request) keeps the legacy early
    // return. `&Rc<TaskContract>` deref-coerces to `&TaskContract`.
    let task_contract = match super::task_classification::task_contract_authority(agent) {
        Some(contract) => contract,
        None => return,
    };
    let projection = agent
        .artifact_ledger
        .required_artifacts_completed_projection(&task_contract);
    if let Some(job) = agent.artifact_completion_job.as_mut() {
        job.record_satisfied_from_ledger(&projection);
    }
}

pub(super) fn tool_policy_violation_exit_reason(recovery_owner: RecoveryOwner) -> ExitReason {
    if recovery_owner == RecoveryOwner::MissingVerifierJob {
        ExitReason::MissingVerification
    } else if recovery_owner.is_verifier_owned() {
        ExitReason::VerifierFailed
    } else {
        ExitReason::ToolCallFormatError
    }
}

pub(super) fn record_missing_verifier_setup_failure(
    agent: &mut Agent,
    last_iter: usize,
    reason: &str,
) -> bool {
    let Some(job) = agent.missing_verifier_job.as_mut() else {
        return false;
    };
    let exhausted = job.record_invalid_setup_attempt();
    let attempt = job.setup_attempts_used as usize;
    let attempt_limit = job.retry_budget as usize;
    if exhausted {
        super::safe_stop_emit::emit_safe_stop_report_for_verifier_missing(agent);
        return true;
    }
    write_stdout_rendered(
        &format_iteration_status(
            last_iter,
            agent.config.max_iterations,
            "Verification missing",
            &format!("Verifier setup still needs an in-scope repository edit ({reason})."),
            agent.footer.current_cols(),
        ),
        true,
    );
    super::message_push::push_system_note(
        agent,
        task_contract_no_verifier_note(
            attempt,
            attempt_limit,
            super::workspace_access::active_request_text(agent)
                .unwrap_or_default()
                .as_str(),
        ),
    );
    false
}

pub(super) fn current_assistant_model(agent: &Agent) -> String {
    assistant_model_for_mode(
        agent.session.mode_state.mode,
        &agent.models.main,
        agent.plan_model_override.as_deref(),
    )
}
