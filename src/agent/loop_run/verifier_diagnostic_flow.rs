//! Verifier-diagnostic pass flow extracted from `turn.rs` (parent
//! #680).
//!
//! Hosts the Issue #637 / #638 / #654 verifier-diagnostic pass
//! lifecycle for `repair_job.assessment`:
//!
//! - `prepare_verifier_diagnostic_pass` — clears stale assessment state,
//!   reserves an attempt slot, derives `PreparedVerifierDiagnosticPass`,
//!   and emits the behavior-contract projection event.
//! - `request_verifier_diagnostic_reply` — invokes the verifier
//!   diagnostic chat with overridden timeout / max_predict; surfaces
//!   client / chat / tool-call errors via the failure handler.
//! - `handle_verifier_diagnostic_failure` — records the failure +
//!   decides between `RetryPending` and terminal `Unavailable`.
//! - `record_verifier_diagnostic_failure` (private) — store-boundary
//!   SSOT sanitize + repair-job mutate + bounded snapshot refresh + emit
//!   `agent.verifier_diagnostic.failed`.
//! - `record_verifier_diagnostic_unavailable` (private) — same
//!   sanitize / snapshot / emit, then forwards to
//!   `safe_stop_emit::emit_safe_stop_report_for_diagnostic_target_missing`.
//! - `reset_verifier_diagnostic_state_if_needed` (private) — clears
//!   stale assessment after cluster advance or target exhaustion.
//!
//! Originally `impl Agent` methods; converted to free functions taking
//! `&mut Agent` / `&Agent`, matching the `actor_loop_flow` /
//! `reply_retry` / earlier vertical-slice precedent. `pub(super)`
//! limited / no facade re-export (DR3-001).

use super::Agent;
use super::verifier_diagnostic_attempt::{
    VERIFIER_DIAGNOSTIC_ATTEMPT_LIMIT, verifier_diagnostic_attempt_spec,
};
use super::verifier_orchestration::{
    PreparedVerifierDiagnosticPass, VerifierDiagnosticPassOutcome, verifier_diagnostic_messages,
};
use crate::logging::log_llm_event;

const VERIFIER_DIAGNOSTIC_MAX_PREDICT: usize = 2_048;

pub(super) fn prepare_verifier_diagnostic_pass(
    agent: &mut Agent,
) -> Result<PreparedVerifierDiagnosticPass, VerifierDiagnosticPassOutcome> {
    reset_verifier_diagnostic_state_if_needed(agent);
    let Some(context) = agent.repair_job.clone() else {
        return Err(VerifierDiagnosticPassOutcome::Skipped);
    };
    if context.diagnostic_unavailable || context.assessment.is_some() {
        return Err(VerifierDiagnosticPassOutcome::Skipped);
    }
    let Some(attempt_spec) = verifier_diagnostic_attempt_spec(
        &agent.models.main,
        agent.models.sidecar.as_deref(),
        context.assessment_attempts,
    ) else {
        let error = context
            .diagnostic_error
            .clone()
            .unwrap_or_else(|| "diagnostic attempts exhausted".to_string());
        record_verifier_diagnostic_unavailable(agent, error.clone());
        return Err(VerifierDiagnosticPassOutcome::Unavailable { error });
    };
    if let Some(current) = agent.repair_job.as_mut() {
        current.diagnostic_attempted = true;
        current.assessment_attempts = current.assessment_attempts.saturating_add(1);
    }
    let active_request = agent.active_request_text().unwrap_or_default();
    let task_contract = super::task_contract::TaskContract::from_request(&active_request);
    let behavior_projection = super::required_behavior::project_behavior_contract(&task_contract);
    super::active_job_emit::emit_behavior_contract_projected_if_changed(
        agent,
        behavior_projection.as_ref(),
        super::required_behavior::BEHAVIOR_CONTRACT_CONSUMER_VERIFIER_DIAGNOSTIC,
    );
    Ok(PreparedVerifierDiagnosticPass {
        context,
        attempt_spec,
        active_request,
        behavior_projection,
    })
}

fn reset_verifier_diagnostic_state_if_needed(agent: &mut Agent) {
    let stale_advance = agent.repair_job.as_ref().is_some_and(|job| {
        super::repair_job::has_stale_assessment_after_cluster_advance(job, &agent.work_root)
    });
    let target_exhausted = agent
        .repair_job
        .as_ref()
        .is_some_and(|job| job.needs_diagnostic_after_target_exhaustion());
    if (stale_advance || target_exhausted)
        && let Some(current) = agent.repair_job.as_mut()
    {
        current.assessment = None;
        current.diagnostic_attempted = false;
        if target_exhausted {
            current.repair_target_hint = None;
            current.assessment_bound_cluster_id = None;
        }
    }
}

