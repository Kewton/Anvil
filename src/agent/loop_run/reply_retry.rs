//! Assistant-reply retry orchestration extracted from `turn.rs`
//! (parent #680).
//!
//! Hosts the retry loop and its branch-by-branch error handlers:
//!
//! - `request_assistant_reply_with_retry` (pub(super) entry point) —
//!   freezes the footer / starts the spinner / loops over
//!   `request_assistant_reply` with branch-specific recovery.
//! - `handle_assistant_reply_retry_error` — error-routing core.
//! - `maybe_disable_native_tools_after_request_error` — opportunistic
//!   protocol downgrade.
//! - `maybe_handle_assistant_reply_format_error` — tool-call format
//!   error recovery (scaffold materialization / deterministic edits /
//!   capability-gated finish / retry-with-note).
//! - `push_tool_call_format_retry_note` — focused-edit-aware note
//!   builder.
//! - `maybe_handle_assistant_reply_timeout_error` — timeout recovery
//!   (deterministic fallback / focused-edit retry).
//! - `maybe_handle_assistant_reply_transport_error` — transport-error
//!   backoff + plan-mode fallback bridge.
//! - `finish_assistant_reply_retry` — generic per-attempt backoff
//!   terminator.
//! - `request_assistant_reply` / `request_streaming_assistant_reply` —
//!   actual Ollama dispatch.
//! - `maybe_finish_after_edit_format_error` — Issue #634 generic
//!   format-error-after-successful-edit terminator.
//!
//! Originally `impl Agent` methods; converted to free functions taking
//! `&mut Agent` / `&Agent`, matching `actor_loop_flow` /
//! `anti_pattern_flow` / `case_record_flow` pattern. `current_assistant_model`
//! stays on `Agent` because it has 5+ external call sites; this module
//! reaches it via `agent.current_assistant_model()`.
//!
//! `pub(super)` limited / no facade re-export (DR3-001).

use std::io::{self, IsTerminal};
use std::thread;
use std::time::Duration;

use super::Agent;
use super::active_job_arbiter::RecoveryDispatchGate;
use super::actor_loop_flow::build_feedback_for_deterministic_content_fallback;
use super::interrupt::InterruptFlag;
use super::lifecycle;
use super::model_request::{build_assistant_request_plan, request_non_streaming_assistant_reply};
use super::spinner::{Spinner, SpinnerStopSignal};
use super::tool_display::progress_path_display;
use super::tool_history::successful_non_plan_repo_edit_count;
use super::turn::{AssistantReplyRetryDecision, AssistantReplyRetryState, USER_INTERRUPT_ERROR};
use crate::agent::prompting;
use crate::agent::recovery;
use crate::model_capabilities::model_capabilities;
use crate::modes::plan_act::ExecutionMode;
use crate::ollama::client::AssistantReply;
use crate::session::store::ConversationMessage;
use crate::tools::registry::ToolSpec;

pub(super) fn request_assistant_reply_with_retry(
    agent: &mut Agent,
    stream_output: bool,
    interrupt_flag: &InterruptFlag,
    recovery_dispatch_gate: RecoveryDispatchGate,
) -> Result<AssistantReply, String> {
    // Issue #430 Phase D: freeze the footer for the entire LLM call (the
    // thinking spinner writes to stderr, but stream chunks land on stdout
    // and would otherwise race the footer rewrite). Guard drops on
    // function exit alongside the spinner, restoring redraws.
    let _footer_freeze = agent.footer.freeze_for_inference();
    // Start spinner once at function entry; retries share the same
    // animation (no flicker between attempts). Dropped automatically on
    // function exit (Ok / Err / early-return), clearing the line.
    let sp = Spinner::start(format!("thinking... ({})", agent.current_assistant_model()));
    let mut retry_state =
        AssistantReplyRetryState::new(agent.config.chat_retries, agent.session.messages.len());
    loop {
        // Only streaming paths need first-chunk stop; oneshot blocks until
        // the whole reply is assembled so Drop is sufficient.
        let stop_signal = sp.stop_signal();
        match request_assistant_reply(agent, stream_output, stop_signal, interrupt_flag) {
            Ok(reply) => return Ok(reply),
            Err(err) => match handle_assistant_reply_retry_error(
                agent,
                err,
                recovery_dispatch_gate,
                &mut retry_state,
            )? {
                AssistantReplyRetryDecision::Retry => continue,
                AssistantReplyRetryDecision::ReturnReply(reply) => return Ok(reply),
                AssistantReplyRetryDecision::Fail(err) => return Err(err),
            },
        }
    }
}

