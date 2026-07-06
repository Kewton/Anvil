#![recursion_limit = "256"]

pub mod bounded_process;
pub mod build_info;
pub mod cli;
pub mod config;
pub mod eval_events;
pub mod minimal_loop;
pub mod mode;
pub mod planner;
pub mod provider_call;
pub mod providers;
pub mod repl;
pub mod state;
pub mod tools;
pub mod tui;
pub mod util;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Context;
use cli::Cli;
use config::{Action, Config};
use serde_json::json;
use signal_hook::consts::SIGINT;
use signal_hook::iterator::{Handle as SignalHandle, Signals};
use tui::OutputRenderer;
use tui::TerminalUi;
use tui::markdown::{PlainRenderer, TerminalMarkdownRenderer};

pub fn run(cli: Cli) -> anyhow::Result<()> {
    let config = Config::from_cli(cli)?;
    emit_run_start(&config);
    let direct_command_guard = DirectCommandCompletionGuard::start(&config);
    let result = (|| -> anyhow::Result<()> {
        match config.action.clone() {
            Action::Repl => repl::run_repl(config.clone()),
            Action::Prompt(prompt) => {
                let mut client = providers::client_from_config(&config, false)?;
                let ui = DirectActionUi::new(&config);
                let resume = if config.fresh_session {
                    None
                } else {
                    config.resume.as_deref()
                };
                let mut session =
                    state::SessionStore::new(config.state_dir.clone()).load_or_create(resume)?;
                let reply = minimal_loop::loop_run::run_session_with_required_paths_with_ui(
                    &mut *client,
                    &mut session,
                    &prompt,
                    &[],
                    &config,
                    ui.as_interaction(),
                )?;
                state::SessionStore::new(config.state_dir.clone()).save(&session)?;
                drop(ui);
                PlainRenderer.render_assistant(&reply)?;
                Ok(())
            }
            Action::PlanSteps(goal) => {
                let mut planner = providers::client_from_config(&config, true)?;
                let ui = DirectActionUi::new(&config);
                let plan = planner::generate_step_plan_with_ui(
                    &mut *planner,
                    &goal,
                    &config,
                    ui.as_interaction(),
                )
                .context("failed to generate step plan")?;
                let path = planner::save_step_plan(&config.workspace_root, &plan)?;
                drop(ui);
                println!("{}", path.display());
                Ok(())
            }
            Action::PlanRun(goal) => {
                let mut execution = providers::client_from_config(&config, false)?;
                let mut planner_client = providers::client_from_config(&config, true)?;
                let ui = DirectActionUi::new(&config);
                let report = planner::generate_and_run_step_plan_with_ui(
                    &mut *planner_client,
                    &mut *execution,
                    &goal,
                    &config,
                    ui.as_interaction(),
                )?;
                drop(ui);
                println!("{report}");
                Ok(())
            }
            Action::RunPlan(path) => {
                let mut execution = providers::client_from_config(&config, false)?;
                let ui = DirectActionUi::new(&config);
                let report = planner::run_plan_file_with_ui(
                    &mut *execution,
                    &path,
                    &config,
                    ui.as_interaction(),
                )?;
                drop(ui);
                println!("{report}");
                Ok(())
            }
            Action::UltraPlan(goal) => {
                let mut planner_client = providers::client_from_config(&config, true)?;
                let ui = DirectActionUi::new(&config);
                let plan = planner::generate_ultra_plan_with_ui(
                    &mut *planner_client,
                    &goal,
                    &config,
                    ui.as_interaction(),
                )?;
                let path = planner::save_ultra_plan(&config.workspace_root, &plan)?;
                drop(ui);
                println!("{}", path.display());
                Ok(())
            }
            Action::UltraPlanRun(goal) => {
                let mut execution = providers::client_from_config(&config, false)?;
                let mut planner_client = providers::client_from_config(&config, true)?;
                let ui = DirectActionUi::new(&config);
                let report = planner::generate_and_run_ultra_plan_with_ui(
                    &mut *planner_client,
                    &mut *execution,
                    &goal,
                    &config,
                    ui.as_interaction(),
                )?;
                drop(ui);
                println!("{report}");
                Ok(())
            }
            Action::RunUltraPlan(path) => {
                let mut execution = providers::client_from_config(&config, false)?;
                let mut planner_client = providers::client_from_config(&config, true)?;
                let ui = DirectActionUi::new(&config);
                let report = planner::run_ultra_plan_file_with_ui(
                    &mut *planner_client,
                    &mut *execution,
                    &path,
                    &config,
                    ui.as_interaction(),
                )?;
                drop(ui);
                println!("{report}");
                Ok(())
            }
            Action::SetupInteractionProbe => {
                let report =
                    minimal_loop::interaction_probe::setup_interaction_probe_with_stdout_progress(
                    )?;
                for line in report.summary_lines() {
                    println!("{line}");
                }
                Ok(())
            }
        }
    })();
    if let Some(guard) = direct_command_guard.as_ref() {
        guard.finalize(&result);
    }
    emit_run_stop(&config, &result);
    result
}

