//! Configuration loading with file / environment / CLI precedence.
//!
//! [`EffectiveConfig`] is the single source of truth for all runtime
//! settings.  It is assembled once at startup and then treated as
//! immutable for the lifetime of the session.

pub mod cli_args;
pub mod custom_tools;
pub use cli_args::CliArgs;
pub use custom_tools::{
    CUSTOM_TOOL_PREFIX, CustomToolDef, MAX_CUSTOM_TOOLS, custom_tool_display_name,
    expand_command_template, json_value_to_params, parse_tools_section, shell_escape,
    strip_custom_prefix,
};

use crate::provider::transport::{DEFAULT_HTTP_TIMEOUT_SECS, normalize_http_timeout};
use clap::Parser;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::{
    error::Error,
    fmt::{Display, Formatter},
};

// ---------------------------------------------------------------------------
// Language constants and helpers (Issue #162)
// ---------------------------------------------------------------------------

/// Default UI language fallback.
pub const DEFAULT_UI_LANGUAGE: &str = "ja";

/// Supported languages: (code, display_name).
pub const SUPPORTED_LANGUAGES: &[(&str, &str)] = &[("ja", "Japanese"), ("en", "English")];

/// Look up display name for a language code using SUPPORTED_LANGUAGES table.
/// Returns the display name, or DEFAULT_UI_LANGUAGE's display name as fallback.
pub fn lang_display_name(code: &str) -> &'static str {
    SUPPORTED_LANGUAGES
        .iter()
        .find(|(c, _)| *c == code)
        .map(|(_, name)| *name)
        .unwrap_or("Japanese")
}

/// Return the effective UI language code.
/// `None` or unsupported values fall back to [`DEFAULT_UI_LANGUAGE`].
pub fn effective_ui_language_code(configured: Option<&str>) -> &str {
    configured.unwrap_or(DEFAULT_UI_LANGUAGE)
}

/// Shared language constraint prompt for main/sub-agent.
pub fn language_constraint_prompt(ui_language: &str) -> String {
    let lang_name = lang_display_name(ui_language);
    format!(
        "\n\n## Language constraint\nYou MUST respond in {lang_name}. \
         All explanations, comments, plans, and summaries must be written in {lang_name}. \
         Do NOT switch to any other language during your response."
    )
}

/// Determines where the user prompt originates from.
#[derive(Debug, Clone)]
pub enum PromptSource {
    /// Interactive REPL mode (default).
    Interactive,
    /// Read prompt from stdin (--oneshot).
    Stdin,
    /// Prompt provided directly via --exec flag.
    Exec(String),
    /// Prompt read from a file via --exec-file flag.
    ExecFile(PathBuf),
}

impl PromptSource {
    /// Returns true if this source represents an interactive session.
    pub fn is_interactive(&self) -> bool {
        matches!(self, PromptSource::Interactive)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WebSearchProvider {
    #[default]
    DuckDuckGo,
    SerperApi,
}

#[derive(Clone)]
/// Provider, model, and transport settings.
pub struct RuntimeConfig {
    pub provider: String,
    pub provider_url: String,
    pub model: String,
    pub sidecar_model: Option<String>,
    pub api_key: Option<String>,
    pub context_window: u32,
    pub context_budget: Option<u32>,
    pub max_agent_iterations: usize,
    pub max_console_messages: usize,
    pub auto_compact_threshold: usize,
    pub tool_result_max_chars: usize,
    pub stream: bool,
    pub web_search_provider: WebSearchProvider,
    pub serper_api_key: Option<String>,
    /// `true` when the user explicitly set `context_window` via config file,
    /// environment variable, or CLI flag.  When `false`, the auto-detect
    /// logic in `cli.rs` may override `context_window` from the provider.
    pub context_window_explicitly_set: bool,
    /// Tag-based tool protocol mode override.
    /// `Some(true)` = force tag-based, `Some(false)` = force JSON, `None` = auto-detect from model name.
    pub tag_protocol: Option<bool>,
    /// Prompt tier override: "full", "compact", or "tiny".
    /// `None` = auto-detect from model name.
    pub prompt_tier: Option<String>,
    /// Smart compact threshold ratio (0.1..=0.95, default 0.75).
    /// When estimated tokens exceed context_window * ratio, token-based compaction triggers.
    pub smart_compact_threshold_ratio: f64,
    /// Maximum number of LLM turns a sub-agent may perform (Issue #129).
    pub subagent_max_iterations: u32,
    /// Wall-clock timeout in seconds for the entire sub-agent run (Issue #129).
    pub subagent_timeout_secs: u64,
    /// Loop detection threshold: number of identical tool calls before detection triggers (Issue #145).
    pub loop_detection_threshold: usize,
    /// HTTP request timeout in seconds (Issue #146).
    pub http_timeout_secs: u64,
    /// Phase estimator: consecutive read calls to enter "exploring" phase (Issue #159).
    pub phase_explore_threshold: usize,
    /// Phase estimator: consecutive read calls to force transition to implementation (Issue #159).
    pub phase_force_transition_threshold: usize,
    /// Phase estimator: consecutive reads after last write for fallback completion (Issue #159).
    pub phase_completion_read_threshold: usize,
    /// Edit/write fallback strategy: edit-first or write-first (Issue #158).
    pub edit_strategy: crate::app::edit_fail_tracker::EditStrategy,
    /// Consecutive edit failures before re-read hint (Issue #158).
    pub edit_reread_threshold: u32,
    /// Consecutive edit failures before write fallback hint (Issue #158).
    pub edit_write_fallback_threshold: u32,
    /// Maximum line count for file.write on existing files (0 = disabled, Issue #156).
    pub safe_write_max_lines: usize,
    /// Deletion ratio threshold for diff warning (0.0-1.0, Issue #156).
    pub safe_write_deletion_ratio: f64,
    /// ReadRepeatTracker: per-path read count to trigger warn hint (Issue #187).
    pub read_repeat_warn_threshold: u32,
    /// ReadRepeatTracker: per-path read count to trigger strong-warn hint (Issue #187).
    pub read_repeat_strong_warn_threshold: u32,
    /// UI language for LLM responses (Issue #162).
    /// `None` means "use default language via effective_ui_language_code()".
    /// Supported: "ja", "en".
    pub ui_language: Option<String>,
    /// Maximum total tool calls per agentic turn (Issue #172).
    pub max_tool_calls: usize,
}

#[derive(Debug, Clone)]
pub struct ModeConfig {
    pub prompt_source: PromptSource,
    pub interactive: bool,
    pub approval_required: bool,
    pub fresh_session: bool,
    pub reasoning_visibility: ReasoningVisibility,
    pub debug_logging: bool,
    pub log_filter: Option<String>,
    pub offline: bool,
    /// Trust mode: auto-approve built-in tool execution.
    /// Set only via `--trust` CLI flag (not from config file deserialization).
    pub trust_all: bool,
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
    pub logs_dir: PathBuf,
    pub mcp_config_file: PathBuf,
    pub hooks_config_file: PathBuf,
}

#[derive(Debug, Clone)]
pub struct EffectiveConfig {
    pub runtime: RuntimeConfig,
    pub mode: ModeConfig,
    pub paths: PathConfig,
    project_instructions: Option<String>,
    custom_tools: Vec<CustomToolDef>,
}

#[derive(Debug)]
pub enum ConfigError {
    CurrentDirUnavailable(std::io::Error),
    ConfigFileUnreadable(std::io::Error),
    InvalidConfigLine(String),
    InvalidNumericValue(String),
    InvalidReasoningVisibility(String),
    InvalidWebSearchProvider(String),
    ValidationError(String),
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
            Self::InvalidWebSearchProvider(value) => {
                write!(f, "invalid web search provider: {value}")
            }
            Self::ValidationError(msg) => write!(f, "config validation failed: {msg}"),
        }
    }
}