fn handle_assistant_reply_retry_error(
    agent: &mut Agent,
    err: String,
    recovery_dispatch_gate: RecoveryDispatchGate,
    retry_state: &mut AssistantReplyRetryState,
) -> Result<AssistantReplyRetryDecision, String> {
    if err == USER_INTERRUPT_ERROR {
        return Ok(AssistantReplyRetryDecision::Fail(err));
    }
    if maybe_disable_native_tools_after_request_error(agent, &err, retry_state) {
        return Ok(AssistantReplyRetryDecision::Retry);
    }
    if let Some(decision) =
        maybe_handle_assistant_reply_format_error(agent, &err, recovery_dispatch_gate, retry_state)?
    {
        return Ok(decision);
    }
    if let Some(decision) =
        maybe_handle_assistant_reply_timeout_error(agent, &err, recovery_dispatch_gate, retry_state)
    {
        return Ok(decision);
    }
    if let Some(decision) = maybe_handle_assistant_reply_transport_error(agent, &err, retry_state)?
    {
        return Ok(decision);
    }
    Ok(finish_assistant_reply_retry(agent, err, retry_state))
}

fn maybe_disable_native_tools_after_request_error(
    agent: &mut Agent,
    err: &str,
    retry_state: &mut AssistantReplyRetryState,
) -> bool {
    if agent.native_tools_enabled
        && !retry_state.downgraded_native_tools
        && (lifecycle::is_native_tool_parser_failure(err)
            || lifecycle::is_native_tool_transport_failure(err))
    {
        retry_state.downgraded_native_tools = true;
        agent.disable_native_tools_for_session();
        return true;
    }
    false
}

fn maybe_handle_assistant_reply_format_error(
    agent: &mut Agent,
    err: &str,
    recovery_dispatch_gate: RecoveryDispatchGate,
    retry_state: &mut AssistantReplyRetryState,
) -> Result<Option<AssistantReplyRetryDecision>, String> {
    if let Some(reply) =
        super::scaffold_pipeline::maybe_materialize_plan_after_tool_call_format_error(agent, err)?
    {
        return Ok(Some(AssistantReplyRetryDecision::ReturnReply(reply)));
    }
    // Issue #634: Format-error 経路の制御フロー不変条件 (SSOT)
    //   (1) 評価順序固定: `maybe_apply_*` → `maybe_finish_*` の順で呼ぶ
    //       (順序を変えると edit-then-finish の意味が崩れる)。
    //   (2) flag off で apply は no-op (`Ok(None)`)。loop は次の
    //       handler (`maybe_finish_*`) にフォールスルー。
    //   (3) `maybe_finish_*` は capability gate (`finish_after_edit_format_error`)
    //       のみで動く汎用 path (experimental flag 非依存)。
    //       qwen3.5 ユーザーの format-error 後 finish は flag off
    //       でも維持される。
    if recovery_dispatch_gate.allows_deterministic_fallback()
        && let Some(reply) =
            super::scaffold_pipeline::maybe_apply_deterministic_edit_after_format_error(agent, err)?
    {
        return Ok(Some(AssistantReplyRetryDecision::ReturnReply(reply)));
    }
    if recovery_dispatch_gate.allows_generic_repo_change_recovery()
        && let Some(reply) = maybe_finish_after_edit_format_error(agent, err)
    {
        return Ok(Some(AssistantReplyRetryDecision::ReturnReply(reply)));
    }
    if lifecycle::is_tool_call_format_error(err)
        && retry_state.tool_call_format_retries_remaining > 0
    {
        retry_state.tool_call_format_retry_count += 1;
        retry_state.tool_call_format_retries_remaining -= 1;
        push_tool_call_format_retry_note(agent, err, retry_state.tool_call_format_retry_count);
        return Ok(Some(AssistantReplyRetryDecision::Retry));
    }
    Ok(None)
}

