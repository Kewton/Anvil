//! Focused-edit / repo-change / verifier-repair recovery target +
//! note builders extracted from `turn.rs` (parent #680).
//!
//! Hosts the recovery-target-selection helpers that decide which file
//! to push the assistant back toward when its previous reply did not
//! produce useful repo state:
//!
//! - `focused_edit_recovery_target` — the focused-edit policy's
//!   target chain (forced-small-edit → post-scaffold edit recovery →
//!   post-scaffold continuation recovery).
//! - `repo_change_no_edit_recovery_target` (private) — repo-change
//!   policy's target chain (Act mode + repo-edit required + active
//!   task expects a repo change + no successful non-plan edit yet →
//!   artifact-recovery path / first-existing-impl-target /
//!   latest-turn-preferred-read-edit target / last-read-tool fallback).
//! - `push_repo_change_no_edit_recovery_note` — pushes a system note
//!   for the repo-change recovery target (missing target vs.
//!   read-then-no-edit form).
//! - `push_verifier_repair_recovery_note` — pushes a system note for
//!   the verifier-repair policy (RequestDiagnostic / RequestPatch
//!   branches). RerunVerifier / SafeStop / VerifiedDone return false.
//!
//! Originally `impl Agent` methods; converted to free functions taking
//! `&mut Agent` / `&Agent`, matching the `actor_loop_flow` /
//! `reply_retry` / earlier vertical-slice precedent. `pub(super)`
//! limited / no facade re-export (DR3-001).

use std::path::PathBuf;

use super::Agent;
use super::read_target_helpers::{last_read_tool_path, latest_turn_preferred_read_edit_target};
use super::tool_display::progress_path_display;
use super::tool_history::{focused_edit_target_already_read, has_successful_non_plan_repo_edit};
use super::turn_constants::TASK_CONTRACT_VERIFIER_ATTEMPT_LIMIT;
use super::verifier_orchestration::{
    task_contract_verifier_targeted_edit_required_note, verifier_repair_diagnostic_pending_note,
    verifier_repair_target_display,
};
use crate::agent::recovery;
use crate::modes::plan_act::ExecutionMode;
use crate::safety::path_guard::resolve_user_path;

pub(super) fn focused_edit_recovery_target(agent: &Agent) -> Option<PathBuf> {
    super::forced_small_edit::forced_small_edit_recovery_target(agent)
        .or_else(|| super::scaffold_pipeline::post_scaffold_edit_recovery_target(agent))
        .or_else(|| super::scaffold_pipeline::post_scaffold_continuation_recovery_target(agent))
}

fn repo_change_no_edit_recovery_target(agent: &Agent) -> Option<PathBuf> {
    if agent.session.mode_state.mode != ExecutionMode::Act
        || !agent.session.mode_state.policy().repo_edit_required
        || !super::workspace_access::active_task_expects_repo_change(agent)
        || has_successful_non_plan_repo_edit(
            &agent.session.messages,
            &agent.work_root,
            agent.session.mode_state.active_plan_path.as_deref(),
        )
    {
        return None;
    }
    if let Some(candidate) = super::artifact_recovery_flow::artifact_recovery_target_path(agent) {
        return Some(candidate);
    }
    if let Some(candidate) = super::quality::first_existing_impl_target(&agent.work_root)
        && focused_edit_target_already_read(&agent.session.messages, &candidate, &agent.work_root)
    {
        return Some(candidate);
    }
    latest_turn_preferred_read_edit_target(&agent.session.messages, &agent.work_root).or_else(
        || {
            let path = last_read_tool_path(&agent.session.messages)?;
            let candidate = resolve_user_path(&agent.work_root, &path).ok()?;
            candidate.is_file().then_some(candidate)
        },
    )
}

pub(super) fn push_repo_change_no_edit_recovery_note(agent: &mut Agent, attempt: usize) -> bool {
    let Some(target) = repo_change_no_edit_recovery_target(agent) else {
        return false;
    };
    let target_display = progress_path_display(
        &target.display().to_string(),
        &agent.work_root,
        agent.session.mode_state.active_plan_path.as_deref(),
        120,
    );
    let note = if !target.is_file() {
        recovery::focused_edit_missing_target_recovery_note(&target_display, attempt)
    } else {
        recovery::repo_change_after_read_no_edit_note(&target_display, attempt)
    };
    super::message_push::push_system_note(agent, note);
    true
}

pub(super) fn push_verifier_repair_recovery_note(agent: &mut Agent, attempt: usize) -> bool {
    let Some(context) = agent.repair_job.as_ref() else {
        return false;
    };
    match context.next_action() {
        super::repair_job::RepairNextAction::RequestDiagnostic
        | super::repair_job::RepairNextAction::Replan => {
            // Issue #665 Phase 5: caller-side projection (S5-005 では
            // raw label/excerpt は system note に出さないため helper
            // 内部で metadata のみに縮退する)。
            let active_request =
                super::workspace_access::active_request_text(agent).unwrap_or_default();
            let task_contract = super::task_contract::TaskContract::from_request(&active_request);
            let behavior_projection =
                super::required_behavior::project_behavior_contract(&task_contract);
            let note =
                verifier_repair_diagnostic_pending_note(context, behavior_projection.as_ref());
            super::message_push::push_system_note(agent, note);
            true
        }
        super::repair_job::RepairNextAction::RequestPatch { target_hint } => {
            let Some(relative) = super::repair_job::safe_relative_path_string(&target_hint.path)
            else {
                return false;
            };
            let target = agent.work_root.join(relative);
            let target = std::fs::canonicalize(&target).unwrap_or(target);
            if !target.is_file() {
                let target_display = verifier_repair_target_display(&target, &agent.work_root);
                super::message_push::push_system_note(
                    agent,
                    recovery::focused_edit_missing_target_recovery_note(&target_display, attempt),
                );
                return true;
            }
            super::message_push::push_system_note(
                agent,
                task_contract_verifier_targeted_edit_required_note(
                    context,
                    &agent.work_root,
                    focused_edit_target_already_read(
                        &agent.session.messages,
                        &target,
                        &agent.work_root,
                    ),
                    attempt,
                    TASK_CONTRACT_VERIFIER_ATTEMPT_LIMIT,
                ),
            );
            true
        }
        super::repair_job::RepairNextAction::RerunVerifier
        | super::repair_job::RepairNextAction::SafeStop { .. }
        | super::repair_job::RepairNextAction::VerifiedDone => false,
    }
}