impl Error for ConfigError {}

impl EffectiveConfig {
    pub fn project_instructions(&self) -> Option<&str> {
        self.project_instructions.as_deref()
    }

    /// Custom tool definitions parsed from ANVIL.md `## tools` section.
    pub fn custom_tools(&self) -> &[CustomToolDef] {
        &self.custom_tools
    }

    /// Test-only setter for project_instructions.
    /// In production code, this field is set only via `load()`.
    pub fn set_project_instructions_for_test(&mut self, instructions: Option<String>) {
        self.project_instructions = instructions;
    }

    /// Test-compatible entry point. In production, use `load_with_args()` via
    /// `main.rs -> CliArgs::parse() -> run_with_args() -> load_with_args()`.
    ///
    /// Attempts to parse CLI args from `std::env::args()`. Falls back to
    /// `CliArgs::default()` when parsing fails (e.g. cargo-test harness args).
    pub fn load() -> Result<Self, ConfigError> {
        match CliArgs::try_parse_from(std::env::args()) {
            Ok(mut cli) => {
                cli.resolve_tag_protocol();
                Self::load_with_args(&cli)
            }
            Err(_) => Self::load_with_args(&CliArgs::default()),
        }
    }

    /// Production entry point: apply file, env, then CLI arg overrides.
    pub fn load_with_args(cli: &CliArgs) -> Result<Self, ConfigError> {
        let cwd = std::env::current_dir().map_err(ConfigError::CurrentDirUnavailable)?;
        let workspace_dir = cwd.join("workspace");
        let config_file = cwd.join(".anvil").join("config");
        let mut config = Self::default_for_paths(cwd, workspace_dir, config_file);
        config.apply_file_and_env_overrides()?;
        config.apply_cli_args(cli)?;

        // Check .gitignore for .anvil/ directory
        if let Some(repo_root) = find_repo_root(&config.paths.cwd)
            && let Some(warning) = check_gitignore_anvil_dir(&repo_root)
        {
            eprintln!("{warning}");
        }

        config.validate()?;
        let (instructions, custom_tools) = config.paths.load_project_instructions();
        config.project_instructions = instructions;
        config.custom_tools = custom_tools;
        Ok(config)
    }

    fn default_for_paths(cwd: PathBuf, workspace_dir: PathBuf, config_file: PathBuf) -> Self {
        let state_dir = cwd.join(".anvil").join("state");
        let session_dir = cwd.join(".anvil").join("sessions");
        let session_file = session_dir.join("default.json");
        let logs_dir = cwd.join(".anvil").join("logs");
        Self {
            runtime: RuntimeConfig {
                provider: "ollama".to_string(),
                provider_url: "http://127.0.0.1:11434".to_string(),
                model: "local-default".to_string(),
                sidecar_model: None,
                api_key: None,
                context_window: 200_000,
                context_budget: None,
                max_agent_iterations: DEFAULT_MAX_AGENT_ITERATIONS,
                max_console_messages: 5,
                auto_compact_threshold: 64,
                tool_result_max_chars: 8000,
                stream: true,
                web_search_provider: WebSearchProvider::default(),
                serper_api_key: None,
                context_window_explicitly_set: false,
                tag_protocol: None,
                prompt_tier: None,
                smart_compact_threshold_ratio: 0.75,
                subagent_max_iterations: DEFAULT_SUBAGENT_MAX_ITERATIONS,
                subagent_timeout_secs: DEFAULT_SUBAGENT_TIMEOUT_SECS,
                loop_detection_threshold: 3,
                http_timeout_secs: DEFAULT_HTTP_TIMEOUT_SECS,
                phase_explore_threshold: 5,
                phase_force_transition_threshold: 15,
                phase_completion_read_threshold: 5,
                edit_strategy: crate::app::edit_fail_tracker::EditStrategy::EditFirst,
                edit_reread_threshold: 3,
                edit_write_fallback_threshold: 5,
                safe_write_max_lines: 500,
                safe_write_deletion_ratio: 0.5,
                read_repeat_warn_threshold: 3,
                read_repeat_strong_warn_threshold: 6,
                ui_language: None,
                max_tool_calls: DEFAULT_MAX_TOOL_CALLS,
            },
            mode: ModeConfig {
                prompt_source: PromptSource::Interactive,
                interactive: true,
                approval_required: true,
                fresh_session: false,
                reasoning_visibility: ReasoningVisibility::Summary,
                debug_logging: false,
                log_filter: None,
                offline: false,
                trust_all: false,
            },
            paths: PathConfig {
                mcp_config_file: cwd.join(".anvil").join("mcp.json"),
                hooks_config_file: cwd.join(".anvil").join("hooks.json"),
                cwd,
                workspace_dir,
                config_file,
                state_dir,
                session_dir,
                session_file,
                logs_dir,
            },
            project_instructions: None,
            custom_tools: Vec::new(),
        }
    }

    /// Set `context_window` and mark it as explicitly set by the user.
    fn set_context_window(&mut self, value: u32) {
        self.runtime.context_window = value;
        self.runtime.context_window_explicitly_set = true;
    }