fn push_tool_call_format_retry_note(agent: &mut Agent, err: &str, retry_count: usize) {
    let lower_err = err.to_ascii_lowercase();
    let effective_tool_policy = agent.effective_tool_policy();
    if let Some(policy) = effective_tool_policy.focused_edit_policy() {
        let target = &policy.target;
        let target_already_read = policy.target_already_read;
        let target_display = progress_path_display(
            &target.display().to_string(),
            &agent.work_root,
            agent.session.mode_state.active_plan_path.as_deref(),
            120,
        );
        if !target.is_file() {
            agent.push_system_note(recovery::focused_edit_missing_target_recovery_note(
                &target_display,
                retry_count,
            ));
            return;
        }
        if lower_err.contains("truncated tool call") {
            agent.push_system_note(recovery::focused_edit_truncated_tool_call_note(
                &target_display,
                target_already_read,
                retry_count,
            ));
            return;
        }
        if lower_err.contains("unterminated <anvil_tool_call> block") {
            agent.push_system_note(recovery::focused_edit_unterminated_tool_call_note(
                &target_display,
                target_already_read,
                retry_count,
            ));
            return;
        }
    }
    agent.push_system_note(recovery::tool_call_format_recovery_note(err, retry_count));
}

fn maybe_handle_assistant_reply_timeout_error(
    agent: &mut Agent,
    err: &str,
    recovery_dispatch_gate: RecoveryDispatchGate,
    retry_state: &mut AssistantReplyRetryState,
) -> Option<AssistantReplyRetryDecision> {
    if !err.to_ascii_lowercase().contains("timed out") {
        return None;
    }
    if recovery_dispatch_gate.allows_deterministic_fallback()
        && let Some(reply) =
            super::scaffold_pipeline::maybe_apply_deterministic_polish_fallback_after_timeout(
                agent, err,
            )
    {
        agent
            .session
            .record_feedback_if_unset(build_feedback_for_deterministic_content_fallback(
                &agent.work_root,
            ));
        return Some(AssistantReplyRetryDecision::ReturnReply(reply));
    }
    let timeout_focused_policy = agent.effective_tool_policy().focused_edit_policy().cloned();
    if let Some(policy) = timeout_focused_policy {
        if recovery_dispatch_gate.allows_deterministic_fallback()
            && let Some(reply) =
                super::scaffold_pipeline::maybe_apply_deterministic_quality_fallback_after_timeout(
                    agent, err,
                )
        {
            agent.session.record_feedback_if_unset(
                build_feedback_for_deterministic_content_fallback(&agent.work_root),
            );
            return Some(AssistantReplyRetryDecision::ReturnReply(reply));
        }
        retry_state.focused_edit_timeout_retry_count += 1;
        if retry_state.focused_edit_timeout_retry_count >= 2 {
            return Some(AssistantReplyRetryDecision::Fail(err.to_string()));
        }
        agent.push_system_note(recovery::focused_edit_timeout_recovery_note(
            &progress_path_display(
                &policy.target.display().to_string(),
                &agent.work_root,
                agent.session.mode_state.active_plan_path.as_deref(),
                120,
            ),
            policy.target_already_read,
            retry_state.focused_edit_timeout_retry_count,
        ));
        return Some(AssistantReplyRetryDecision::Retry);
    }
    None
}

fn maybe_handle_assistant_reply_transport_error(
    agent: &mut Agent,
    err: &str,
    retry_state: &mut AssistantReplyRetryState,
) -> Result<Option<AssistantReplyRetryDecision>, String> {
    if !lifecycle::is_transport_error(err) || retry_state.extra_transport_retries == 0 {
        return Ok(None);
    }
    if let Some(reply) = super::scaffold_pipeline::maybe_materialize_plan_after_timeout(agent, err)?
    {
        return Ok(Some(AssistantReplyRetryDecision::ReturnReply(reply)));
    }
    if super::scaffold_pipeline::maybe_fallback_plan_model_after_timeout(agent, err) {
        return Ok(Some(AssistantReplyRetryDecision::Retry));
    }
    retry_state.transport_retry_count += 1;
    retry_state.extra_transport_retries -= 1;
    thread::sleep(Duration::from_secs(
        (retry_state.transport_retry_count as u64) * 4,
    ));
    Ok(Some(AssistantReplyRetryDecision::Retry))
}

