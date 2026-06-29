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
    match config.action.clone() {
        Action::Repl => repl::run_repl(config),
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
    }
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
}