pub(super) fn request_verifier_diagnostic_reply(
    agent: &mut Agent,
    prepared: &PreparedVerifierDiagnosticPass,
) -> Result<String, VerifierDiagnosticPassOutcome> {
    let messages = verifier_diagnostic_messages(
        &agent.work_root,
        &prepared.context,
        &prepared.active_request,
        prepared.behavior_projection.as_ref(),
    );
    let diagnostic_client = match agent.client.clone_with_overrides(
        prepared.attempt_spec.timeout_secs,
        VERIFIER_DIAGNOSTIC_MAX_PREDICT,
    ) {
        Ok(client) => client,
        Err(err) => {
            return Err(handle_verifier_diagnostic_failure(
                agent,
                format!("client clone failed: {err}"),
                prepared.attempt_spec.role,
            ));
        }
    };
    let reply =
        match diagnostic_client.chat_text_json_control(&prepared.attempt_spec.model, &messages) {
            Ok(reply) => reply,
            Err(err) => {
                return Err(handle_verifier_diagnostic_failure(
                    agent,
                    err,
                    prepared.attempt_spec.role,
                ));
            }
        };
    if !reply.tool_calls.is_empty() {
        return Err(handle_verifier_diagnostic_failure(
            agent,
            "diagnostic reply contained unexpected tool calls".to_string(),
            prepared.attempt_spec.role,
        ));
    }
    Ok(reply.content)
}

pub(super) fn handle_verifier_diagnostic_failure(
    agent: &mut Agent,
    error: String,
    model_role: &'static str,
) -> VerifierDiagnosticPassOutcome {
    let compact = record_verifier_diagnostic_failure(agent, error, model_role);
    let attempts_done = agent
        .repair_job
        .as_ref()
        .map(|context| context.assessment_attempts)
        .unwrap_or(VERIFIER_DIAGNOSTIC_ATTEMPT_LIMIT);
    if verifier_diagnostic_attempt_spec(
        &agent.models.main,
        agent.models.sidecar.as_deref(),
        attempts_done,
    )
    .is_some()
    {
        VerifierDiagnosticPassOutcome::RetryPending { error: compact }
    } else {
        record_verifier_diagnostic_unavailable(agent, compact.clone());
        VerifierDiagnosticPassOutcome::Unavailable { error: compact }
    }
}

fn record_verifier_diagnostic_failure(
    agent: &mut Agent,
    error: String,
    model_role: &'static str,
) -> String {
    // Issue #637 (CB-002): sanitize at the RepairJob store boundary so
    // the SSOT pipeline (mask_secrets + mask_header_family +
    // control-char neutralization) is applied before the text reaches
    // prompt payloads / log events.
    let error = super::repair_job::sanitize_repair_job_text_with_char_cap(&error, 180);
    if let Some(context) = agent.repair_job.as_mut() {
        context.repair_target_hint = None;
        context.assessment = None;
        context.diagnostic_error = Some(error.clone());
        context.apply_event(super::repair_job::RepairJobEvent::DiagnosticMalformed);
    }
    // Issue #638 (CB-001 reflected): also refresh the bounded snapshot
    // on every retry-pending diagnostic failure. Without this, attempts
    // remaining mid-loop leave `repair_failure_snapshot` stale and the
    // production wiring acceptance condition (work-plan Task 1.4) only
    // holds at terminal `record_verifier_diagnostic_unavailable`.
    agent.repair_failure_snapshot = agent.repair_job.as_ref().map(|job| job.failure_snapshot());
    log_llm_event(
        "agent.verifier_diagnostic.failed",
        serde_json::json!({
            "session_id": agent.session_store.session_id(),
            "role": model_role,
            "error": error,
        }),
    );
    error
}

fn record_verifier_diagnostic_unavailable(agent: &mut Agent, error: String) {
    // Issue #637 (CB-002): same SSOT sanitization as
    // `record_verifier_diagnostic_failure`. We deliberately call the
    // sanitizer (not the raw `compact_verifier_failure_text`) so the
    // store-boundary invariant holds for every diagnostic_error write.
    let error = super::repair_job::sanitize_repair_job_text_with_char_cap(&error, 180);
    if let Some(context) = agent.repair_job.as_mut() {
        context.diagnostic_unavailable = true;
        context.diagnostic_error = Some(error.clone());
        context.apply_event(super::repair_job::RepairJobEvent::DiagnosticUnavailable);
    }
    // Issue #638 (Task 1.4): capture a bounded snapshot when diagnostic
    // becomes unavailable. Turn-local only — not pushed to
    // session.messages (design policy §5, A-only). Re-runs SSOT
    // sanitizers as defence-in-depth.
    agent.repair_failure_snapshot = agent.repair_job.as_ref().map(|job| job.failure_snapshot());
    log_llm_event(
        "agent.verifier_diagnostic.unavailable",
        serde_json::json!({
            "session_id": agent.session_store.session_id(),
            "error": error,
        }),
    );
    // Issue #654: also emit the bounded structured safe stop report so
    // downstream consumers (`/bug-fix`, semantic repair plan) can see
    // role / expected target / actual actions / hypothesis ledger in a
    // single structured event. Per-StopReason dedup is enforced by
    // `record_safe_stop_report`.
    super::safe_stop_emit::emit_safe_stop_report_for_diagnostic_target_missing(agent);
}
