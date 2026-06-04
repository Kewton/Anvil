//! Recovery / verifier-repair policy message builders extracted from
//! `turn.rs` (parent #680).
//!
//! Hosts the prompt-text builders that drive the focused-edit /
//! artifact-directed / verifier-repair recovery policies:
//!
//! - `focused_edit_no_tool_note_for_target` — focused-edit recovery
//!   note (target-based: missing target vs. compact-read/edit form).
//! - `focused_edit_no_tool_note_for_policy` — focused-edit policy
//!   variant that branches on `EffectiveToolPolicyReason::VerifierRepair`
//!   for the Read-only / Edit-only / Write-only allowlists.
//! - `artifact_directed_recovery_message` — artifact-directed recovery
//!   policy note (role + allowed tools + read-state guidance).
//! - `verifier_repair_policy_message` — verifier-repair policy
//!   dispatcher: routes to diagnostic / request-patch / safe-stop /
//!   transition / setup notes per `RepairNextAction`.
//! - `verifier_repair_diagnostic_policy_message` (private) — diagnostic
//!   pending note with behavior projection.
//! - `verifier_repair_request_patch_message` (private) — request-patch
//!   note with target-file existence + already-read state branches.
//! - `artifact_directed_policy_violation_message` — feedback note for
//!   a policy violation against an artifact-directed target.
//! - `push_deterministic_ui_recovery_continuation_note` — system-note
//!   push reminding the agent that a deterministic UI recovery is a
//!   recovery, not a completion.
//!
//! Originally `impl Agent` methods; converted to free functions taking
//! `&mut Agent` / `&Agent`, matching the `actor_loop_flow` /
//! `reply_retry` / earlier vertical-slice precedent. `pub(super)`
//! limited / no facade re-export (DR3-001).

use std::path::Path;

use super::Agent;
use super::tool_display::progress_path_display;
use super::tool_history::focused_edit_target_already_read;
use super::tool_policy::{
    EffectiveToolPolicy, EffectiveToolPolicyReason, FocusedEditPolicy,
    focused_edit_policy_violation_feedback_note,
};
use super::verifier_orchestration::{
    verifier_repair_context_diagnostics, verifier_repair_diagnostic_pending_note,
    verifier_repair_safe_stop_message, verifier_repair_target_display,
    verifier_repair_transition_message, verifier_repair_unsafe_target_message,
    verifier_setup_policy_message,
};
use crate::agent::recovery;

pub(super) fn focused_edit_no_tool_note_for_target(
    agent: &Agent,
    target: &Path,
    target_already_read: bool,
    attempt: usize,
) -> String {
    let target_display = progress_path_display(
        &target.display().to_string(),
        &agent.work_root,
        agent.session.mode_state.active_plan_path.as_deref(),
        120,
    );
    focused_edit_no_tool_note_for_target_body(
        &target_display,
        target.is_file(),
        target_already_read,
        attempt,
    )
}

/// Issue #931 (Choke B): pure render-point helper for the focused-edit
/// no-tool target note. Masks `path` INTERNALLY via the recovery-field SSOT
/// (the path is then re-masked inside the `recovery.rs` builder; `mask_secrets`
/// is idempotent and a no-op on ordinary paths, so output is byte-identical).
pub(super) fn focused_edit_no_tool_note_for_target_body(
    path: &str,
    target_is_file: bool,
    target_already_read: bool,
    attempt: usize,
) -> String {
    let target_display = super::task_contract::mask_and_cap_recovery_field(path);
    if !target_is_file {
        recovery::focused_edit_missing_target_recovery_note(&target_display, attempt)
    } else {
        recovery::focused_edit_no_tool_recovery_note(&target_display, target_already_read, attempt)
    }
}

pub(super) fn focused_edit_no_tool_note_for_policy(
    agent: &Agent,
    policy: &FocusedEditPolicy,
    effective_tool_policy: &EffectiveToolPolicy,
    attempt: usize,
) -> String {
    let target_display = progress_path_display(
        &policy.target.display().to_string(),
        &agent.work_root,
        agent.session.mode_state.active_plan_path.as_deref(),
        120,
    );
    let verifier_repair_allowed = (effective_tool_policy.reason()
        == EffectiveToolPolicyReason::VerifierRepair)
        .then(|| effective_tool_policy.allowed_tool_names_for_prompt())
        .flatten();
    if let Some(note) =
        focused_edit_no_tool_note_for_policy_arm(&target_display, verifier_repair_allowed, attempt)
    {
        return note;
    }
    focused_edit_no_tool_note_for_target_body(
        &target_display,
        policy.target.is_file(),
        policy.target_already_read,
        attempt,
    )
}