enum DirectActionUi {
    Terminal(TerminalUi),
    Noop,
}

impl DirectActionUi {
    fn new(config: &Config) -> Self {
        if tui::footer::FooterEnv::detect(config).enabled {
            Self::Terminal(TerminalUi::new(config))
        } else {
            Self::Noop
        }
    }

    fn as_interaction(&self) -> &dyn tui::InteractionUi {
        match self {
            Self::Terminal(ui) => ui,
            Self::Noop => &tui::NOOP_UI,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DirectCommandStatus {
    Completed,
    Partial,
    Failed,
    Interrupted,
}

impl DirectCommandStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Partial => "partial",
            Self::Failed => "failed",
            Self::Interrupted => "interrupted",
        }
    }

    fn ok(self) -> bool {
        matches!(self, Self::Completed | Self::Partial)
    }

    fn failure_kind(self) -> &'static str {
        match self {
            Self::Completed | Self::Partial => "",
            Self::Failed => "direct_cli_command_failed",
            Self::Interrupted => "direct_cli_command_interrupted",
        }
    }
}

struct DirectCommandCompletionGuard {
    config: Config,
    command: String,
    finalized: Arc<AtomicBool>,
    signal_handle: Option<SignalHandle>,
    _signal_thread: Option<std::thread::JoinHandle<()>>,
}

impl DirectCommandCompletionGuard {
    fn start(config: &Config) -> Option<Self> {
        let command = direct_command_for_action(&config.action)?.to_string();
        let finalized = Arc::new(AtomicBool::new(false));
        let mut signal_handle = None;
        let mut signal_thread = None;

        match Signals::new([SIGINT]) {
            Ok(mut signals) => {
                let handle = signals.handle();
                let signal_config = config.clone();
                let signal_command = command.clone();
                let signal_finalized = finalized.clone();
                signal_thread = Some(std::thread::spawn(move || {
                    if signals.forever().next().is_some() {
                        if !signal_finalized.swap(true, Ordering::AcqRel) {
                            let result: anyhow::Result<()> =
                                Err(anyhow::anyhow!("interrupted by user"));
                            emit_direct_command_stop_with_status(
                                &signal_config,
                                &signal_command,
                                &result,
                                DirectCommandStatus::Interrupted,
                            );
                        }
                        std::process::exit(130);
                    }
                }));
                signal_handle = Some(handle);
            }
            Err(err) => {
                eprintln!("warning: failed to install direct CLI SIGINT finalizer: {err}");
            }
        }

        Some(Self {
            config: config.clone(),
            command,
            finalized,
            signal_handle,
            _signal_thread: signal_thread,
        })
    }

    fn finalize(&self, result: &anyhow::Result<()>) {
        let status = match result {
            Ok(()) => DirectCommandStatus::Completed,
            Err(err) if error_is_interrupted(err) => DirectCommandStatus::Interrupted,
            Err(_) => DirectCommandStatus::Failed,
        };
        self.finalize_with_status(result, status);
    }

    fn finalize_with_status(&self, result: &anyhow::Result<()>, status: DirectCommandStatus) {
        if self.finalized.swap(true, Ordering::AcqRel) {
            return;
        }
        emit_direct_command_stop_with_status(&self.config, &self.command, result, status);
    }
}

impl Drop for DirectCommandCompletionGuard {
    fn drop(&mut self) {
        if let Some(handle) = self.signal_handle.take() {
            handle.close();
        }
        if self.finalized.load(Ordering::Acquire) {
            return;
        }
        let result: anyhow::Result<()> = Err(anyhow::anyhow!(
            "direct CLI command exited before completion finalizer"
        ));
        self.finalize_with_status(&result, DirectCommandStatus::Failed);
    }
}

