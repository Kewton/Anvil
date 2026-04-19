use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use crate::cli::CliArgs;
use crate::safety::host_validation::validate_localhost_url;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub cwd: PathBuf,
    pub requested_model: Option<String>,
    pub requested_sidecar_model: Option<String>,
    pub ollama_host: String,
    pub context_budget: usize,
    pub max_iterations: usize,
    pub chat_timeout_secs: u64,
    pub chat_retries: usize,
    pub debug: bool,
    pub stream: bool,
    pub yes_mode: bool,
    pub fresh_session: bool,
    pub oneshot: bool,
    pub prompt: Option<String>,
    pub state_dir_override: Option<PathBuf>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PartialConfig {
    pub model: Option<String>,
    pub sidecar_model: Option<String>,
    pub ollama_host: Option<String>,
    pub context_budget: Option<usize>,
    pub max_iterations: Option<usize>,
    pub chat_timeout_secs: Option<u64>,
    pub chat_retries: Option<usize>,
    pub debug: Option<bool>,
    pub stream: Option<bool>,
    pub yes_mode: Option<bool>,
    pub fresh_session: Option<bool>,
    pub state_dir_override: Option<PathBuf>,
}

impl Config {
    pub fn load(args: CliArgs) -> Result<Self, String> {
        let cwd = match args.cwd {
            Some(path) => path,
            None => env::current_dir().map_err(|err| format!("failed to resolve cwd: {err}"))?,
        };

        let file_config = load_config_file(&cwd.join(".anvil").join("config"))?;
        let env_config = load_env_config();
        let cli_config = PartialConfig {
            model: args.model.clone(),
            sidecar_model: args.sidecar_model.clone(),
            ollama_host: args.ollama_host.clone(),
            context_budget: args.context_budget,
            max_iterations: args.max_iterations,
            chat_timeout_secs: args.chat_timeout_secs,
            chat_retries: args.chat_retries,
            debug: args.debug.then_some(true),
            stream: args.stream.then_some(true),
            yes_mode: args.yes.then_some(true),
            fresh_session: args.fresh_session.then_some(true),
            state_dir_override: args.state_dir.clone(),
        };
        let merged = merge_partial_configs(&[file_config, env_config, cli_config]);
        let ollama_host = validate_localhost_url(
            merged
                .ollama_host
                .unwrap_or_else(|| "http://127.0.0.1:11434".to_string()),
        )?;

        Ok(Self {
            cwd,
            requested_model: merged.model,
            requested_sidecar_model: merged.sidecar_model,
            ollama_host,
            context_budget: merged.context_budget.unwrap_or(24_000),
            max_iterations: merged.max_iterations.unwrap_or(12),
            chat_timeout_secs: merged.chat_timeout_secs.unwrap_or(300),
            chat_retries: merged.chat_retries.unwrap_or(2),
            debug: merged.debug.unwrap_or(false),
            stream: merged.stream.unwrap_or(false),
            yes_mode: merged.yes_mode.unwrap_or(false),
            fresh_session: merged.fresh_session.unwrap_or(false),
            oneshot: args.oneshot || args.prompt.is_some(),
            prompt: args.prompt,
            state_dir_override: merged.state_dir_override,
        })
    }
}

pub fn merge_partial_configs(configs: &[PartialConfig]) -> PartialConfig {
    let mut merged = PartialConfig::default();
    for config in configs {
        if config.model.is_some() {
            merged.model = config.model.clone();
        }
        if config.sidecar_model.is_some() {
            merged.sidecar_model = config.sidecar_model.clone();
        }
        if config.ollama_host.is_some() {
            merged.ollama_host = config.ollama_host.clone();
        }
        if config.context_budget.is_some() {
            merged.context_budget = config.context_budget;
        }
        if config.max_iterations.is_some() {
            merged.max_iterations = config.max_iterations;
        }
        if config.chat_timeout_secs.is_some() {
            merged.chat_timeout_secs = config.chat_timeout_secs;
        }
        if config.chat_retries.is_some() {
            merged.chat_retries = config.chat_retries;
        }
        if config.debug.is_some() {
            merged.debug = config.debug;
        }
        if config.stream.is_some() {
            merged.stream = config.stream;
        }
        if config.yes_mode.is_some() {
            merged.yes_mode = config.yes_mode;
        }
        if config.fresh_session.is_some() {
            merged.fresh_session = config.fresh_session;
        }
        if config.state_dir_override.is_some() {
            merged.state_dir_override = config.state_dir_override.clone();
        }
    }
    merged
}

