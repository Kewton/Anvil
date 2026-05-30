//! Per-Agent tool-policy decision helpers extracted from `turn.rs`
//! (parent #680).
//!
//! Hosts the per-Agent policy decisions that the tool-call pipeline,
//! the effective-tool-policy flow, and the photon feedback derivation
//! consult:
//!
//! - `answer_only_mode_active` — Issue #576 / DR3-001 SSOT: trust the
//!   second-pass-corrected `session.mode_state.work_mode` (no OR with
//!   the lexical pre-classifier).
//! - `script_execution_requested` — request explicitly requests a
//!   local script / shell execution.
//! - `answer_only_policy_error` — answer-only mode read-only gate;
//!   allows Read / Glob / Grep and (when scripted) `Bash` whose
//!   command matches the allowlist.
//! - `effective_tool_policy_error` — fallback when no explicit
//!   `EffectiveToolPolicy` is supplied: derives the policy via
//!   `effective_tool_policy_flow::effective_tool_policy` and applies
//!   it (with workspace scope when a `MissingVerifierJob` is active).
//!
//! Originally `impl Agent` methods; converted to free functions taking
//! `&Agent`, matching the `actor_loop_flow` / `reply_retry` / earlier
//! vertical-slice precedent. `pub(super)` limited / no facade
//! re-export (DR3-001).

use super::Agent;
use super::answer_only_mode::answer_only_script_command_allowed;
use super::photon_feedback_derive::request_explicitly_requests_script_execution;
use super::tool_policy::effective_tool_policy_error_for_call_with_scope;
use crate::modes::plan_act::WorkMode;

pub(super) fn answer_only_policy_error(
    agent: &Agent,
    name: &str,
    arguments: &serde_json::Value,
) -> Option<String> {
    if !answer_only_mode_active(agent) {
        return None;
    }
    if matches!(name, "Read" | "Glob" | "Grep") {
        return None;
    }
    if name == "Bash"
        && script_execution_requested(agent)
        && arguments
            .get("command")
            .and_then(serde_json::Value::as_str)
            .is_some_and(answer_only_script_command_allowed)
    {
        return None;
    }
    Some(format!(
        "Error: answer-only mode is read-only. Use Read, Glob, or Grep if inspection is needed, and only run Bash for an explicitly requested local script or read-only command. Blocked tool: {name}."
    ))
}

pub(super) fn answer_only_mode_active(agent: &Agent) -> bool {
    // Issue #576 / DR3-001: tool policy must honour the second-pass-
    // corrected `session.mode_state.work_mode` as the single source of
    // truth. The previous OR with
    // `infer_work_mode_from_text(active_request_text())` bypassed the
    // second-pass result whenever the lexical pre-classifier still
    // inferred `AnswerOnly`, defeating the whole point of this Issue.
    agent.session.mode_state.work_mode == WorkMode::AnswerOnly
}

pub(super) fn script_execution_requested(agent: &Agent) -> bool {
    super::workspace_access::active_request_text(agent)
        .as_deref()
        .is_some_and(request_explicitly_requests_script_execution)
}

pub(super) fn effective_tool_policy_error(
    agent: &Agent,
    name: &str,
    arguments: &serde_json::Value,
) -> Option<String> {
    let effective_tool_policy = super::effective_tool_policy_flow::effective_tool_policy(agent);
    let scope = if agent.missing_verifier_job.is_some() {
        Some(super::workspace_access::current_workspace_scope(agent))
    } else {
        None
    };
    effective_tool_policy_error_for_call_with_scope(
        &effective_tool_policy,
        name,
        arguments,
        &agent.work_root,
        scope.as_ref(),
    )
}
