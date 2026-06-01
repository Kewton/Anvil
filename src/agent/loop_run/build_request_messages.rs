//! Request-message assembly extracted from `turn.rs` (parent #680).
//!
//! Hosts the per-iteration prompt builder used by the actor loop:
//!
//! - `build_request_messages` (pub(super) entry point) — builds the
//!   complete `Vec<ConversationMessage>` for the next assistant request,
//!   composing system prompt + mode policy + working memory + retrieval
//!   injections + focused-edit anchors + conversation history.
//!
//! Private helpers:
//! - `append_general_request_context_messages` — workspace scaffold +
//!   working memory + case/anti-pattern retrieval + repo context.
//! - `maybe_send_request_context_pack` — photon context-pack POST with
//!   PAM advisory bridge.
//! - `append_common_request_messages` — offline / plan alias / scaffold
//!   recovery / verifier repair / artifact-directed notes.
//! - `append_focused_edit_request_messages` — focused-edit anchor + slice
//!   notes + filtered history.
//!
//! Originally `impl Agent` methods; converted to free functions taking
//! `&mut Agent` / `&Agent`, matching `actor_loop_flow` / `reply_retry` /
//! `repair_job_dispatch` pattern. `pub(super)` limited / no facade
//! re-export (DR3-001).

use std::path::Path;

use super::Agent;
use super::feedback_builders::extract_current_request_paths;
use super::focused_edit_recovery::{
    focused_edit_compact_anchor_note, focused_edit_compact_recovery_anchor,
    focused_edit_exact_anchor_history, focused_edit_exact_recovery_anchor,
    focused_edit_first_slice_note, focused_edit_guidance_note_for_policy, focused_edit_history,
    focused_edit_second_slice_note,
};
use super::lifecycle;
use super::plan_mode_helpers::plan_file_alias;
use super::tool_history::{
    build_recent_tool_summary, recent_truncated_tool_call_attempt,
    successful_non_plan_repo_edit_count,
};
use super::tool_policy::{EffectiveToolPolicy, focused_edit_policy_violation_feedback_note};
use super::turn_helpers::RetrievalInjection;
use crate::agent::prompting;
use crate::agent::recovery;
use crate::modes::plan_act::ExecutionMode;
use crate::session::store::ConversationMessage;
use crate::system_prompt::build_system_prompt;

pub(super) fn build_request_messages(
    agent: &mut Agent,
    protocol: prompting::ToolProtocol,
    effective_tool_policy: &EffectiveToolPolicy,
) -> Vec<ConversationMessage> {
    let mut messages = Vec::new();
    let focused_edit_policy = effective_tool_policy.focused_edit_policy().cloned();
    let focused_edit_target = focused_edit_policy
        .as_ref()
        .map(|policy| policy.target.clone());
    let successful_repo_edits = successful_non_plan_repo_edit_count(
        &agent.session.messages,
        &agent.work_root,
        agent.session.mode_state.active_plan_path.as_deref(),
    );
    let focused_edit_target_already_read = focused_edit_policy
        .as_ref()
        .is_some_and(|policy| policy.target_already_read);
    let plan_contents = if agent.session.mode_state.mode == ExecutionMode::Plan {
        agent.current_plan_contents().ok().flatten()
    } else {
        None
    };
    let plan_stage = plan_contents
        .as_deref()
        .map(lifecycle::current_plan_stage)
        .or_else(|| {
            (agent.session.mode_state.mode == ExecutionMode::Plan)
                .then_some(agent.session.mode_state.plan_stage)
        });
    let next_sections = plan_contents
        .as_deref()
        .map(lifecycle::plan_next_stage_sections)
        .unwrap_or_default();

    messages.push(ConversationMessage::system(build_system_prompt(
        agent.session.mode_state.mode,
        agent.session.mode_state.active_plan_path.as_deref(),
        agent.session.mode_state.task_profile,
        protocol,
        plan_stage,
        &next_sections,
        effective_tool_policy.allowed_tool_names_for_prompt(),
    )));
    if let Some(message) = super::tool_prep::mode_policy_message(agent) {
        messages.push(message);
    }
    if focused_edit_target.is_none() {
        append_general_request_context_messages(agent, &mut messages);
    }
    append_common_request_messages(agent, &mut messages, protocol, effective_tool_policy);
    if let Some(target) = focused_edit_target {
        append_focused_edit_request_messages(
            agent,
            &mut messages,
            effective_tool_policy,
            &target,
            focused_edit_target_already_read,
            successful_repo_edits,
        );
    } else {
        messages.extend(agent.session.messages.clone());
    }
    messages
}