/// Issue #931 (Choke B): pure render-point helper for the verifier-repair
/// allowlist arms of the focused-edit no-tool policy note. Masks `path`
/// INTERNALLY. Returns `None` when no verifier-repair arm applies (the caller
/// then falls through to the target-based note).
pub(super) fn focused_edit_no_tool_note_for_policy_arm(
    path: &str,
    verifier_repair_allowed: Option<&[&str]>,
    attempt: usize,
) -> Option<String> {
    let target_display = super::task_contract::mask_and_cap_recovery_field(path);
    match verifier_repair_allowed {
        Some(["Read"]) => Some(format!(
            "Verifier repair is waiting for a fresh read of {target_display}. The previous response was not executed. Emit exactly one Read tool call on that file now. Do not call Edit, Bash, Glob, Grep, or answer in prose. verifier_repair_read_attempt={attempt}"
        )),
        Some(["Edit"]) => Some(format!(
            "Verifier repair is waiting for a compact edit of {target_display}. The previous response was not executed. Emit exactly one Edit tool call on that file now. Do not call Read, Bash, Glob, Grep, or answer in prose. verifier_repair_edit_attempt={attempt}"
        )),
        Some(["Write"]) => Some(format!(
            "Verifier repair is waiting for the missing target {target_display}. The previous response was not executed. Emit exactly one Write tool call on that exact path now. Do not call Read, Bash, Glob, Grep, or answer in prose. verifier_repair_write_attempt={attempt}"
        )),
        _ => None,
    }
}

pub(super) fn artifact_directed_recovery_message(
    agent: &Agent,
    effective_tool_policy: &EffectiveToolPolicy,
) -> Option<String> {
    let policy = effective_tool_policy.artifact_directed_policy()?;
    let target = agent.current_artifact_recovery_target.as_ref()?;
    let target_display = progress_path_display(
        &policy.target.display().to_string(),
        &agent.work_root,
        agent.session.mode_state.active_plan_path.as_deref(),
        120,
    );
    let allowed = effective_tool_policy
        .allowed_tool_names_for_prompt()
        .map(|tools| tools.join(", "))
        .unwrap_or_else(|| "Read, Write, Edit".to_string());
    Some(artifact_directed_recovery_message_body(
        &target_display,
        target.role.label(),
        &allowed,
        policy.target_already_read,
    ))
}

/// Issue #931 (Choke B): pure render-point helper for the artifact-directed
/// recovery message. Masks `path` INTERNALLY via the recovery-field SSOT.
pub(super) fn artifact_directed_recovery_message_body(
    path: &str,
    role_label: &str,
    allowed: &str,
    target_already_read: bool,
) -> String {
    let target_display = super::task_contract::mask_and_cap_recovery_field(path);
    let read_guidance = if target_already_read {
        " The target has already been read in this session, so do not call Read again."
    } else {
        ""
    };
    format!(
        "[Artifact Directed Recovery] Missing role: {role_label}. Target file: {target_display}. Allowed tools for this turn are {allowed} on that exact target path only.{read_guidance} Do not call Bash, Glob, Grep, or switch files. Use Write if a small scaffold file should be replaced; otherwise use a compact Edit."
    )
}

pub(super) fn verifier_repair_policy_message(
    agent: &Agent,
    effective_tool_policy: &EffectiveToolPolicy,
) -> Option<String> {
    (effective_tool_policy.reason() == EffectiveToolPolicyReason::VerifierRepair).then(|| {
        let context = agent.repair_job.as_ref();
        let diagnostics = verifier_repair_context_diagnostics(context);
        match context.map(|context| context.next_action()) {
            Some(
                super::repair_job::RepairNextAction::RequestDiagnostic
                | super::repair_job::RepairNextAction::Replan,
            ) => verifier_repair_diagnostic_policy_message(agent),
            Some(super::repair_job::RepairNextAction::RequestPatch { target_hint }) => {
                verifier_repair_request_patch_message(agent, &diagnostics, &target_hint)
            }
            Some(super::repair_job::RepairNextAction::SafeStop { .. }) => {
                verifier_repair_safe_stop_message()
            }
            Some(
                super::repair_job::RepairNextAction::RerunVerifier
                | super::repair_job::RepairNextAction::VerifiedDone,
            ) => verifier_repair_transition_message(),
            None if agent.missing_verifier_job.is_some() => verifier_setup_policy_message(
                &super::workspace_access::active_request_text(agent).unwrap_or_default(),
            ),
            None => verifier_repair_transition_message(),
        }
    })
}

