use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{Context, bail};
use serde::{Deserialize, Serialize};

use crate::cli::{Cli, ProviderArg};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Provider {
    Ollama,
    Openai,
    Gemini,
}

impl From<ProviderArg> for Provider {
    fn from(value: ProviderArg) -> Self {
        match value {
            ProviderArg::Ollama => Self::Ollama,
            ProviderArg::Openai => Self::Openai,
            ProviderArg::Gemini => Self::Gemini,
        }
    }
}

#[derive(Debug, Clone)]
pub enum Action {
    Repl,
    Prompt(String),
    PlanSteps(String),
    PlanRun(String),
    RunPlan(PathBuf),
    UltraPlan(String),
    UltraPlanRun(String),
    RunUltraPlan(PathBuf),
}

#[derive(Debug, Clone)]
pub struct Config {
    pub workspace_root: PathBuf,
    pub state_dir: PathBuf,
    pub yes: bool,
    pub offline: bool,
    pub context_budget: usize,
    pub model: String,
    pub provider: Provider,
    pub planner_model: String,
    pub planner_provider: Provider,
    pub ollama_host: String,
    pub num_predict: usize,
    pub max_iterations: usize,
    pub chat_timeout_secs: u64,
    pub chat_retries: usize,
    pub resume: Option<String>,
    pub fresh_session: bool,
    pub no_footer: bool,
    pub profile: String,
    pub style: String,
    pub action: Action,
}

impl Config {
    pub fn from_cli(cli: Cli) -> anyhow::Result<Self> {
        let provider = Provider::from(cli.provider);
        let planner_provider = cli.planner_provider.map(Provider::from).unwrap_or(provider);
        let planner_model = match cli.planner_model.clone() {
            Some(model) => model,
            None if planner_provider == provider => cli.model.clone(),
            None => {
                bail!("--planner-model is required when --planner-provider differs from --provider")
            }
        };
        let workspace_root = cli
            .cwd
            .clone()
            .unwrap_or(std::env::current_dir().context("failed to read current directory")?)
            .canonicalize()
            .context("failed to canonicalize workspace root")?;
        let state_dir = cli.state_dir.clone().unwrap_or_else(default_state_dir);
        let action = action_from_cli(&cli)?;
        Ok(Self {
            workspace_root,
            state_dir,
            yes: cli.yes,
            offline: cli.offline,
            context_budget: cli.context_budget,
            model: cli.model,
            provider,
            planner_model,
            planner_provider,
            ollama_host: cli.ollama_host,
            num_predict: cli.num_predict,
            max_iterations: cli.max_iterations,
            chat_timeout_secs: cli.chat_timeout_secs,
            chat_retries: cli.chat_retries,
            resume: cli.resume,
            fresh_session: cli.fresh_session,
            no_footer: cli.no_footer,
            profile: cli.profile,
            style: cli.style,
            action,
        })
    }
}

fn action_from_cli(cli: &Cli) -> anyhow::Result<Action> {
    let mut count = 0usize;
    count += cli.prompt.is_some() as usize;
    count += cli.plan_steps as usize;
    count += cli.plan_run as usize;
    count += cli.run_plan.is_some() as usize;
    count += cli.ultra_plan as usize;
    count += cli.ultra_plan_run as usize;
    count += cli.run_ultra_plan.is_some() as usize;
    if count > 1 {
        bail!("only one action selector can be used at a time");
    }
    if let Some(prompt) = cli.prompt.clone() {
        return Ok(Action::Prompt(prompt));
    }
    if let Some(path) = cli.run_plan.clone() {
        return Ok(Action::RunPlan(path));
    }
    if let Some(path) = cli.run_ultra_plan.clone() {
        return Ok(Action::RunUltraPlan(path));
    }
    let goal = cli.trailing_goal();
    if cli.plan_steps {
        return Ok(Action::PlanSteps(required_goal(goal, "--plan-steps")?));
    }
    if cli.plan_run {
        return Ok(Action::PlanRun(required_goal(goal, "--plan-run")?));
    }
    if cli.ultra_plan {
        return Ok(Action::UltraPlan(required_goal(goal, "--ultra-plan")?));
    }
    if cli.ultra_plan_run {
        return Ok(Action::UltraPlanRun(required_goal(
            goal,
            "--ultra-plan-run",
        )?));
    }
    Ok(Action::Repl)
}

fn required_goal(goal: Option<String>, action: &str) -> anyhow::Result<String> {
    goal.filter(|goal| !goal.trim().is_empty())
        .ok_or_else(|| anyhow::anyhow!("{action} requires a trailing goal"))
}

pub fn default_state_dir() -> PathBuf {
    if let Some(xdg) = std::env::var_os("XDG_STATE_HOME") {
        return PathBuf::from(xdg).join("anvilminimal");
    }
    let home = std::env::var_os("HOME").unwrap_or_else(|| ".".into());
    PathBuf::from(home)
        .join(".local")
        .join("state")
        .join("anvilminimal")
}

pub fn load_api_key(workspace_root: &std::path::Path, name: &str) -> anyhow::Result<String> {
    if let Ok(value) = std::env::var(name)
        && !value.trim().is_empty()
    {
        return Ok(value);
    }
    let env = read_dotenv(workspace_root);
    env.get(name)
        .filter(|value| !value.trim().is_empty())
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("{name} is not set"))
}

pub fn read_dotenv(workspace_root: &std::path::Path) -> HashMap<String, String> {
    let path = workspace_root.join(".env");
    let Ok(content) = std::fs::read_to_string(path) else {
        return HashMap::new();
    };
    content
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                return None;
            }
            let (key, value) = line.split_once('=')?;
            let value = value
                .trim()
                .trim_matches('"')
                .trim_matches('\'')
                .to_string();
            Some((key.trim().to_string(), value))
        })
        .collect()
}

pub fn redact(value: &str) -> String {
    if value.is_empty() {
        "<empty>".to_string()
    } else {
        "<redacted>".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn cross_provider_planner_model_error() {
        let cli = Cli::parse_from([
            "anvilminimal",
            "--provider",
            "ollama",
            "--planner-provider",
            "gemini",
        ]);
        let err = Config::from_cli(cli).unwrap_err().to_string();
        assert!(err.contains("--planner-model is required"));
    }

    #[test]
    fn same_provider_defaults_planner_model() {
        let cli = Cli::parse_from(["anvilminimal", "--provider", "ollama", "--model", "m"]);
        let config = Config::from_cli(cli).unwrap();
        assert_eq!(config.planner_model, "m");
    }
}