fn append_general_request_context_messages(
    agent: &mut Agent,
    messages: &mut Vec<ConversationMessage>,
) {
    if super::workspace_access::active_task_expects_repo_change(agent)
        && super::workspace_access::workspace_appears_empty(agent)
    {
        if let Some(framework) =
            super::scaffold_pipeline::active_task_requested_scaffold_framework(agent)
        {
            messages.push(ConversationMessage::system(
                recovery::framework_scaffold_now_note(framework.label()),
            ));
        }
        messages.push(ConversationMessage::system(
            recovery::empty_workspace_scaffold_note(),
        ));
    }
    if let Some(memory_message) = super::working_memory_messages::working_memory_message(agent) {
        messages.push(memory_message);
    }
    let case_injection = super::case_record_flow::try_inject_case_retrieval_message(agent);
    if let Some(ref inj) = case_injection {
        messages.push(inj.message.clone());
    }
    let anti_injection = super::anti_pattern_flow::try_inject_anti_pattern_message(agent);
    if let Some(ref inj) = anti_injection {
        messages.push(inj.message.clone());
    }
    maybe_send_request_context_pack(agent, &case_injection, &anti_injection);
    if let Some(repo_context_message) = super::working_memory_messages::repo_context_message(agent)
    {
        messages.push(repo_context_message);
    }
}

fn maybe_send_request_context_pack(
    agent: &mut Agent,
    case_injection: &Option<RetrievalInjection>,
    anti_injection: &Option<RetrievalInjection>,
) {
    if agent.session.context_pack_sent_this_turn {
        return;
    }
    let selected_case_ids: Vec<String> = case_injection
        .as_ref()
        .map(|inj| inj.selected_ids.clone())
        .unwrap_or_default();
    let selected_anti_ids: Vec<String> = anti_injection
        .as_ref()
        .map(|inj| inj.selected_ids.clone())
        .unwrap_or_default();
    let selected_precaution_ids: Vec<String> = agent
        .session
        .working_memory
        .active_precautions
        .iter()
        .filter(|p| p.status == crate::session::precaution::PrecautionStatus::Active)
        .map(|p| p.id.clone())
        .collect();
    let recent_tool_summary = build_recent_tool_summary(&agent.session.messages);
    let gate = crate::photon::mapper::PhotonGateInputs {
        photon_present: agent.photon.is_some(),
        shadow_mode: agent.config.photon_shadow_mode,
        canary: agent.config.photon_canary,
        session_id: agent.session_store.session_id(),
        turn_idx: agent.current_turn_index,
    };
    if !crate::photon::mapper::should_send_context_pack(&gate) {
        let reason = if agent.photon.is_none() {
            "photon_unavailable"
        } else if agent.config.photon_shadow_mode {
            "shadow_mode"
        } else {
            "canary_gate"
        };
        agent.record_pam_unused_reason(reason);
        return;
    }
    let resp_opt = match &agent.photon {
        Some(photon) => {
            let working_memory_text = agent.session.working_memory.format_for_prompt();
            let inputs = crate::photon::mapper::ContextPackInputs {
                task: agent.session.working_memory.active_task.as_deref(),
                repo_path: &agent.work_root,
                branch: None,
                commit: None,
                working_memory_text: working_memory_text.as_deref(),
                touched_files: &agent.session.working_memory.touched_files,
                recent_tool_summary: &recent_tool_summary,
                selected_case_ids: &selected_case_ids,
                selected_anti_pattern_ids: &selected_anti_ids,
                selected_precaution_ids: &selected_precaution_ids,
            };
            let req = crate::photon::mapper::build_context_pack_request(&inputs);
            let rid = req.0["request_id"].as_str().map(|s| s.to_string());
            let resp = photon.context_pack(&req);
            if agent.last_context_pack_id.is_none() {
                agent.last_context_pack_id = rid;
            }
            resp
        }
        None => None,
    };
    if resp_opt.is_none() {
        agent.record_pam_unused_reason("context_pack_failed");
    }
    if let Some(resp) = resp_opt.as_ref() {
        let blocked_ids: std::collections::HashSet<String> = if agent.config.photon_respect_warnings
        {
            let (ids, _stats) = crate::photon::prompt::extract_blocked_summary_ids(resp);
            ids
        } else {
            std::collections::HashSet::new()
        };
        let shadow_input = agent.config.photon_shadow_mode;
        let _ = agent.record_pam_advisory_decision(resp, &blocked_ids, shadow_input);
    }
    agent.session.context_pack_sent_this_turn = true;
}

