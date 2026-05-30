//! Artifact-recovery target + completion-job lifecycle extracted from
//! `turn.rs` (parent #680).
//!
//! Hosts four production helpers that own the
//! `ArtifactCompletionJob` SSOT and the projected
//! `current_artifact_recovery_target`:
//!
//! - `clear_artifact_recovery_target` — drop the projection + job and
//!   emit the structured `agent.artifact_recovery_target.cleared`
//!   event.
//! - `maybe_install_artifact_completion_job_for_hint` — Issue #652
//!   identity-refresh + atomic-swap installer for a fresh
//!   `RecoveryTargetHint`.
//! - `maybe_emit_artifact_completion_failed_diagnostic` — Issue #652
//!   turn-local exhaustion diagnostic emitter (system note +
//!   working-memory error + masked JSON event).
//! - `artifact_recovery_target_path` — PR-001 SSOT projection from the
//!   active job (with legacy fallback for non-Test roles).
//!
//! Originally `impl Agent` methods; converted to free functions taking
//! `&mut Agent` / `&Agent`, matching `actor_loop_flow` / `reply_retry`
//! / earlier vertical-slice patterns. `pub(super)` limited / no facade
//! re-export (DR3-001).

use std::path::PathBuf;

use super::Agent;
use super::artifact_completion_job::ArtifactCompletionJob;
use super::task_contract::RecoveryTargetHint;
use super::verifier_orchestration::JobInstallOutcome;
use crate::logging::{log_llm_event, mask_payload_inplace};
use crate::safety::path_guard::resolve_user_path;

pub(super) fn clear_artifact_recovery_target(agent: &mut Agent, reason: &'static str) {
    if let Some(target) = agent.current_artifact_recovery_target.take() {
        log_llm_event(
            "agent.artifact_recovery_target.cleared",
            serde_json::json!({
                "session_id": agent.session_store.session_id(),
                "turn_index": agent.current_turn_index,
                "role": target.role.label(),
                "path": target.path,
                "reason": reason,
            }),
        );
    }
    // Issue #652: drop the SSOT artifact-completion job along with the
    // projection so a subsequent role change cannot reuse stale budget.
    agent.artifact_completion_job = None;
}

/// Issue #652: install (or refresh) the `ArtifactCompletionJob` for a
/// fresh `RecoveryTargetHint` when the target role is `Test` (the only
/// role for which `RequiredBehaviorContract::requires_test_execution()`
/// currently fires). Refresh is identity-based on `(role, target_path)`
/// — re-pointing at the same path leaves the existing job (and its
/// retry budget) intact so wrong-target attempts already recorded keep
/// counting.
///
/// Returns:
/// - `InstalledOrSkipped` when the job was installed, the existing
///   identity-refresh was kept, or the hint role does not require a
///   job (non-Test). The caller may commit the projection.
/// - `ValidationFailed` ONLY when a Test-role hint did not pass
///   `ArtifactCompletionJob::new` validation. The caller MUST clear
///   `current_artifact_recovery_target` as well (PR-001 atomic clear)
///   so no stale projection survives.
pub(super) fn maybe_install_artifact_completion_job_for_hint(
    agent: &mut Agent,
    hint: &RecoveryTargetHint,
) -> JobInstallOutcome {
    // Issue #663 (Phase C / AD5): the legacy `hint.role != Test`
    // early-return is removed — all required roles (Implementation /
    // Test / UsageDocs / Setup) install/refresh an
    // `ArtifactCompletionJob` so the role-specific budget and
    // attempt-history apply uniformly.
    let trimmed = hint.path.trim();
    // Identity refresh: same role + same target → keep the existing
    // job (and its retry budget) intact.
    if let Some(job) = agent.artifact_completion_job.as_ref()
        && job.role() == hint.role
        && job.target_path() == trimmed
    {
        return JobInstallOutcome::InstalledOrSkipped;
    }
    // CB-005: when the new hint points at a *different* target than
    // the current job, the prior job's expected target is now stale.
    // Drop it BEFORE attempting to validate the new hint so a
    // validation failure cannot leave the agent with a stale job
    // whose budget belongs to an old `current_artifact_recovery_target`.
    // The atomic ordering is: clear → validate-and-install. If the
    // new hint validates, we install it (atomic SWAP). PR-001: if it
    // does NOT validate, the caller MUST also clear
    // `current_artifact_recovery_target` so no stale projection
    // remains (signalled by `JobInstallOutcome::ValidationFailed`).
    agent.artifact_completion_job = None;
    let scope = agent.current_workspace_scope();
    match ArtifactCompletionJob::new(
        &agent.work_root,
        &scope,
        hint.clone(),
        agent.turn_edited_relative_paths.contains(trimmed),
        false,
    ) {
        Ok(job) => {
            agent.artifact_completion_job = Some(job);
            JobInstallOutcome::InstalledOrSkipped
        }
        Err(_) => {
            // Validation failure for a Test-role hint: signal the
            // caller to drop the projection too (PR-001 SSOT).
            JobInstallOutcome::ValidationFailed
        }
    }
}

