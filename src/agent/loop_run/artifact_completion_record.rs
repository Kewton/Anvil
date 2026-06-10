//! Artifact-completion attempt-recording cluster extracted from
//! `turn.rs` (parent #680).
//!
//! Hosts the Issue #652 / #664 lifecycle for pushing the
//! `artifact_directed_recovery` system note + appending attempt
//! outcomes to the active `ArtifactCompletionJob`:
//!
//! - `push_artifact_directed_recovery_note` — pushes the recovery
//!   system note unless a focused-edit target is already selected.
//! - `record_artifact_completion_attempt` — appends a regular outcome.
//! - `record_artifact_completion_bash_violation` (private) — appends a
//!   bash-policy-violation outcome; raw command bytes are sanitised at
//!   `ArtifactAttemptOutcome::new` (`mask_secrets` + length cap +
//!   control-char neutralize) and hashed at projection time (AD5 /
//!   CB-004).
//! - `record_artifact_completion_evidence_failure` — appends a generic
//!   `EvidenceFailed` outcome when an owned target artifact fails a contract
//!   obligation diagnostic.
//! - `record_artifact_completion_outcome` (private) — shared core that
//!   appends the outcome + triggers the turn-local
//!   `artifact_completion_failed` diagnostic on Exhausted transition.
//!
//! Originally `impl Agent` methods; converted to free functions taking
//! `&mut Agent`, matching the `actor_loop_flow` / `reply_retry` /
//! earlier vertical-slice precedent. `pub(super)` limited / no facade
//! re-export (DR3-001).

use super::Agent;
use crate::agent::recovery;

pub(super) fn push_artifact_directed_recovery_note(agent: &mut Agent, attempt: usize) -> bool {
    super::set_artifact_recovery_target::realign_current_artifact_recovery_target_to_contract(
        agent,
    );
    if super::recovery_targets::focused_edit_recovery_target(agent).is_some() {
        return false;
    }
    let Some(target) = agent.current_artifact_recovery_target.as_ref() else {
        return false;
    };
    let mut note =
        recovery::artifact_directed_recovery_note(target.role.label(), &target.path, attempt);
    if let Some(contract) = super::task_classification::task_contract_authority(agent)
        && let Some(api_context) = artifact_directed_api_contract_context(contract.as_ref())
    {
        note.push(' ');
        note.push_str(&api_context);
    }
    super::message_push::push_system_note(agent, note);
    true
}

fn artifact_directed_api_contract_context(
    contract: &super::task_contract::TaskContract,
) -> Option<String> {
    super::api_contract_expectation::api_contract_artifact_directed_context(
        &contract.api_contract_expectations,
    )
}

/// Issue #652: record an attempt against the active
/// `ArtifactCompletionJob` (no-op when no job exists). Triggers the
/// turn-local `artifact_completion_failed` diagnostic when the
/// recording causes the job to transition to `Exhausted`.
pub(super) fn record_artifact_completion_attempt(
    agent: &mut Agent,
    kind: super::artifact_completion_job::ArtifactAttemptOutcomeKind,
    actual_actions: Vec<String>,
) -> bool {
    let expected_target = match agent.artifact_completion_job.as_ref() {
        Some(job) => job.target_path().to_string(),
        None => return false,
    };
    let outcome = super::artifact_completion_job::ArtifactAttemptOutcome::new(
        kind,
        actual_actions,
        expected_target,
    );
    record_artifact_completion_outcome(agent, outcome)
}

/// Issue #664 iteration-2 (CB-003): record a Bash policy violation
/// against the active `ArtifactCompletionJob`. The outcome carries the
/// non-raw `bash_policy_violation = true` marker so
/// `attempt_outcome_to_json_value` emits
/// `category = "bash_out_of_policy"`.
///
/// Raw command bytes are NOT stored verbatim — `actual_actions` is
/// sanitized at `ArtifactAttemptOutcome::new` (`mask_secrets` + length
/// cap + control-char neutralize) and hashed via `stable_path_hash` at
/// projection time (AD5 / CB-004).
pub(super) fn record_artifact_completion_bash_violation(
    agent: &mut Agent,
    actual_actions: Vec<String>,
) -> bool {
    let expected_target = match agent.artifact_completion_job.as_ref() {
        Some(job) => job.target_path().to_string(),
        None => return false,
    };
    let outcome = super::artifact_completion_job::ArtifactAttemptOutcome::new_bash_policy_violation(
        actual_actions,
        expected_target,
    );
    record_artifact_completion_outcome(agent, outcome)
}