pub fn load_config_file(path: &Path) -> Result<PartialConfig, String> {
    if !path.exists() {
        return Ok(PartialConfig::default());
    }

    let contents = fs::read_to_string(path)
        .map_err(|err| format!("failed to read {}: {err}", path.display()))?;
    let map = parse_key_value_config(&contents);

    Ok(PartialConfig {
        model: map.get("model").cloned(),
        sidecar_model: map.get("sidecar_model").cloned(),
        ollama_host: map
            .get("ollama_host")
            .cloned()
            .or_else(|| map.get("provider_url").cloned()),
        context_budget: map
            .get("context_budget")
            .and_then(|value| value.parse().ok()),
        max_iterations: map
            .get("max_iterations")
            .and_then(|value| value.parse().ok()),
        chat_timeout_secs: map
            .get("chat_timeout_secs")
            .and_then(|value| value.parse().ok()),
        chat_retries: map.get("chat_retries").and_then(|value| value.parse().ok()),
        debug: map.get("debug").and_then(|value| parse_bool(value)),
        stream: map.get("stream").and_then(|value| parse_bool(value)),
        yes_mode: map.get("yes_mode").and_then(|value| parse_bool(value)),
        fresh_session: map.get("fresh_session").and_then(|value| parse_bool(value)),
        state_dir_override: map.get("state_dir").map(PathBuf::from),
    })
}

pub fn load_env_config() -> PartialConfig {
    PartialConfig {
        model: env::var("ANVIL_MODEL").ok(),
        sidecar_model: env::var("ANVIL_SIDECAR_MODEL").ok(),
        ollama_host: env::var("ANVIL_OLLAMA_HOST")
            .ok()
            .or_else(|| env::var("ANVIL_PROVIDER_URL").ok()),
        context_budget: env::var("ANVIL_CONTEXT_BUDGET")
            .ok()
            .and_then(|value| value.parse().ok()),
        max_iterations: env::var("ANVIL_MAX_ITERATIONS")
            .ok()
            .and_then(|value| value.parse().ok()),
        chat_timeout_secs: env::var("ANVIL_CHAT_TIMEOUT_SECS")
            .ok()
            .and_then(|value| value.parse().ok()),
        chat_retries: env::var("ANVIL_CHAT_RETRIES")
            .ok()
            .and_then(|value| value.parse().ok()),
        debug: env::var("ANVIL_DEBUG")
            .ok()
            .and_then(|value| parse_bool(&value)),
        stream: env::var("ANVIL_STREAM")
            .ok()
            .and_then(|value| parse_bool(&value)),
        yes_mode: env::var("ANVIL_YES")
            .ok()
            .and_then(|value| parse_bool(&value)),
        fresh_session: env::var("ANVIL_FRESH_SESSION")
            .ok()
            .and_then(|value| parse_bool(&value)),
        state_dir_override: env::var("ANVIL_STATE_DIR").ok().map(PathBuf::from),
    }
}

pub fn parse_key_value_config(contents: &str) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for line in contents.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        let without_inline_comment = match trimmed.split_once('#') {
            Some((prefix, _)) => prefix.trim(),
            None => trimmed,
        };
        if let Some((raw_key, raw_value)) = without_inline_comment.split_once('=') {
            let key = raw_key.trim().to_lowercase().replace('-', "_");
            let value = raw_value
                .trim()
                .trim_matches('"')
                .trim_matches('\'')
                .to_string();
            if !value.is_empty() {
                map.insert(key, value);
            }
        }
    }
    map
}

pub fn parse_bool(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}