fn append_common_request_messages(
    agent: &mut Agent,
    messages: &mut Vec<ConversationMessage>,
    protocol: prompting::ToolProtocol,
    effective_tool_policy: &EffectiveToolPolicy,
) {
    if let Some(ctx) = super::photon_feedback_derive::photon_context_pack_injection_message(agent) {
        messages.push(ctx);
    }
    if agent.config.offline {
        messages.push(ConversationMessage::system(
            "[Runtime Policy] Offline mode is enabled. Do not use network access, package installs, or general-purpose shell commands. If shell is necessary, keep it read-only or build-test only."
                .to_string(),
        ));
    }
    if agent.session.mode_state.mode == ExecutionMode::Plan
        && let Some(plan_path) = agent.session.mode_state.active_plan_path.as_deref()
    {
        messages.push(ConversationMessage::system(format!(
            "[Plan File Alias] The active plan file may live outside the project root, but it is still accessible. Treat these two paths as the same file: {} and {}. Do not loop on Read because of the outside-workspace path; continue updating the same active plan file.",
            plan_path.display(),
            plan_file_alias(plan_path)
        )));
    }
    if let Some(note) = super::forced_small_edit::forced_small_edit_recovery_message(agent) {
        messages.push(ConversationMessage::system(note));
    }
    if let Some(note) = super::scaffold_pipeline::post_scaffold_edit_recovery_message(agent) {
        messages.push(ConversationMessage::system(note));
    }
    if let Some(note) = super::scaffold_pipeline::post_scaffold_continuation_recovery_message(agent)
    {
        messages.push(ConversationMessage::system(note));
    }
    if let Some(note) =
        super::recovery_messages::verifier_repair_policy_message(agent, effective_tool_policy)
    {
        messages.push(ConversationMessage::system(note));
    }
    if let Some(note) = super::recovery_messages::artifact_directed_policy_violation_message(
        agent,
        effective_tool_policy,
    ) {
        messages.push(ConversationMessage::system(note));
    }
    if let Some(note) =
        super::recovery_messages::artifact_directed_recovery_message(agent, effective_tool_policy)
    {
        messages.push(ConversationMessage::system(note));
    }
    let current_request_paths = extract_current_request_paths(agent, &agent.work_root);
    let last_suspected = agent
        .session
        .last_feedback
        .as_ref()
        .map(|f| f.suspected_files.as_slice());
    messages.extend(prompting::runtime_context_messages(
        &agent.config.cwd,
        &agent.work_root,
        protocol,
        &agent.session.working_memory.touched_files,
        last_suspected,
        &current_request_paths,
    ));
}

fn append_focused_edit_request_messages(
    agent: &Agent,
    messages: &mut Vec<ConversationMessage>,
    effective_tool_policy: &EffectiveToolPolicy,
    target: &Path,
    focused_edit_target_already_read: bool,
    successful_repo_edits: usize,
) {
    let recovery_anchor = focused_edit_exact_recovery_anchor(
        &agent.session.messages,
        target,
        &agent.work_root,
        focused_edit_target_already_read,
        successful_repo_edits,
    );
    let compact_anchor = (recovery_anchor.is_none()
        && focused_edit_target_already_read
        && recent_truncated_tool_call_attempt(&agent.session.messages) > 0)
        .then(|| {
            focused_edit_compact_recovery_anchor(&agent.session.messages, target, &agent.work_root)
        })
        .flatten();
    let exact_anchor = recovery_anchor.or_else(|| compact_anchor.clone());
    let target_display = target
        .strip_prefix(&agent.work_root)
        .unwrap_or(target)
        .to_string_lossy()
        .replace('\\', "/");
    if let Some(note) = focused_edit_policy_violation_feedback_note(
        &agent.session.working_memory.unresolved_errors,
        effective_tool_policy.allowed_tool_names_for_prompt(),
        Some(&target_display),
    ) {
        messages.push(ConversationMessage::system(note));
    }
    messages.push(ConversationMessage::system(
        focused_edit_guidance_note_for_policy(
            effective_tool_policy,
            target,
            &agent.work_root,
            focused_edit_target_already_read,
        ),
    ));
    if compact_anchor.is_some() {
        messages.push(ConversationMessage::system(
            focused_edit_compact_anchor_note(target, &agent.work_root),
        ));
    }
    if successful_repo_edits == 0
        && let Some(note) = focused_edit_first_slice_note(
            &agent.session.messages,
            target,
            &agent.work_root,
            focused_edit_target_already_read,
        )
    {
        messages.push(ConversationMessage::system(note));
    }
    if successful_repo_edits == 1
        && let Some(note) = focused_edit_second_slice_note(
            &agent.session.messages,
            target,
            &agent.work_root,
            focused_edit_target_already_read,
        )
    {
        messages.push(ConversationMessage::system(note));
    }
    if let Some(anchor) = exact_anchor {
        messages.extend(focused_edit_exact_anchor_history(
            &agent.session.messages,
            target,
            &agent.work_root,
            &anchor,
        ));
    } else {
        messages.extend(focused_edit_history(
            &agent.session.messages,
            target,
            &agent.work_root,
        ));
    }
}
