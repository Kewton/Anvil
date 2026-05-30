//! Per-Agent `#[cfg(test)]` test seams extracted from `turn.rs`
//! (parent #680).
//!
//! Hosts the test-only `pub(super)` seams that the projection seams in
//! `loop_run.rs` (`effective_tool_policy_for_test`,
//! `build_arbiter_candidates_for_test`,
//! `drive_artifact_directed_policy_error_for_test`) and the in-crate
//! test mods (`turn_tests`, `progress_tests`) call to drive the
//! production wiring without widening visibility (DR3-001 / AD19 /
//! DR2-005):
//!
//! - `effective_tool_policy_pub_for_test` — Issue #664 test seam over
//!   the private `effective_tool_policy()`.
//! - `build_arbiter_candidates_pub_for_test` — Issue #664 test seam
//!   over the private `build_arbiter_candidates()`.
//! - `drive_policy_error_for_test` — Issue #664 iteration-3 (CB2-003)
//!   chokepoint exerciser that mirrors `execute_tool_call`'s
//!   rejection / artifact-completion-attempt branches without any
//!   network / cancellation / approval side effects.
//! - `artifact_directed_policy_for_test` — Issue #664 iteration-3
//!   builder for an `artifact_directed_from_job` policy from the
//!   currently-installed `ArtifactCompletionJob`.
//! - `last_attempt_bash_policy_violation_for_test` — inspects the
//!   latest attempt's `bash_policy_violation()` marker.
//! - `artifact_completion_job_attempts_len_for_test` — counts
//!   recorded attempts on the active job.
//!
//! Originally `impl Agent` methods; converted to `#[cfg(test)] pub(super)`
//! free functions taking `&mut Agent` / `&Agent`, matching the
//! `actor_loop_flow` / `reply_retry` / earlier vertical-slice
//! precedent. `pub(super)` limited / no facade re-export (DR3-001).
//! Production binary excludes this module.

#![cfg(test)]

use super::Agent;
use super::tool_history::focused_edit_target_already_read;
use super::tool_policy::{EffectiveToolPolicy, effective_tool_policy_error_for_call_with_scope};

pub(super) fn effective_tool_policy_pub_for_test(agent: &Agent) -> EffectiveToolPolicy {
    super::effective_tool_policy_flow::effective_tool_policy(agent)
}

pub(super) fn build_arbiter_candidates_pub_for_test(
    agent: &Agent,
) -> Vec<super::active_job_arbiter::JobCandidate> {
    super::effective_tool_policy_flow::build_arbiter_candidates(agent)
}

pub(super) fn drive_policy_error_for_test(
    agent: &mut Agent,
    policy: &EffectiveToolPolicy,
    name: &str,
    arguments: &serde_json::Value,
) -> (Option<String>, usize) {
    let before = agent
        .artifact_completion_job
        .as_ref()
        .map(|j| j.attempts().len())
        .unwrap_or(0);
    let scope_for_policy = if agent.missing_verifier_job.is_some() {
        Some(super::workspace_access::current_workspace_scope(agent))
    } else {
        None
    };
    let err = effective_tool_policy_error_for_call_with_scope(
        policy,
        name,
        arguments,
        &agent.work_root,
        scope_for_policy.as_ref(),
    );
    if let Some(err) = err.as_ref() {
        // Mirror the recording branch in `execute_tool_call` so the
        // test seam exercises the exact production wiring.
        if err.contains("artifact-directed recovery rejected") {
            if name == "Bash" {
                let command_arg = arguments
                    .get("command")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let _ =
                    super::artifact_completion_record::record_artifact_completion_bash_violation(
                        agent,
                        vec![command_arg],
                    );
            } else {
                let actual_path = arguments
                    .get("path")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let _ = super::artifact_completion_record::record_artifact_completion_attempt(
                    agent,
                    super::artifact_completion_job::ArtifactAttemptOutcomeKind::WrongTarget,
                    vec![format!("{name} on {actual_path}")],
                );
            }
        } else if err.starts_with("setup bootstrap")
            && name == "Bash"
            && agent.artifact_completion_job.is_some()
        {
            let command_arg = arguments
                .get("command")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .to_string();
            let _ = super::artifact_completion_record::record_artifact_completion_bash_violation(
                agent,
                vec![command_arg],
            );
        }
    }
    let after = agent
        .artifact_completion_job
        .as_ref()
        .map(|j| j.attempts().len())
        .unwrap_or(0);
    (err, after.saturating_sub(before))
}

pub(super) fn artifact_directed_policy_for_test(agent: &Agent) -> Option<EffectiveToolPolicy> {
    let job = agent.artifact_completion_job.as_ref()?;
    let target = super::artifact_recovery_flow::artifact_recovery_target_path(agent)?;
    let target_already_read =
        focused_edit_target_already_read(&agent.session.messages, &target, &agent.work_root);
    Some(EffectiveToolPolicy::artifact_directed_from_job(
        target,
        target_already_read,
        job.allowed_write_actions(),
        job.allowed_read_scope(),
    ))
}

pub(super) fn last_attempt_bash_policy_violation_for_test(agent: &Agent) -> Option<bool> {
    let job = agent.artifact_completion_job.as_ref()?;
    let attempts = job.attempts();
    attempts.last().map(|a| a.bash_policy_violation())
}

pub(super) fn artifact_completion_job_attempts_len_for_test(agent: &Agent) -> Option<usize> {
    agent
        .artifact_completion_job
        .as_ref()
        .map(|j| j.attempts().len())
}
