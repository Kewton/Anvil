//! Artifact-recovery target installer helpers extracted from `turn.rs`
//! (parent #680).
//!
//! Hosts the projection-write half of the artifact-recovery target
//! lifecycle (the projection-read + clear half lives in
//! `artifact_recovery_flow`). Three production entry points + two
//! private helpers:
//!
//! - `set_artifact_recovery_target_for_decision` — decision → hint →
//!   atomic-install via `set_artifact_recovery_target_from_hint`.
//! - `set_artifact_recovery_target_for_action` — action → hint →
//!   atomic-install.
//! - `set_artifact_recovery_target_from_hint` — the SSOT atomic
//!   installer. Orders: align → install/refresh job → commit
//!   projection or atomic-clear on validation failure.
//! - `synthesized_missing_implementation_target_path` (private) —
//!   per-role synthesised target derivation.
//! - `align_recovery_target_hint_to_request` (private) — Test-role
//!   path alignment to the requested stack family.
//!
//! Originally `impl Agent` methods; converted to free functions taking
//! `&mut Agent` / `&Agent`, matching `actor_loop_flow` / `reply_retry`
//! / earlier vertical-slice patterns. `pub(super)` limited / no facade
//! re-export (DR3-001).

use super::Agent;
use super::task_contract::{
    ArtifactRecoveryAction, ArtifactRole, CompletionDecision, RecoveryTarget, RecoveryTargetHint,
};
use super::verifier_orchestration::{
    JobInstallOutcome, synthesized_missing_test_target_path_for_request,
    test_target_path_compatible_with_request,
};
use crate::logging::log_llm_event;

pub(super) fn set_artifact_recovery_target_for_decision(
    agent: &mut Agent,
    decision: &CompletionDecision,
    attempt: usize,
) -> Option<RecoveryTargetHint> {
    let hint = super::task_contract_recovery::task_contract_recovery_target(agent, decision)?;
    set_artifact_recovery_target_from_hint(agent, hint, attempt)
}

pub(super) fn set_artifact_recovery_target_for_action(
    agent: &mut Agent,
    action: &ArtifactRecoveryAction,
    attempt: usize,
) -> Option<RecoveryTargetHint> {
    let hint = match action {
        ArtifactRecoveryAction::Continue { target_hint, .. }
        | ArtifactRecoveryAction::RepairArtifact { target_hint } => target_hint.clone()?,
        _ => return None,
    };
    set_artifact_recovery_target_from_hint(agent, hint, attempt)
}

fn align_recovery_target_hint_to_request(
    agent: &Agent,
    mut hint: RecoveryTargetHint,
) -> RecoveryTargetHint {
    if hint.role != ArtifactRole::Test {
        return hint;
    }
    let Some(request) = super::workspace_access::active_request_text(agent) else {
        return hint;
    };
    let Some((target_path, stack_label)) =
        synthesized_missing_test_target_path_for_request(&request)
    else {
        return hint;
    };
    if hint.path == target_path || test_target_path_compatible_with_request(&hint.path, &request) {
        return hint;
    }
    hint.path = target_path.to_string();
    hint.reason =
        format!("synthesized test artifact aligned with requested {stack_label} project family");
    hint
}

pub(super) fn set_artifact_recovery_target_from_hint(
    agent: &mut Agent,
    hint: RecoveryTargetHint,
    attempt: usize,
) -> Option<RecoveryTargetHint> {
    let hint = align_recovery_target_hint_to_request(agent, hint);
    // Issue #652 PR-001: the `ArtifactCompletionJob` is the SSOT for
    // target + role-specific retry budget. We must NOT update
    // `current_artifact_recovery_target` before the job has been
    // validated and installed — otherwise a validation failure would
    // leave the projection set with no job attached, and
    // `EffectiveToolPolicy::artifact_directed` would grant write
    // access for a target with no role-specific budget. Ordering:
    //   1. attempt to install / refresh the job for the hint
    //   2. on success → commit the projection (atomic SWAP from any
    //      prior state)
    //   3. on failure (only possible for Test role) → clear BOTH
    //      the projection and the job so no stale slot remains.
    let install =
        super::artifact_recovery_flow::maybe_install_artifact_completion_job_for_hint(agent, &hint);
    match install {
        JobInstallOutcome::InstalledOrSkipped => {
            let target = RecoveryTarget::from_hint(hint.clone(), attempt);
            let changed = agent.current_artifact_recovery_target.as_ref() != Some(&target);
            if changed {
                log_llm_event(
                    "agent.artifact_recovery_target.selected",
                    serde_json::json!({
                        "session_id": agent.session_store.session_id(),
                        "turn_index": agent.current_turn_index,
                        "role": target.role.label(),
                        "path": target.path,
                        "reason": target.reason,
                        "attempt": target.attempt,
                    }),
                );
            }
            agent.current_artifact_recovery_target = Some(target);
            Some(hint)
        }
        JobInstallOutcome::ValidationFailed => {
            // PR-001 atomic clear: the new Test hint failed
            // `ArtifactCompletionJob::new` validation. Drop the prior
            // projection too — otherwise the artifact-directed
            // policy would keep granting write permission for a
            // target with no attached role-specific budget.
            if agent.current_artifact_recovery_target.take().is_some() {
                log_llm_event(
                    "agent.artifact_recovery_target.cleared",
                    serde_json::json!({
                        "session_id": agent.session_store.session_id(),
                        "turn_index": agent.current_turn_index,
                        "reason": "artifact_completion_job_validation_failed",
                    }),
                );
            }
            None
        }
    }
}