fn verifier_repair_diagnostic_policy_message(agent: &Agent) -> String {
    // Issue #917: per-turn classification authority. Preserve the legacy
    // `unwrap_or_default()` semantics: a missing request maps to the empty-input
    // contract (`from_request("")`).
    let task_contract = super::task_classification::task_contract_authority(agent)
        .unwrap_or_else(|| std::rc::Rc::new(super::task_contract::TaskContract::from_request("")));
    let behavior_projection = super::required_behavior::project_behavior_contract(&task_contract);
    agent
        .repair_job
        .as_ref()
        .map(|context| verifier_repair_diagnostic_pending_note(context, behavior_projection.as_ref()))
        .unwrap_or_else(|| {
            "[Verifier Repair Policy] A verifier failure is pending. Output a compact diagnosis JSON object only; do not call tools.".to_string()
        })
}

fn verifier_repair_request_patch_message(
    agent: &Agent,
    diagnostics: &str,
    target_hint: &super::task_contract::RecoveryTargetHint,
) -> String {
    let Some(relative) = super::repair_job::safe_relative_path_string(&target_hint.path) else {
        return verifier_repair_unsafe_target_message();
    };
    let target = agent.work_root.join(relative);
    let target = std::fs::canonicalize(&target).unwrap_or(target);
    let target_display = verifier_repair_target_display(&target, &agent.work_root);
    let already_read =
        focused_edit_target_already_read(&agent.session.messages, &target, &agent.work_root);
    verifier_repair_request_patch_message_body(
        &target_display,
        diagnostics,
        target.is_file(),
        already_read,
    )
}

/// Issue #931 (Choke B): pure render-point helper for the verifier-repair
/// request-patch note (missing / already-read / fresh-read arms). Masks `path`
/// INTERNALLY via the recovery-field SSOT.
pub(super) fn verifier_repair_request_patch_message_body(
    path: &str,
    diagnostics: &str,
    target_is_file: bool,
    target_already_read: bool,
) -> String {
    let target_display = super::task_contract::mask_and_cap_recovery_field(path);
    if !target_is_file {
        format!(
            "[Verifier Repair Policy] A verifier failure is pending.{diagnostics} Missing target file: {target_display}. Next required action: exactly one Write on that target. Do not use Bash, switch files, or finish with prose. Anvil will rerun the verifier after the write."
        )
    } else if target_already_read {
        format!(
            "[Verifier Repair Policy] A verifier failure is pending.{diagnostics} Target file: {target_display}. Next required action: exactly one compact Edit on that target. Do not call Read again, Bash, switch files, or finish with prose. Anvil will rerun the verifier after the edit."
        )
    } else {
        format!(
            "[Verifier Repair Policy] A verifier failure is pending.{diagnostics} Target file: {target_display}. Next required action: exactly one Read on that target. Do not use Edit, Bash, switch files, or finish with prose."
        )
    }
}

pub(super) fn artifact_directed_policy_violation_message(
    agent: &Agent,
    effective_tool_policy: &EffectiveToolPolicy,
) -> Option<String> {
    let policy = effective_tool_policy.artifact_directed_policy()?;
    // PR #930 review (High-2): mask + cap the displayed target path (prompt path).
    let target_display = super::task_contract::mask_and_cap_recovery_field(
        &policy
            .target
            .strip_prefix(&agent.work_root)
            .unwrap_or(&policy.target)
            .to_string_lossy()
            .replace('\\', "/"),
    );
    focused_edit_policy_violation_feedback_note(
        &agent.session.working_memory.unresolved_errors,
        effective_tool_policy.allowed_tool_names_for_prompt(),
        Some(&target_display),
    )
}

pub(super) fn push_deterministic_ui_recovery_continuation_note(
    agent: &mut Agent,
    target_path: &str,
    attempt: usize,
) {
    super::message_push::push_system_note(
        agent,
        deterministic_ui_recovery_continuation_note_body(target_path, attempt),
    );
}

/// Issue #931 (Choke B): pure render-point helper for the deterministic-UI
/// recovery continuation note. Masks `path` INTERNALLY via the recovery-field
/// SSOT.
pub(super) fn deterministic_ui_recovery_continuation_note_body(
    target_path: &str,
    attempt: usize,
) -> String {
    let target_path = super::task_contract::mask_and_cap_recovery_field(target_path);
    format!(
        "Deterministic UI recovery updated {target_path}, but this is recovery context, not completion. Inspect the file if needed, then make one small model-produced Edit or run the project verifier before finalizing. deterministic_ui_recovery_attempt={attempt}"
    )
}
