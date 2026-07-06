use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use serde::{Deserialize, Serialize};

use crate::cli::{Cli, ProviderArg};
use crate::planner::profile::{ProfileInference, infer_profile};

pub const LOCAL_PROVIDER_CHAT_TIMEOUT_SECS: u64 = 600;
pub const REMOTE_PROVIDER_CHAT_TIMEOUT_SECS: u64 = 180;
pub const DEFAULT_CONTEXT_BUDGET: usize = 65_536;
pub const DEFAULT_MODEL: &str = "qwen3.6:27b-coding-nvfp4";

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
    pub field_sources: ConfigFieldSources,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigFieldSources {
    pub model: String,
    pub provider: String,
    pub planner_model: String,
    pub planner_provider: String,
    pub context_budget: String,
    pub chat_timeout_secs: String,
    pub profile: String,
    pub narration: String,
}

impl Default for ConfigFieldSources {
    fn default() -> Self {
        Self {
            model: "default".to_string(),
            provider: "default".to_string(),
            planner_model: "default".to_string(),
            planner_provider: "default".to_string(),
            context_budget: "default".to_string(),
            chat_timeout_secs: "default".to_string(),
            profile: "default".to_string(),
            narration: "default".to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Sourced<T> {
    value: T,
    source: String,
}

fn sourced<T>(value: T, source: impl Into<String>) -> Sourced<T> {
    Sourced {
        value,
        source: source.into(),
    }
}

#[derive(Debug, Clone, Default)]
struct PresetConfig {
    model: Option<Sourced<String>>,
    provider: Option<Sourced<Provider>>,
    planner_model: Option<Sourced<String>>,
    planner_provider: Option<Sourced<Provider>>,
    context_budget: Option<Sourced<usize>>,
    chat_timeout_secs: Option<Sourced<u64>>,
    profile: Option<Sourced<String>>,
    narration: Option<Sourced<NarrationMode>>,
}

#[derive(Debug, Clone, Default)]
struct ConfigFile {
    presets: HashMap<String, PresetConfig>,
    narration: Option<Sourced<NarrationMode>>,
}

impl Config {
    pub fn from_cli(cli: Cli) -> anyhow::Result<Self> {
        let workspace_root = cli
            .cwd
            .clone()
            .unwrap_or(std::env::current_dir().context("failed to read current directory")?)
            .canonicalize()
            .context("failed to canonicalize workspace root")?;
        let preset = load_named_preset(&workspace_root, cli.preset.as_deref())?;
        let model = cli
            .model
            .clone()
            .map(|value| sourced(value, "flag"))
            .or_else(|| preset.as_ref().and_then(|preset| preset.model.clone()))
            .unwrap_or_else(|| sourced(DEFAULT_MODEL.to_string(), "default"));
        let provider = cli
            .provider
            .map(|value| sourced(Provider::from(value), "flag"))
            .or_else(|| preset.as_ref().and_then(|preset| preset.provider.clone()))
            .unwrap_or_else(|| sourced(Provider::Ollama, "default"));
        let planner_provider = cli
            .planner_provider
            .map(|value| sourced(Provider::from(value), "flag"))
            .or_else(|| {
                preset
                    .as_ref()
                    .and_then(|preset| preset.planner_provider.clone())
            })
            .unwrap_or_else(|| sourced(provider.value, "default"));
        let planner_model = cli
            .planner_model
            .clone()
            .map(|value| sourced(value, "flag"))
            .or_else(|| {
                preset
                    .as_ref()
                    .and_then(|preset| preset.planner_model.clone())
            })
            .or_else(|| {
                (planner_provider.value == provider.value)
                    .then(|| sourced(model.value.clone(), "default"))
            });
        let Some(planner_model) = planner_model else {
            bail!("--planner-model is required when --planner-provider differs from --provider");
        };
        let context_budget = cli
            .context_budget
            .map(|value| sourced(value, "flag"))
            .or_else(|| {
                preset
                    .as_ref()
                    .and_then(|preset| preset.context_budget.clone())
            })
            .unwrap_or_else(|| sourced(DEFAULT_CONTEXT_BUDGET, "default"));
        let chat_timeout = cli
            .chat_timeout_secs
            .map(|value| sourced(value, "flag"))
            .or_else(|| {
                preset
                    .as_ref()
                    .and_then(|preset| preset.chat_timeout_secs.clone())
            });
        let (chat_timeout_secs, chat_timeout_source) = match chat_timeout {
            Some(value) => (value.value, value.source),
            None => resolve_chat_timeout(None, provider.value, planner_provider.value),
        };
        let state_dir = cli.state_dir.clone().unwrap_or_else(default_state_dir);
        let action = action_from_cli(&cli)?;
        let eval_events_path = crate::eval_events::path_from_env_or_default(&workspace_root);
        let narration = if cli.quiet {
            sourced(NarrationMode::Quiet, "flag")
        } else {
            preset
                .as_ref()
                .and_then(|preset| preset.narration.clone())
                .or_else(|| config_file_narration(&workspace_root))
                .unwrap_or_else(|| sourced(NarrationMode::Normal, "default"))
        };
        let profile_from_preset = preset.as_ref().and_then(|preset| preset.profile.clone());
        let profile_explicit = cli.profile.is_some() || profile_from_preset.is_some();
        let profile_inference = if profile_explicit {
            None
        } else {
            infer_profile(action_goal(&action), &workspace_root)
        };
        let profile = cli
            .profile
            .clone()
            .map(|value| sourced(value, "flag"))
            .or(profile_from_preset.clone())
            .or_else(|| {
                profile_inference
                    .map(|inference| sourced(inference.profile.to_string(), "default:inferred"))
            })
            .unwrap_or_else(|| sourced("generic".to_string(), "default"));
        let field_sources = ConfigFieldSources {
            model: model.source.clone(),
            provider: provider.source.clone(),
            planner_model: planner_model.source.clone(),
            planner_provider: planner_provider.source.clone(),
            context_budget: context_budget.source.clone(),
            chat_timeout_secs: chat_timeout_source.clone(),
            profile: profile.source.clone(),
            narration: narration.source.clone(),
        };
        Ok(Self {
            workspace_root,
            state_dir,
            eval_events_path,
            completion_contract_path: cli.completion_contract_json,
            yes: cli.yes,
            offline: cli.offline,
            context_budget: context_budget.value,
            model: model.value,
            provider: provider.value,
            planner_model: planner_model.value,
            planner_provider: planner_provider.value,
            ollama_host: cli.ollama_host,
            num_predict: cli.num_predict,
            max_iterations: cli.max_iterations,
            chat_timeout_secs,
            chat_timeout_source,
            field_sources,
            chat_retries: cli.chat_retries,
            resume: cli.resume,
            fresh_session: cli.fresh_session,
            no_footer: cli.no_footer,
            narration: narration.value,
            profile: profile.value,
            profile_explicit,
            profile_inference,
            style: cli.style,
            action,
        })
    }
}

fn load_named_preset(root: &Path, name: Option<&str>) -> anyhow::Result<Option<PresetConfig>> {
    let Some(name) = name.map(str::trim).filter(|name| !name.is_empty()) else {
        return Ok(None);
    };
    let mut found = false;
    let mut merged = PresetConfig::default();
    for path in config_paths(root) {
        let Some(file) = parse_config_file_if_present(&path)? else {
            continue;
        };
        let Some(preset) = file.presets.get(name) else {
            continue;
        };
        found = true;
        merge_preset_field(&mut merged.model, &preset.model);
        merge_preset_field(&mut merged.provider, &preset.provider);
        merge_preset_field(&mut merged.planner_model, &preset.planner_model);
        merge_preset_field(&mut merged.planner_provider, &preset.planner_provider);
        merge_preset_field(&mut merged.context_budget, &preset.context_budget);
        merge_preset_field(&mut merged.chat_timeout_secs, &preset.chat_timeout_secs);
        merge_preset_field(&mut merged.profile, &preset.profile);
        merge_preset_field(&mut merged.narration, &preset.narration);
        if preset_complete(&merged) {
            break;
        }
    }
    if !found {
        let paths = config_paths(root)
            .into_iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        bail!("preset '{name}' not found in {paths} (key: preset.{name})");
    }
    Ok(Some(merged))
}

fn preset_complete(preset: &PresetConfig) -> bool {
    preset.model.is_some()
        && preset.provider.is_some()
        && preset.planner_model.is_some()
        && preset.planner_provider.is_some()
        && preset.context_budget.is_some()
        && preset.chat_timeout_secs.is_some()
        && preset.profile.is_some()
        && preset.narration.is_some()
}

fn merge_preset_field<T: Clone>(target: &mut Option<Sourced<T>>, source: &Option<Sourced<T>>) {
    if target.is_none() {
        *target = source.clone();
    }
}

fn config_file_narration(root: &Path) -> Option<Sourced<NarrationMode>> {
    for path in config_paths(root) {
        if let Ok(Some(file)) = parse_config_file_if_present(&path)
            && let Some(narration) = file.narration
        {
            return Some(narration);
        }
    }
    legacy_config_file_narration(root)
}

fn legacy_config_file_narration(root: &Path) -> Option<Sourced<NarrationMode>> {
    let path = root.join(".anvil").join("config");
    let text = std::fs::read_to_string(&path).ok()?;
    text.lines().find_map(|line| {
        let line = line.split('#').next().unwrap_or("").trim();
        let value = line.strip_prefix("narration")?.trim();
        let value = value.strip_prefix('=')?.trim();
        let value = value.trim_matches('"').trim_matches('\'');
        NarrationMode::from_config_value(value).map(|mode| {
            sourced(
                mode,
                format!(
                    "config:{}",
                    path.file_name().unwrap_or_default().to_string_lossy()
                ),
            )
        })
    })
}

fn config_paths(root: &Path) -> Vec<PathBuf> {
    let mut paths = vec![root.join(".anvil").join("config.toml")];
    if let Some(home) = std::env::var_os("HOME") {
        paths.push(PathBuf::from(home).join(".anvil").join("config.toml"));
    }
    paths
}

fn parse_config_file_if_present(path: &Path) -> anyhow::Result<Option<ConfigFile>> {
    match std::fs::read_to_string(path) {
        Ok(text) => parse_config_file(path, &text).map(Some),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err).with_context(|| format!("failed to read {}", path.display())),
    }
}

fn parse_config_file(path: &Path, text: &str) -> anyhow::Result<ConfigFile> {
    let mut file = ConfigFile::default();
    let mut section = ConfigSection::Top;
    for (index, raw_line) in text.lines().enumerate() {
        let line_no = index + 1;
        let line = strip_toml_comment(raw_line).trim();
        if line.is_empty() {
            continue;
        }
        if let Some(section_name) = line
            .strip_prefix('[')
            .and_then(|line| line.strip_suffix(']'))
        {
            section = if let Some(name) = section_name.trim().strip_prefix("preset.") {
                let name = name.trim();
                if name.is_empty() {
                    bail!(
                        "{}:{} invalid empty preset section name",
                        path.display(),
                        line_no
                    );
                }
                ConfigSection::Preset(name.to_string())
            } else {
                ConfigSection::Other
            };
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            bail!("{}:{} invalid config line", path.display(), line_no);
        };
        let key = key.trim();
        let value = value.trim();
        match &section {
            ConfigSection::Top => parse_top_level_key(path, line_no, key, value, &mut file)?,
            ConfigSection::Preset(name) => {
                let source = format!("preset:{name}");
                let preset = file.presets.entry(name.clone()).or_default();
                parse_preset_key(path, line_no, name, key, value, &source, preset)?;
            }
            ConfigSection::Other => {}
        }
    }
    Ok(file)
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ConfigSection {
    Top,
    Preset(String),
    Other,
}

fn parse_top_level_key(
    path: &Path,
    line_no: usize,
    key: &str,
    value: &str,
    file: &mut ConfigFile,
) -> anyhow::Result<()> {
    match key {
        "narration" => {
            file.narration = Some(sourced(
                parse_narration_value(path, line_no, key, value)?,
                format!(
                    "config:{}",
                    path.file_name().unwrap_or_default().to_string_lossy()
                ),
            ));
            Ok(())
        }
        _ => bail!("{}:{} unknown config key '{key}'", path.display(), line_no),
    }
}

fn parse_preset_key(
    path: &Path,
    line_no: usize,
    name: &str,
    key: &str,
    value: &str,
    source: &str,
    preset: &mut PresetConfig,
) -> anyhow::Result<()> {
    let full_key = format!("preset.{name}.{key}");
    match key {
        "model" => {
            preset.model = Some(sourced(
                parse_string_value(path, line_no, &full_key, value)?,
                source,
            ))
        }
        "provider" => {
            preset.provider = Some(sourced(
                parse_provider_value(path, line_no, &full_key, value)?,
                source,
            ))
        }
        "planner_model" => {
            preset.planner_model = Some(sourced(
                parse_string_value(path, line_no, &full_key, value)?,
                source,
            ))
        }
        "planner_provider" => {
            preset.planner_provider = Some(sourced(
                parse_provider_value(path, line_no, &full_key, value)?,
                source,
            ))
        }
        "context_budget" => {
            preset.context_budget = Some(sourced(
                parse_usize_value(path, line_no, &full_key, value)?,
                source,
            ))
        }
        "chat_timeout_secs" => {
            preset.chat_timeout_secs = Some(sourced(
                parse_u64_value(path, line_no, &full_key, value)?,
                source,
            ))
        }
        "profile" => {
            preset.profile = Some(sourced(
                parse_string_value(path, line_no, &full_key, value)?,
                source,
            ))
        }
        "narration" => {
            preset.narration = Some(sourced(
                parse_narration_value(path, line_no, &full_key, value)?,
                source,
            ))
        }
        _ => bail!(
            "{}:{} unknown config key '{full_key}'",
            path.display(),
            line_no
        ),
    }
    Ok(())
}

fn strip_toml_comment(line: &str) -> &str {
    let mut quoted = false;
    let mut escaped = false;
    for (index, ch) in line.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match ch {
            '\\' if quoted => escaped = true,
            '"' => quoted = !quoted,
            '#' if !quoted => return &line[..index],
            _ => {}
        }
    }
    line
}

fn parse_string_value(
    path: &Path,
    line_no: usize,
    key: &str,
    value: &str,
) -> anyhow::Result<String> {
    let Some(value) = value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
    else {
        bail!(
            "{}:{} {key} expects a quoted string",
            path.display(),
            line_no
        );
    };
    Ok(value.replace("\\\"", "\"").replace("\\\\", "\\"))
}

fn parse_provider_value(
    path: &Path,
    line_no: usize,
    key: &str,
    value: &str,
) -> anyhow::Result<Provider> {
    match parse_string_value(path, line_no, key, value)?.as_str() {
        "ollama" => Ok(Provider::Ollama),
        "openai" => Ok(Provider::Openai),
        "gemini" => Ok(Provider::Gemini),
        _ => bail!(
            "{}:{} {key} expects provider ollama|openai|gemini",
            path.display(),
            line_no
        ),
    }
}

fn parse_narration_value(
    path: &Path,
    line_no: usize,
    key: &str,
    value: &str,
) -> anyhow::Result<NarrationMode> {
    let value = parse_string_value(path, line_no, key, value)?;
    NarrationMode::from_config_value(&value).ok_or_else(|| {
        anyhow::anyhow!(
            "{}:{} {key} expects narration normal|quiet",
            path.display(),
            line_no
        )
    })
}

fn parse_usize_value(path: &Path, line_no: usize, key: &str, value: &str) -> anyhow::Result<usize> {
    value
        .parse::<usize>()
        .with_context(|| format!("{}:{} {key} expects an integer", path.display(), line_no))
}

fn parse_u64_value(path: &Path, line_no: usize, key: &str, value: &str) -> anyhow::Result<u64> {
    value
        .parse::<u64>()
        .with_context(|| format!("{}:{} {key} expects an integer", path.display(), line_no))
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
        assert_eq!(config.chat_timeout_source, "flag");
    }

    #[test]
    fn preset_resolution_uses_flag_preset_default_precedence_and_sources() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().to_string_lossy().to_string();
        std::fs::create_dir_all(dir.path().join(".anvil")).unwrap();
        std::fs::write(
            dir.path().join(".anvil/config.toml"),
            r#"
[preset.local]
model = "preset-model"
provider = "ollama"
planner_model = "preset-planner"
planner_provider = "gemini"
context_budget = 12345
chat_timeout_secs = 77
profile = "nextjs"
narration = "quiet"
"#,
        )
        .unwrap();

        let config = Config::from_cli(Cli::parse_from([
            "anvilminimal",
            "--cwd",
            &cwd,
            "--preset",
            "local",
            "--model",
            "flag-model",
            "--context-budget",
            "999",
            "--ultra-plan-run",
            "Web app",
        ]))
        .unwrap();

        assert_eq!(config.model, "flag-model");
        assert_eq!(config.field_sources.model, "flag");
        assert_eq!(config.context_budget, 999);
        assert_eq!(config.field_sources.context_budget, "flag");
        assert_eq!(config.provider, Provider::Ollama);
        assert_eq!(config.field_sources.provider, "preset:local");
        assert_eq!(config.planner_provider, Provider::Gemini);
        assert_eq!(config.field_sources.planner_provider, "preset:local");
        assert_eq!(config.planner_model, "preset-planner");
        assert_eq!(config.field_sources.planner_model, "preset:local");
        assert_eq!(config.chat_timeout_secs, 77);
        assert_eq!(config.field_sources.chat_timeout_secs, "preset:local");
        assert_eq!(config.profile, "nextjs");
        assert_eq!(config.field_sources.profile, "preset:local");
        assert!(config.profile_explicit);
        assert!(config.profile_inference.is_none());
        assert_eq!(config.narration, NarrationMode::Quiet);
        assert_eq!(config.field_sources.narration, "preset:local");
    }