fn direct_command_for_action(action: &Action) -> Option<&'static str> {
    match action {
        Action::Repl => None,
        Action::Prompt(_) => Some("--prompt"),
        Action::PlanSteps(_) => Some("--plan-steps"),
        Action::PlanRun(_) => Some("--plan-run"),
        Action::RunPlan(_) => Some("--run-plan"),
        Action::UltraPlan(_) => Some("--ultra-plan"),
        Action::UltraPlanRun(_) => Some("--ultra-plan-run"),
        Action::RunUltraPlan(_) => Some("--run-ultra-plan"),
        Action::SetupInteractionProbe => Some("--setup-interaction-probe"),
    }
}

fn emit_direct_command_stop_with_status(
    config: &Config,
    command: &str,
    result: &anyhow::Result<()>,
    requested_status: DirectCommandStatus,
) -> eval_events::CompletionProjection {
    let requested_ok = requested_status.ok();
    let mut completion_snapshot =
        eval_events::latest_completion_snapshot(config.eval_events_path.as_deref());
    apply_config_completion_metadata(config, &mut completion_snapshot);
    let completion = eval_events::project_completion(requested_ok, &completion_snapshot);
    let terminal_status = effective_direct_status(requested_status, &completion);
    let ok = terminal_status.ok();
    let failure_kind = terminal_status.failure_kind();
    let stop_reason = direct_stop_reason_for_result(result, terminal_status);
    let event_projection = direct_event_projection_for_status(&completion, terminal_status);

    eval_events::emit(
        config.eval_events_path.as_deref(),
        json!({
            "event": "tui_command_stop",
            "lifecycle_stage": "direct_cli_command",
            "command": command,
            "ok": ok,
            "status": terminal_status.as_str(),
            "build_commit": build_info::COMMIT,
            "build_dirty": build_info::dirty(),
            "build_timestamp": build_info::TIMESTAMP,
            "failure_kind": failure_kind,
            "stop_reason": stop_reason,
            "completion_status": &completion.status,
            "task_status": &event_projection.task_status,
            "profile": &completion.profile,
            "effective_profile": &completion.effective_profile,
            "contract_origin": &completion.contract_origin,
            "assurance_level": &completion.assurance_level,
            "assurance_reason": &completion.assurance_reason,
            "profile_inferred": &completion.profile_inferred,
            "profile_inference_source": &completion.profile_inference_source,
            "session_status": "process_exited",
            "repl_status": "not_applicable",
            "command_completion_state": &event_projection.command_completion,
            "runtime_acceptance_status": &completion.runtime_acceptance,
            "final_acceptance_status": &completion.final_acceptance,
            "release_gate_status": &completion.release_gate,
            "next_action": &event_projection.next_action,
            "recovery_next_action": &event_projection.next_action,
            "recovery_prompt_path": &completion.recovery_prompt_path,
            "recovery_ultra_plan_path": &completion.recovery_ultra_plan_path,
            "suggested_recovery_command": &completion.suggested_recovery_command,
            "suggested_recovery_yaml_command": &completion.suggested_recovery_yaml_command,
        }),
    );
    eval_events::write_tui_command_completion_summary(
        config.eval_events_path.as_deref(),
        command,
        &stop_reason,
        failure_kind,
        terminal_status.as_str(),
        &completion,
    );
    render_terminal_summary_card_to_stdout(
        config.eval_events_path.as_deref(),
        &stop_reason,
        &event_projection,
    );
    event_projection
}

fn direct_stop_reason_for_result(
    result: &anyhow::Result<()>,
    terminal_status: DirectCommandStatus,
) -> String {
    match result {
        Ok(()) => "completed".to_string(),
        Err(err) => {
            let reason = eval_events::render_stop_reason_text(&err.to_string());
            if terminal_status == DirectCommandStatus::Interrupted && reason.trim().is_empty() {
                "interrupted by user".to_string()
            } else {
                reason
            }
        }
    }
}

fn effective_direct_status(
    requested: DirectCommandStatus,
    completion: &eval_events::CompletionProjection,
) -> DirectCommandStatus {
    if requested == DirectCommandStatus::Completed
        && completion.release_gate == "partial"
        && completion
            .release_gate_reasons
            .iter()
            .any(|reason| reason.contains("interaction_unverified:probe_unavailable"))
    {
        DirectCommandStatus::Partial
    } else {
        requested
    }
}

