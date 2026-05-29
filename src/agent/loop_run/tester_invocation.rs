//! Tester invocation flow extracted from `turn.rs` (parent #680).
//!
//! Hosts the smoke-test invocation entry point plus its private helpers
//! (gate check / candidate detection / runs-root materialization /
//! outcome handling / telemetry). Originally `impl Agent` methods;
//! converted to free functions taking `&mut Agent` / `&Agent`, matching
//! the `actor_loop_flow` / `anti_pattern_flow` / `case_record_flow`
//! pattern.
//!
//! Entry point:
//! - `try_invoke_tester(&mut Agent, &[String]) -> bool` — called by
//!   `success.rs` for the post-loop tester smoke-run hook.
//!
//! `pub(super)` limited / no facade re-export (DR3-001).

use std::io::{self, IsTerminal};
use std::path::Path;

use super::Agent;
use super::tester;
use crate::logging::log_llm_event;
use crate::modes::plan_act::ExecutionMode;

pub(super) fn try_invoke_tester(agent: &mut Agent, changed_files: &[String]) -> bool {
    if let Some(reason) = tester::check_invocation_gate(
        agent.tester_called_this_turn,
        agent.session.mode_state.mode == ExecutionMode::Plan,
        tester::tester_disabled(|key| std::env::var(key).ok()),
    ) {
        log_tester_skip(agent, reason.as_str());
        return false;
    }
    let candidate = match detect_tester_candidate(agent, changed_files) {
        Some(candidate) => candidate,
        None => return false,
    };
    let session_dir = agent
        .session_store
        .state_root()
        .join("sessions")
        .join(agent.session_store.session_id());
    let tmp_tests_root = session_dir.join("tmp-tests");
    let tester_runs_root = session_dir.join("tester-runs");
    if !ensure_tester_runs_root(agent, &tester_runs_root) {
        return false;
    }
    let approval_mode =
        tester::tester_approval_mode(agent.config.yes_mode, io::stdin().is_terminal());
    agent.tester_called_this_turn = true;

    let work_root = agent.work_root.clone();
    let session_id = agent.session_store.session_id().to_string();
    let run = tester::TesterRun {
        work_root: &work_root,
        tmp_tests_root: &tmp_tests_root,
        tester_runs_root: &tester_runs_root,
        approval_mode,
        plan_mode: false,
        no_tester_env: false,
        session_id: std::borrow::Cow::Owned(session_id.clone()),
    };

    let session_id_for_log = session_id.clone();
    let tester_client = agent.client.clone();
    let tester_main_model = agent.models.main.clone();
    let llm_call = move |prompt: &tester::TesterPrompt| -> Result<String, tester::TesterLlmError> {
        tester::run_tester_llm_call(
            &tester_client,
            &tester_main_model,
            &session_id_for_log,
            prompt,
        )
    };

    let offline = agent.config.offline;
    let run_bash = move |cmd: &str,
                         cwd: &Path,
                         timeout: Option<std::time::Duration>|
          -> Result<crate::tools::bash::BashExecutionOutcome, String> {
        // No cancel_flag propagation: Tester's smoke run sits past the
        // main interrupt monitor scope (post-loop hook). The 30s
        // explicit_timeout still caps wall time.
        //
        // CB-003 (Issue #459): pass `BashEnvPolicy::TesterSanitized` so
        // LLM-generated smoke code cannot read parent-process secrets
        // (`OPENAI_API_KEY`, `GITHUB_TOKEN`, `AWS_*`, anything `*_TOKEN`/
        // `*_SECRET`/`*_PASSWORD`). Only the explicit allowlist in
        // `bash::TESTER_ENV_ALLOWLIST_EXACT` is forwarded.
        crate::tools::bash::run_with_outcome(
            cmd,
            cwd,
            None,
            offline,
            timeout,
            Some(crate::tools::bash::BashEnvPolicy::TesterSanitized),
        )
        .map(|(_, outcome)| outcome)
    };

    let approver =
        move |mode: tester::ApprovalMode, command: &[String]| -> Result<(), tester::AbortReason> {
            // Honour the same write/run/promote 3-gate symmetry: Auto bypass /
            // Forbidden deny / Interactive y/N. Production prompt goes through
            // `prompt_for_approval(stdout, stdin)` so both ends are real TTY
            // streams; CI takes the Forbidden branch above.
            let mut stdout = std::io::stdout().lock();
            let stdin_handle = std::io::stdin();
            let mut stdin = stdin_handle.lock();
            tester::prompt_for_approval(mode, command, &mut stdout, &mut stdin)
        };

    let outcome = tester::run_tester_with_strategy(run, candidate, llm_call, run_bash, approver);
    handle_tester_outcome(agent, outcome, &session_id)
}

fn log_tester_event(_agent: &Agent, event: &'static str, payload: serde_json::Value) {
    log_llm_event(event, payload);
}

fn log_tester_skip(agent: &Agent, reason: &str) {
    log_tester_event(
        agent,
        "agent.tester.skipped",
        serde_json::json!({
            "session_id": agent.session_store.session_id(),
            "skip_reason": reason,
        }),
    );
}

fn detect_tester_candidate(
    agent: &Agent,
    changed_files: &[String],
) -> Option<tester::TesterCandidate> {
    let candidate = tester::TesterCandidate::detect(&agent.work_root, changed_files);
    if candidate.is_none() {
        log_tester_skip(agent, tester::NotInvokedReason::NoCandidate.as_str());
    }
    candidate
}

fn ensure_tester_runs_root(agent: &Agent, tester_runs_root: &Path) -> bool {
    if let Err(err) = std::fs::create_dir_all(tester_runs_root) {
        log_tester_event(
            agent,
            "agent.tester.failed",
            serde_json::json!({
                "session_id": agent.session_store.session_id(),
                "failure_reason": format!("mkdir tester-runs: {err}"),
            }),
        );
        return false;
    }
    true
}

fn handle_tester_outcome(
    agent: &mut Agent,
    outcome: tester::TesterOutcome,
    session_id: &str,
) -> bool {
    match outcome {
        tester::TesterOutcome::Recorded(frame) => {
            let kind_value = serde_json::to_value(&frame.kind).unwrap_or(serde_json::Value::Null);
            agent.session.record_feedback_if_unset(frame);
            log_tester_event(
                agent,
                "agent.tester.completed",
                serde_json::json!({
                    "session_id": session_id,
                    "feedback_kind": kind_value,
                }),
            );
            true
        }
        tester::TesterOutcome::NotInvoked(reason) => {
            log_tester_skip(agent, reason.as_str());
            false
        }
        tester::TesterOutcome::Aborted(reason) => {
            log_tester_event(
                agent,
                "agent.tester.failed",
                serde_json::json!({
                    "session_id": session_id,
                    "failure_reason": reason.as_str(),
                    "detail": reason.detail().map(|d| tester::sanitize_tester_log(d, tester::TESTER_LOG_CAP)),
                }),
            );
            false
        }
    }
}
