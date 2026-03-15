//! Configuration loading with file / environment / CLI precedence.
//!
//! [`EffectiveConfig`] is the single source of truth for all runtime
//! settings.  It is assembled once at startup and then treated as
//! immutable for the lifetime of the session.

use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::{
    error::Error,
    fmt::{Display, Formatter},
};

#[derive(Debug, Clone)]
/// Provider, model, and transport settings.
pub struct RuntimeConfig {
    pub provider: String,
    pub provider_url: String,
    pub model: String,
    pub sidecar_model: Option<String>,
    pub api_key: Option<String>,
    pub context_window: u32,
    pub context_budget: Option<u32>,
    pub stream: bool,
}

#[derive(Debug, Clone)]
pub struct ModeConfig {
    pub interactive: bool,
    pub approval_required: bool,
    pub fresh_session: bool,
    pub reasoning_visibility: ReasoningVisibility,
    pub debug_logging: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReasoningVisibility {
    Hidden,
    Summary,
}

#[derive(Debug, Clone)]
pub struct PathConfig {
    pub cwd: PathBuf,
    pub workspace_dir: PathBuf,
    pub config_file: PathBuf,
    pub state_dir: PathBuf,
    pub session_dir: PathBuf,
    pub session_file: PathBuf,
}

#[derive(Debug, Clone)]
pub struct EffectiveConfig {
    pub runtime: RuntimeConfig,
    pub mode: ModeConfig,
    pub paths: PathConfig,
}

#[derive(Debug)]
pub enum ConfigError {
    CurrentDirUnavailable(std::io::Error),
    ConfigFileUnreadable(std::io::Error),
    InvalidConfigLine(String),
    InvalidNumericValue(String),
    InvalidReasoningVisibility(String),
}

impl Display for ConfigError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CurrentDirUnavailable(err) => {
                write!(f, "failed to determine current directory: {err}")
            }
            Self::ConfigFileUnreadable(err) => write!(f, "failed to read config file: {err}"),
            Self::InvalidConfigLine(line) => write!(f, "invalid config line: {line}"),
            Self::InvalidNumericValue(value) => write!(f, "invalid numeric config value: {value}"),
            Self::InvalidReasoningVisibility(value) => {
                write!(f, "invalid reasoning visibility: {value}")
            }
        }
    }
}

impl Error for ConfigError {}

impl EffectiveConfig {
    pub fn load() -> Result<Self, ConfigError> {
        let cwd = std::env::current_dir().map_err(ConfigError::CurrentDirUnavailable)?;
        let workspace_dir = cwd.join("workspace");
        let config_file = cwd.join(".anvil").join("config");
        let mut config = Self::default_for_paths(cwd, workspace_dir, config_file);
        config.apply_standard_sources()?;
        Ok(config)
    }

    fn default_for_paths(cwd: PathBuf, workspace_dir: PathBuf, config_file: PathBuf) -> Self {
        let state_dir = cwd.join(".anvil").join("state");
        let session_dir = cwd.join(".anvil").join("sessions");
        let session_file = session_dir.join(format!("{}.json", session_key_for_cwd(&cwd)));
        Self {
            runtime: RuntimeConfig {
                provider: "ollama".to_string(),
                provider_url: "http://127.0.0.1:11434".to_string(),
                model: "local-default".to_string(),
                sidecar_model: None,
                api_key: None,
                context_window: 200_000,
                context_budget: None,
                stream: true,
            },
            mode: ModeConfig {
                interactive: true,
                approval_required: true,
                fresh_session: false,
                reasoning_visibility: ReasoningVisibility::Summary,
                debug_logging: false,
            },
            paths: PathConfig {
                cwd,
                workspace_dir,
                config_file,
                state_dir,
                session_dir,
                session_file,
            },
        }
    }

    fn apply_standard_sources(&mut self) -> Result<(), ConfigError> {
        if self.paths.config_file.exists() {
            self.apply_file_overrides()?;
        }
        self.apply_env_overrides()?;
        self.apply_cli_overrides()?;
        Ok(())
    }

    fn apply_file_overrides(&mut self) -> Result<(), ConfigError> {
        let contents = std::fs::read_to_string(&self.paths.config_file)
            .map_err(ConfigError::ConfigFileUnreadable)?;

        let mut map = HashMap::new();
        for raw_line in contents.lines() {
            let line = raw_line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                return Err(ConfigError::InvalidConfigLine(line.to_string()));
            };
            map.insert(
                key.trim().to_string(),
                value.trim().trim_matches('"').to_string(),
            );
        }