fn direct_event_projection_for_status(
    completion: &eval_events::CompletionProjection,
    terminal_status: DirectCommandStatus,
) -> eval_events::CompletionProjection {
    let mut projection = completion.clone();
    if matches!(
        terminal_status,
        DirectCommandStatus::Completed | DirectCommandStatus::Partial
    ) {
        return projection;
    }
    projection.status = terminal_status.as_str().to_string();
    projection.command_completion = terminal_status.as_str().to_string();
    projection.task_status = terminal_status.as_str().to_string();
    projection.next_action = match terminal_status {
        DirectCommandStatus::Interrupted => "resume_or_rerun_command".to_string(),
        DirectCommandStatus::Failed => "fix_command_failure".to_string(),
        DirectCommandStatus::Completed | DirectCommandStatus::Partial => projection.next_action,
    };
    projection
}

fn error_is_interrupted(err: &anyhow::Error) -> bool {
    err.to_string().contains("interrupted by user")
}

fn emit_run_start(config: &Config) {
    let host_env_contamination = minimal_loop::verifier_env::host_env_contamination();
    eval_events::emit(
        config.eval_events_path.as_deref(),
        json!({
            "event": "run_start",
            "workspace_root": eval_events::body_snippet(&config.workspace_root.display().to_string()),
            "provider": format!("{:?}", config.provider).to_ascii_lowercase(),
            "model": eval_events::body_snippet(&config.model),
            "planner_provider": format!("{:?}", config.planner_provider).to_ascii_lowercase(),
            "planner_model": eval_events::body_snippet(&config.planner_model),
            "chat_timeout_secs": config.chat_timeout_secs,
            "chat_timeout_source": config.chat_timeout_source,
            "profile": config.profile,
            "profile_inferred": config
                .profile_inference
                .map(|inference| inference.profile)
                .unwrap_or(""),
            "profile_inference_source": config
                .profile_inference
                .map(|inference| inference.source.as_str())
                .unwrap_or(""),
            "style": config.style,
            "action": format!("{:?}", config.action),
            "eval_events_override": eval_events::is_eval_events_override(),
            "build_commit": build_info::COMMIT,
            "build_dirty": build_info::dirty(),
            "build_timestamp": build_info::TIMESTAMP,
        }),
    );
    if let Some(inference) = config.profile_inference {
        eval_events::emit(
            config.eval_events_path.as_deref(),
            json!({
                "event": "profile_inferred",
                "profile": inference.profile,
                "from": inference.source.as_str(),
                "lifecycle_stage": "process",
            }),
        );
    }
    if !host_env_contamination.is_empty() {
        eval_events::emit(
            config.eval_events_path.as_deref(),
            json!({
                "event": "host_env_contamination",
                "contamination": host_env_contamination.clone(),
                "lifecycle_stage": "process",
            }),
        );
    }
    let events_path = config
        .eval_events_path
        .as_ref()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "unavailable".to_string());
    let host_env_line = if host_env_contamination.is_empty() {
        String::new()
    } else {
        format!(
            "\nHost env: {} detected (verifiers ran with a cleaned environment)",
            host_env_contamination.join(", ")
        )
    };
    eval_events::write_run_summary(
        config.eval_events_path.as_deref(),
        &format!(
            "Status: running\nAction: {:?}\nEvents: {}{}{}",
            config.action,
            events_path,
            config
                .profile_inference
                .map(|inference| format!("\n{}", inference.summary_line()))
                .unwrap_or_default(),
            host_env_line
        ),
    );
}

