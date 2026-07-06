use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{Context, bail};
use serde::{Deserialize, Serialize};

use crate::cli::{Cli, ProviderArg};
use crate::planner::profile::{ProfileInference, infer_profile};

pub const LOCAL_PROVIDER_CHAT_TIMEOUT_SECS: u64 = 600;
pub const REMOTE_PROVIDER_CHAT_TIMEOUT_SECS: u64 = 180;

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
    SetupInteractionProbe,
    Runs,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum NarrationMode {
    #[default]
    Normal,
    Quiet,
}

impl NarrationMode {
    pub fn is_quiet(self) -> bool {
        matches!(self, Self::Quiet)
    }

    fn from_config_value(value: &str) -> Option<Self> {
        match value.trim() {
            "normal" => Some(Self::Normal),
            "quiet" => Some(Self::Quiet),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Config {
    pub workspace_root: PathBuf,
    pub state_dir: PathBuf,
    pub eval_events_path: Option<PathBuf>,
    pub completion_contract_path: Option<PathBuf>,
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
    pub chat_timeout_source: String,
    pub chat_retries: usize,
    pub resume: Option<String>,
    pub fresh_session: bool,
    pub no_footer: bool,
    pub narration: NarrationMode,
    pub profile: String,
    pub profile_explicit: bool,
    pub profile_inference: Option<ProfileInference>,
    pub style: String,
    pub action: Action,
}

impl Config {
    pub fn from_cli(cli: Cli) -> anyhow::Result<Self> {
        let provider = Provider::from(cli.provider);
        let planner_provider = cli.planner_provider.map(Provider::from).unwrap_or(provider);
        let (chat_timeout_secs, chat_timeout_source) =
            resolve_chat_timeout(cli.chat_timeout_secs, provider, planner_provider);
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
        let eval_events_path = crate::eval_events::path_from_env_or_default(&workspace_root);
        let narration = if cli.quiet {
            NarrationMode::Quiet
        } else {
            config_file_narration(&workspace_root).unwrap_or(NarrationMode::Normal)
        };
        let profile_explicit = cli.profile.is_some();
        let profile_inference = if profile_explicit {
            None
        } else {
            infer_profile(action_goal(&action), &workspace_root)
        };
        let profile = cli
            .profile
            .clone()
            .or_else(|| profile_inference.map(|inference| inference.profile.to_string()))
            .unwrap_or_else(|| "generic".to_string());
        Ok(Self {
            workspace_root,
            state_dir,
            eval_events_path,
            completion_contract_path: cli.completion_contract_json,
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
            chat_timeout_secs,
            chat_timeout_source,
            chat_retries: cli.chat_retries,
            resume: cli.resume,
            fresh_session: cli.fresh_session,
            no_footer: cli.no_footer,
            narration,
            profile,
            profile_explicit,
            profile_inference,
            style: cli.style,
            action,
        })
    }
}

fn config_file_narration(root: &std::path::Path) -> Option<NarrationMode> {
    let text = std::fs::read_to_string(root.join(".anvil").join("config")).ok()?;
    text.lines().find_map(|line| {
        let line = line.split('#').next().unwrap_or("").trim();
        let value = line.strip_prefix("narration")?.trim();
        let value = value.strip_prefix('=')?.trim();
        let value = value.trim_matches('"').trim_matches('\'');
        NarrationMode::from_config_value(value)
    })
}

fn resolve_chat_timeout(
    override_secs: Option<u64>,
    provider: Provider,
    planner_provider: Provider,
) -> (u64, String) {
    if let Some(secs) = override_secs {
        return (secs, "override:cli".to_string());
    }
    if matches!(provider, Provider::Ollama) || matches!(planner_provider, Provider::Ollama) {
        (
            LOCAL_PROVIDER_CHAT_TIMEOUT_SECS,
            "default:local_provider".to_string(),
        )
    } else {
        (
            REMOTE_PROVIDER_CHAT_TIMEOUT_SECS,
            "default:remote_provider".to_string(),
        )
    }
}

pub fn action_goal(action: &Action) -> Option<&str> {
    match action {
        Action::Prompt(goal)
        | Action::PlanSteps(goal)
        | Action::PlanRun(goal)
        | Action::UltraPlan(goal)
        | Action::UltraPlanRun(goal) => Some(goal.as_str()),
        Action::Repl
        | Action::RunPlan(_)
        | Action::RunUltraPlan(_)
        | Action::SetupInteractionProbe
        | Action::Runs => None,
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
    count += cli.setup_interaction_probe as usize;
    count += cli.runs as usize;
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
    if cli.setup_interaction_probe {
        return Ok(Action::SetupInteractionProbe);
    }
    if cli.runs {
        return Ok(Action::Runs);
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

    #[test]
    fn runs_action_is_read_only_selector() {
        let dir = tempfile::tempdir().unwrap();
        let cli = Cli::parse_from([
            "anvilminimal",
            "--cwd",
            dir.path().to_str().unwrap(),
            "--runs",
        ]);
        let config = Config::from_cli(cli).unwrap();

        assert!(matches!(config.action, Action::Runs));
        assert!(action_goal(&config.action).is_none());
    }

    #[test]
    fn narration_quiet_is_read_from_cli_or_config_file() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().to_string_lossy().to_string();
        std::fs::create_dir_all(dir.path().join(".anvil")).unwrap();
        std::fs::write(dir.path().join(".anvil/config"), "narration = \"quiet\"\n").unwrap();
        let config = Config::from_cli(Cli::parse_from(["anvilminimal", "--cwd", &cwd])).unwrap();
        assert_eq!(config.narration, NarrationMode::Quiet);

        std::fs::write(dir.path().join(".anvil/config"), "narration = \"normal\"\n").unwrap();
        let config =
            Config::from_cli(Cli::parse_from(["anvilminimal", "--cwd", &cwd, "--quiet"])).unwrap();
        assert_eq!(config.narration, NarrationMode::Quiet);
    }

    #[test]
    fn ollama_chat_timeout_defaults_to_local_provider_budget() {
        let cli = Cli::parse_from(["anvilminimal", "--provider", "ollama"]);
        let config = Config::from_cli(cli).unwrap();
        assert_eq!(config.chat_timeout_secs, LOCAL_PROVIDER_CHAT_TIMEOUT_SECS);
        assert_eq!(config.chat_timeout_source, "default:local_provider");
    }

    #[test]
    fn remote_chat_timeout_defaults_to_remote_provider_budget() {
        let cli = Cli::parse_from([
            "anvilminimal",
            "--provider",
            "openai",
            "--model",
            "gpt-test",
        ]);
        let config = Config::from_cli(cli).unwrap();
        assert_eq!(config.chat_timeout_secs, REMOTE_PROVIDER_CHAT_TIMEOUT_SECS);
        assert_eq!(config.chat_timeout_source, "default:remote_provider");
    }

    #[test]
    fn explicit_chat_timeout_wins_for_local_provider() {
        let cli = Cli::parse_from([
            "anvilminimal",
            "--provider",
            "ollama",
            "--chat-timeout-secs",
            "42",
        ]);
        let config = Config::from_cli(cli).unwrap();
        assert_eq!(config.chat_timeout_secs, 42);
        assert_eq!(config.chat_timeout_source, "override:cli");
    }

    #[test]
    fn profile_infers_from_goal_when_absent() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().to_string_lossy().to_string();
        let cli = Cli::parse_from([
            "anvilminimal",
            "--cwd",
            &cwd,
            "--ultra-plan-run",
            "Web アプリを作って",
        ]);
        let config = Config::from_cli(cli).unwrap();
        assert_eq!(config.profile, "nextjs");
        let inference = config.profile_inference.expect("profile inference");
        assert_eq!(inference.source.as_str(), "goal");
        assert!(!config.profile_explicit);
    }

    #[test]
    fn profile_infers_python_cli_from_goal_when_absent() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().to_string_lossy().to_string();
        let cli = Cli::parse_from([
            "anvilminimal",
            "--cwd",
            &cwd,
            "--ultra-plan-run",
            "Python コマンドラインを作って",
        ]);
        let config = Config::from_cli(cli).unwrap();
        assert_eq!(config.profile, "python-cli");
        let inference = config.profile_inference.expect("profile inference");
        assert_eq!(inference.source.as_str(), "goal");
    }

    #[test]
    fn profile_infers_from_workspace_manifest_after_goal_miss() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"dependencies":{"next":"^14.2.0"}}"#,
        )
        .unwrap();
        let cwd = dir.path().to_string_lossy().to_string();
        let cli = Cli::parse_from([
            "anvilminimal",
            "--cwd",
            &cwd,
            "--ultra-plan-run",
            "実装して",
        ]);
        let config = Config::from_cli(cli).unwrap();
        assert_eq!(config.profile, "nextjs");
        let inference = config.profile_inference.expect("profile inference");
        assert_eq!(inference.source.as_str(), "workspace");
    }

    #[test]
    fn profile_infers_python_cli_from_pyproject_after_goal_miss() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("pyproject.toml"),
            "[project]\nname = \"x\"\n",
        )
        .unwrap();
        let cwd = dir.path().to_string_lossy().to_string();
        let cli = Cli::parse_from([
            "anvilminimal",
            "--cwd",
            &cwd,
            "--ultra-plan-run",
            "実装して",
        ]);
        let config = Config::from_cli(cli).unwrap();
        assert_eq!(config.profile, "python-cli");
        let inference = config.profile_inference.expect("profile inference");
        assert_eq!(inference.source.as_str(), "workspace");
    }

    #[test]
    fn explicit_generic_profile_suppresses_auto_inference() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("pyproject.toml"),
            "[project]\nname = \"x\"\n",
        )
        .unwrap();
        let cwd = dir.path().to_string_lossy().to_string();
        let cli = Cli::parse_from([
            "anvilminimal",
            "--cwd",
            &cwd,
            "--profile",
            "generic",
            "--ultra-plan-run",
            "Python CLI を作って",
        ]);
        let config = Config::from_cli(cli).unwrap();
        assert_eq!(config.profile, "generic");
        assert!(config.profile_explicit);
        assert!(config.profile_inference.is_none());
    }
}
