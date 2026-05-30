//! Tool-call execution dispatch extracted from `turn.rs` (parent #680).
//!
//! Hosts the per-tool-call execution lifecycle:
//!
//! - `execute_tool_call` — production chokepoint. Order: answer-only
//!   policy gate → empty-workspace scaffold policy gate →
//!   effective-tool-policy gate → cancel-flag check → Bash vs.
//!   non-Bash dispatch.
//! - `effective_tool_policy_error_for_execution` (private) —
//!   per-execution policy error: routes through the explicit
//!   `EffectiveToolPolicy` (with the active workspace scope when a
//!   `MissingVerifierJob` is selected) or falls back to the per-Agent
//!   `effective_tool_policy_error`.
//! - `handle_tool_execution_rejection` (private) — records the
//!   rejection into working memory + appends an artifact-completion
//!   attempt (WrongTarget for non-Bash, Bash policy violation for Bash)
//!   when relevant, then renders the masked tool error string.
//! - `tool_context` (private) — builds `ToolContext` snapshot.
//! - `execute_bash_tool_call` (private) — Bash execution + feedback
//!   recording + bash-outcome evidence observation + DangerousBlock
//!   unsafe-feedback path.
//! - `capture_pre_tool_hash_if_needed` (private) — `Write` / `Edit`
//!   pre-tool hash capture for the per-turn baseline cache.
//! - `execute_non_bash_tool_call` (private) — non-Bash execution +
//!   working-memory touched-file + repo-edit evidence observation +
//!   Edit-failure feedback.
//! - `observe_evidence_from_bash_outcome` (private) — Issue #606 T-1.6
//!   post-hoc Bash → VerifierExitZero evidence observation +
//!   `last_verifier_invocation` recording.
//!
//! Originally `impl Agent` methods; converted to free functions taking
//! `&mut Agent` / `&Agent`, matching the `actor_loop_flow` /
//! `reply_retry` / earlier vertical-slice precedent. `pub(super)`
//! limited / no facade re-export (DR3-001).

use std::io::{self, IsTerminal};

use super::Agent;
use super::feedback_builders::{
    build_feedback_for_bash, build_feedback_for_edit_failure,
    build_feedback_for_unsafe_block_reason,
};
use super::file_excerpt::current_file_hash_for_relative_path;
use super::lifecycle;
use super::path_helpers::normalize_memory_path;
use super::small_helpers::{rfc3339_now_utc, user_interrupt_result};
use super::tool_execution::{
    failed_outcome_for_call, rejected_outcome_for_call, success_outcome_for_call,
};
use super::tool_policy::{
    EffectiveToolPolicy, effective_tool_policy_error_for_call_with_scope,
    workspace_relative_path_for_tool_arg,
};
use super::verifier_orchestration::build_verifier_exit_zero_evidence;
use crate::tools::registry::{BashErrorClass, ToolContext};

pub(super) fn execute_tool_call(
    agent: &mut Agent,
    name: &str,
    arguments: &serde_json::Value,
    effective_tool_policy: Option<&EffectiveToolPolicy>,
    cancel_flag: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
) -> String {
    if let Some(err) =
        super::tool_policy_decisions::answer_only_policy_error(agent, name, arguments)
    {
        agent.session.working_memory.note_error(err.clone());
        return lifecycle::format_tool_error(&err);
    }
    if let Some(err) =
        super::scaffold_pipeline::empty_workspace_scaffold_policy_error(agent, name, arguments)
    {
        agent.session.working_memory.note_error(err.clone());
        return lifecycle::format_tool_error(&err);
    }
    // Issue #646 (A1/A3): when a first-class MissingVerifierJob is
    // active, hand the active workspace scope to the policy gate so
    // out-of-scope `Write`/`Edit` paths are rejected even when the
    // restricted whitelist would otherwise admit them.
    if let Some(err) =
        effective_tool_policy_error_for_execution(agent, name, arguments, effective_tool_policy)
    {
        return handle_tool_execution_rejection(agent, name, arguments, &err);
    }
    if cancel_flag
        .as_ref()
        .is_some_and(|flag| flag.load(std::sync::atomic::Ordering::SeqCst))
    {
        return user_interrupt_result();
    }
    let context = tool_context(agent, cancel_flag);
    if name == "Bash" {
        return execute_bash_tool_call(agent, name, arguments, &context);
    }
    execute_non_bash_tool_call(agent, name, arguments, &context)
}