fn emit_run_stop(config: &Config, result: &anyhow::Result<()>) {
    let (ok, stop_reason, failure_kind) = match result {
        Ok(()) => (true, "completed".to_string(), ""),
        Err(err) => (
            false,
            eval_events::render_stop_reason_text(&err.to_string()),
            "process_failure",
        ),
    };
    let mut completion_snapshot =
        eval_events::latest_completion_snapshot(config.eval_events_path.as_deref());
    apply_config_completion_metadata(config, &mut completion_snapshot);
    let completion = eval_events::project_completion(ok, &completion_snapshot);
    eval_events::emit(
        config.eval_events_path.as_deref(),
        json!({
            "event": "run_stop",
            "ok": ok,
            "lifecycle_stage": "process",
            "action": format!("{:?}", config.action),
            "stop_reason": stop_reason,
            "failure_kind": failure_kind,
            "completion_status": &completion.status,
            "task_status": &completion.task_status,
            "profile": &completion.profile,
            "effective_profile": &completion.effective_profile,
            "contract_origin": &completion.contract_origin,
            "assurance_level": &completion.assurance_level,
            "assurance_reason": &completion.assurance_reason,
            "profile_inferred": &completion.profile_inferred,
            "profile_inference_source": &completion.profile_inference_source,
            "requested_port": &completion.requested_port,
            "session_status": "process_exited",
            "repl_status": "not_applicable",
            "process_completion_state": &completion.command_completion,
            "command_completion_state": &completion.command_completion,
            "runtime_acceptance_status": &completion.runtime_acceptance,
            "final_acceptance_status": &completion.final_acceptance,
            "release_gate_status": &completion.release_gate,
            "completion_contract_verification_enabled": completion.completion_contract_verification_enabled,
            "completion_contract_path_merge_enabled": completion.completion_contract_path_merge_enabled,
            "completion_contract_path": &completion.completion_contract_path,
            "completion_contract_generated": completion.completion_contract_generated,
            "external_contract_checked": completion.external_contract_checked,
            "external_contract_ok": completion.external_contract_ok,
            "release_gate_reasons": &completion.release_gate_reasons,
            "browser_readiness_applicable": completion.browser_readiness_applicable,
            "browser_readiness_execution_status": &completion.browser_readiness_execution_status,
            "browser_readiness_status": &completion.browser_readiness,
            "browser_readiness_evidence_path": &completion.browser_readiness_evidence_path,
            "interaction_evidence_applicable": completion.interaction_evidence_applicable,
            "interaction_evidence_execution_status": &completion.interaction_evidence_execution_status,
            "interaction_evidence_status": &completion.interaction_evidence,
            "interaction_evidence_path": &completion.interaction_evidence_path,
            "release_quality_completion": &completion.release_quality_completion,
            "next_action": &completion.next_action,
            "recovery_next_action": &completion.next_action,
            "recovery_prompt_path": &completion.recovery_prompt_path,
            "recovery_ultra_plan_path": &completion.recovery_ultra_plan_path,
            "suggested_recovery_command": &completion.suggested_recovery_command,
            "suggested_recovery_yaml_command": &completion.suggested_recovery_yaml_command,
            "planner_verify_normalization_count": completion.planner_verify_normalization_count,
            "planner_retry_count": completion.planner_retry_count,
            "planner_quality_warning_count": completion.planner_quality_warning_count,
            "planner_quality_issue_count": completion.planner_quality_issue_count,
            "planner_repaired": completion.planner_repaired,
            "planner_release_risk": completion.planner_release_risk,
        }),
    );
    eval_events::append_completion_summary(
        config.eval_events_path.as_deref(),
        "process",
        Some(&format!("{:?}", config.action)),
        None,
        &stop_reason,
        failure_kind,
        &completion,
    );
}

fn render_terminal_summary_card_to_stdout(
    path: Option<&std::path::Path>,
    stop_reason: &str,
    projection: &eval_events::CompletionProjection,
) {
    if !tui::terminal::stdout_is_tty() {
        return;
    }
    let card = eval_events::render_terminal_summary_card(path, stop_reason, projection);
    let renderer = TerminalMarkdownRenderer::for_stdout();
    let _ = renderer.render_assistant(&card);
}