/// Record that the active artifact target exists but failed contract
/// obligation evidence, for example a schema mismatch or missing required
/// document section. Domain-specific diagnosis stays in the verifier /
/// obligation layer; this function only maps that diagnosis into the generic
/// artifact lifecycle budget.
pub(super) fn record_artifact_completion_evidence_failure(
    agent: &mut Agent,
    diagnostic_reason: &str,
) -> bool {
    let (expected_target, role) = match agent.artifact_completion_job.as_ref() {
        Some(job) => (job.target_path().to_string(), job.role()),
        None => return false,
    };
    let cluster = super::semantic_failure::build_failure_cluster_from_observation(
        diagnostic_reason,
        "artifact obligation evidence satisfied",
        &expected_target,
        "artifact_obligation_diagnostic",
        &[role],
        Vec::new(),
    );
    let outcome = super::artifact_completion_job::ArtifactAttemptOutcome::with_failure_cluster(
        super::artifact_completion_job::ArtifactAttemptOutcomeKind::EvidenceFailed,
        cluster.cluster_key,
        vec![format!("evidence_failed:{diagnostic_reason}")],
        expected_target,
    );
    record_artifact_completion_outcome(agent, outcome)
}

/// Shared core: append `outcome` to the active job's attempt history
/// and trigger the turn-local exhaustion diagnostic when the job
/// transitions to `Exhausted`.
fn record_artifact_completion_outcome(
    agent: &mut Agent,
    outcome: super::artifact_completion_job::ArtifactAttemptOutcome,
) -> bool {
    let status_after = match agent.artifact_completion_job.as_mut() {
        Some(job) => job.record_attempt(outcome),
        None => return false,
    };
    if matches!(
        status_after,
        super::artifact_completion_job::ArtifactCompletionStatus::Exhausted { .. }
    )
    // Issue #663 (Phase A): with 5 variants the `Exhausted` match
    // remains the only terminal-failure trigger; new states do not
    // alter the diagnostic surface.
    {
        // CB-002: tag the turn so the actor loop terminates with
        // `MissingRepoEdits` even on call paths that previously
        // dropped the return value (artifact-directed policy
        // WrongTarget). Emit the diagnostic only once per
        // exhaustion (`maybe_emit_..._diagnostic` is gated by the
        // same flag).
        let first_exhaustion = !agent.artifact_completion_exhausted_this_turn;
        agent.artifact_completion_exhausted_this_turn = true;
        if first_exhaustion {
            super::artifact_recovery_flow::maybe_emit_artifact_completion_failed_diagnostic(agent);
        }
        true
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::super::task_contract::TaskContract;
    use super::*;

    #[test]
    fn artifact_directed_api_context_projects_typed_json_body_contract() {
        let contract = TaskContract::from_request(
            "Create app.py and tests/test_app.py for an HTTP API. Implement POST /notes accepting JSON with title and body, returning the created note with id=1.",
        );
        let context = artifact_directed_api_contract_context(&contract).expect("api context");

        assert!(context.contains("api_contracts=method=POST,path=/notes"));
        assert!(context.contains("request_body=json"));
        assert!(context.contains("request_binding=json_body_object"));
        assert!(
            context.contains("JSON request-body object fields"),
            "{context}"
        );
        assert!(
            context.contains("not query or form parameters"),
            "{context}"
        );
    }

    #[test]
    fn artifact_directed_api_context_is_absent_without_http_contract() {
        let contract = TaskContract::from_request("Create README.md with Usage and Validation.");

        assert!(artifact_directed_api_contract_context(&contract).is_none());
    }
}