fn effective_tool_policy_error_for_execution(
    agent: &Agent,
    name: &str,
    arguments: &serde_json::Value,
    effective_tool_policy: Option<&EffectiveToolPolicy>,
) -> Option<String> {
    let scope_for_policy = agent
        .missing_verifier_job
        .as_ref()
        .map(|_| super::workspace_access::current_workspace_scope(agent));
    if let Some(policy) = effective_tool_policy {
        effective_tool_policy_error_for_call_with_scope(
            policy,
            name,
            arguments,
            &agent.work_root,
            scope_for_policy.as_ref(),
        )
    } else {
        super::tool_policy_decisions::effective_tool_policy_error(agent, name, arguments)
    }
}

fn handle_tool_execution_rejection(
    agent: &mut Agent,
    name: &str,
    arguments: &serde_json::Value,
    err: &str,
) -> String {
    let _tool_outcome = rejected_outcome_for_call(name, err);
    agent.session.working_memory.note_error(err.to_string());
    if err.contains("artifact-directed recovery rejected") {
        if name == "Bash" {
            let command_arg = arguments
                .get("command")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .to_string();
            let _ = super::artifact_completion_record::record_artifact_completion_bash_violation(
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
    lifecycle::format_tool_error(err)
}

fn tool_context(
    agent: &Agent,
    cancel_flag: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
) -> ToolContext {
    let tmp_tests_root = Some(
        agent
            .session_store
            .state_root()
            .join("sessions")
            .join(agent.session_store.session_id())
            .join("tmp-tests"),
    );
    ToolContext {
        root: agent.work_root.clone(),
        mode: agent.session.mode_state.mode,
        plan_path: agent.session.mode_state.active_plan_path.clone(),
        plan_stage: agent.session.mode_state.plan_stage,
        auto_approve: agent.config.yes_mode,
        interactive_approval: io::stdin().is_terminal(),
        offline: agent.config.offline,
        cancel_flag,
        tmp_tests_root,
        tester_active: agent.tester_called_this_turn,
    }
}

fn execute_bash_tool_call(
    agent: &mut Agent,
    name: &str,
    arguments: &serde_json::Value,
    context: &ToolContext,
) -> String {
    let (result, outcome) = agent
        .tool_registry
        .execute_bash_with_outcome(arguments, context);
    if let Some(outcome) = outcome.as_ref()
        && let Some(frame) = build_feedback_for_bash(outcome, &agent.work_root)
    {
        agent.session.record_feedback(frame);
    }
    if let Some(outcome) = outcome.as_ref() {
        observe_evidence_from_bash_outcome(agent, outcome);
    }
    match result {
        Ok(text) => {
            agent.maybe_update_work_root(name, arguments, &text);
            text
        }
        Err((err, class)) => {
            agent
                .session
                .working_memory
                .note_error(format!("{name}: {err}"));
            if class == BashErrorClass::DangerousBlock {
                let cmd = arguments
                    .get("command")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("");
                let frame = build_feedback_for_unsafe_block_reason(cmd, &err, &agent.work_root);
                agent.session.record_feedback(frame);
                agent.session.unsafe_blocks_this_turn =
                    agent.session.unsafe_blocks_this_turn.saturating_add(1);
            }
            lifecycle::format_tool_error(&err)
        }
    }
}

fn capture_pre_tool_hash_if_needed(agent: &mut Agent, arguments: &serde_json::Value, name: &str) {
    if matches!(name, "Write" | "Edit")
        && let Some(raw_path) = arguments.get("path").and_then(serde_json::Value::as_str)
        && let Some(rel) = workspace_relative_path_for_tool_arg(&agent.work_root, raw_path)
    {
        let pre_hash = current_file_hash_for_relative_path(&agent.work_root, &rel);
        agent.turn_pre_tool_file_hashes.insert(rel, pre_hash);
    }
}

fn execute_non_bash_tool_call(
    agent: &mut Agent,
    name: &str,
    arguments: &serde_json::Value,
    context: &ToolContext,
) -> String {
    capture_pre_tool_hash_if_needed(agent, arguments, name);
    match agent.tool_registry.execute(name, arguments, context) {
        Ok(result) => {
            let tool_outcome = success_outcome_for_call(name, arguments, &agent.work_root);
            if let Some(edit) = tool_outcome.repo_edit_evidence() {
                agent
                    .session
                    .working_memory
                    .note_touched_file(normalize_memory_path(edit.raw_path(), &agent.work_root));
                agent.session.repo_edit_succeeded_this_turn = true;
                super::repo_edit_observation::observe_evidence_from_repo_edit(
                    agent,
                    edit.raw_path(),
                );
            }
            agent.maybe_update_work_root(name, arguments, &result);
            result
        }
        Err(err) => {
            let _tool_outcome = failed_outcome_for_call(name, &err);
            agent
                .session
                .working_memory
                .note_error(format!("{name}: {err}"));
            if name == "Edit" {
                let path = arguments.get("path").and_then(serde_json::Value::as_str);
                let frame = build_feedback_for_edit_failure(path, &err, &agent.work_root);
                agent.session.record_feedback(frame);
            }
            lifecycle::format_tool_error(&err)
        }
    }
}

/// Issue #606 (T-1.6): post-hoc observation of a Bash invocation as
/// `VerifierExitZero` completion evidence. A signal is recorded **only**
/// when:
///
/// 1. `outcome.exit_code == Some(0)` — non-zero / timeout / interrupted
///    invocations are explicit failures, not silent passes.
/// 2. `outcome.class == BuildTest` — read-only / network / mutating
///    classes don't represent verification work even when they happen
///    to exit 0.
/// 3. `is_completion_verifier_command(&outcome.command) == true` —
///    rejects commands containing shell control operators that can
///    mask the real exit code (DR4-002, e.g. `cargo test || true`).
///
/// The evidence is consumed by `ProtocolKind::evidence_set_satisfies`
/// in `success.rs`.
fn observe_evidence_from_bash_outcome(
    agent: &mut Agent,
    outcome: &crate::tools::bash::BashExecutionOutcome,
) {
    use crate::tools::bash::BashCommandClass;
    // Issue #608 Phase α-2 (AP-09): record `last_verifier_command` /
    // `last_verifier_invocation` for any BuildTest invocation
    // (regardless of exit code) so the rerun-trigger handler can
    // surface the most recent verifier attempt — even failed ones (the
    // user often types `再実行` precisely because the last run failed).
    if matches!(outcome.class, BashCommandClass::BuildTest)
        && super::completion_evidence::is_completion_verifier_command(&outcome.command)
    {
        let redacted =
            crate::session::feedback::redact_verifier_command_for_storage(&outcome.command);
        // Drop empty redacted commands (e.g. all-control-char input).
        if !redacted.trim().is_empty() {
            agent.session.last_verifier_command = Some(redacted.clone());
            agent.session.last_verifier_invocation =
                Some(crate::session::store::VerifierInvocationRecord {
                    command: redacted,
                    exit_code: outcome.exit_code.unwrap_or(-1),
                    recorded_at: rfc3339_now_utc(),
                });
        }
    }

    // Issue #607 (β): build VerifierExitZero evidence for BuildTest |
    // EnvSetup exit-zero outcomes (per `build_verifier_exit_zero_evidence`).
    let Some(evidence) = build_verifier_exit_zero_evidence(outcome) else {
        return;
    };
    let crate::agent::loop_run::completion_evidence::CompletionEvidence::VerifierExitZero {
        class,
        ..
    } = evidence
    else {
        // `build_verifier_exit_zero_evidence` only ever constructs
        // VerifierExitZero today; the match keeps us honest if a
        // future helper returns a different variant.
        agent.evidence_set_this_turn.push(evidence.clone());
        if agent.current_artifact_recovery_target.is_none() {
            agent.task_contract_evidence_set_this_turn.push(evidence);
        }
        return;
    };
    agent.evidence_set_this_turn.push(evidence.clone());
    if agent.current_artifact_recovery_target.is_none() {
        agent
            .task_contract_evidence_set_this_turn
            .push(evidence.clone());
    }
    crate::logging::log_completion_evidence_observed(
        agent.current_turn_index,
        0, // α-1: iter_index plumbing is α-2 work; emit 0 for now.
        "verifier_exit_zero",
        serde_json::json!({
            // Issue #607 BP-07 / S3-002: snake_case label matches
            // serde rename_all so `command_class` reads `"env_setup"` /
            // `"build_test"` instead of `"EnvSetup"` / `"BuildTest"`.
            "command_class": class.as_str(),
        }),
    );
}