    #[test]
    fn missing_preset_error_names_file_and_key() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().to_string_lossy().to_string();
        std::fs::create_dir_all(dir.path().join(".anvil")).unwrap();
        std::fs::write(dir.path().join(".anvil/config.toml"), "# no preset\n").unwrap();

        let err = Config::from_cli(Cli::parse_from([
            "anvilminimal",
            "--cwd",
            &cwd,
            "--preset",
            "missing-pre-ux-final",
        ]))
        .unwrap_err()
        .to_string();

        assert!(
            err.contains("preset 'missing-pre-ux-final' not found"),
            "{err}"
        );
        assert!(err.contains(".anvil/config.toml"), "{err}");
        assert!(err.contains("preset.missing-pre-ux-final"), "{err}");
    }

    #[test]
    fn invalid_preset_value_error_names_file_and_key() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().to_string_lossy().to_string();
        std::fs::create_dir_all(dir.path().join(".anvil")).unwrap();
        std::fs::write(
            dir.path().join(".anvil/config.toml"),
            "[preset.bad]\nprovider = \"bogus\"\n",
        )
        .unwrap();

        let err = Config::from_cli(Cli::parse_from([
            "anvilminimal",
            "--cwd",
            &cwd,
            "--preset",
            "bad",
        ]))
        .unwrap_err()
        .to_string();

        assert!(err.contains(".anvil/config.toml"), "{err}");
        assert!(err.contains("preset.bad.provider"), "{err}");
        assert!(err.contains("ollama|openai|gemini"), "{err}");
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
