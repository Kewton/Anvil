pub mod cli;
pub mod config;
pub mod eval_events;
pub mod minimal_loop;
pub mod mode;
pub mod planner;
pub mod providers;
pub mod repl;
pub mod state;
pub mod tools;
pub mod tui;

use anyhow::Context;
use cli::Cli;
use config::{Action, Config};
use serde_json::json;
use tui::OutputRenderer;
use tui::markdown::PlainRenderer;

pub fn run(cli: Cli) -> anyhow::Result<()> {
    let config = Config::from_cli(cli)?;
    emit_run_start(&config);
    let result = match config.action.clone() {
        Action::Repl => repl::run_repl(config.clone()),
        Action::Prompt(prompt) => {
            let mut client = providers::client_from_config(&config, false)?;
            let resume = if config.fresh_session {
                None
            } else {
                config.resume.as_deref()
            };
            let mut session =
                state::SessionStore::new(config.state_dir.clone()).load_or_create(resume)?;
            let reply = minimal_loop::run_session(&mut *client, &mut session, &prompt, &config)?;
            state::SessionStore::new(config.state_dir.clone()).save(&session)?;
            PlainRenderer.render_assistant(&reply)?;
            Ok(())
        }
        Action::PlanSteps(goal) => {
            let mut planner = providers::client_from_config(&config, true)?;
            let plan = planner::generate_step_plan(&mut *planner, &goal, &config)
                .context("failed to generate step plan")?;
            let path = planner::save_step_plan(&config.workspace_root, &plan)?;
            println!("{}", path.display());
            Ok(())
        }
        Action::PlanRun(goal) => {
            let mut execution = providers::client_from_config(&config, false)?;
            let mut planner_client = providers::client_from_config(&config, true)?;
            let report = planner::generate_and_run_step_plan(
                &mut *planner_client,
                &mut *execution,
                &goal,
                &config,
            )?;
            println!("{report}");
            Ok(())
        }
        Action::RunPlan(path) => {
            let mut execution = providers::client_from_config(&config, false)?;
            let report = planner::run_plan_file(&mut *execution, &path, &config)?;
            println!("{report}");
            Ok(())
        }
        Action::UltraPlan(goal) => {
            let mut planner_client = providers::client_from_config(&config, true)?;
            let plan = planner::generate_ultra_plan(&mut *planner_client, &goal, &config)?;
            let path = planner::save_ultra_plan(&config.workspace_root, &plan)?;
            println!("{}", path.display());
            Ok(())
        }
        Action::UltraPlanRun(goal) => {
            let mut execution = providers::client_from_config(&config, false)?;
            let mut planner_client = providers::client_from_config(&config, true)?;
            let report = planner::generate_and_run_ultra_plan(
                &mut *planner_client,
                &mut *execution,
                &goal,
                &config,
            )?;
            println!("{report}");
            Ok(())
        }
        Action::RunUltraPlan(path) => {
            let mut execution = providers::client_from_config(&config, false)?;
            let mut planner_client = providers::client_from_config(&config, true)?;
            let report = planner::run_ultra_plan_file(
                &mut *planner_client,
                &mut *execution,
                &path,
                &config,
            )?;
            println!("{report}");
            Ok(())
        }
    };
    emit_run_stop(&config, &result);
    result
}

fn emit_run_start(config: &Config) {
    eval_events::emit(
        config.eval_events_path.as_deref(),
        json!({
            "event": "run_start",
            "workspace_root": eval_events::body_snippet(&config.workspace_root.display().to_string()),
            "provider": format!("{:?}", config.provider).to_ascii_lowercase(),
            "model": eval_events::body_snippet(&config.model),
            "planner_provider": format!("{:?}", config.planner_provider).to_ascii_lowercase(),
            "planner_model": eval_events::body_snippet(&config.planner_model),
            "profile": config.profile,
            "style": config.style,
            "action": format!("{:?}", config.action),
            "eval_events_override": eval_events::is_eval_events_override(),
        }),
    );
    let events_path = config
        .eval_events_path
        .as_ref()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "unavailable".to_string());
    eval_events::write_run_summary(
        config.eval_events_path.as_deref(),
        &format!(
            "Status: running\nAction: {:?}\nEvents: {}",
            config.action, events_path
        ),
    );
}

fn emit_run_stop(config: &Config, result: &anyhow::Result<()>) {
    let (ok, stop_reason, failure_kind) = match result {
        Ok(()) => (true, "completed".to_string(), ""),
        Err(err) => (
            false,
            eval_events::body_snippet(&err.to_string()),
            "process_failure",
        ),
    };
    eval_events::emit(
        config.eval_events_path.as_deref(),
        json!({
            "event": "run_stop",
            "ok": ok,
            "lifecycle_stage": "process",
            "action": format!("{:?}", config.action),
            "stop_reason": stop_reason,
            "failure_kind": failure_kind,
        }),
    );
    let summary = if ok {
        format!(
            "Status: complete\nAction: {:?}\nStop reason: {}",
            config.action, stop_reason
        )
    } else {
        format!(
            "Status: incomplete\nAction: {:?}\nStop reason: {}\nFailure kind: {}",
            config.action, stop_reason, failure_kind
        )
    };
    eval_events::append_run_summary(config.eval_events_path.as_deref(), &summary);
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::config::{Action, Provider};

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
            chat_retries: 1,
            resume: None,
            fresh_session: false,
            no_footer: false,
            profile: "generic".to_string(),
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
        let summary = std::fs::read_to_string(events.parent().unwrap().join("summary.md")).unwrap();
        assert!(summary.contains("Status: running"));
        assert!(summary.contains("Action: Repl"));
        assert!(summary.contains("Status: complete"));
        assert!(summary.contains("Stop reason: completed"));
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
        assert!(summary.contains("Stop reason: boom"));
        assert!(summary.contains("Failure kind: process_failure"));
    }
}