/// Issue #652: emit a turn-local `artifact_completion_failed`
/// diagnostic when the active job has exhausted its budget. Sinks are
/// limited to: system note, working-memory error, and an agent-
/// controlled failure JSON event (design judgement #7). The payload is
/// rendered from the sanitized `failure_snapshot()` (mask + cap +
/// control-char neutralize already applied) and passed through
/// `mask_payload_inplace` as the defensive final-defence line.
pub(super) fn maybe_emit_artifact_completion_failed_diagnostic(agent: &mut Agent) -> bool {
    let snapshot = match agent.artifact_completion_job.as_ref() {
        Some(job) => match job.failure_snapshot() {
            Some(s) => s,
            None => return false,
        },
        None => return false,
    };
    // CB-002 / CB2-003: once the current turn has already emitted the
    // exhaustion diagnostic, subsequent identical-kind attempts (which
    // are no-ops on `record_attempt`) must NOT re-fire the system note
    // / log / working-memory tuple. The dedup is gated on a
    // **turn-local** flag (`artifact_completion_failed_diagnostic_emitted_this_turn`),
    // reset at every `handle_user_message` head.
    if agent.artifact_completion_failed_diagnostic_emitted_this_turn {
        return false;
    }
    let role_label = snapshot.current_role.label();
    let expected_target = snapshot.expected_target.clone();
    let actions_preview = snapshot
        .actual_actions
        .iter()
        .take(3)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    // Sink 1: system note — sanitized snapshot fields only.
    super::message_push::push_system_note(
        agent,
        format!(
            "[Artifact Completion Failed] Missing role: {role_label}. Expected target: {expected_target}. Recent actions: {actions_preview}. Retry budget exhausted."
        ),
    );
    // Sink 2: working-memory error.
    agent.session.working_memory.note_error(format!(
        "artifact_completion_failed role={role_label} target={expected_target}"
    ));
    // Sink 3: agent-controlled failure result emitted as a JSON event
    // run through `mask_payload_inplace` as a defensive final pass.
    let mut payload = serde_json::json!({
        "session_id": agent.session_store.session_id(),
        "turn_index": agent.current_turn_index,
        "role": role_label,
        "expected_target": expected_target,
        "actual_actions": snapshot.actual_actions,
        "attempts": snapshot.attempts.len(),
    });
    mask_payload_inplace(&mut payload);
    log_llm_event("agent.artifact_completion_failed", payload);
    // CB2-003: flip the turn-local dedup flag AFTER the three sinks
    // have actually run, so a within-turn second call short-circuits
    // at the top guard above. Cross-turn dedup is handled by the
    // per-turn reset in `handle_user_message`, which restores this
    // flag to `false` at every fresh user turn.
    agent.artifact_completion_failed_diagnostic_emitted_this_turn = true;
    true
}

pub(super) fn artifact_recovery_target_path(agent: &Agent) -> Option<PathBuf> {
    // Issue #652 PR-001 SSOT: when an `ArtifactCompletionJob` is
    // active, read the target straight from the job — that is the
    // single source of truth for the in-flight artifact-completion
    // task this turn. `current_artifact_recovery_target` is kept in
    // sync at `set_artifact_recovery_target_from_hint` (atomic
    // install + commit), but reading the job first makes the SSOT
    // invariant explicit and means that any future drift between
    // the two surfaces still resolves to the job's authoritative
    // path. For non-Test roles (no attached job today) we still
    // fall through to the legacy projection so the existing
    // artifact-directed recovery semantics for Implementation /
    // UsageDocs / Setup roles continue to work.
    let path_str = if let Some(job) = agent.artifact_completion_job.as_ref() {
        job.target_path().to_string()
    } else {
        agent
            .current_artifact_recovery_target
            .as_ref()?
            .path
            .clone()
    };
    resolve_user_path(&agent.work_root, &path_str).ok()
}