    fn apply_file_and_env_overrides(&mut self) -> Result<(), ConfigError> {
        if self.paths.config_file.exists() {
            self.apply_file_overrides()?;
        }
        self.apply_env_overrides()?;
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

        // Security warnings for API keys in config file
        for warning in check_config_security_warnings(&map) {
            eprintln!("{warning}");
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
            "ANVIL_MAX_AGENT_ITERATIONS",
            "ANVIL_MAX_CONSOLE_MESSAGES",
            "ANVIL_AUTO_COMPACT_THRESHOLD",
            "ANVIL_TOOL_RESULT_MAX_CHARS",
            "ANVIL_STREAM",
            "ANVIL_INTERACTIVE",
            "ANVIL_APPROVAL_REQUIRED",
            "ANVIL_FRESH_SESSION",
            "ANVIL_REASONING_VISIBILITY",
            "ANVIL_DEBUG",
            "ANVIL_WEB_SEARCH_PROVIDER",
            "SERPER_API_KEY",
            "ANVIL_LOG",
            "ANVIL_OFFLINE",
            "ANVIL_TAG_PROTOCOL",
            "ANVIL_PROMPT_TIER",
            "ANVIL_SMART_COMPACT_THRESHOLD_RATIO",
            "ANVIL_SUBAGENT_MAX_ITERATIONS",
            "ANVIL_SUBAGENT_TIMEOUT",
            "ANVIL_LOOP_DETECTION_THRESHOLD",
            "ANVIL_HTTP_TIMEOUT",
            "ANVIL_CURL_TIMEOUT",
            "ANVIL_EDIT_STRATEGY",
            "ANVIL_EDIT_REREAD_THRESHOLD",
            "ANVIL_EDIT_WRITE_FALLBACK_THRESHOLD",
            "ANVIL_SAFE_WRITE_MAX_LINES",
            "ANVIL_SAFE_WRITE_DELETION_RATIO",
            "ANVIL_UI_LANGUAGE",
            "ANVIL_MAX_TOOL_CALLS",
        ] {
            if let Ok(value) = std::env::var(key) {
                map.insert(key.to_string(), value);
            }
        }
        self.apply_map(&map)
    }