fn apply_config_completion_metadata(
    config: &Config,
    snapshot: &mut eval_events::CompletionSnapshot,
) {
    if let Some(inference) = config.profile_inference {
        snapshot.profile_inferred = inference.profile.to_string();
        snapshot.profile_inference_source = inference.source.as_str().to_string();
    }
    if snapshot.profile.trim().is_empty() {
        snapshot.profile = config.profile.clone();
    }
    if crate::planner::profile::canonical_profile_name(&snapshot.profile) == "generic" {
        if snapshot.assurance_level == "static" {
            snapshot.assurance_reason = eval_events::GENERIC_STATIC_ASSURANCE_REASON.to_string();
        } else {
            snapshot.assurance_level = "reduced".to_string();
            snapshot.assurance_reason = eval_events::GENERIC_REDUCED_ASSURANCE_REASON.to_string();
        }
    } else {
        snapshot.assurance_level = "full".to_string();
        snapshot.assurance_reason.clear();
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::config::{Action, Provider};
    use clap::Parser;
    use serde_json::json;

    fn config(root: PathBuf) -> Config {
        Config {
            workspace_root: root,
            state_dir: PathBuf::from("state"),
            eval_events_path: None,
            completion_contract_path: None,
            yes: true,
            offline: false,
            context_budget: 1000,
            model: "m".to_string(),
            provider: Provider::Ollama,
            planner_model: "m".to_string(),
            planner_provider: Provider::Ollama,
            ollama_host: "http://localhost:11434".to_string(),
            num_predict: 100,
            max_iterations: 4,
            chat_timeout_secs: 1,
            chat_timeout_source: "override:test".to_string(),
            chat_retries: 1,
            resume: None,
            fresh_session: false,
            no_footer: false,
            profile: "generic".to_string(),
            profile_explicit: false,
            profile_inference: None,
            style: "default".to_string(),
            action: Action::Repl,
        }
    }

    #[test]
    fn run_lifecycle_writes_events_and_summary_for_tui_exit() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join(".anvil/runs/test-run/events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events.clone());

        emit_run_start(&cfg);
        let result: anyhow::Result<()> = Ok(());
        emit_run_stop(&cfg, &result);

        let event_text = std::fs::read_to_string(&events).unwrap();
        assert!(event_text.contains("\"event\":\"run_start\""));
        assert!(event_text.contains("\"event\":\"run_stop\""));
        let first_event = event_text.lines().next().unwrap();
        let first_event: serde_json::Value = serde_json::from_str(first_event).unwrap();
        assert_eq!(
            first_event
                .get("build_commit")
                .and_then(|value| value.as_str()),
            Some(build_info::COMMIT)
        );
        assert_eq!(
            first_event
                .get("build_dirty")
                .and_then(|value| value.as_bool()),
            Some(build_info::dirty())
        );
        assert_eq!(
            first_event
                .get("build_timestamp")
                .and_then(|value| value.as_str()),
            Some(build_info::TIMESTAMP)
        );
        assert!(event_text.contains("\"task_status\":\"completed (reduced assurance)\""));
        assert!(event_text.contains("\"assurance_level\":\"reduced\""));
        assert!(event_text.contains("\"session_status\":\"process_exited\""));
        assert!(event_text.contains("\"repl_status\":\"not_applicable\""));
        let summary = std::fs::read_to_string(events.parent().unwrap().join("summary.md")).unwrap();
        assert!(
            summary.starts_with(&format!("{}\n", build_info::summary_line())),
            "{summary}"
        );
        assert!(summary.contains("Status: running"));
        assert!(summary.contains("Action: Repl"));
        assert!(summary.contains("Status: complete"));
        assert!(summary.contains("Command status: completed"));
        assert!(summary.contains("Command completion: completed"));
        assert!(summary.contains("Task status: completed (reduced assurance)"));
        assert!(summary.contains(
            "Assurance: reduced (generic profile — no capability contract, no behavioral verification)"
        ));
        assert!(summary.contains("Session/REPL status: process_exited"));
        assert!(summary.contains("Final acceptance: not_checked"));
        assert!(summary.contains("Stop reason: completed"));
    }

    #[test]
    fn direct_cli_ultra_plan_run_finalizer_rewrites_interrupted_summary() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir
            .path()
            .join(".anvil/runs/direct-ultra-plan-run/events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.action = Action::UltraPlanRun("Create a smoke app".to_string());
        cfg.eval_events_path = Some(events.clone());

        emit_run_start(&cfg);
        let guard = DirectCommandCompletionGuard::start(&cfg).expect("direct command guard");
        let result: anyhow::Result<()> = Err(anyhow::anyhow!("interrupted by user"));
        guard.finalize_with_status(&result, DirectCommandStatus::Interrupted);
        drop(guard);

        let event_text = std::fs::read_to_string(&events).unwrap();
        assert!(event_text.contains("\"event\":\"tui_command_stop\""));
        assert!(event_text.contains("\"lifecycle_stage\":\"direct_cli_command\""));
        assert!(event_text.contains("\"command\":\"--ultra-plan-run\""));
        assert!(event_text.contains("\"status\":\"interrupted\""));
        assert!(event_text.contains("\"failure_kind\":\"direct_cli_command_interrupted\""));

        let summary = std::fs::read_to_string(events.parent().unwrap().join("summary.md")).unwrap();
        assert!(summary.contains("Status: interrupted"), "{summary}");
        assert!(summary.contains("Command status: interrupted"), "{summary}");
        assert!(!summary.contains("Status: running"), "{summary}");
    }

    #[test]
    fn direct_cli_error_finalizes_before_run_stop() {
        let dir = tempfile::tempdir().unwrap();
        let state_dir = dir.path().join("state");
        let cli = Cli::parse_from([
            "anvilminimal".to_string(),
            "--cwd".to_string(),
            dir.path().display().to_string(),
            "--state-dir".to_string(),
            state_dir.display().to_string(),
            "--run-plan".to_string(),
            "missing-plan.yaml".to_string(),
            "--model".to_string(),
            "m".to_string(),
            "--yes".to_string(),
        ]);

        let err = run(cli).expect_err("missing run plan should fail");

        assert!(
            err.to_string().contains("missing-plan.yaml")
                || err.to_string().contains("No such file")
                || err.to_string().contains("failed to"),
            "{err:?}"
        );
        let runs_dir = dir.path().join(".anvil/runs");
        let events_path = std::fs::read_dir(&runs_dir)
            .unwrap()
            .flatten()
            .map(|entry| entry.path().join("events.jsonl"))
            .find(|path| path.is_file())
            .expect("events path");
        let event_text = std::fs::read_to_string(events_path).unwrap();
        assert!(
            !event_text.contains("direct CLI command exited before completion finalizer"),
            "{event_text}"
        );
        let command_stop = event_text.find("\"event\":\"tui_command_stop\"").unwrap();
        let run_stop = event_text.find("\"event\":\"run_stop\"").unwrap();
        assert!(command_stop < run_stop, "{event_text}");
        assert!(event_text.contains("\"command\":\"--run-plan\""));
        assert!(event_text.contains("\"status\":\"failed\""));
        assert!(event_text.contains("\"failure_kind\":\"direct_cli_command_failed\""));
    }

    #[test]
    fn run_lifecycle_does_not_label_known_profile_full_without_acceptance() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join(".anvil/runs/test-run/events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.profile = "nextjs".to_string();
        cfg.eval_events_path = Some(events.clone());

        emit_run_start(&cfg);
        let result: anyhow::Result<()> = Ok(());
        emit_run_stop(&cfg, &result);

        let event_text = std::fs::read_to_string(&events).unwrap();
        assert!(event_text.contains("\"assurance_level\":\"reduced\""));
        assert!(!event_text.contains("\"assurance_level\":\"full\""));
        let summary = std::fs::read_to_string(events.parent().unwrap().join("summary.md")).unwrap();
        assert!(summary.contains("Assurance: reduced"));
        assert!(!summary.contains("Assurance: full"));
        assert!(summary.contains("Task status: complete"));
        assert!(!summary.contains("completed (full assurance)"));
    }

    #[test]
    fn run_start_reports_host_env_contamination_once() {
        let status = run_ignored_self_test(
            "tests::run_start_reports_host_env_contamination_once_child",
            &[("NODE_ENV", "production")],
        );
        assert!(status.success(), "{status}");
    }

    #[test]
    #[ignore]
    fn run_start_reports_host_env_contamination_once_child() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join(".anvil/runs/test-run/events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events.clone());

        emit_run_start(&cfg);

        let event_text = std::fs::read_to_string(&events).unwrap();
        assert_eq!(
            event_text
                .matches("\"event\":\"host_env_contamination\"")
                .count(),
            1
        );
        assert!(event_text.contains("NODE_ENV=production"), "{event_text}");
        let summary = std::fs::read_to_string(events.parent().unwrap().join("summary.md")).unwrap();
        assert!(
            summary.contains(
                "Host env: NODE_ENV=production detected (verifiers ran with a cleaned environment)"
            ),
            "{summary}"
        );
    }

    #[test]
    fn run_lifecycle_records_incomplete_stop_reason() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join(".anvil/runs/test-run/events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events.clone());

        emit_run_start(&cfg);
        let result: anyhow::Result<()> = Err(anyhow::anyhow!("boom"));
        emit_run_stop(&cfg, &result);

        let summary = std::fs::read_to_string(events.parent().unwrap().join("summary.md")).unwrap();
        assert!(summary.contains("Status: incomplete"));
        assert!(summary.contains("Command status: failed"));
        assert!(summary.contains("Command completion: failed"));
        assert!(summary.contains("Task status: failed"));
        assert!(summary.contains("Process: REPL exited cleanly (not task status)"));
        assert!(summary.contains("Session/REPL status: process_exited"));
        assert!(summary.contains("Recovery next action: fix_command_failure"));
        assert!(summary.contains("Stop reason: boom"));
        assert!(summary.contains("Failure kind: process_failure"));
    }

    #[test]
    fn run_lifecycle_does_not_mask_partial_release_gate_as_complete() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join(".anvil/runs/test-run/events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events.clone());

        emit_run_start(&cfg);
        eval_events::emit(
            cfg.eval_events_path.as_deref(),
            json!({
                "event": "ultra_final_acceptance",
                "runtime_acceptance_passed": true,
                "runtime_acceptance_status": "pass",
                "final_acceptance_status": "partial",
                "release_gate_status": "partial",
                "release_gate_reasons": ["browser_readiness_or_interaction_evidence_required:browser_readiness_evidence_missing"],
                "browser_readiness_status": "unavailable:browser_readiness_evidence_missing",
                "interaction_evidence_status": "unavailable:interaction_evidence_missing",
            }),
        );
        let result: anyhow::Result<()> = Ok(());
        emit_run_stop(&cfg, &result);

        let event_text = std::fs::read_to_string(&events).unwrap();
        assert!(
            event_text.contains("\"completion_status\":\"complete_with_partial_release_gate\"")
        );
        assert!(event_text.contains("\"task_status\":\"partial\""));
        assert!(event_text.contains("\"release_gate_status\":\"partial\""));
        let summary = std::fs::read_to_string(events.parent().unwrap().join("summary.md")).unwrap();
        assert!(summary.contains("Status: complete_with_partial_release_gate"));
        assert!(summary.contains("Task status: partial"));
        assert!(summary.contains("Command completion: completed"));
        assert!(summary.contains("Runtime acceptance: pass"));
        assert!(summary.contains("Final acceptance: partial"));
        assert!(summary.contains("Release gate: partial"));
        assert!(
            summary.contains(
                "Next action: collect_missing_release_evidence_or_continue_release_recovery"
            ),
            "{summary}"
        );
        assert!(!summary.contains("\nStatus: complete\nAction: Repl\nStop reason: completed"));
    }

    #[test]
    fn run_lifecycle_does_not_mask_browser_http_500_release_failure_as_complete() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join(".anvil/runs/test-run/events.jsonl");
        let mut cfg = config(dir.path().to_path_buf());
        cfg.eval_events_path = Some(events.clone());

        emit_run_start(&cfg);
        eval_events::emit(
            cfg.eval_events_path.as_deref(),
            json!({
                "event": "ultra_final_acceptance",
                "runtime_acceptance_passed": true,
                "runtime_acceptance_status": "pass",
                "final_acceptance_status": "incomplete",
                "release_gate_status": "failed",
                "release_gate_reasons": ["browser_readiness_failed:http_500"],
                "browser_readiness_status": "failed:http_500",
                "interaction_evidence_status": "not_checked",
            }),
        );
        let result: anyhow::Result<()> = Ok(());
        emit_run_stop(&cfg, &result);

        let event_text = std::fs::read_to_string(&events).unwrap();
        assert!(event_text.contains("\"completion_status\":\"incomplete_release_gate_failed\""));
        assert!(event_text.contains("\"task_status\":\"failed\""));
        assert!(event_text.contains("\"browser_readiness_status\":\"failed:http_500\""));
        let summary = std::fs::read_to_string(events.parent().unwrap().join("summary.md")).unwrap();
        assert!(summary.contains("Status: incomplete_release_gate_failed"));
        assert!(summary.contains("Task status: failed"));
        assert!(summary.contains("Release gate: failed"));
        assert!(summary.contains("- browser_readiness_failed:http_500"));
        assert!(!summary.contains("\nStatus: complete\nAction: Repl\nStop reason: completed"));
    }

    fn run_ignored_self_test(test_name: &str, envs: &[(&str, &str)]) -> std::process::ExitStatus {
        let exe = std::env::current_exe().unwrap();
        let mut command = std::process::Command::new(exe);
        command.args(["--ignored", "--exact", test_name, "--nocapture"]);
        for (key, value) in envs {
            command.env(key, value);
        }
        command.status().unwrap()
    }
}