fn finish_assistant_reply_retry(
    agent: &Agent,
    err: String,
    retry_state: &mut AssistantReplyRetryState,
) -> AssistantReplyRetryDecision {
    if retry_state.retries_remaining == 0 {
        return AssistantReplyRetryDecision::Fail(err);
    }
    let sleep_secs = (agent.config.chat_retries - retry_state.retries_remaining + 1) as u64 * 2;
    retry_state.retries_remaining -= 1;
    thread::sleep(Duration::from_secs(sleep_secs));
    AssistantReplyRetryDecision::Retry
}

fn request_assistant_reply(
    agent: &mut Agent,
    stream_output: bool,
    stop_signal: Option<SpinnerStopSignal>,
    interrupt_flag: &InterruptFlag,
) -> Result<AssistantReply, String> {
    let protocol = prompting::ToolProtocol::from_native_tools_enabled(agent.native_tools_enabled);
    let native_tools_enabled = protocol.native_tools_enabled();
    let effective_tool_policy = agent.effective_tool_policy();
    let focused_edit_target = effective_tool_policy
        .focused_edit_policy()
        .map(|policy| policy.target.as_path());
    let messages = agent.build_request_messages(protocol, &effective_tool_policy);
    let assistant_model = agent.current_assistant_model();
    let request_plan = build_assistant_request_plan(
        assistant_model.as_str(),
        native_tools_enabled,
        stream_output,
        io::stdin().is_terminal(),
        &agent.session.messages,
        focused_edit_target,
        &agent.work_root,
    );
    let tool_specs = agent.tool_specs_for_policy(&effective_tool_policy);

    if request_plan.use_streaming_transport {
        request_streaming_assistant_reply(
            agent,
            &messages,
            &tool_specs,
            native_tools_enabled,
            stream_output,
            stop_signal,
            interrupt_flag,
        )
    } else {
        request_non_streaming_assistant_reply(
            &agent.client,
            assistant_model.as_str(),
            &messages,
            &tool_specs,
            native_tools_enabled,
            request_plan.focused_edit_timeout_override,
            request_plan.focused_edit_max_predict_override,
        )
    }
}

fn request_streaming_assistant_reply(
    agent: &Agent,
    messages: &[ConversationMessage],
    tool_specs: &[ToolSpec],
    native_tools_enabled: bool,
    stream_output: bool,
    stop_signal: Option<SpinnerStopSignal>,
    interrupt_flag: &InterruptFlag,
) -> Result<AssistantReply, String> {
    let assistant_model = agent.current_assistant_model();
    let mut render_state = super::streaming_reply::StreamingReplyRenderState::new();
    let reply = agent.client.chat_streaming_with_mode(
        assistant_model.as_str(),
        messages,
        tool_specs,
        native_tools_enabled,
        |chunk| {
            super::streaming_reply::handle_streaming_assistant_chunk(
                &mut render_state,
                chunk,
                stream_output,
                stop_signal.as_ref(),
                interrupt_flag,
            )
        },
    )?;
    super::streaming_reply::finish_streaming_assistant_reply(&mut render_state, stream_output);
    Ok(reply)
}

/// Issue #634: 旧名 `maybe_finish_after_qwen35_edit_format_error`。
/// 「format error でも edit success なら finish」というモデル非依存の汎用
/// (experimental flag 非依存)。
fn maybe_finish_after_edit_format_error(agent: &Agent, err: &str) -> Option<AssistantReply> {
    if !lifecycle::is_tool_call_format_error(err)
        || !model_capabilities(&agent.current_assistant_model()).finish_after_edit_format_error
        || agent.session.mode_state.mode != ExecutionMode::Act
    {
        return None;
    }
    if agent.active_python_request_requires_tests() && !agent.python_test_artifact_exists() {
        return None;
    }
    let edits = successful_non_plan_repo_edit_count(
        &agent.session.messages,
        &agent.work_root,
        agent.session.mode_state.active_plan_path.as_deref(),
    );
    (edits > 0).then(|| AssistantReply {
        content: "Applied the focused edit; stopping after a malformed follow-up tool call."
            .to_string(),
        tool_calls: Vec::new(),
        prompt_tokens: None,
        completion_tokens: None,
    })
}