        self.apply_map(&map)
    }

    fn apply_env_overrides(&mut self) -> Result<(), ConfigError> {
        let mut map = HashMap::new();
        for key in [
            "ANVIL_PROVIDER",
            "ANVIL_MODEL",
            "ANVIL_PROVIDER_URL",
            "ANVIL_SIDECAR_MODEL",
            "ANVIL_API_KEY",
            "ANVIL_CONTEXT_WINDOW",
            "ANVIL_CONTEXT_BUDGET",
            "ANVIL_STREAM",
            "ANVIL_INTERACTIVE",
            "ANVIL_APPROVAL_REQUIRED",
            "ANVIL_FRESH_SESSION",
            "ANVIL_REASONING_VISIBILITY",
            "ANVIL_DEBUG",
        ] {
            if let Ok(value) = std::env::var(key) {
                map.insert(key.to_string(), value);
            }
        }
        self.apply_map(&map)
    }

    fn apply_cli_overrides(&mut self) -> Result<(), ConfigError> {
        let mut args = std::env::args().skip(1);
        let mut map = HashMap::new();

        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--provider" => {
                    if let Some(value) = args.next() {
                        map.insert("ANVIL_PROVIDER".to_string(), value);
                    }
                }
                "--model" => {
                    if let Some(value) = args.next() {
                        map.insert("ANVIL_MODEL".to_string(), value);
                    }
                }
                "--provider-url" => {
                    if let Some(value) = args.next() {
                        map.insert("ANVIL_PROVIDER_URL".to_string(), value);
                    }
                }
                "--sidecar-model" => {
                    if let Some(value) = args.next() {
                        map.insert("ANVIL_SIDECAR_MODEL".to_string(), value);
                    }
                }
                "--context-window" => {
                    if let Some(value) = args.next() {
                        map.insert("ANVIL_CONTEXT_WINDOW".to_string(), value);
                    }
                }
                "--context-budget" => {
                    if let Some(value) = args.next() {
                        map.insert("ANVIL_CONTEXT_BUDGET".to_string(), value);
                    }
                }
                "--no-stream" => {
                    map.insert("ANVIL_STREAM".to_string(), "false".to_string());
                }
                "--debug" => {
                    map.insert("ANVIL_DEBUG".to_string(), "true".to_string());
                }
                "--no-approval" => {
                    map.insert("ANVIL_APPROVAL_REQUIRED".to_string(), "false".to_string());
                }
                "--fresh-session" => {
                    map.insert("ANVIL_FRESH_SESSION".to_string(), "true".to_string());
                }
                "--oneshot" => {
                    map.insert("ANVIL_INTERACTIVE".to_string(), "false".to_string());
                }
                "--reasoning-visibility" => {
                    if let Some(value) = args.next() {
                        map.insert("ANVIL_REASONING_VISIBILITY".to_string(), value);
                    }
                }
                _ => {}
            }
        }

        self.apply_map(&map)
    }

    fn apply_map(&mut self, map: &HashMap<String, String>) -> Result<(), ConfigError> {
        for (key, value) in map {
            match key.as_str() {
                "provider" | "ANVIL_PROVIDER" => self.runtime.provider = value.clone(),
                "provider_url" | "ANVIL_PROVIDER_URL" => self.runtime.provider_url = value.clone(),
                "model" | "ANVIL_MODEL" => self.runtime.model = value.clone(),
                "sidecar_model" | "ANVIL_SIDECAR_MODEL" => {
                    self.runtime.sidecar_model = if value.is_empty() {
                        None
                    } else {
                        Some(value.clone())
                    };
                }
                "api_key" | "ANVIL_API_KEY" => {
                    self.runtime.api_key = if value.is_empty() {
                        None
                    } else {
                        Some(value.clone())
                    };
                }
                "context_window" | "ANVIL_CONTEXT_WINDOW" => {
                    self.runtime.context_window = value
                        .parse()
                        .map_err(|_| ConfigError::InvalidNumericValue(value.clone()))?;
                }
                "context_budget" | "ANVIL_CONTEXT_BUDGET" => {
                    self.runtime.context_budget = if value.is_empty() {
                        None
                    } else {
                        Some(
                            value
                                .parse()
                                .map_err(|_| ConfigError::InvalidNumericValue(value.clone()))?,
                        )
                    };
                }
                "stream" | "ANVIL_STREAM" => {
                    self.runtime.stream = parse_bool(value);
                }
                "interactive" | "ANVIL_INTERACTIVE" => {
                    self.mode.interactive = parse_bool(value);
                }
                "approval_required" | "ANVIL_APPROVAL_REQUIRED" => {
                    self.mode.approval_required = parse_bool(value);
                }
                "fresh_session" | "ANVIL_FRESH_SESSION" => {
                    self.mode.fresh_session = parse_bool(value);
                }
                "debug" | "ANVIL_DEBUG" => {
                    self.mode.debug_logging = parse_bool(value);
                }
                "reasoning_visibility" | "ANVIL_REASONING_VISIBILITY" => {
                    self.mode.reasoning_visibility = parse_reasoning_visibility(value)?;
                }
                _ => {}
            }
        }
        Ok(())
    }

    pub fn apply_overrides_for_test(
        &mut self,
        file_values: &HashMap<String, String>,
        env_values: &HashMap<String, String>,
        cli_values: &HashMap<String, String>,
    ) -> Result<(), ConfigError> {
        self.apply_map(file_values)?;
        self.apply_map(env_values)?;
        self.apply_map(cli_values)?;
        Ok(())
    }

    pub fn default_for_test() -> Result<Self, ConfigError> {
        let cwd = std::env::current_dir().map_err(ConfigError::CurrentDirUnavailable)?;
        Ok(Self::default_for_paths(
            cwd.clone(),
            cwd.join("workspace"),
            cwd.join(".anvil").join("config"),
        ))
    }
}

fn session_key_for_cwd(cwd: &std::path::Path) -> String {
    let mut hasher = DefaultHasher::new();
    cwd.hash(&mut hasher);
    format!("session_{:x}", hasher.finish())
}

fn parse_bool(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

fn parse_reasoning_visibility(value: &str) -> Result<ReasoningVisibility, ConfigError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "hidden" => Ok(ReasoningVisibility::Hidden),
        "summary" => Ok(ReasoningVisibility::Summary),
        other => Err(ConfigError::InvalidReasoningVisibility(other.to_string())),
    }
}
