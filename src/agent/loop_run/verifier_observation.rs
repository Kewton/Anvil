//! Task-contract verifier observation hooks extracted from `turn.rs`
//! (parent #680).
//!
//! Hosts three production entry points that record the outcome of a
//! task-contract verifier invocation onto Agent state:
//!
//! - `record_task_contract_verifier_invocation` — capture the
//!   most-recent verifier command + exit code in session state for
//!   downstream consumers (case_photon_bridge, eval_log).
//! - `observe_task_contract_verifier_exit_zero` — record a
//!   verifier_exit_zero completion-evidence entry from a non-bound run.
//! - `observe_task_contract_verifier_exit_zero_bound` — Issue #651 PR-001
//!   structured-runner variant; records a bound `bound_test_artifacts_count`
//!   so `TaskContract::evaluate_with_owned_test_artifacts` can satisfy
//!   `test_execution_required = true`.
//!
//! Originally `impl Agent` methods; converted to free functions taking
//! `&mut Agent`, matching the `actor_loop_flow` / `python_markers` /
//! `emit_verifier_events` pattern. `pub(super)` limited / no facade
//! re-export (DR3-001).

use super::Agent;
use super::small_helpers::rfc3339_now_utc;
use super::verifier_orchestration::{
    build_task_contract_verifier_exit_zero_evidence,
    build_task_contract_verifier_exit_zero_evidence_bound,
};
use crate::logging::log_completion_evidence_observed;
use crate::session::feedback::redact_verifier_command_for_storage;
use crate::session::store::VerifierInvocationRecord;

pub(super) fn record_task_contract_verifier_invocation(
    agent: &mut Agent,
    command: &str,
    exit_code: Option<i32>,
) {
    let redacted = redact_verifier_command_for_storage(command);
    if redacted.trim().is_empty() {
        return;
    }
    agent.session.last_verifier_command = Some(redacted.clone());
    agent.session.last_verifier_invocation = Some(VerifierInvocationRecord {
        command: redacted,
        exit_code: exit_code.unwrap_or(-1),
        recorded_at: rfc3339_now_utc(),
    });
}

pub(super) fn observe_task_contract_verifier_exit_zero(agent: &mut Agent, command: &str) {
    if let Some(evidence) = build_task_contract_verifier_exit_zero_evidence(command) {
        agent.evidence_set_this_turn.push(evidence.clone());
        agent.task_contract_evidence_set_this_turn.push(evidence);
        log_completion_evidence_observed(
            agent.current_turn_index,
            0,
            "verifier_exit_zero",
            serde_json::json!({
                "command_class": "build_test",
                "source": "task_contract_verifier",
            }),
        );
    }
}

/// Issue #651 PR-001: structured-runner variant of
/// `observe_task_contract_verifier_exit_zero`. Records a verifier
/// success that came through `AutoTestRunner::run_structured`, i.e.
/// the runner's argv was validated against the owned test artifact
/// list. The recorded `bound_test_artifacts_count` is the only proof
/// `TaskContract::evaluate_with_owned_test_artifacts` accepts to
/// satisfy `test_execution_required = true`.
pub(super) fn observe_task_contract_verifier_exit_zero_bound(
    agent: &mut Agent,
    command: &str,
    bound_count: usize,
) {
    if let Some(evidence) =
        build_task_contract_verifier_exit_zero_evidence_bound(command, bound_count)
    {
        agent.evidence_set_this_turn.push(evidence.clone());
        agent.task_contract_evidence_set_this_turn.push(evidence);
        log_completion_evidence_observed(
            agent.current_turn_index,
            0,
            "verifier_exit_zero",
            serde_json::json!({
                "command_class": "build_test",
                "source": "task_contract_verifier_structured",
                "bound_test_artifacts_count": bound_count,
            }),
        );
    }
}