    /// Apply CLI argument overrides from a parsed [`CliArgs`] struct.
    ///
    /// Uses direct field assignment (not `apply_map`) to avoid redundant
    /// string round-trips for already-typed values.
    pub fn apply_cli_args(&mut self, cli: &CliArgs) -> Result<(), ConfigError> {
        // Determine PromptSource from CLI args
        if let Some(ref prompt) = cli.exec {
            self.mode.prompt_source = PromptSource::Exec(prompt.clone());
        } else if let Some(ref path) = cli.exec_file {
            self.mode.prompt_source = PromptSource::ExecFile(path.clone());
        } else if cli.oneshot {
            self.mode.prompt_source = PromptSource::Stdin;
        }

        // Derive interactive / approval_required / fresh_session for non-interactive modes
        if !self.mode.prompt_source.is_interactive() {
            self.mode.interactive = false;
            self.mode.approval_required = false;
            self.mode.fresh_session = true;
        }

        // String fields
        if let Some(ref v) = cli.provider {
            self.runtime.provider = v.clone();
        }
        if let Some(ref v) = cli.model {
            self.runtime.model = v.clone();
        }
        if let Some(ref v) = cli.provider_url {
            self.runtime.provider_url = v.clone();
        }
        if let Some(ref v) = cli.sidecar_model {
            self.runtime.sidecar_model = Some(v.clone());
        }

        // Numeric fields (already parsed by clap)
        if let Some(v) = cli.context_window {
            self.set_context_window(v);
        }
        if let Some(v) = cli.context_budget {
            self.runtime.context_budget = Some(v);
        }
        if let Some(v) = cli.max_iterations {
            self.runtime.max_agent_iterations = v;
        }

        // Boolean flags: only apply when set (true)
        if cli.no_stream {
            self.runtime.stream = false;
        }
        if cli.debug {
            self.mode.debug_logging = true;
        }
        if cli.no_approval {
            self.mode.approval_required = false;
        }
        if cli.fresh_session {
            self.mode.fresh_session = true;
        }
        if cli.oneshot {
            self.mode.interactive = false;
        }

        // Enum field
        if let Some(ref v) = cli.reasoning_visibility {
            self.mode.reasoning_visibility = parse_reasoning_visibility(v)?;
        }

        if cli.offline {
            self.mode.offline = true;
        }
        if cli.trust {
            self.mode.trust_all = true;
        }

        // Tag protocol flag
        if let Some(v) = cli.tag_protocol {
            self.runtime.tag_protocol = Some(v);
        }

        // HTTP timeout
        if let Some(timeout) = cli.timeout {
            self.runtime.http_timeout_secs = timeout;
        }

        // Prompt tier override
        if let Some(ref v) = cli.prompt_tier {
            self.runtime.prompt_tier = Some(v.clone());
        }

        // Edit strategy override (Issue #158)
        if let Some(ref v) = cli.edit_strategy {
            self.runtime.edit_strategy = v
                .parse::<crate::app::edit_fail_tracker::EditStrategy>()
                .map_err(ConfigError::ValidationError)?;
        }

        // Safe write settings
        if let Some(v) = cli.safe_write_max_lines {
            self.runtime.safe_write_max_lines = v;
        }
        if let Some(v) = cli.safe_write_deletion_ratio {
            if !is_valid_deletion_ratio(v) {
                return Err(ConfigError::InvalidNumericValue(format!(
                    "safe_write_deletion_ratio must be between 0.0 and 1.0, got {v}"
                )));
            }
            self.runtime.safe_write_deletion_ratio = v;
        }

        // Max tool calls (Issue #172)
        if let Some(v) = cli.max_tool_calls {
            self.runtime.max_tool_calls = v;
        }

        // --session flag: override session file path
        if let Some(ref name) = cli.session {
            crate::session::validate_session_name(name)
                .map_err(|e| ConfigError::ValidationError(e.to_string()))?;
            self.paths.session_file = self.paths.session_dir.join(format!("{name}.json"));
        }

        Ok(())
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
                    let v: u32 = value
                        .parse()
                        .map_err(|_| ConfigError::InvalidNumericValue(value.clone()))?;
                    self.set_context_window(v);
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
                "max_agent_iterations" | "ANVIL_MAX_AGENT_ITERATIONS" => {
                    self.runtime.max_agent_iterations = value
                        .parse()
                        .map_err(|_| ConfigError::InvalidNumericValue(value.clone()))?;
                }
                "max_console_messages" | "ANVIL_MAX_CONSOLE_MESSAGES" => {
                    self.runtime.max_console_messages = value
                        .parse()
                        .map_err(|_| ConfigError::InvalidNumericValue(value.clone()))?;
                }
                "auto_compact_threshold" | "ANVIL_AUTO_COMPACT_THRESHOLD" => {
                    self.runtime.auto_compact_threshold = value
                        .parse()
                        .map_err(|_| ConfigError::InvalidNumericValue(value.clone()))?;
                }
                "tool_result_max_chars" | "ANVIL_TOOL_RESULT_MAX_CHARS" => {
                    self.runtime.tool_result_max_chars = value
                        .parse()
                        .map_err(|_| ConfigError::InvalidNumericValue(value.clone()))?;
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
                "web_search_provider" | "ANVIL_WEB_SEARCH_PROVIDER" => {
                    self.runtime.web_search_provider = parse_web_search_provider(value)?;
                }
                "serper_api_key" | "SERPER_API_KEY" => {
                    self.runtime.serper_api_key = if value.is_empty() {
                        None
                    } else {
                        Some(value.clone())
                    };
                }
                "log_filter" | "ANVIL_LOG" => {
                    self.mode.log_filter = Some(value.clone());
                }
                "offline" | "ANVIL_OFFLINE" => {
                    self.mode.offline = parse_bool(value);
                }
                "tag_protocol" | "ANVIL_TAG_PROTOCOL" => {
                    self.runtime.tag_protocol = Some(parse_bool(value));
                }
                "prompt_tier" | "ANVIL_PROMPT_TIER" => {
                    self.runtime.prompt_tier = if value.is_empty() {
                        None
                    } else {
                        Some(value.clone())
                    };
                }
                "smart_compact_threshold_ratio" | "ANVIL_SMART_COMPACT_THRESHOLD_RATIO" => {
                    self.runtime.smart_compact_threshold_ratio = value
                        .parse()
                        .map_err(|_| ConfigError::InvalidNumericValue(value.clone()))?;
                }
                "subagent_max_iterations" | "ANVIL_SUBAGENT_MAX_ITERATIONS" => {
                    self.runtime.subagent_max_iterations = value
                        .parse()
                        .map_err(|_| ConfigError::InvalidNumericValue(value.clone()))?;
                }
                "subagent_timeout_secs" | "ANVIL_SUBAGENT_TIMEOUT" => {
                    self.runtime.subagent_timeout_secs = value
                        .parse()
                        .map_err(|_| ConfigError::InvalidNumericValue(value.clone()))?;
                }
                "loop_detection_threshold" | "ANVIL_LOOP_DETECTION_THRESHOLD" => {
                    let v: usize = value
                        .parse()
                        .map_err(|_| ConfigError::InvalidNumericValue(value.clone()))?;
                    if !(2..=20).contains(&v) {
                        return Err(ConfigError::InvalidNumericValue(value.clone()));
                    }
                    self.runtime.loop_detection_threshold = v;
                }
                "http_timeout_secs" | "ANVIL_HTTP_TIMEOUT" | "ANVIL_CURL_TIMEOUT" => {
                    self.runtime.http_timeout_secs = value
                        .parse()
                        .map_err(|_| ConfigError::InvalidNumericValue(value.clone()))?;
                }
                "phase_explore_threshold" | "ANVIL_PHASE_EXPLORE_THRESHOLD" => {
                    let v: usize = value
                        .parse()
                        .map_err(|_| ConfigError::InvalidNumericValue(value.clone()))?;
                    if !(2..=20).contains(&v) {
                        return Err(ConfigError::InvalidNumericValue(value.clone()));
                    }
                    self.runtime.phase_explore_threshold = v;
                }
                "phase_force_transition_threshold" | "ANVIL_PHASE_FORCE_TRANSITION_THRESHOLD" => {
                    let v: usize = value
                        .parse()
                        .map_err(|_| ConfigError::InvalidNumericValue(value.clone()))?;
                    if !(3..=30).contains(&v) {
                        return Err(ConfigError::InvalidNumericValue(value.clone()));
                    }
                    self.runtime.phase_force_transition_threshold = v;
                }
                "phase_completion_read_threshold" | "ANVIL_PHASE_COMPLETION_READ_THRESHOLD" => {
                    let v: usize = value
                        .parse()
                        .map_err(|_| ConfigError::InvalidNumericValue(value.clone()))?;
                    if !(2..=20).contains(&v) {
                        return Err(ConfigError::InvalidNumericValue(value.clone()));
                    }
                    self.runtime.phase_completion_read_threshold = v;
                }
                "edit_strategy" | "ANVIL_EDIT_STRATEGY" => {
                    self.runtime.edit_strategy = value
                        .parse::<crate::app::edit_fail_tracker::EditStrategy>()
                        .map_err(ConfigError::ValidationError)?;
                }
                "edit_reread_threshold" | "ANVIL_EDIT_REREAD_THRESHOLD" => {
                    let v: u32 = value
                        .parse()
                        .map_err(|_| ConfigError::InvalidNumericValue(value.clone()))?;
                    if !(1..=20).contains(&v) {
                        return Err(ConfigError::InvalidNumericValue(value.clone()));
                    }
                    self.runtime.edit_reread_threshold = v;
                }
                "edit_write_fallback_threshold" | "ANVIL_EDIT_WRITE_FALLBACK_THRESHOLD" => {
                    let v: u32 = value
                        .parse()
                        .map_err(|_| ConfigError::InvalidNumericValue(value.clone()))?;
                    if !(1..=20).contains(&v) {
                        return Err(ConfigError::InvalidNumericValue(value.clone()));
                    }
                    self.runtime.edit_write_fallback_threshold = v;
                }
                "read_repeat_warn_threshold" | "ANVIL_READ_REPEAT_WARN_THRESHOLD" => {
                    let v: u32 = value
                        .parse()
                        .map_err(|_| ConfigError::InvalidNumericValue(value.clone()))?;
                    if !(1..=20).contains(&v) {
                        return Err(ConfigError::InvalidNumericValue(value.clone()));
                    }
                    self.runtime.read_repeat_warn_threshold = v;
                }
                "read_repeat_strong_warn_threshold" | "ANVIL_READ_REPEAT_STRONG_WARN_THRESHOLD" => {
                    let v: u32 = value
                        .parse()
                        .map_err(|_| ConfigError::InvalidNumericValue(value.clone()))?;
                    if !(2..=40).contains(&v) {
                        return Err(ConfigError::InvalidNumericValue(value.clone()));
                    }
                    self.runtime.read_repeat_strong_warn_threshold = v;
                }
                "safe_write_max_lines" | "ANVIL_SAFE_WRITE_MAX_LINES" => {
                    self.runtime.safe_write_max_lines = value
                        .parse()
                        .map_err(|_| ConfigError::InvalidNumericValue(value.clone()))?;
                }
                "safe_write_deletion_ratio" | "ANVIL_SAFE_WRITE_DELETION_RATIO" => {
                    let v: f64 = value
                        .parse()
                        .map_err(|_| ConfigError::InvalidNumericValue(value.clone()))?;
                    if !is_valid_deletion_ratio(v) {
                        return Err(ConfigError::InvalidNumericValue(value.clone()));
                    }
                    self.runtime.safe_write_deletion_ratio = v;
                }
                "ui_language" | "ANVIL_UI_LANGUAGE" => {
                    self.runtime.ui_language = if value.is_empty() {
                        None
                    } else {
                        Some(value.clone())
                    };
                }
                "max_tool_calls" | "ANVIL_MAX_TOOL_CALLS" => {
                    self.runtime.max_tool_calls = value
                        .parse()
                        .map_err(|_| ConfigError::InvalidNumericValue(value.clone()))?;
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

    fn validate(&mut self) -> Result<(), ConfigError> {
        self.check_provider_url()?;
        self.check_model()?;
        self.clamp_context_window();
        self.clamp_context_budget();
        self.clamp_agent_iterations();
        self.clamp_smart_compact_ratio();
        self.clamp_subagent_settings();
        self.clamp_loop_detection_threshold();
        self.clamp_phase_thresholds();
        self.clamp_edit_thresholds();
        self.clamp_read_repeat_thresholds();
        self.clamp_http_timeout();
        self.clamp_max_tool_calls();
        self.sanitize_ui_language();
        Ok(())
    }

    /// Validate `ui_language`: keep `None` and supported values, reset invalid to `None`.
    fn sanitize_ui_language(&mut self) {
        let is_supported = |code: &str| SUPPORTED_LANGUAGES.iter().any(|(c, _)| *c == code);
        match self.runtime.ui_language.as_deref() {
            None => {}
            Some(code) if is_supported(code) => {}
            Some(invalid) => {
                eprintln!(
                    "Warning: ui_language=\"{}\" is not supported, falling back to \"{}\"",
                    invalid.escape_debug(),
                    DEFAULT_UI_LANGUAGE
                );
                self.runtime.ui_language = None;
            }
        }
    }

    fn clamp_http_timeout(&mut self) {
        let old = self.runtime.http_timeout_secs;
        let normalized = normalize_http_timeout(old);
        if normalized != old {
            self.runtime.http_timeout_secs = normalized;
            eprintln!("Warning: http_timeout_secs={old} adjusted to {normalized}");
        }
    }

    /// Clamp max_tool_calls to [1, MAX_TOOL_CALLS_LIMIT] (Issue #172).
    fn clamp_max_tool_calls(&mut self) {
        let old = self.runtime.max_tool_calls;
        self.runtime.max_tool_calls = old.clamp(1, MAX_TOOL_CALLS_LIMIT);
        if self.runtime.max_tool_calls != old {
            eprintln!(
                "Warning: max_tool_calls={old} adjusted to {}",
                self.runtime.max_tool_calls
            );
        }
    }

    /// Clamp smart_compact_threshold_ratio to [0.1, 0.95].
    /// NaN/Infinity are not handled correctly by clamp(), so check is_finite() first
    /// and fall back to the default value (0.75) if the value is non-finite.
    pub fn clamp_smart_compact_ratio(&mut self) {
        if !self.runtime.smart_compact_threshold_ratio.is_finite() {
            self.runtime.smart_compact_threshold_ratio = 0.75;
        }
        self.runtime.smart_compact_threshold_ratio =
            self.runtime.smart_compact_threshold_ratio.clamp(0.1, 0.95);
    }

    fn check_provider_url(&self) -> Result<(), ConfigError> {
        if self.runtime.provider_url.is_empty() {
            return Err(ConfigError::ValidationError(
                "provider_url must not be empty — set it in .anvil/config or ANVIL_PROVIDER_URL"
                    .to_string(),
            ));
        }
        if !self.runtime.provider_url.starts_with("http://")
            && !self.runtime.provider_url.starts_with("https://")
        {
            return Err(ConfigError::ValidationError(format!(
                "provider_url must start with http:// or https://, got: {} — fix in .anvil/config or ANVIL_PROVIDER_URL",
                self.runtime.provider_url
            )));
        }
        Ok(())
    }

    fn check_model(&self) -> Result<(), ConfigError> {
        if self.runtime.model.is_empty() {
            return Err(ConfigError::ValidationError(
                "model must not be empty — set it in .anvil/config or ANVIL_MODEL".to_string(),
            ));
        }
        Ok(())
    }

    pub(crate) fn clamp_context_window(&mut self) {
        if self.runtime.context_window < MIN_CONTEXT_WINDOW {
            let old = self.runtime.context_window;
            self.runtime.context_window = MIN_CONTEXT_WINDOW;
            eprintln!(
                "Warning: context_window={old} is below minimum ({MIN_CONTEXT_WINDOW}), adjusted to {MIN_CONTEXT_WINDOW}"
            );
        }
    }

    pub(crate) fn clamp_context_budget(&mut self) {
        if let Some(budget) = self.runtime.context_budget
            && budget >= self.runtime.context_window
        {
            let old = budget;
            let new = self.runtime.context_window.saturating_sub(1);
            self.runtime.context_budget = Some(new);
            eprintln!(
                "Warning: context_budget={old} >= context_window ({}), adjusted to {new}",
                self.runtime.context_window
            );
        }
    }

    fn clamp_agent_iterations(&mut self) {
        if self.runtime.max_agent_iterations < MIN_AGENT_ITERATIONS {
            let old = self.runtime.max_agent_iterations;
            self.runtime.max_agent_iterations = MIN_AGENT_ITERATIONS;
            eprintln!(
                "Warning: max_agent_iterations={old} is below minimum ({MIN_AGENT_ITERATIONS}), adjusted to {MIN_AGENT_ITERATIONS}"
            );
        } else if self.runtime.max_agent_iterations > MAX_AGENT_ITERATIONS {
            let old = self.runtime.max_agent_iterations;
            self.runtime.max_agent_iterations = MAX_AGENT_ITERATIONS;
            eprintln!(
                "Warning: max_agent_iterations={old} exceeds maximum ({MAX_AGENT_ITERATIONS}), adjusted to {MAX_AGENT_ITERATIONS}"
            );
        }
    }

    /// Clamp sub-agent iteration/timeout settings.
    /// If 0, restore to default values. Enforce upper bounds.
    fn clamp_subagent_settings(&mut self) {
        if self.runtime.subagent_max_iterations == 0 {
            self.runtime.subagent_max_iterations = DEFAULT_SUBAGENT_MAX_ITERATIONS;
        } else if self.runtime.subagent_max_iterations > MAX_SUBAGENT_ITERATIONS {
            let old = self.runtime.subagent_max_iterations;
            self.runtime.subagent_max_iterations = MAX_SUBAGENT_ITERATIONS;
            eprintln!(
                "Warning: subagent_max_iterations={old} exceeds maximum ({MAX_SUBAGENT_ITERATIONS}), adjusted to {MAX_SUBAGENT_ITERATIONS}"
            );
        }
        if self.runtime.subagent_timeout_secs == 0 {
            self.runtime.subagent_timeout_secs = DEFAULT_SUBAGENT_TIMEOUT_SECS;
        } else if self.runtime.subagent_timeout_secs > MAX_SUBAGENT_TIMEOUT_SECS {
            let old = self.runtime.subagent_timeout_secs;
            self.runtime.subagent_timeout_secs = MAX_SUBAGENT_TIMEOUT_SECS;
            eprintln!(
                "Warning: subagent_timeout_secs={old} exceeds maximum ({MAX_SUBAGENT_TIMEOUT_SECS}), adjusted to {MAX_SUBAGENT_TIMEOUT_SECS}"
            );
        }
    }

    fn clamp_loop_detection_threshold(&mut self) {
        self.runtime.loop_detection_threshold = self.runtime.loop_detection_threshold.clamp(2, 20);
    }

    /// Clamp phase estimator thresholds and enforce N < M constraint (Issue #159).
    fn clamp_phase_thresholds(&mut self) {
        self.runtime.phase_explore_threshold = self.runtime.phase_explore_threshold.clamp(2, 20);
        self.runtime.phase_force_transition_threshold =
            self.runtime.phase_force_transition_threshold.clamp(3, 30);
        self.runtime.phase_completion_read_threshold =
            self.runtime.phase_completion_read_threshold.clamp(2, 20);
        // Enforce N < M: if explore >= force_transition, adjust force_transition.
        if self.runtime.phase_explore_threshold >= self.runtime.phase_force_transition_threshold {
            self.runtime.phase_force_transition_threshold =
                (self.runtime.phase_explore_threshold + 5).min(30);
        }
    }

    /// Clamp read-repeat tracker thresholds and enforce warn < strong_warn (Issue #187).
    fn clamp_read_repeat_thresholds(&mut self) {
        self.runtime.read_repeat_warn_threshold =
            self.runtime.read_repeat_warn_threshold.clamp(1, 20);
        self.runtime.read_repeat_strong_warn_threshold =
            self.runtime.read_repeat_strong_warn_threshold.clamp(2, 40);
        // Enforce warn < strong_warn
        if self.runtime.read_repeat_warn_threshold >= self.runtime.read_repeat_strong_warn_threshold
        {
            self.runtime.read_repeat_strong_warn_threshold =
                self.runtime.read_repeat_warn_threshold + 3;
        }
    }

    fn clamp_edit_thresholds(&mut self) {
        self.runtime.edit_reread_threshold = self.runtime.edit_reread_threshold.clamp(1, 20);
        self.runtime.edit_write_fallback_threshold =
            self.runtime.edit_write_fallback_threshold.clamp(1, 20);
        // Ensure write_fallback > reread
        if self.runtime.edit_write_fallback_threshold <= self.runtime.edit_reread_threshold {
            self.runtime.edit_write_fallback_threshold = self.runtime.edit_reread_threshold + 2;
        }
    }

    pub fn validate_for_test(&mut self) -> Result<(), ConfigError> {
        self.validate()
    }

    pub fn session_key(&self) -> &str {
        self.paths
            .session_file
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown")
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

const MAX_PROJECT_INSTRUCTIONS_CHARS: usize = 4000;
const MIN_CONTEXT_WINDOW: u32 = 1000;
const DEFAULT_MAX_AGENT_ITERATIONS: usize = 30;
const DEFAULT_SUBAGENT_MAX_ITERATIONS: u32 = 20;
const DEFAULT_SUBAGENT_TIMEOUT_SECS: u64 = 120;
const MAX_SUBAGENT_ITERATIONS: u32 = 100;
const MAX_SUBAGENT_TIMEOUT_SECS: u64 = 3600;
const MIN_AGENT_ITERATIONS: usize = 1;

/// Validate that a deletion ratio value is finite and within [0.0, 1.0].
fn is_valid_deletion_ratio(v: f64) -> bool {
    v.is_finite() && (0.0..=1.0).contains(&v)
}
const MAX_AGENT_ITERATIONS: usize = 100;
/// Default maximum total tool calls per agentic turn (Issue #172).
pub const DEFAULT_MAX_TOOL_CALLS: usize = 200;
/// Hard upper limit for max_tool_calls (Issue #172).
pub const MAX_TOOL_CALLS_LIMIT: usize = 10000;

/// Sensitive keys and their recommended environment variable names.
/// To add a new sensitive key, simply add an entry to this array.
const SENSITIVE_KEYS: &[(&str, &str)] = &[
    ("api_key", "ANVIL_API_KEY"),
    ("serper_api_key", "SERPER_API_KEY"),
];

/// Detect API keys in config file map and return warning messages.
/// Only checks keys from the config file; env/CLI keys are not warned about.
pub fn check_config_security_warnings(map: &HashMap<String, String>) -> Vec<String> {
    SENSITIVE_KEYS
        .iter()
        .filter(|(key, _)| map.contains_key(*key))
        .map(|(key, env_var)| {
            format!(
                "⚠ Warning: {key} found in config file. \
                Consider using {env_var} environment variable \
                instead for better security."
            )
        })
        .collect()
}

/// Walk up from `start` looking for a `.git` directory to find the repo root.
fn find_repo_root(start: &Path) -> Option<PathBuf> {
    let mut current = start.to_path_buf();
    loop {
        if current.join(".git").exists() {
            return Some(current);
        }
        if !current.pop() {
            return None;
        }
    }
}

/// Check whether `.anvil/` is listed in `.gitignore`.
/// Returns `Some(warning)` if it is missing or `.gitignore` does not exist.
pub fn check_gitignore_anvil_dir(repo_root: &Path) -> Option<String> {
    let gitignore_path = repo_root.join(".gitignore");
    let Ok(contents) = std::fs::read_to_string(&gitignore_path) else {
        return Some(
            "⚠ Warning: .gitignore not found. Consider adding .anvil/ \
            to .gitignore to prevent config files from being committed."
                .to_string(),
        );
    };

    if contents.lines().any(|line| {
        let trimmed = line.trim();
        !trimmed.starts_with('#') && trimmed.contains(".anvil")
    }) {
        None
    } else {
        Some(
            "⚠ Warning: .anvil/ is not in .gitignore. Consider adding it \
            to prevent config files from being committed."
                .to_string(),
        )
    }
}

impl PathConfig {
    /// Load project instructions from ANVIL.md files.
    /// Delegates to `load_project_instructions_from()` for testability.
    pub fn load_project_instructions(&self) -> (Option<String>, Vec<CustomToolDef>) {
        let home_dir = std::env::var("HOME").ok().map(PathBuf::from);
        Self::load_project_instructions_from(&self.cwd, home_dir.as_deref())
    }

    /// Internal method: accepts cwd and home_dir as arguments so tests can
    /// pass temp directories without depending on the HOME environment variable.
    ///
    /// Returns `(project_instructions, custom_tool_defs)`.
    /// The `## tools` section is extracted from instructions and returned separately.
    pub fn load_project_instructions_from(
        cwd: &Path,
        home_dir: Option<&Path>,
    ) -> (Option<String>, Vec<CustomToolDef>) {
        let mut parts: Vec<String> = Vec::new();
        let mut sources: Vec<String> = Vec::new();
        let mut has_user_scope = false;
        let mut has_project_scope = false;

        // 1. User scope: ~/.anvil/ANVIL.md
        if let Some(home) = home_dir {
            let user_path = home.join(".anvil").join("ANVIL.md");
            match std::fs::read_to_string(&user_path) {
                Ok(content) => {
                    sources.push(format!("{}", user_path.display()));
                    parts.push(content);
                    has_user_scope = true;
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => {
                    eprintln!("Warning: failed to read {}: {}", user_path.display(), e);
                }
            }
        }

        // 2. Project scope (exclusive: .anvil/ANVIL.md takes priority)
        let project_path = {
            let dotdir = cwd.join(".anvil").join("ANVIL.md");
            if dotdir.exists() {
                Some(dotdir)
            } else {
                let root = cwd.join("ANVIL.md");
                if root.exists() { Some(root) } else { None }
            }
        };

        if let Some(path) = project_path {
            match std::fs::read_to_string(&path) {
                Ok(content) => {
                    sources.push(format!("{}", path.display()));
                    parts.push(content);
                    has_project_scope = true;
                }
                Err(e) => {
                    eprintln!("Warning: failed to read {}: {}", path.display(), e);
                }
            }
        }

        if parts.is_empty() {
            return (None, Vec::new());
        }

        eprintln!("ANVIL.md loaded from: {}", sources.join(", "));

        // Combine with scope labels
        let mut combined = if has_user_scope && has_project_scope {
            format!(
                "## User scope\n{}\n\n---\n\n## Project scope\n{}",
                parts[0], parts[1]
            )
        } else if has_user_scope {
            format!("## User scope\n{}", parts[0])
        } else {
            format!("## Project scope\n{}", parts[0])
        };

        // SEC-3/SEC-9: Sanitize ANVIL_TOOL/ANVIL_FINAL markers
        let (sanitized, had_markers) = sanitize_markers(&combined);
        if had_markers {
            eprintln!(
                "Warning: ANVIL.md contains ANVIL_TOOL/ANVIL_FINAL markers. \
                 These have been sanitized to prevent interference with tool protocol."
            );
        }
        combined = sanitized;

        // 4000-character limit with newline-boundary snap
        if combined.chars().count() > MAX_PROJECT_INSTRUCTIONS_CHARS {
            eprintln!(
                "Warning: ANVIL.md content exceeds {} characters, truncating",
                MAX_PROJECT_INSTRUCTIONS_CHARS
            );
            let truncated: String = combined
                .chars()
                .take(MAX_PROJECT_INSTRUCTIONS_CHARS)
                .collect();
            combined = match truncated.rfind('\n') {
                Some(pos) => format!("{}\n[...truncated]", &truncated[..pos]),
                None => format!("{}\n[...truncated]", truncated),
            };
        }

        // Parse and extract ## tools section before returning.
        let (instructions, custom_tools) = parse_tools_section(&combined);
        let instructions = if instructions.trim().is_empty() {
            None
        } else {
            Some(instructions)
        };
        if !custom_tools.is_empty() {
            eprintln!(
                "Custom tools registered from ANVIL.md: {} tool(s)",
                custom_tools.len()
            );
        }
        (instructions, custom_tools)
    }
}

/// Sanitize ANVIL_TOOL/ANVIL_FINAL markers in content (SEC-3/SEC-9).
/// Replaces backtick-triple markers with full-width backticks to neutralize them.
pub fn sanitize_markers(content: &str) -> (String, bool) {
    let mut sanitized = content.to_string();
    let mut found = false;
    for marker in &["```ANVIL_TOOL", "```ANVIL_FINAL"] {
        if sanitized.contains(marker) {
            found = true;
            sanitized =
                sanitized.replace(marker, &marker.replace("```", "\u{FF40}\u{FF40}\u{FF40}"));
        }
    }
    (sanitized, found)
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

impl std::fmt::Debug for RuntimeConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RuntimeConfig")
            .field("provider", &self.provider)
            .field("provider_url", &self.provider_url)
            .field("model", &self.model)
            .field("sidecar_model", &self.sidecar_model)
            .field("api_key", &"[REDACTED]")
            .field("context_window", &self.context_window)
            .field("context_budget", &self.context_budget)
            .field("max_agent_iterations", &self.max_agent_iterations)
            .field("max_console_messages", &self.max_console_messages)
            .field("auto_compact_threshold", &self.auto_compact_threshold)
            .field("tool_result_max_chars", &self.tool_result_max_chars)
            .field("stream", &self.stream)
            .field("web_search_provider", &self.web_search_provider)
            .field("serper_api_key", &"[REDACTED]")
            .field(
                "context_window_explicitly_set",
                &self.context_window_explicitly_set,
            )
            .field("tag_protocol", &self.tag_protocol)
            .field(
                "smart_compact_threshold_ratio",
                &self.smart_compact_threshold_ratio,
            )
            .field("http_timeout_secs", &self.http_timeout_secs)
            .field("edit_strategy", &self.edit_strategy)
            .field("edit_reread_threshold", &self.edit_reread_threshold)
            .field(
                "edit_write_fallback_threshold",
                &self.edit_write_fallback_threshold,
            )
            .field("safe_write_max_lines", &self.safe_write_max_lines)
            .field("safe_write_deletion_ratio", &self.safe_write_deletion_ratio)
            .finish()
    }
}

fn parse_web_search_provider(value: &str) -> Result<WebSearchProvider, ConfigError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "duckduckgo" => Ok(WebSearchProvider::DuckDuckGo),
        "serper_api" | "serperapi" | "serper" => Ok(WebSearchProvider::SerperApi),
        other => Err(ConfigError::InvalidWebSearchProvider(other.to_string())),
    }
}

// --- MCP configuration loading ---

use crate::mcp::{McpConfigFile, McpServerConfig};

/// Load MCP configuration from `.anvil/mcp.json`.
///
/// Returns `Ok(None)` if the file does not exist (MCP is optional).
/// Returns `Ok(Some(...))` with expanded environment variables on success.
/// Returns `Err(...)` on parse or env-var expansion errors.
pub fn load_mcp_config(
    paths: &PathConfig,
) -> Result<Option<HashMap<String, McpServerConfig>>, String> {
    if !paths.mcp_config_file.exists() {
        return Ok(None);
    }
    let content = std::fs::read_to_string(&paths.mcp_config_file)
        .map_err(|e| format!("Failed to read mcp.json: {e}"))?;
    let config: McpConfigFile =
        serde_json::from_str(&content).map_err(|e| format!("Failed to parse mcp.json: {e}"))?;

    // Expand environment variable references ($VAR_NAME → actual value)
    // [D4-008] Undefined variables cause an error for the affected server
    let expanded = expand_env_vars(config.mcp_servers)?;

    // [D4-008] Check for plaintext API key values in env fields
    check_mcp_config_security_warnings(&expanded);

    Ok(Some(expanded))
}

/// Expand `$VAR_NAME` references in MCP server config env values.
///
/// [D4-008] If a `$VAR_NAME` reference is undefined, returns an error
/// naming the variable and the server that requires it.
fn expand_env_vars(
    configs: HashMap<String, McpServerConfig>,
) -> Result<HashMap<String, McpServerConfig>, String> {
    let mut result = HashMap::new();
    for (server_name, mut config) in configs {
        let mut expanded_env = HashMap::new();
        for (key, value) in &config.env {
            if let Some(var_name) = value.strip_prefix('$') {
                match std::env::var(var_name) {
                    Ok(resolved) => {
                        expanded_env.insert(key.clone(), resolved);
                    }
                    Err(_) => {
                        return Err(format!(
                            "Environment variable ${var_name} is not set (required by server '{server_name}')"
                        ));
                    }
                }
            } else {
                expanded_env.insert(key.clone(), value.clone());
            }
        }
        config.env = expanded_env;
        result.insert(server_name, config);
    }
    Ok(result)
}

/// Known prefixes that indicate an API key or token.
pub(crate) const KNOWN_SECRET_PREFIXES: &[&str] =
    &["ghp_", "sk-", "xoxb-", "xoxp-", "ghu_", "ghs_"];

/// [D4-008] Check MCP config env fields for plaintext API keys.
///
/// Warns if an env value appears to be a hardcoded secret rather than
/// an environment variable reference (`$VAR`).
fn check_mcp_config_security_warnings(configs: &HashMap<String, McpServerConfig>) {
    for (server_name, config) in configs {
        for (key, value) in &config.env {
            // Skip env-var references
            if value.starts_with('$') {
                continue;
            }
            // Check length and known prefixes
            if value.len() >= 20 {
                let looks_like_secret = KNOWN_SECRET_PREFIXES
                    .iter()
                    .any(|prefix| value.starts_with(prefix));
                if looks_like_secret {
                    eprintln!(
                        "⚠ Warning: MCP server '{server_name}' env key '{key}' appears to contain \
                         a plaintext API key. Consider using an environment variable reference \
                         (e.g., \"${key}\") instead for better security."
                    );
                }
            }
        }
    }
}

// --- Hooks configuration loading ---

use crate::hooks::{HookError, HooksConfig, MAX_ENTRIES_PER_HOOK_POINT};

/// Load hooks configuration from `.anvil/hooks.json`.
///
/// Returns `Ok(None)` if the file does not exist (hooks are optional).
/// Returns `Ok(Some(...))` on successful parse.
/// Returns `Err(...)` on parse errors (DR2-009).
///
/// DR4-005: Validates the hooks.json path is within cwd/.anvil/.
/// DR4-007: Caps entries per hook point to 16.
pub fn load_hooks_config(paths: &PathConfig) -> Result<Option<HooksConfig>, HookError> {
    if !paths.hooks_config_file.exists() {
        return Ok(None);
    }

    // DR4-005: Validate path is within cwd
    if let Ok(canonical) = paths.hooks_config_file.canonicalize()
        && let Ok(cwd_canonical) = paths.cwd.canonicalize()
        && !canonical.starts_with(&cwd_canonical)
    {
        return Err(HookError::ParseError {
            file: paths.hooks_config_file.clone(),
            reason: "hooks.json path is outside the project directory".to_string(),
        });
    }

    tracing::info!(path = %paths.hooks_config_file.display(), "hooks.json detected");

    let content =
        std::fs::read_to_string(&paths.hooks_config_file).map_err(|e| HookError::ParseError {
            file: paths.hooks_config_file.clone(),
            reason: format!("failed to read: {e}"),
        })?;

    let mut config: HooksConfig =
        serde_json::from_str(&content).map_err(|e| HookError::ParseError {
            file: paths.hooks_config_file.clone(),
            reason: format!("failed to parse: {e}"),
        })?;

    // DR4-007: Cap entries per hook point
    for (point, entries) in config.hooks.iter_mut() {
        if entries.len() > MAX_ENTRIES_PER_HOOK_POINT {
            tracing::warn!(
                hook_point = ?point,
                count = entries.len(),
                max = MAX_ENTRIES_PER_HOOK_POINT,
                "hook entries exceed limit, truncating"
            );
            entries.truncate(MAX_ENTRIES_PER_HOOK_POINT);
        }
    }

    Ok(Some(config))
}
