/// Core application orchestrator.
///
/// [`App`] owns the session, state machine, tool registry and config,
/// coordinating turns between the user, the LLM provider, and the tool
/// executor.
pub mod agentic;
pub mod alternating_loop_detector;
pub mod cli;
mod context;
pub(crate) mod edit_fail_tracker;
pub(crate) mod execution_plan;
pub mod loop_detector;
pub mod mock;
pub mod phase_estimator;
pub mod plan;
pub mod policy;
pub(crate) mod read_repeat_tracker;
pub mod read_transition_guard;
pub mod render;
pub mod stagnation_state;
pub(crate) mod write_fail_tracker;
pub(crate) mod write_repeat_tracker;

use std::collections::HashMap;
use std::time::Instant;

use crate::agent::BasicAgentLoop;
use crate::agent::{AgentEvent, AgentRuntime, PendingTurnState, ProjectLanguage, PromptTier};
use crate::config::EffectiveConfig;
use crate::contracts::tokens::TokenCalibrationStore;
use crate::contracts::{
    AppEvent, AppStateSnapshot, ConsoleRenderContext, ContextUsageView, ContextWarningLevel,
    RuntimeState,
};
use crate::extensions::{ExtensionLoadError, ExtensionRegistry, SlashCommandAction, TrustAction};
use crate::provider::{
    ProviderBootstrapError, ProviderClient, ProviderErrorKind, ProviderErrorRecord, ProviderEvent,
    ProviderRuntimeContext, ProviderTurnError,
};
use crate::retrieval::{
    DEFAULT_SEARCH_LIMIT, RepositoryIndex, default_cache_path, render_retrieval_result,
};
use crate::session::{
    MessageRole, MessageStatus, SessionError, SessionMessage, SessionRecord, SessionStore,
    new_assistant_message, new_user_message,
};
use crate::spinner::Spinner;
use crate::state::{StateMachine, StateTransition};
use crate::tooling::{
    CheckpointStack, ExecutionClass, ExecutionMode, PermissionClass, PlanModePolicy,
    RollbackPolicy, ToolKind, ToolRegistry, ToolSpec,
};
use crate::tui::Tui;
use std::collections::HashSet;
use std::fmt::{Display, Formatter};
use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

// Re-export render helpers that form the public API.
pub use render::{cli_prompt, render_help_frame, slash_commands};

/// Detect project languages from the project root directory.
///
/// Checks for the presence of language-specific manifest files and returns
/// a list of detected languages. Called once at session start and cached.
pub fn detect_project_languages(project_root: &std::path::Path) -> Vec<ProjectLanguage> {
    let mut languages = Vec::new();
    if project_root.join("Cargo.toml").exists() {
        languages.push(ProjectLanguage::Rust);
    }
    if project_root.join("package.json").exists() {
        languages.push(ProjectLanguage::NodeJs);
    }
    languages
}

/// Tracks context overflow warning state to avoid duplicate notifications.
///
/// Private to the app module; not persisted across sessions.
struct ContextWarningTracker {
    warned_warning: bool,
    warned_critical: bool,
}

impl ContextWarningTracker {
    fn new() -> Self {
        Self {
            warned_warning: false,
            warned_critical: false,
        }
    }

    /// Evaluate current context usage and return a warning level if not yet notified.
    fn evaluate(&mut self, usage: &ContextUsageView) -> Option<ContextWarningLevel> {
        let level = usage.warning_level();
        match level {
            Some(ContextWarningLevel::Critical) if !self.warned_critical => {
                self.warned_critical = true;
                self.warned_warning = true;
                Some(ContextWarningLevel::Critical)
            }
            Some(ContextWarningLevel::Warning) if !self.warned_warning => {
                self.warned_warning = true;
                Some(ContextWarningLevel::Warning)
            }
            _ => None,
        }
    }

    /// Reset flags when usage drops below thresholds (e.g. after /compact).
    fn reset_if_below_threshold(&mut self, usage: &ContextUsageView) {
        let ratio = usage.usage_ratio();
        if ratio < 0.8 {
            self.warned_warning = false;
        }
        if ratio < 0.9 {
            self.warned_critical = false;
        }
    }
}

/// Shared formatter for tool call counts (DRY: used by both `summarize_tool_names` and `SessionStats::tool_calls_summary`).
///
/// Produces a string like `"file.read x3, file.edit x2, shell.exec"` sorted by count descending.
pub fn format_tool_counts(counts: impl Iterator<Item = (String, u32)>) -> String {
    let mut entries: Vec<_> = counts.collect();
    entries.sort_by(|a, b| b.1.cmp(&a.1));
    entries
        .iter()
        .map(|(name, count)| {
            if *count > 1 {
                format!("{} x{}", name, count)
            } else {
                name.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Session statistics tracker for session-end summary reporting.
#[derive(Debug, Default)]
pub struct SessionStats {
    pub total_turns: u32,
    pub session_start: Option<Instant>,
    pub tool_calls: HashMap<String, u32>,
    /// Unique file paths that were actually persisted to disk (Issue #259).
    pub files_modified: std::collections::HashSet<String>,
    pub lines_added: u32,
    pub lines_deleted: u32,
    pub compact_count: u32,
    pub sidecar_count: u32,
}

impl SessionStats {
    pub fn new() -> Self {
        Self {
            session_start: Some(Instant::now()),
            ..Default::default()
        }
    }

    pub fn record_tool_call(&mut self, tool_name: &str) {
        *self.tool_calls.entry(tool_name.to_string()).or_insert(0) += 1;
    }

    pub fn record_file_change(&mut self, added: u32, deleted: u32) {
        self.lines_added += added;
        self.lines_deleted += deleted;
    }

    pub fn record_turn(&mut self) {
        self.total_turns += 1;
    }

    pub fn record_compact(&mut self, sidecar: bool) {
        self.compact_count += 1;
        if sidecar {
            self.sidecar_count += 1;
        }
    }

    pub fn total_tool_calls(&self) -> u32 {
        self.tool_calls.values().sum()
    }

    /// Format tool call counts summary (e.g. `"file.read x18, file.edit x12"`).
    pub fn tool_calls_summary(&self) -> String {
        format_tool_counts(self.tool_calls.iter().map(|(k, v)| (k.clone(), *v)))
    }
}

/// Extract added/deleted line counts from a unified diff string.
pub fn count_diff_lines(diff: &str) -> (u32, u32) {
    let mut added = 0u32;
    let mut deleted = 0u32;
    for line in diff.lines() {
        if line.starts_with('+') && !line.starts_with("+++") {
            added += 1;
        } else if line.starts_with('-') && !line.starts_with("---") {
            deleted += 1;
        }
    }
    (added, deleted)
}

/// Compact operation metadata for turn summary reporting.
#[derive(Debug, Clone)]
pub struct CompactInfo {
    pub sidecar_model: Option<String>,
    pub before_messages: usize,
    pub after_messages: usize,
}

/// Central application state.
pub struct App {
    config: EffectiveConfig,
    provider: ProviderRuntimeContext,
    state_machine: StateMachine,
    session_store: SessionStore,
    session: SessionRecord,
    extensions: ExtensionRegistry,
    tools: ToolRegistry,
    detected_languages: Vec<ProjectLanguage>,
    mcp_descriptions: Option<String>,
    project_instructions: Option<String>,
    shutdown_flag: Arc<AtomicBool>,
    warning_tracker: ContextWarningTracker,
    /// Undo checkpoint stack. In-memory only, discarded on session exit.
    checkpoint_stack: CheckpointStack,
    /// Loop detector for preventing infinite tool call repetitions (Issue #145).
    loop_detector: loop_detector::LoopDetector,
    /// Alternating/cyclic loop detector (Issue #172).
    alternating_loop_detector: alternating_loop_detector::AlternatingLoopDetector,
    /// Hooks engine. `None` when hooks.json is absent or initialization failed.
    /// Declared before mcp_manager to maintain Drop order (DR3-007).
    hooks_engine: Option<crate::hooks::HooksEngine>,
    /// MCP server manager. `None` when mcp.json is absent or initialization failed.
    /// [D2-010] Declared last so it is dropped last (Drop order = declaration order).
    mcp_manager: Option<crate::mcp::McpManager>,
    /// Current session name (derived from session file stem).
    current_session_name: String,
    /// Trust mode: auto-approve built-in (non-MCP) tool execution.
    trust_all: bool,
    /// Individually trusted tool names (including MCP tools).
    trusted_tools: HashSet<String>,
    /// Session-scoped model override (Issue #77).
    active_model: Option<String>,
    /// Session-scoped context window override (Issue #77).
    active_context_window: Option<u32>,
    /// Token calibration store: accumulates actual vs estimated ratios per model.
    calibration_store: TokenCalibrationStore,
    /// Last estimated prompt tokens from build_turn_request_calibrated.
    /// Consumed by apply_agent_event Done handler via Option::take.
    last_estimated_prompt_tokens: Option<usize>,
    /// System prompt verbosity tier, determined at session start.
    prompt_tier: PromptTier,
    /// Tracks consecutive file.edit failures per path for recovery hints.
    edit_fail_tracker: edit_fail_tracker::EditFailTracker,
    /// Phase estimator for fallback phase control (Issue #159).
    phase_estimator: phase_estimator::PhaseEstimator,
    /// Guard that forces a transition from exploration to implementation.
    read_transition_guard: read_transition_guard::ReadTransitionGuard,
    /// Tracks consecutive file.write failures per path for recovery hints.
    write_fail_tracker: write_fail_tracker::WriteFailTracker,
    /// Tracks repeated file.read calls per path for hint injection (Issue #185).
    read_repeat_tracker: read_repeat_tracker::ReadRepeatTracker,
    /// Tracks repeated successful file.write calls per path for warning hints.
    write_repeat_tracker: write_repeat_tracker::WriteRepeatTracker,
    /// File read cache: reduces redundant file.read calls within a session.
    file_read_cache: Arc<Mutex<crate::tooling::file_cache::FileReadCache>>,
    /// Session statistics for end-of-session summary (Issue #206).
    session_stats: SessionStats,
    /// Last compact operation info for turn summary reporting (Issue #206).
    last_compact_info: Option<CompactInfo>,
    /// Execution plan for Plan → Execute mode (Issue #249).
    execution_plan: crate::contracts::ExecutionPlan,
    /// Agent telemetry for session-level metrics (Issue #255).
    agent_telemetry: crate::contracts::AgentTelemetry,
    /// Per-turn stagnation telemetry (Issue #263).
    stagnation_state: stagnation_state::StagnationState,
    /// Whether forced mode is active for the current turn (Issue #263).
    forced_mode_active: bool,
}

/// Whether the session loop should continue or exit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionControl {
    Continue,
    Exit,
}

/// Output of a single CLI turn: rendered frames and control signal.
pub struct CliTurnOutput {
    pub frames: Vec<String>,
    pub control: SessionControl,
}

/// Errors raised by the application layer.
#[derive(Debug)]
pub enum AppError {
    Config(crate::config::ConfigError),
    ProviderBootstrap(ProviderBootstrapError),
    Extension(ExtensionLoadError),
    Session(SessionError),
    ProviderTurn(ProviderTurnError),
    StateTransition(crate::state::StateTransitionError),
    ToolExecution(String),
    NoPendingApproval,
    PendingApprovalRequired,
}

impl Display for AppError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Config(err) => write!(f, "{err}"),
            Self::ProviderBootstrap(err) => write!(f, "{err}"),
            Self::Extension(err) => write!(f, "{err}"),
            Self::Session(err) => write!(f, "{err}"),
            Self::ProviderTurn(err) => write!(f, "{err}"),
            Self::StateTransition(err) => write!(f, "{err}"),
            Self::ToolExecution(err) => write!(f, "{err}"),
            Self::NoPendingApproval => write!(f, "no pending approval to continue"),
            Self::PendingApprovalRequired => {
                write!(f, "resolve the pending approval before starting a new turn")
            }
        }
    }
}

impl std::error::Error for AppError {}

impl AppError {
    /// Return the process exit code for non-interactive mode.
    /// - ToolExecution errors → 2 (tool failure)
    /// - All other errors → 1 (general error)
    pub fn exit_code(&self) -> u8 {
        match self {
            AppError::ToolExecution(_) => 2,
            _ => 1,
        }
    }
}

impl From<crate::config::ConfigError> for AppError {
    fn from(value: crate::config::ConfigError) -> Self {
        Self::Config(value)
    }
}

impl From<ProviderBootstrapError> for AppError {
    fn from(value: ProviderBootstrapError) -> Self {
        Self::ProviderBootstrap(value)
    }
}

impl From<ExtensionLoadError> for AppError {
    fn from(value: ExtensionLoadError) -> Self {
        Self::Extension(value)
    }
}

impl From<SessionError> for AppError {
    fn from(value: SessionError) -> Self {
        Self::Session(value)
    }
}

impl From<ProviderTurnError> for AppError {
    fn from(value: ProviderTurnError) -> Self {
        Self::ProviderTurn(value)
    }
}

impl From<crate::state::StateTransitionError> for AppError {
    fn from(value: crate::state::StateTransitionError) -> Self {
        Self::StateTransition(value)
    }
}

/// Parameters for auto-compaction (token-based and/or message-based).
struct CompactParams {
    keep_recent: usize,
}

/// Compute compaction parameters from session state and context window.
/// Returns None if neither token-based nor message-based threshold is exceeded.
/// When `context_budget` is set, uses `min(context_window, context_budget)` for thresholds.
fn compute_compact_params(
    session: &SessionRecord,
    context_window: u32,
    context_budget: Option<u32>,
) -> Option<CompactParams> {
    let token_triggered = session.should_smart_compact(context_window, context_budget);
    let msg_triggered = session.should_compact();

    if !token_triggered && !msg_triggered {
        return None;
    }

    let effective_limit = match context_budget {
        Some(budget) => context_window.min(budget),
        None => context_window,
    };
    let token_based = if token_triggered {
        let target_tokens = (effective_limit as f64 * crate::session::TARGET_TOKEN_RATIO) as usize;
        crate::session::compute_token_based_keep_recent(&session.messages, target_tokens)
    } else {
        usize::MAX
    };
    let msg_based = if msg_triggered {
        session.auto_compact_threshold / 2
    } else {
        usize::MAX
    };
    let keep_recent = std::cmp::min(token_based, msg_based);

    Some(CompactParams { keep_recent })
}

impl App {
    pub fn new(
        config: EffectiveConfig,
        provider: ProviderRuntimeContext,
        shutdown_flag: Arc<AtomicBool>,
    ) -> Result<Self, AppError> {
        let session_store = SessionStore::from_config(&config);
        let mut session = if config.mode.fresh_session {
            SessionRecord::new(config.paths.cwd.clone())
        } else {
            session_store.load_or_create(&config.paths.cwd)?
        };
        let initial_state_snapshot = session
            .last_snapshot
            .clone()
            .unwrap_or_else(|| AppStateSnapshot::new(RuntimeState::Ready));
        let home_dir = std::env::var("HOME").ok().map(std::path::PathBuf::from);
        let extensions = ExtensionRegistry::load(&config.paths.cwd, home_dir.as_deref())?;
        session.auto_compact_threshold = config.runtime.auto_compact_threshold;
        session.smart_compact_threshold_ratio = config.runtime.smart_compact_threshold_ratio;

        // --- MCP initialization (Task 3.2) ---
        // [D1-007] shutdown_flag is managed by App side (YAGNI)
        // Offline mode: skip MCP initialization entirely (load_mcp_config included)
        let mcp_manager = if config.mode.offline {
            None
        } else {
            match crate::config::load_mcp_config(&config.paths) {
                Ok(Some(mcp_configs)) => {
                    match crate::mcp::McpManager::start_all(mcp_configs) {
                        Ok(manager) => Some(manager),
                        Err(e) => {
                            eprintln!("Warning: MCP initialization failed: {e}");
                            None // graceful degradation
                        }
                    }
                }
                Ok(None) => None, // mcp.json not found → skip completely
                Err(e) => {
                    eprintln!("Warning: MCP config parse error: {e}");
                    None // graceful degradation
                }
            }
        };

        // --- Hooks initialization (MCP pattern, DR3-010) ---
        let hooks_engine = match crate::config::load_hooks_config(&config.paths) {
            Ok(Some(hooks_config)) => {
                if hooks_config.is_empty() {
                    None
                } else {
                    Some(crate::hooks::HooksEngine::new(
                        hooks_config,
                        Arc::clone(&shutdown_flag),
                    ))
                }
            }
            Ok(None) => None, // hooks.json not found → skip completely
            Err(e) => {
                eprintln!("Warning: hooks config error: {e}");
                None // graceful degradation
            }
        };

        // [D1-009] ToolSpec conversion and ToolRegistry registration done by App side (SRP)
        // [D2-003] standard_tool_registry() is a free function
        let mut tools = standard_tool_registry(config.custom_tools().to_vec());
        // Register sub-agent tools separately (design decision #6, DR1-008)
        tools.register_agent_explore();
        tools.register_agent_plan();
        if let Some(ref manager) = mcp_manager {
            let mcp_tools = manager.get_tools();
            for (server_name, tool_list) in &mcp_tools {
                for tool_info in tool_list {
                    // [D1-011, D2-001] Default ToolSpec values for MCP tools
                    let spec = ToolSpec {
                        version: 1,
                        name: format!("mcp__{server_name}__{}", tool_info.name),
                        kind: ToolKind::Mcp,
                        execution_class: ExecutionClass::Network,
                        permission_class: PermissionClass::Confirm,
                        execution_mode: ExecutionMode::SequentialOnly,
                        plan_mode: PlanModePolicy::Allowed,
                        rollback_policy: RollbackPolicy::None,
                    };
                    tools.register(spec);
                }
            }
        }

        // [D1-009] System prompt generation with MCP tool descriptions done by App side
        let mcp_descriptions = mcp_manager.as_ref().map(|manager| {
            let mcp_tools = manager.get_tools();
            crate::agent::generate_mcp_tool_descriptions(&mcp_tools)
        });

        let detected_languages = detect_project_languages(&config.paths.cwd);
        let project_instructions = config.project_instructions().map(|s| s.to_string());

        // Offline mode: warn about shell.exec network access
        if config.mode.offline {
            eprintln!(
                "Warning: shell.exec のネットワークコマンド（curl, wget等）は offline mode でブロックされます。エイリアスやスクリプト経由のアクセスは検出されない場合があります。完全なネットワーク遮断には OS/ファイアウォールレベルの制御を使用してください。"
            );
        }

        let current_session_name = config
            .paths
            .session_file
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("default")
            .to_string();

        let trust_all = config.mode.trust_all;

        let file_read_cache = Arc::new(Mutex::new(crate::tooling::file_cache::FileReadCache::new(
            config.paths.cwd.clone(),
        )));

        // Determine prompt tier from config override or model name heuristic
        let prompt_tier = {
            use crate::agent::model_classifier::classify_model_capability;
            let capability = classify_model_capability(
                &config.runtime.model,
                config.runtime.tag_protocol,
                config.runtime.prompt_tier.as_deref(),
            );
            capability.prompt_tier
        };

        let loop_detection_threshold = config.runtime.loop_detection_threshold;
        let phase_explore = config.runtime.phase_explore_threshold;
        let phase_force = config.runtime.phase_force_transition_threshold;
        let phase_completion = config.runtime.phase_completion_read_threshold;
        let read_transition_threshold = config.runtime.read_transition_threshold;
        let read_transition_reinject_interval = config.runtime.read_transition_reinject_interval;
        let edit_reread_threshold = config.runtime.edit_reread_threshold;
        let edit_write_fallback_threshold = config.runtime.edit_write_fallback_threshold;
        let read_repeat_warn = config.runtime.read_repeat_warn_threshold;
        let read_repeat_strong_warn = config.runtime.read_repeat_strong_warn_threshold;

        Ok(Self {
            tools,
            config,
            provider,
            state_machine: StateMachine::from_snapshot(initial_state_snapshot),
            session_store,
            session,
            extensions,
            detected_languages,
            mcp_descriptions,
            project_instructions,
            shutdown_flag,
            warning_tracker: ContextWarningTracker::new(),
            checkpoint_stack: CheckpointStack::new(),
            loop_detector: loop_detector::LoopDetector::new(loop_detection_threshold),
            alternating_loop_detector: alternating_loop_detector::AlternatingLoopDetector::new(
                alternating_loop_detector::DEFAULT_CYCLE_THRESHOLD,
            ),
            hooks_engine,
            mcp_manager,
            current_session_name,
            trust_all,
            trusted_tools: HashSet::new(),
            active_model: None,
            active_context_window: None,
            calibration_store: TokenCalibrationStore::new(),
            last_estimated_prompt_tokens: None,
            prompt_tier,
            edit_fail_tracker: edit_fail_tracker::EditFailTracker::new(
                edit_reread_threshold,
                edit_write_fallback_threshold,
            ),
            phase_estimator: phase_estimator::PhaseEstimator::new(
                phase_explore,
                phase_force,
                phase_completion,
            ),
            read_transition_guard: read_transition_guard::ReadTransitionGuard::new(
                read_transition_threshold,
                read_transition_reinject_interval,
            ),
            write_fail_tracker: write_fail_tracker::WriteFailTracker::new(2),
            read_repeat_tracker: read_repeat_tracker::ReadRepeatTracker::new(
                read_repeat_warn,
                read_repeat_strong_warn,
            ),
            write_repeat_tracker: write_repeat_tracker::WriteRepeatTracker::new(3, 4),
            file_read_cache,
            session_stats: SessionStats::new(),
            last_compact_info: None,
            execution_plan: crate::contracts::ExecutionPlan::default(),
            agent_telemetry: crate::contracts::AgentTelemetry::new(),
            stagnation_state: stagnation_state::StagnationState::new(),
            forced_mode_active: false,
        })
    }

    pub fn initial_snapshot(&mut self) -> Result<AppStateSnapshot, AppError> {
        let snapshot = self.build_initial_snapshot();
        self.apply_transition(snapshot, StateTransition::ResetToReady)
    }

    fn build_initial_snapshot(&self) -> AppStateSnapshot {
        let mut status = format!(
            "Ready. provider={} model={} stream={} tools={}",
            self.config.runtime.provider,
            self.effective_model(),
            self.provider.capabilities.streaming,
            self.provider.capabilities.tool_calling
        );
        if self.config.mode.offline {
            status.push_str(" offline=true");
        }
        AppStateSnapshot::new(RuntimeState::Ready)
            .with_event(AppEvent::StartupCompleted)
            .with_status(status)
            .with_context_usage(
                self.session.estimated_token_count(),
                self.effective_context_window(),
            )
    }

    pub fn state_machine(&self) -> &StateMachine {
        &self.state_machine
    }

    pub fn session(&self) -> &SessionRecord {
        &self.session
    }

    pub fn session_store(&self) -> &SessionStore {
        &self.session_store
    }

    pub(crate) fn config(&self) -> &EffectiveConfig {
        &self.config
    }

    /// Prepare turn context: system prompt generation, pruning pre-check,
    /// context_notice update, and Critical-level warning injection.
    /// Returns (system_prompt, calibration_ratio).
    fn prepare_turn_context(&mut self) -> (String, f64) {
        use crate::contracts::tokens::{ContentKind, estimate_tokens_calibrated};

        let calibration_ratio = self.calibration_store.get_ratio(self.effective_model());

        // 1. Preliminary system prompt (context_notice may be from previous turn)
        let preliminary_prompt = self.build_dynamic_system_prompt();
        let system_prompt_tokens =
            estimate_tokens_calibrated(&preliminary_prompt, ContentKind::Text, calibration_ratio);

        // 2. Pruning pre-check
        let context_window = self.effective_context_window();
        let (pruned_count, selected_tokens) = BasicAgentLoop::estimate_pruned_message_count(
            &self.session,
            context_window,
            system_prompt_tokens,
            calibration_ratio,
            self.config.runtime.context_budget,
        );

        // 3. Update context_notice
        let old_notice = self.session.working_memory.context_notice.clone();
        if pruned_count > 0 {
            let notice = format!(
                "{} earlier messages were omitted due to context limits. \
                 Files listed in 'Touched files' have already been modified. \
                 Do not re-read or re-edit them unless necessary.",
                pruned_count
            );
            self.session.working_memory.set_context_notice(Some(notice));
        } else {
            self.session.working_memory.set_context_notice(None);
        }

        // 4. Regenerate prompt if context_notice changed
        let system_prompt = if self.session.working_memory.context_notice != old_notice {
            self.build_dynamic_system_prompt()
        } else {
            preliminary_prompt
        };

        // 5. Critical-level warning injection
        let estimated_total = system_prompt_tokens + selected_tokens;
        let usage = ContextUsageView {
            estimated_tokens: estimated_total,
            max_tokens: context_window,
        };
        let mut final_prompt = system_prompt;
        if usage.warning_level() == Some(ContextWarningLevel::Critical) {
            final_prompt.push_str(
                "\n\n## CRITICAL: Context limit approaching\n\
                 Context usage exceeds 90%. Prioritize completing pending file \
                 changes and report progress via ANVIL_FINAL. Avoid reading \
                 files already listed in Touched files above.",
            );
        }

        (final_prompt, calibration_ratio)
    }

    /// Build a dynamic system prompt based on current session state.
    ///
    /// Called each turn to include only relevant tool descriptions.
    /// Offline mode filters out web.* tools from the effective used_tools set.
    fn build_dynamic_system_prompt(&self) -> String {
        use crate::agent::tool_protocol_system_prompt;

        // Offline mode: exclude web.* tools from used_tools.
        // Non-offline: pass a reference directly to avoid cloning.
        let filtered_tools;
        let effective_used_tools = if self.config.mode.offline {
            filtered_tools = self
                .session
                .used_tools
                .iter()
                .filter(|t| !t.starts_with("web."))
                .cloned()
                .collect();
            &filtered_tools
        } else {
            &self.session.used_tools
        };

        let mut prompt = tool_protocol_system_prompt(
            &self.detected_languages,
            self.mcp_descriptions.as_deref(),
            effective_used_tools,
            self.config.mode.offline,
            self.prompt_tier,
        );

        // Current date and timezone (dynamic, re-evaluated per turn)
        prompt.push_str(&context::format_date_prompt());

        // Custom tools prompt (from ANVIL.md ## tools section)
        let custom_tools = self.tools.custom_tools();
        if !custom_tools.is_empty() {
            prompt.push_str("\n\n## Custom tools (from ANVIL.md)\n\n");
            prompt.push_str("The following project-specific tools are available. Use them like built-in tools.\n\n");
            for tool in custom_tools {
                let display_name = crate::config::custom_tool_display_name(&tool.name);
                prompt.push_str(&format!("### {display_name}\n"));
                prompt.push_str(&format!("Description: {}\n", tool.description));
                if !tool.attributes.is_empty() {
                    prompt.push_str(&format!("Attributes: {}\n", tool.attributes.join(", ")));
                }
                prompt.push('\n');
            }
        }

        // Project instructions (from ANVIL.md)
        if let Some(ref instructions) = self.project_instructions {
            prompt.push_str("\n\n## Project instructions (from ANVIL.md)\n");
            prompt.push_str(instructions);
        }

        // Offline mode annotation
        if self.config.mode.offline {
            prompt.push_str(
                "\n\nNote: Offline mode is active. web.fetch and web.search are unavailable. Do not use shell.exec to make network requests (curl, wget, etc.). Use local tools only."
            );
        }

        // Language constraint (Issue #162)
        {
            use crate::config::{effective_ui_language_code, language_constraint_prompt};
            let lang = effective_ui_language_code(self.config.runtime.ui_language.as_deref());
            prompt.push_str(&language_constraint_prompt(lang));
        }

        // Working memory injection (Issue #130)
        if let Some(wm_prompt) = self.session.working_memory.format_for_prompt() {
            prompt.push_str("\n\n");
            prompt.push_str(&wm_prompt);
        }

        prompt
    }

    /// Convert an absolute path to a cwd-relative string for working memory.
    ///
    /// Returns `None` if the path is not under the session cwd.
    fn relative_path_for_working_memory(&self, abs_path: &std::path::Path) -> Option<String> {
        let cwd = std::path::Path::new(&self.session.metadata.cwd);
        abs_path
            .strip_prefix(cwd)
            .ok()
            .map(|rel| rel.to_string_lossy().into_owned())
    }

    /// Prepare for write fallback: take checkpoint snapshot (Issue #158, DR4-007).
    fn prepare_write_fallback(&mut self) {
        self.checkpoint_stack.mark();
    }

    /// Check whether a shutdown has been requested via the shared flag.
    pub fn is_shutdown_requested(&self) -> bool {
        self.shutdown_flag.load(Ordering::Relaxed)
    }

    /// Check whether the last turn had any tool execution failures.
    /// Used by non-interactive mode to determine exit code.
    pub fn has_tool_execution_failure(&self) -> bool {
        self.session
            .last_turn_tool_results()
            .any(|result| result.is_error)
    }

    /// Check whether a provider error was recorded during this session.
    /// Used by non-interactive mode to detect errors that `run_live_turn`
    /// converted to `AgentEvent::Failed` (returning `Ok` instead of `Err`).
    pub fn has_provider_error(&self) -> bool {
        !self.session.provider_errors.is_empty()
    }

    /// Return the last recorded provider error, if any.
    pub fn last_provider_error(&self) -> Option<&crate::provider::ProviderErrorRecord> {
        self.session.provider_errors.last()
    }

    /// Save the session on exit (wrapper for flush_session).
    pub(crate) fn save_session_on_exit(&mut self) {
        let _ = self.flush_session();
    }

    /// Log session-level summary (Issue #206 CB-003).
    ///
    /// Should be called once when the session actually ends (interactive exit
    /// or non-interactive completion), not per-turn.
    pub(crate) fn log_session_summary(&self) {
        let session_elapsed = self
            .session_stats
            .session_start
            .map(|s| s.elapsed())
            .unwrap_or_default();
        tracing::info!(
            total_turns = self.session_stats.total_turns,
            total_tool_calls = self.session_stats.total_tool_calls(),
            tools = %self.session_stats.tool_calls_summary(),
            files_modified = self.session_stats.files_modified.len(),
            lines_added = self.session_stats.lines_added,
            lines_deleted = self.session_stats.lines_deleted,
            compact_count = self.session_stats.compact_count,
            sidecar_count = self.session_stats.sidecar_count,
            elapsed_s = format!("{:.1}", session_elapsed.as_secs_f64()),
            "session completed"
        );

        // Issue #255: Log agent telemetry (Stage 0 observability).
        let tel = &self.agent_telemetry;
        if tel.total_final_requests > 0 || tel.plan_registration_count > 0 {
            let completion = tel
                .completion_kind
                .map(|k| k.to_string())
                .unwrap_or_else(|| "none".to_string());
            tracing::info!(
                completion_kind = %completion,
                premature_final_count = tel.premature_final_count,
                total_final_requests = tel.total_final_requests,
                pfrr = format!("{:.2}", tel.premature_final_request_rate()),
                plan_registration_count = tel.plan_registration_count,
                plan_update_count = tel.plan_update_count,
                sync_from_touched_files_count = tel.sync_from_touched_files_count,
                no_op_mutation_count = tel.no_op_mutation_count,
                rolled_back_mutation_count = tel.rolled_back_mutation_count,
                initial_plan_miss_count = tel.initial_plan_miss_count,
                "agent telemetry"
            );
        }

        // Issue #271: Write telemetry artifact (opt-in via ANVIL_TELEMETRY_DIR).
        if let Err(err) = self
            .agent_telemetry
            .write_artifact(&self.session.metadata.session_id)
        {
            tracing::warn!("telemetry artifact write failed: {err}");
        }
    }

    /// Run PostSession hook (DR2-005, DR2-007 facade method).
    ///
    /// Builds PostSessionEvent from config and session, then delegates to
    /// HooksEngine. Soft-fail: errors are logged but not propagated.
    pub(crate) fn run_post_session_hook(&mut self) {
        let Some(ref engine) = self.hooks_engine else {
            return;
        };
        let event = crate::hooks::PostSessionEvent {
            hook_point: "PostSession",
            session_id: self.session.metadata.session_id.clone(),
            mode: if self.config.mode.interactive {
                "interactive".to_string()
            } else {
                "non-interactive".to_string()
            },
        };
        if let Err(err) = engine.run_post_session(event) {
            tracing::warn!("PostSession hook error: {err}");
        }
    }

    /// Run PreCompact hook + compact_history (DR1-005 wrapper, DR2-003).
    ///
    /// trigger: "auto" or "manual"
    /// For auto: checks both token-based and message-count thresholds
    /// For manual: unconditional, keep_recent = 8
    fn compact_with_hooks(&mut self, trigger: &str) -> bool {
        let keep_recent = if trigger == "auto" {
            let context_window = self.effective_context_window();
            let context_budget = self.config.runtime.context_budget;
            let params = match compute_compact_params(&self.session, context_window, context_budget)
            {
                Some(p) => p,
                None => return false,
            };
            params.keep_recent
        } else {
            8
        };

        // Run PreCompact hook (soft-fail)
        if let Some(ref engine) = self.hooks_engine {
            let event = crate::hooks::PreCompactEvent {
                hook_point: "PreCompact",
                session_id: self.session.metadata.session_id.clone(),
                trigger: trigger.to_string(),
                message_count: self.session.messages.len(),
                estimated_tokens: self.session.estimated_token_count(),
            };
            if let Err(err) = engine.run_pre_compact(event) {
                tracing::warn!("PreCompact hook error: {err}");
            }
        }

        // Sidecar LLM summarization (Issue #195)
        let llm_summary = self.try_sidecar_summarize();
        let sidecar_used = llm_summary.is_some();

        let before_messages = self.session.messages.len();

        let compacted = self
            .session
            .compact_history_with_llm_summary(keep_recent, llm_summary);

        // Reset context warning tracker after successful compaction (auto/manual)
        if compacted {
            // CB-004: Record compact stats and CompactInfo
            let after_messages = self.session.messages.len();
            self.session_stats.record_compact(sidecar_used);
            self.last_compact_info = Some(CompactInfo {
                sidecar_model: if sidecar_used {
                    self.config.runtime.sidecar_model.clone()
                } else {
                    None
                },
                before_messages,
                after_messages,
            });

            let usage = ContextUsageView {
                estimated_tokens: self.session.estimated_token_count(),
                max_tokens: self.effective_context_window(),
            };
            self.warning_tracker.reset_if_below_threshold(&usage);
        }

        compacted
    }

    /// Attempt sidecar model LLM summarization.
    ///
    /// Returns `None` if sidecar_model is not configured or if the
    /// summarization fails (network error, timeout, etc.).
    fn try_sidecar_summarize(&self) -> Option<String> {
        /// Maximum number of recent messages to include in sidecar summarization input.
        const SIDECAR_SUMMARY_MAX_MESSAGES: usize = 50;
        /// Maximum characters per message in sidecar summarization input.
        /// Increased from 500 to 1000 to preserve function/type signatures
        /// in file.read results (Issue #209).
        const SIDECAR_SUMMARY_MAX_CHARS_PER_MSG: usize = 1000;
        /// Maximum total characters for sidecar summarization input.
        /// Increased from 8000 to 12000 to compensate for the per-message
        /// limit increase while staying well within sidecar model context
        /// windows (Issue #209).
        const SIDECAR_SUMMARY_MAX_TOTAL_CHARS: usize = 12000;

        let model = self.config.runtime.sidecar_model.as_ref()?;
        let sidecar_url = self
            .config
            .runtime
            .sidecar_provider_url
            .as_deref()
            .unwrap_or(crate::config::DEFAULT_OLLAMA_URL);

        tracing::info!(
            sidecar_model = %model,
            sidecar_url = %sidecar_url,
            "Starting sidecar summarization"
        );

        let conversation_text = self.session.conversation_text_for_summary(
            SIDECAR_SUMMARY_MAX_MESSAGES,
            SIDECAR_SUMMARY_MAX_CHARS_PER_MSG,
            SIDECAR_SUMMARY_MAX_TOTAL_CHARS,
        );

        let sidecar_client = crate::provider::OllamaProviderClient::new(sidecar_url);
        sidecar_client.sidecar_summarize(model, &conversation_text)
    }

    /// Get a clone of the shutdown flag for injection into sub-components.
    pub(crate) fn shutdown_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.shutdown_flag)
    }

    pub(crate) fn session_mut(&mut self) -> &mut SessionRecord {
        &mut self.session
    }

    pub fn current_session_name(&self) -> &str {
        &self.current_session_name
    }

    /// Return the effective model name for this session.
    ///
    /// Returns the session-scoped override if set, otherwise falls back to
    /// the config default.
    pub fn effective_model(&self) -> &str {
        self.active_model
            .as_deref()
            .unwrap_or(&self.config.runtime.model)
    }

    /// Return the effective context window for this session.
    ///
    /// Returns the session-scoped override if set, otherwise falls back to
    /// the config default.
    pub fn effective_context_window(&self) -> u32 {
        self.active_context_window
            .unwrap_or(self.config.runtime.context_window)
    }

    /// Return the effective token budget for this session.
    ///
    /// When `context_budget` is configured, returns that value (clamped to
    /// the context window). Otherwise falls back to the context window.
    pub fn effective_token_budget(&self) -> usize {
        let cw = self.effective_context_window();
        match self.config.runtime.context_budget {
            Some(budget) => cw.min(budget) as usize,
            None => cw as usize,
        }
    }

    /// Switch to a different named session.
    ///
    /// Validates the name, saves the current session, builds a new
    /// `SessionStore`, loads or creates the target session, and resets
    /// internal state.
    fn switch_session(&mut self, name: &str) -> Result<Vec<String>, AppError> {
        crate::session::validate_session_name(name)?;

        if self.session.has_pending_turn() {
            return Err(AppError::PendingApprovalRequired);
        }

        // Save current session
        self.flush_session()?;

        // Build new SessionStore
        let new_path = self.config.paths.session_dir.join(format!("{name}.json"));
        let new_store = SessionStore::new(new_path.clone(), self.config.paths.session_dir.clone());

        // Load or create
        let new_session = if new_path.exists() {
            new_store.load()?
        } else {
            SessionRecord::new_named(name, self.config.paths.cwd.clone())?
        };

        // Reset fields
        self.session_store = new_store;
        self.session = new_session;
        self.session.auto_compact_threshold = self.config.runtime.auto_compact_threshold;
        self.session.smart_compact_threshold_ratio =
            self.config.runtime.smart_compact_threshold_ratio;
        self.state_machine =
            StateMachine::from_snapshot(AppStateSnapshot::new(RuntimeState::Ready));
        self.warning_tracker = ContextWarningTracker::new();
        self.current_session_name = name.to_string();
        self.active_model = None;
        self.active_context_window = None;

        // Clear file read cache on session switch (DR2-004)
        if let Ok(mut cache) = self.file_read_cache.lock() {
            cache.clear();
        }

        tracing::info!(session_name = name, "Session switched");

        Ok(vec![format!("Switched to session: {name}")])
    }

    pub fn render_console(&self, tui: &Tui) -> Result<String, AppError> {
        Ok(tui.render_console(&self.build_console_render_context()))
    }

    pub fn startup_console(&mut self, tui: &Tui) -> Result<String, AppError> {
        if self.session.message_count() == 0 && self.session.last_snapshot.is_none() {
            let snapshot = self.initial_snapshot()?;
            return Ok(tui.render_startup(
                &self.config,
                &snapshot,
                self.effective_model(),
                self.effective_context_window(),
            ));
        }

        Ok(format!(
            "{}\n{}",
            render::render_resume_header(
                self.effective_model(),
                self.effective_context_window(),
                &self.config,
                &self.current_session_name,
            ),
            tui.render_console(&self.build_startup_render_context())
        ))
    }

    pub fn startup_events(&self) -> [AppEvent; 3] {
        [
            AppEvent::ConfigLoaded,
            AppEvent::ProviderBootstrapped,
            AppEvent::StartupCompleted,
        ]
    }

    pub fn record_user_input(
        &mut self,
        message_id: impl Into<String>,
        content: impl Into<String>,
    ) -> Result<(), AppError> {
        let content = content.into();
        self.update_active_task_from_user_input(&content);
        self.session
            .push_message(new_user_message(message_id, content));
        self.persist_session(AppEvent::SessionSaved)?;
        Ok(())
    }

    /// Record user input with optional @file expanded content.
    ///
    /// Like `record_user_input` but sets `expanded_content` on the message
    /// before pushing, so that `push_message` uses `effective_content()` for
    /// accurate token estimation (IC-006).
    fn record_user_input_with_expansion(
        &mut self,
        msg_id: &str,
        user_input: &str,
        expanded_content: Option<String>,
    ) -> Result<(), AppError> {
        self.update_active_task_from_user_input(user_input);
        let mut message = new_user_message(msg_id, user_input);
        message.expanded_content = expanded_content;
        self.session.push_message(message);
        self.persist_session(AppEvent::SessionSaved)?;
        Ok(())
    }

    /// Expand @file references in user input (DR1-002: separated from run_live_turn).
    ///
    /// Returns `Some(expanded_text)` when at least one reference was expanded,
    /// or `None` when no @file references are present or all failed.
    fn prepare_expanded_content(&self, user_input: &str) -> Option<String> {
        let (expanded, errors) = context::expand_at_references(
            user_input,
            &self.config.paths.cwd,
            102_400, // 100KB
        );
        for err in &errors {
            eprintln!("@file展開エラー: {}", err);
        }
        expanded
    }

    fn update_active_task_from_user_input(&mut self, user_input: &str) {
        let trimmed = user_input.trim();
        if trimmed.is_empty() {
            return;
        }
        self.session
            .working_memory
            .set_active_task(Some(trimmed.to_string()));
    }

    pub fn record_assistant_output(
        &mut self,
        message_id: impl Into<String>,
        content: impl Into<String>,
    ) -> Result<(), AppError> {
        self.session.push_message(new_assistant_message(
            message_id,
            content,
            MessageStatus::Committed,
        ));
        self.persist_session(AppEvent::SessionSaved)?;
        Ok(())
    }

    pub fn run_runtime_turn(
        &mut self,
        user_input: impl Into<String>,
        runtime: &AgentRuntime,
        tui: &Tui,
    ) -> Result<Vec<String>, AppError> {
        if self.session.has_pending_turn() {
            return Err(AppError::PendingApprovalRequired);
        }
        let user_input = user_input.into();
        self.record_user_input(self.next_message_id("user"), user_input)?;
        self.execute_runtime_events(runtime.events(), tui)
    }

    pub fn run_live_turn<C: ProviderClient>(
        &mut self,
        user_input: impl Into<String>,
        provider_client: &C,
        tui: &Tui,
    ) -> Result<Vec<String>, AppError> {
        if self.session.has_pending_turn() {
            return Err(AppError::PendingApprovalRequired);
        }

        let user_input = user_input.into();

        // @file expansion: expand before recording so push_message gets
        // accurate token estimation via effective_content() (DR1-004).
        let expanded_content = self.prepare_expanded_content(&user_input);
        self.record_user_input_with_expansion(
            &self.next_message_id("user"),
            &user_input,
            expanded_content,
        )?;
        self.begin_live_turn_state()?;

        let (system_prompt, calibration_ratio) = self.prepare_turn_context();
        let (mut request, estimated_prompt_tokens) = BasicAgentLoop::build_turn_request_calibrated(
            self.effective_model().to_string(),
            &self.session,
            self.provider.capabilities.streaming && self.config.runtime.stream,
            self.effective_context_window(),
            &system_prompt,
            calibration_ratio,
            self.config.runtime.context_budget,
        );
        request.max_output_tokens = self.config.runtime.max_output_tokens;
        self.last_estimated_prompt_tokens = Some(estimated_prompt_tokens);

        // Phase 1: Collect events from provider with spinner + streaming output.
        let mut spinner_opt = Some(Spinner::start(
            format!("Thinking. model={}", self.effective_model()),
            self.config.mode.interactive,
        ));

        let mut token_buffer = String::new();
        let mut collected_events: Vec<ProviderEvent> = Vec::new();
        let mut first_token = true;

        let stream_result = provider_client.stream_turn(&request, &mut |event| {
            // Stop spinner completely before any output (joins the thread)
            if let Some(s) = spinner_opt.take() {
                s.stop();
            }

            match &event {
                ProviderEvent::TokenDelta(delta) => {
                    token_buffer.push_str(delta);
                    if first_token {
                        first_token = false;
                    }
                    let _ = write!(io::stderr(), "{delta}");
                    let _ = io::stderr().flush();
                }
                ProviderEvent::Agent(_) => {}
            }
            collected_events.push(event);
        });

        // Ensure spinner is stopped if no events arrived
        if let Some(s) = spinner_opt.take() {
            s.stop();
        }

        // End streaming output with newline
        if !first_token {
            let _ = writeln!(io::stderr());
        }

        // Phase 2: Process collected events for state management.
        let result = match stream_result {
            Ok(()) => {
                let mut frames = Vec::new();
                for (index, event) in collected_events.iter().enumerate() {
                    match event {
                        ProviderEvent::Agent(agent_event) => {
                            if let Some(structured_frames) =
                                self.handle_structured_done(agent_event, tui, provider_client)?
                            {
                                frames.extend(structured_frames);
                            } else {
                                let snapshot = self.apply_agent_event(agent_event)?;
                                frames.push(self.render_console(tui)?);
                                if snapshot.state == RuntimeState::AwaitingApproval {
                                    let remaining_events = collected_events[index + 1..]
                                        .iter()
                                        .filter_map(|ev| match ev {
                                            ProviderEvent::Agent(ae) => Some(ae.clone()),
                                            ProviderEvent::TokenDelta(_) => None,
                                        })
                                        .collect::<Vec<_>>();
                                    self.set_pending_turn(PendingTurnState {
                                        waiting_tool_call_id: render::approval_tool_call_id(
                                            agent_event,
                                        ),
                                        remaining_events,
                                        pending_tool_calls: Vec::new(),
                                    })?;
                                    break;
                                }
                            }
                        }
                        ProviderEvent::TokenDelta(_) => {
                            // Already streamed to stderr. Check for structured response.
                            if BasicAgentLoop::is_complete_structured_response(&token_buffer) {
                                let structured =
                                    BasicAgentLoop::parse_structured_response_with_registry(
                                        &token_buffer,
                                        &self.tools,
                                    )
                                    .map_err(AppError::ToolExecution)?;
                                // Issue #173: Pass anvil_final_detected from parsed response
                                let anvil_final = structured.anvil_final_detected;
                                frames.extend(self.complete_structured_response(
                                    structured,
                                    "Done. session saved",
                                    "session saved",
                                    0,
                                    None,
                                    tui,
                                    provider_client,
                                    anvil_final,
                                )?);
                                break;
                            }
                        }
                    }
                }
                Ok(frames)
            }
            Err(ProviderTurnError::Cancelled) => {
                self.record_provider_error(ProviderTurnError::Cancelled)?;
                self.execute_runtime_events(
                    &[AgentEvent::Interrupted {
                        status: "Interrupted safely".to_string(),
                        interrupted_what: "provider turn".to_string(),
                        saved_status: "session preserved".to_string(),
                        next_actions: vec!["resume work".to_string(), "inspect status".to_string()],
                        elapsed_ms: 0,
                    }],
                    tui,
                )
            }
            Err(ref err @ ProviderTurnError::ConnectionRefused(ref msg)) => {
                self.record_provider_error(err.clone())?;
                self.execute_runtime_events(
                    &[AgentEvent::Failed {
                        status: "Error. connection refused".to_string(),
                        error_summary: msg.clone(),
                        recommended_actions: vec![
                            "check provider is running (e.g. ollama serve)".to_string(),
                            "verify provider URL".to_string(),
                        ],
                        elapsed_ms: 0,
                    }],
                    tui,
                )
            }
            Err(ref err @ ProviderTurnError::DnsFailure(ref msg)) => {
                self.record_provider_error(err.clone())?;
                self.execute_runtime_events(
                    &[AgentEvent::Failed {
                        status: "Error. DNS resolution failed".to_string(),
                        error_summary: msg.clone(),
                        recommended_actions: vec![
                            "check provider URL for typos".to_string(),
                            "verify DNS settings and network connectivity".to_string(),
                        ],
                        elapsed_ms: 0,
                    }],
                    tui,
                )
            }
            Err(ref err @ ProviderTurnError::ModelNotFound { ref model, .. }) => {
                self.record_provider_error(err.clone())?;
                self.execute_runtime_events(
                    &[AgentEvent::Failed {
                        status: "Error. model not found".to_string(),
                        error_summary: format!("model '{}' not found", model),
                        recommended_actions: vec![
                            format!("download model: ollama pull {}", model),
                            "list available models: ollama list".to_string(),
                        ],
                        elapsed_ms: 0,
                    }],
                    tui,
                )
            }
            Err(ref err @ ProviderTurnError::AuthenticationFailed { ref message, .. }) => {
                self.record_provider_error(err.clone())?;
                self.execute_runtime_events(
                    &[AgentEvent::Failed {
                        status: "Error. authentication failed".to_string(),
                        error_summary: message.clone(),
                        recommended_actions: vec![
                            "check API key: export ANVIL_API_KEY=<key>".to_string(),
                            "verify provider credentials".to_string(),
                        ],
                        elapsed_ms: 0,
                    }],
                    tui,
                )
            }
            Err(
                ref err @ ProviderTurnError::Network(ref msg)
                | ref err @ ProviderTurnError::ServerError {
                    message: ref msg, ..
                }
                | ref err @ ProviderTurnError::Timeout(ref msg)
                | ref err @ ProviderTurnError::Backend(ref msg),
            ) => {
                self.record_provider_error(err.clone())?;
                self.execute_runtime_events(
                    &[AgentEvent::Failed {
                        status: "Error. provider turn failed".to_string(),
                        error_summary: msg.clone(),
                        recommended_actions: vec![
                            "retry turn".to_string(),
                            "inspect provider".to_string(),
                        ],
                        elapsed_ms: 0,
                    }],
                    tui,
                )
            }
            Err(
                ref err @ ProviderTurnError::ClientError {
                    status_code,
                    ref message,
                },
            ) => {
                self.record_provider_error(err.clone())?;
                self.execute_runtime_events(
                    &[AgentEvent::Failed {
                        status: "Error. provider turn failed".to_string(),
                        error_summary: format!("client error ({status_code}): {message}"),
                        recommended_actions: vec![
                            "check authentication credentials".to_string(),
                            "verify API key and provider URL".to_string(),
                        ],
                        elapsed_ms: 0,
                    }],
                    tui,
                )
            }
            Err(ref err @ ProviderTurnError::Parse(ref msg)) => {
                self.record_provider_error(err.clone())?;
                self.execute_runtime_events(
                    &[AgentEvent::Failed {
                        status: "Error. provider turn failed".to_string(),
                        error_summary: format!("parse error: {msg}"),
                        recommended_actions: vec![
                            "retry turn".to_string(),
                            "check model compatibility".to_string(),
                        ],
                        elapsed_ms: 0,
                    }],
                    tui,
                )
            }
        };

        self.flush_session()?;
        result
    }

    pub fn approve_and_continue(
        &mut self,
        _runtime: &AgentRuntime,
        tui: &Tui,
    ) -> Result<Vec<String>, AppError> {
        let pending_turn = self
            .session
            .pending_turn
            .take()
            .ok_or(AppError::NoPendingApproval)?;
        self.persist_session(AppEvent::SessionSaved)?;
        let result = self.execute_runtime_events(&pending_turn.remaining_events, tui);
        self.flush_session()?;
        result
    }

    pub fn deny_and_abort(&mut self, tui: &Tui) -> Result<Vec<String>, AppError> {
        if !self.session.has_pending_turn() {
            return Err(AppError::NoPendingApproval);
        }

        self.clear_pending_turn()?;
        self.record_assistant_output(
            self.next_message_id("assistant"),
            "Approval denied. No tool was executed.",
        )?;
        let snapshot = AppStateSnapshot::new(RuntimeState::Ready)
            .with_status("Approval denied. Ready for the next task".to_string())
            .with_completion_summary("Approval denied. No tool was executed.", "no changes made");
        self.transition_with_context(snapshot, StateTransition::ResetToReady)?;
        self.flush_session()?;
        Ok(vec![self.render_console(tui)?])
    }

    pub fn reset_to_ready(&mut self) -> Result<AppStateSnapshot, AppError> {
        self.clear_pending_turn()?;
        let snapshot = AppStateSnapshot::new(RuntimeState::Ready)
            .with_status("Ready for the next task".to_string());
        self.transition_with_context(snapshot, StateTransition::ResetToReady)
    }

    pub(crate) fn apply_transition_for_mock(
        &mut self,
        snapshot: AppStateSnapshot,
        transition: StateTransition,
    ) -> Result<AppStateSnapshot, AppError> {
        self.apply_transition(snapshot, transition)
    }

    fn apply_transition(
        &mut self,
        snapshot: AppStateSnapshot,
        transition: StateTransition,
    ) -> Result<AppStateSnapshot, AppError> {
        self.state_machine
            .transition_to(snapshot.clone(), transition)?;
        self.session
            .set_last_snapshot(self.state_machine.snapshot().clone());
        self.persist_session(AppEvent::SessionSaved)?;
        Ok(snapshot)
    }

    /// Build a snapshot with context usage attached and apply the transition.
    ///
    /// Reduces boilerplate by combining the common `with_context_usage` +
    /// `apply_transition` pattern.
    fn transition_with_context(
        &mut self,
        snapshot: AppStateSnapshot,
        transition: StateTransition,
    ) -> Result<AppStateSnapshot, AppError> {
        let snapshot = snapshot.with_context_usage(
            self.session.estimated_token_count(),
            self.effective_context_window(),
        );
        self.apply_transition(snapshot, transition)
    }

    /// Evaluate context warning on a snapshot and update the state machine if needed.
    pub(crate) fn evaluate_context_warning(&mut self, snapshot: &mut AppStateSnapshot) {
        if let Some(usage) = &snapshot.context_usage
            && let Some(level) = self.warning_tracker.evaluate(usage)
        {
            snapshot.context_warning = Some(level);
            self.state_machine.replace_snapshot(snapshot.clone());
            self.session
                .set_last_snapshot(self.state_machine.snapshot().clone());
        }
    }

    pub(crate) fn persist_session_event_for_mock(
        &mut self,
        event: AppEvent,
    ) -> Result<(), AppError> {
        self.persist_session(event)
    }

    fn persist_session(&mut self, event: AppEvent) -> Result<(), AppError> {
        self.session.record_event(event);
        Ok(())
    }

    /// Flush session to disk if the dirty flag is set.
    /// Also runs deferred auto-compaction before writing.
    /// In non-interactive mode, skip disk persistence but still run auto-compact.
    fn flush_session(&mut self) -> Result<(), AppError> {
        // Auto-compact must run regardless of interactive mode (Issue #202).
        // Non-interactive sessions (--exec-file, --exec, --oneshot) also need
        // sidecar-LLM summarization to keep context within budget.
        self.compact_with_hooks("auto");
        if !self.config.mode.interactive {
            return Ok(());
        }
        if self.session.dirty {
            self.session_store.save(&self.session)?;
            self.session.clear_dirty();
        }
        Ok(())
    }

    /// Immediately persist session to disk (for crash-safety critical paths).
    /// In non-interactive mode, skip disk persistence entirely.
    fn persist_session_immediate(&mut self, event: AppEvent) -> Result<(), AppError> {
        if !self.config.mode.interactive {
            self.session.record_event(event);
            return Ok(());
        }
        self.session.record_event(event);
        self.session_store.save(&self.session)?;
        self.session.clear_dirty();
        Ok(())
    }

    fn execute_runtime_events(
        &mut self,
        events: &[AgentEvent],
        tui: &Tui,
    ) -> Result<Vec<String>, AppError> {
        let mut frames = Vec::new();

        for (index, event) in events.iter().enumerate() {
            let snapshot = self.apply_agent_event(event)?;
            frames.push(self.render_console(tui)?);

            if snapshot.state == RuntimeState::AwaitingApproval {
                self.set_pending_turn(PendingTurnState {
                    waiting_tool_call_id: render::approval_tool_call_id(event),
                    remaining_events: events[index + 1..].to_vec(),
                    pending_tool_calls: Vec::new(),
                })?;
                break;
            }
        }

        if !self.session.has_pending_turn() {
            self.clear_pending_turn()?;
        }

        self.flush_session()?;
        Ok(frames)
    }

    fn apply_agent_event(&mut self, event: &AgentEvent) -> Result<AppStateSnapshot, AppError> {
        match event {
            AgentEvent::Thinking {
                status,
                plan_items,
                active_index,
                reasoning_summary,
                elapsed_ms,
            } => {
                let snapshot = AppStateSnapshot::new(RuntimeState::Thinking)
                    .with_status(status.clone())
                    .with_plan(plan_items.clone(), *active_index)
                    .with_reasoning_summary(reasoning_summary.clone())
                    .with_elapsed_ms(*elapsed_ms);

                let transition = match self.state_machine.snapshot().state {
                    RuntimeState::Working => StateTransition::ResumeThinking,
                    _ => StateTransition::StartThinking,
                };

                self.transition_with_context(snapshot, transition)
            }
            AgentEvent::ApprovalRequested {
                status,
                tool_name,
                summary,
                risk,
                tool_call_id,
                elapsed_ms,
            } => {
                // Attempt to generate a diff preview from pending tool calls.
                let diff_preview = self
                    .session
                    .pending_turn
                    .as_ref()
                    .and_then(|pending| {
                        pending
                            .pending_tool_calls
                            .iter()
                            .find(|tc| tc.tool_call_id == *tool_call_id)
                    })
                    .and_then(|tc| {
                        let diff_options =
                            crate::tooling::diff::DiffOptions::from_runtime(&self.config.runtime);
                        crate::tooling::diff::generate_diff_preview(
                            &self.config.paths.cwd,
                            &tc.input,
                            &diff_options,
                        )
                    });
                let snapshot = AppStateSnapshot::new(RuntimeState::AwaitingApproval)
                    .with_status(status.clone())
                    .with_approval(
                        tool_name.clone(),
                        summary.clone(),
                        risk.clone(),
                        tool_call_id.clone(),
                    )
                    .with_diff_preview(diff_preview)
                    .with_elapsed_ms(*elapsed_ms);
                self.transition_with_context(snapshot, StateTransition::RequestApproval)
            }
            AgentEvent::Working {
                status,
                plan_items,
                active_index,
                tool_logs,
                elapsed_ms,
            } => {
                let snapshot = AppStateSnapshot::new(RuntimeState::Working)
                    .with_status(status.clone())
                    .with_plan(plan_items.clone(), *active_index)
                    .with_tool_logs(render::build_tool_logs(tool_logs))
                    .with_elapsed_ms(*elapsed_ms);
                self.transition_with_context(snapshot, StateTransition::StartWorking)
            }
            AgentEvent::Done {
                status,
                assistant_message,
                completion_summary,
                saved_status,
                tool_logs,
                elapsed_ms,
                inference_performance,
            } => {
                self.record_assistant_output(self.next_message_id("assistant"), assistant_message)?;

                // Update calibration store with actual vs estimated prompt tokens.
                let estimated = self.last_estimated_prompt_tokens.take();
                if let Some(perf) = inference_performance
                    && let (Some(actual_prompt), Some(est)) = (perf.prompt_tokens, estimated)
                {
                    let model = self.effective_model().to_string();
                    self.calibration_store.update(&model, actual_prompt, est);
                }

                let mut snapshot = AppStateSnapshot::new(RuntimeState::Done)
                    .with_status(status.clone())
                    .with_tool_logs(render::build_tool_logs(tool_logs))
                    .with_completion_summary(completion_summary.clone(), saved_status.clone())
                    .with_elapsed_ms(*elapsed_ms);
                if let Some(perf) = inference_performance {
                    snapshot = snapshot.with_inference_performance(perf.clone());
                }
                let mut snapshot =
                    self.transition_with_context(snapshot, StateTransition::Finish)?;
                self.evaluate_context_warning(&mut snapshot);
                Ok(snapshot)
            }
            AgentEvent::Interrupted {
                status,
                interrupted_what,
                saved_status,
                next_actions,
                elapsed_ms,
            } => {
                self.session.normalize_interrupted_turn(interrupted_what);
                let snapshot = AppStateSnapshot::new(RuntimeState::Interrupted)
                    .with_status(status.clone())
                    .with_interrupt(
                        interrupted_what.clone(),
                        saved_status.clone(),
                        next_actions.clone(),
                    )
                    .with_elapsed_ms(*elapsed_ms);
                self.transition_with_context(snapshot, StateTransition::Interrupt)?;
                self.persist_session(AppEvent::SessionNormalizedAfterInterrupt)?;
                Ok(self.state_machine.snapshot().clone())
            }
            AgentEvent::Failed {
                status,
                error_summary,
                recommended_actions,
                elapsed_ms,
            } => {
                let snapshot = AppStateSnapshot::new(RuntimeState::Error)
                    .with_status(status.clone())
                    .with_error_summary(error_summary.clone(), recommended_actions.clone())
                    .with_elapsed_ms(*elapsed_ms);
                self.transition_with_context(snapshot, StateTransition::Fail)
            }
        }
    }

    fn build_console_render_context(&self) -> ConsoleRenderContext {
        // Exclude all messages from frame rendering because they were
        // already shown during the live turn — streaming to stderr and
        // tool execution output (Issue #1).
        self.session.console_render_context(
            self.state_machine.snapshot(),
            self.effective_model(),
            self.config.runtime.max_console_messages,
            true,
        )
    }

    fn build_startup_render_context(&self) -> ConsoleRenderContext {
        // On startup/resume, include assistant messages so the user can see
        // the conversation history from the previous session.
        self.session.console_render_context(
            self.state_machine.snapshot(),
            self.effective_model(),
            self.config.runtime.max_console_messages,
            false,
        )
    }

    fn next_message_id(&self, prefix: &str) -> String {
        format!("{prefix}_{:04}", self.session.message_count() + 1)
    }

    fn begin_live_turn_state(&mut self) -> Result<(), AppError> {
        let snapshot = AppStateSnapshot::new(RuntimeState::Thinking)
            .with_status(format!("Thinking. model={}", self.effective_model()))
            .with_elapsed_ms(0);
        self.transition_with_context(snapshot, StateTransition::StartThinking)?;
        Ok(())
    }

    fn record_provider_error(&mut self, error: ProviderTurnError) -> Result<(), AppError> {
        let kind = ProviderErrorKind::from(&error);
        let message = error.to_string();

        self.session.push_message(
            SessionMessage::new(MessageRole::System, "provider", message.clone())
                .with_id(self.next_message_id("provider")),
        );
        self.session
            .push_provider_error(ProviderErrorRecord { kind, message });
        self.persist_session(AppEvent::SessionSaved)?;
        self.flush_session()
    }

    pub fn has_pending_runtime_events(&self) -> bool {
        self.session.has_pending_turn()
    }

    /// Process a single line of CLI input.
    ///
    /// Dispatches slash-commands to the extension registry and regular text
    /// to the live provider turn.
    pub fn handle_cli_line<C: ProviderClient>(
        &mut self,
        line: &str,
        provider_client: &C,
        tui: &Tui,
    ) -> Result<CliTurnOutput, AppError> {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return Ok(CliTurnOutput {
                frames: vec![self.render_console(tui)?],
                control: SessionControl::Continue,
            });
        }

        if trimmed.starts_with('/') {
            return self.handle_slash_command(trimmed, provider_client, tui);
        }

        self.run_turn_to_output(trimmed, provider_client, tui)
    }

    /// Run a live turn and wrap the result into a `CliTurnOutput`.
    fn run_turn_to_output<C: ProviderClient>(
        &mut self,
        prompt: impl Into<String>,
        provider_client: &C,
        tui: &Tui,
    ) -> Result<CliTurnOutput, AppError> {
        match self.run_live_turn(prompt, provider_client, tui) {
            Ok(frames) => Ok(CliTurnOutput {
                frames,
                control: SessionControl::Continue,
            }),
            Err(AppError::PendingApprovalRequired) => Ok(CliTurnOutput {
                frames: vec![render::render_pending_approval_frame(
                    self.state_machine.snapshot(),
                )],
                control: SessionControl::Continue,
            }),
            Err(err) => Err(err),
        }
    }

    fn set_pending_turn(&mut self, pending_turn: PendingTurnState) -> Result<(), AppError> {
        self.session.set_pending_turn(pending_turn);
        self.persist_session_immediate(AppEvent::SessionSaved)
    }

    fn clear_pending_turn(&mut self) -> Result<(), AppError> {
        if !self.session.has_pending_turn() {
            return Ok(());
        }
        self.session.clear_pending_turn();
        self.persist_session_immediate(AppEvent::SessionSaved)
    }

    fn handle_slash_command(
        &mut self,
        command: &str,
        provider_client: &impl ProviderClient,
        tui: &Tui,
    ) -> Result<CliTurnOutput, AppError> {
        let output = match self
            .extensions
            .find_slash_command(command)
            .map(|spec| spec.action)
        {
            Some(SlashCommandAction::Help) => CliTurnOutput {
                frames: vec![render::render_help_frame_for(
                    self.extensions.slash_commands(),
                )],
                control: SessionControl::Continue,
            },
            Some(SlashCommandAction::Status) => CliTurnOutput {
                frames: vec![format!(
                    "{}\n{}",
                    self.render_console(tui)?,
                    render::render_status_detail(self.state_machine.snapshot())
                )],
                control: SessionControl::Continue,
            },
            Some(SlashCommandAction::Plan) => CliTurnOutput {
                frames: vec![render::render_plan_frame(self.state_machine.snapshot())],
                control: SessionControl::Continue,
            },
            Some(SlashCommandAction::PlanAdd(item)) => CliTurnOutput {
                frames: vec![self.add_plan_item(item)?],
                control: SessionControl::Continue,
            },
            Some(SlashCommandAction::PlanFocus(index)) => CliTurnOutput {
                frames: vec![self.focus_plan_item(index)?],
                control: SessionControl::Continue,
            },
            Some(SlashCommandAction::PlanClear) => CliTurnOutput {
                frames: vec![self.clear_plan_items()?],
                control: SessionControl::Continue,
            },
            Some(SlashCommandAction::Checkpoint(note)) => CliTurnOutput {
                frames: vec![self.save_plan_checkpoint(note)?],
                control: SessionControl::Continue,
            },
            Some(SlashCommandAction::RepoFind(query)) => CliTurnOutput {
                frames: vec![self.repo_find(&query)?],
                control: SessionControl::Continue,
            },
            Some(SlashCommandAction::Timeline) => CliTurnOutput {
                frames: vec![self.session.render_timeline(8)],
                control: SessionControl::Continue,
            },
            Some(SlashCommandAction::Compact) => CliTurnOutput {
                frames: vec![self.compact_session_history()?],
                control: SessionControl::Continue,
            },
            Some(SlashCommandAction::ModelInfo) => {
                let model = self.effective_model().to_string();
                if self.provider.backend == crate::provider::ProviderBackend::Ollama {
                    if let Some(info) = crate::provider::fetch_model_info_from_ollama(
                        &self.config.runtime.provider_url,
                        &model,
                    ) {
                        CliTurnOutput {
                            frames: vec![render::render_model_info_frame(
                                &model,
                                &info,
                                self.effective_context_window(),
                            )],
                            control: SessionControl::Continue,
                        }
                    } else {
                        CliTurnOutput {
                            frames: vec![render::render_model_frame(
                                &model,
                                &self.config.runtime.provider,
                                self.effective_context_window(),
                            )],
                            control: SessionControl::Continue,
                        }
                    }
                } else {
                    CliTurnOutput {
                        frames: vec![render::render_model_frame(
                            &model,
                            &self.config.runtime.provider,
                            self.effective_context_window(),
                        )],
                        control: SessionControl::Continue,
                    }
                }
            }
            Some(SlashCommandAction::ModelList) => {
                if self.provider.backend == crate::provider::ProviderBackend::Ollama {
                    if let Some(models) = crate::provider::fetch_model_list_from_ollama(
                        &self.config.runtime.provider_url,
                    ) {
                        let model = self.effective_model().to_string();
                        CliTurnOutput {
                            frames: vec![render::render_model_list_frame(&models, &model)],
                            control: SessionControl::Continue,
                        }
                    } else {
                        CliTurnOutput {
                            frames: vec![
                                "[A] anvil > failed to fetch model list from Ollama".to_string(),
                            ],
                            control: SessionControl::Continue,
                        }
                    }
                } else {
                    CliTurnOutput {
                        frames: vec![
                            "[A] anvil > model list is only available for Ollama provider"
                                .to_string(),
                        ],
                        control: SessionControl::Continue,
                    }
                }
            }
            Some(SlashCommandAction::ModelSwitch(name)) => self.switch_model(&name),
            Some(SlashCommandAction::Provider) => CliTurnOutput {
                frames: vec![render::render_provider_frame(
                    self.effective_model(),
                    &self.config,
                    &self.provider,
                )],
                control: SessionControl::Continue,
            },
            Some(SlashCommandAction::Approve) => CliTurnOutput {
                frames: self.approve_and_continue(&AgentRuntime::new(), tui)?,
                control: SessionControl::Continue,
            },
            Some(SlashCommandAction::Deny) => CliTurnOutput {
                frames: self.deny_and_abort(tui)?,
                control: SessionControl::Continue,
            },
            Some(SlashCommandAction::Trust(action)) => {
                let msg = match action {
                    TrustAction::Show => {
                        if self.trust_all {
                            let mut lines = vec![
                                "Trust mode: ON (all)".to_string(),
                                "  Trusted tools: (all non-MCP tools)".to_string(),
                            ];
                            if !self.trusted_tools.is_empty() {
                                let mut tools: Vec<_> =
                                    self.trusted_tools.iter().cloned().collect();
                                tools.sort();
                                lines.push(format!("  Individually trusted: {}", tools.join(", ")));
                            }
                            lines.join("\n")
                        } else if !self.trusted_tools.is_empty() {
                            let mut tools: Vec<_> = self.trusted_tools.iter().cloned().collect();
                            tools.sort();
                            let mut lines = vec!["Trust mode: ON (selective)".to_string()];
                            lines.push("  Trusted tools:".to_string());
                            for tool in &tools {
                                lines.push(format!("    - {tool}"));
                            }
                            lines.join("\n")
                        } else {
                            "Trust mode: OFF\n  No tools are trusted. Use /trust <tool> or /trust all.".to_string()
                        }
                    }
                    TrustAction::Tool(name) => {
                        if self.tools.get(&name).is_some() {
                            self.trusted_tools.insert(name.clone());
                            format!("Trusted: {name}")
                        } else {
                            format!("Unknown tool: {name}. Use /trust to see trusted tools.")
                        }
                    }
                    TrustAction::All => {
                        self.trust_all = true;
                        "Trust mode: ON (all non-MCP tools auto-approved)".to_string()
                    }
                    TrustAction::Off => {
                        self.trust_all = false;
                        self.trusted_tools.clear();
                        "Trust mode: OFF".to_string()
                    }
                };
                CliTurnOutput {
                    frames: vec![msg],
                    control: SessionControl::Continue,
                }
            }
            Some(SlashCommandAction::Reset) => {
                let _ = self.reset_to_ready()?;
                CliTurnOutput {
                    frames: vec![self.render_console(tui)?],
                    control: SessionControl::Continue,
                }
            }
            Some(SlashCommandAction::Exit) => CliTurnOutput {
                frames: vec!["Exiting Anvil.".to_string()],
                control: SessionControl::Exit,
            },
            Some(SlashCommandAction::Undo(n)) => {
                let msg = self.execute_undo(n)?;
                CliTurnOutput {
                    frames: vec![msg],
                    control: SessionControl::Continue,
                }
            }
            Some(SlashCommandAction::Prompt(prompt)) => {
                self.run_turn_to_output(prompt, provider_client, tui)?
            }
            Some(SlashCommandAction::Skill {
                args,
                content,
                skill_dir,
                ..
            }) => {
                let prompt =
                    crate::extensions::skills::expand_variables(&content, &args, &skill_dir);
                self.run_turn_to_output(prompt, provider_client, tui)?
            }
            Some(SlashCommandAction::SessionList) => {
                let sessions = self.session_store.list_sessions()?;
                let output = if sessions.is_empty() {
                    "[A] anvil > no sessions found".to_string()
                } else {
                    let mut lines = vec![format!("[A] anvil > {} session(s)", sessions.len())];
                    for info in &sessions {
                        let marker = if info.name == self.current_session_name {
                            " *"
                        } else {
                            ""
                        };
                        lines.push(format!(
                            "  {}{} ({} messages)",
                            info.name, marker, info.message_count
                        ));
                    }
                    lines.join("\n")
                };
                CliTurnOutput {
                    frames: vec![output],
                    control: SessionControl::Continue,
                }
            }
            Some(SlashCommandAction::SessionDelete(name)) => {
                if name == self.current_session_name {
                    CliTurnOutput {
                        frames: vec![format!(
                            "[A] anvil > cannot delete the active session: {name}"
                        )],
                        control: SessionControl::Continue,
                    }
                } else {
                    match self.session_store.delete_session(&name) {
                        Ok(()) => CliTurnOutput {
                            frames: vec![format!("[A] anvil > deleted session: {name}")],
                            control: SessionControl::Continue,
                        },
                        Err(e) => CliTurnOutput {
                            frames: vec![format!("[A] anvil > {e}")],
                            control: SessionControl::Continue,
                        },
                    }
                }
            }
            Some(SlashCommandAction::SessionSwitch(name)) => match self.switch_session(&name) {
                Ok(frames) => CliTurnOutput {
                    frames,
                    control: SessionControl::Continue,
                },
                Err(e) => CliTurnOutput {
                    frames: vec![format!("[A] anvil > {e}")],
                    control: SessionControl::Continue,
                },
            },
            _ => {
                let suggestion = self.extensions.suggest_command(command);
                let msg = if let Some(suggested) = suggestion {
                    format!(
                        "Unknown command: {command}\nDid you mean: {suggested}?\nTry /help for available commands."
                    )
                } else {
                    format!("Unknown command: {command}\nTry /help for available commands.")
                };
                CliTurnOutput {
                    frames: vec![msg],
                    control: SessionControl::Continue,
                }
            }
        };

        self.flush_session()?;
        Ok(output)
    }

    fn repo_find(&mut self, query: &str) -> Result<String, AppError> {
        let cache_path = default_cache_path(&self.config.paths.state_dir);
        let index = RepositoryIndex::load_or_build(&self.config.paths.cwd, &cache_path)
            .map_err(|err| AppError::ToolExecution(err.to_string()))?;
        let result = index.search(query, DEFAULT_SEARCH_LIMIT);
        if !result.matches.is_empty() {
            let summary = result
                .matches
                .iter()
                .map(|item| format!("{} (score {})", item.path, item.score))
                .collect::<Vec<_>>()
                .join(", ");
            self.session.push_message(
                SessionMessage::new(
                    MessageRole::System,
                    "anvil",
                    format!("[retrieval context] query={query}; matches={summary}"),
                )
                .with_id(self.next_message_id("retrieval")),
            );
            self.persist_session(AppEvent::SessionSaved)?;
        }
        Ok(render_retrieval_result(&result))
    }

    /// Capture a checkpoint for file-mutating tools before execution.
    ///
    /// Returns `Some(CheckpointEntry)` when the tool has
    /// `RollbackPolicy::CheckpointBeforeWrite` and the file can be read.
    pub(crate) fn capture_checkpoint_if_needed(
        &self,
        request: &crate::tooling::ToolExecutionRequest,
        cwd: &std::path::Path,
    ) -> Option<crate::tooling::CheckpointEntry> {
        use crate::tooling::{
            CHECKPOINT_FILE_SIZE_LIMIT, RollbackPolicy, ToolInput, diff::is_binary_content,
        };

        if request.spec.rollback_policy != RollbackPolicy::CheckpointBeforeWrite {
            return None;
        }

        let rel_path = match &request.input {
            ToolInput::FileWrite { path, .. }
            | ToolInput::FileEdit { path, .. }
            | ToolInput::FileEditAnchor { path, .. } => path,
            _ => return None,
        };

        let resolved = crate::tooling::resolve_sandbox_path(cwd, rel_path).ok()?;

        match std::fs::read(&resolved) {
            Ok(bytes) => {
                if is_binary_content(&bytes) || bytes.len() as u64 > CHECKPOINT_FILE_SIZE_LIMIT {
                    return None;
                }
                let content = String::from_utf8(bytes).ok()?;
                let byte_size = content.len();
                Some(crate::tooling::CheckpointEntry {
                    path: resolved,
                    previous_content: Some(content),
                    byte_size,
                })
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                // New file -- record None so undo can delete it
                Some(crate::tooling::CheckpointEntry {
                    path: resolved,
                    previous_content: None,
                    byte_size: 0,
                })
            }
            Err(_) => None,
        }
    }

    /// Execute an undo operation, restoring up to `n` checkpoint entries.
    fn execute_undo(&mut self, n: usize) -> Result<String, AppError> {
        if self.checkpoint_stack.is_empty() {
            return Ok("No changes to undo.".to_string());
        }

        let entries = self.checkpoint_stack.pop_n(n);
        let results: Vec<crate::tooling::RestoreResult> =
            entries.iter().map(|entry| entry.restore()).collect();

        let restored_count = results
            .iter()
            .filter(|r| r.action != crate::tooling::RestoreAction::Skipped)
            .count();

        // Build result message
        let mut lines = Vec::new();
        for result in &results {
            let action_str = match result.action {
                crate::tooling::RestoreAction::ContentRestored => "restored",
                crate::tooling::RestoreAction::FileRemoved => "removed",
                crate::tooling::RestoreAction::Skipped => "skipped",
            };
            lines.push(format!("  {} {}", action_str, result.path.display()));
            if let Some(ref preview) = result.diff_preview {
                for diff_line in preview.lines().take(10) {
                    lines.push(format!("    {diff_line}"));
                }
            }
        }

        let summary = if restored_count == entries.len() {
            format!("Undid {} change(s).", restored_count)
        } else {
            format!("Undid {} of {} requested change(s).", restored_count, n)
        };
        lines.insert(0, summary.clone());

        // Invalidate file read cache for restored paths
        if let Ok(mut cache) = self.file_read_cache.lock() {
            for entry in &entries {
                cache.invalidate(&entry.path);
            }
        }

        // Sync working memory: remove undone files from touched_files (Issue #130)
        for result in &results {
            if result.action != crate::tooling::RestoreAction::Skipped
                && let Some(rel) = self.relative_path_for_working_memory(&result.path)
            {
                self.session.working_memory.remove_touched_file(&rel);
            }
        }

        // Record in session
        self.session.push_message(
            SessionMessage::new(MessageRole::System, "anvil", format!("[undo] {summary}"))
                .with_id(self.next_message_id("undo")),
        );
        self.persist_session(AppEvent::UndoExecuted)?;

        Ok(lines.join("\n"))
    }

    /// Apply a model switch: validate the model and update active overrides.
    fn apply_model_switch(&mut self, model_name: &str) -> Result<u32, String> {
        if self.provider.backend == crate::provider::ProviderBackend::Ollama {
            match crate::provider::fetch_model_info_from_ollama(
                &self.config.runtime.provider_url,
                model_name,
            ) {
                Some(info) => {
                    self.active_model = Some(model_name.to_string());
                    if let Some(ctx) = info.context_length {
                        self.active_context_window = Some(ctx);
                    }
                    Ok(self.effective_context_window())
                }
                None => Err(format!(
                    "model '{}' not found. Use /model list to see available models.",
                    model_name
                )),
            }
        } else {
            // OpenAI-compatible: set model name only (no API validation)
            self.active_model = Some(model_name.to_string());
            Ok(self.effective_context_window())
        }
    }

    /// Handle /model switch command dispatch.
    fn switch_model(&mut self, model_name: &str) -> CliTurnOutput {
        match self.apply_model_switch(model_name) {
            Ok(ctx_window) => CliTurnOutput {
                frames: vec![render::render_model_switch_success(model_name, ctx_window)],
                control: SessionControl::Continue,
            },
            Err(msg) => CliTurnOutput {
                frames: vec![format!("[A] anvil > {}", msg)],
                control: SessionControl::Continue,
            },
        }
    }

    fn compact_session_history(&mut self) -> Result<String, AppError> {
        let changed = self.compact_with_hooks("manual");
        if changed {
            self.persist_session(AppEvent::SessionSaved)?;
            // ContextWarningTracker reset is handled by compact_with_hooks()
            Ok("[A] anvil > compacted older session history".to_string())
        } else {
            Ok("[A] anvil > nothing to compact".to_string())
        }
    }
}

/// Return actionable guidance for an error to help the user recover.
pub fn error_guidance(err: &AppError) -> String {
    match err {
        AppError::Config(_) => concat!(
            "Hint: check your config file at .anvil/config\n",
            "  Valid keys: provider, model, provider_url, context_window, stream,\n",
            "              sidecar_model, sidecar_provider_url\n",
            "  Environment variables also accepted (e.g. ANVIL_MODEL, ANVIL_PROVIDER_URL)"
        )
        .to_string(),
        AppError::ProviderBootstrap(bootstrap_err) => {
            let detail = bootstrap_err.to_string();
            if detail.contains("ollama") || detail.contains("unsupported") {
                concat!(
                    "Hint: Ollama provider could not be reached\n",
                    "  - Is Ollama running? Try: ollama serve\n",
                    "  - Check URL: --provider-url http://127.0.0.1:11434\n",
                    "  - List models: ollama list"
                )
                .to_string()
            } else {
                concat!(
                    "Hint: provider could not be reached\n",
                    "  - For Ollama: ensure `ollama serve` is running\n",
                    "  - For OpenAI-compatible: --provider openai --provider-url <url>\n",
                    "  - For LM Studio: --provider lmstudio\n",
                    "  - Set API key with ANVIL_API_KEY if required"
                )
                .to_string()
            }
        }
        AppError::Session(_) => concat!(
            "Hint: session file may be corrupted or inaccessible\n",
            "  - Try --fresh-session to start a new session\n",
            "  - Check file permissions in .anvil/sessions/"
        )
        .to_string(),
        AppError::Extension(_) => concat!(
            "Hint: failed to load custom slash commands\n",
            "  - Check .anvil/slash-commands.json for valid JSON\n",
            "  - Each entry needs: name, description, prompt"
        )
        .to_string(),
        AppError::ProviderTurn(turn_err) => match turn_err {
            ProviderTurnError::ConnectionRefused(_) => concat!(
                "Hint: Connection refused\n",
                "  - Is the provider running? Try: ollama serve\n",
                "  - Check URL: --provider-url http://127.0.0.1:11434\n",
            )
            .to_string(),
            ProviderTurnError::DnsFailure(_) => concat!(
                "Hint: DNS resolution failed\n",
                "  - Check the provider URL for typos\n",
                "  - Verify network connectivity\n",
            )
            .to_string(),
            ProviderTurnError::ModelNotFound { model, .. } => format!(
                "Hint: Model '{}' not found\n\
                 \x20 - Download it: ollama pull {}\n\
                 \x20 - List available models: ollama list\n",
                model, model
            ),
            ProviderTurnError::AuthenticationFailed { .. } => concat!(
                "Hint: Authentication failed\n",
                "  - Check your API key or server configuration\n",
                "  - Set API key: export ANVIL_API_KEY=<your-key>\n",
                "  - API key format: some providers require 'Bearer <key>' prefix\n",
                "  - IMPORTANT: Never share your API key in error reports or forums\n",
            )
            .to_string(),
            ProviderTurnError::Timeout(_) => concat!(
                "Hint: Request timed out\n",
                "  - The provider may be overloaded\n",
                "  - Try again or use a smaller model\n",
            )
            .to_string(),
            _ => {
                let detail = turn_err.to_string();
                if detail.contains("401") || detail.contains("403") || detail.contains("api key") {
                    "Hint: authentication failed\n  - Set your API key: ANVIL_API_KEY=<key>\n  - Check the key format (some providers require 'Bearer ' prefix)".to_string()
                } else {
                    "Hint: the provider turn failed\n  - Check if the model is available: ollama list\n  - Network issues may cause transient failures — try again".to_string()
                }
            }
        },
        _ => String::new(),
    }
}

fn standard_tool_registry(custom_tools: Vec<crate::config::CustomToolDef>) -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    registry.register_standard_tools();
    if !custom_tools.is_empty() {
        registry.register_custom_tools(custom_tools);
    }
    registry
}

// Re-export CLI entry points from the cli module.
pub use cli::{run, run_session_loop, run_with_args};

#[cfg(test)]
mod tests {
    use super::*;

    fn make_usage(estimated: usize, max: u32) -> ContextUsageView {
        ContextUsageView {
            estimated_tokens: estimated,
            max_tokens: max,
        }
    }

    #[test]
    fn tracker_evaluate_first_warning() {
        let mut tracker = ContextWarningTracker::new();
        let usage = make_usage(8500, 10000);
        assert_eq!(tracker.evaluate(&usage), Some(ContextWarningLevel::Warning));
    }

    #[test]
    fn tracker_evaluate_suppresses_duplicate_warning() {
        let mut tracker = ContextWarningTracker::new();
        let usage = make_usage(8500, 10000);
        assert_eq!(tracker.evaluate(&usage), Some(ContextWarningLevel::Warning));
        assert_eq!(tracker.evaluate(&usage), None);
    }

    #[test]
    fn tracker_evaluate_first_critical() {
        let mut tracker = ContextWarningTracker::new();
        let usage = make_usage(9500, 10000);
        assert_eq!(
            tracker.evaluate(&usage),
            Some(ContextWarningLevel::Critical)
        );
    }

    #[test]
    fn tracker_evaluate_critical_also_sets_warning_flag() {
        let mut tracker = ContextWarningTracker::new();
        let critical = make_usage(9500, 10000);
        assert_eq!(
            tracker.evaluate(&critical),
            Some(ContextWarningLevel::Critical)
        );
        // Warning should also be suppressed since critical set the flag
        let warning = make_usage(8500, 10000);
        assert_eq!(tracker.evaluate(&warning), None);
    }

    #[test]
    fn tracker_evaluate_below_threshold_returns_none() {
        let mut tracker = ContextWarningTracker::new();
        let usage = make_usage(5000, 10000);
        assert_eq!(tracker.evaluate(&usage), None);
    }

    #[test]
    fn tracker_reset_below_80_clears_warning() {
        let mut tracker = ContextWarningTracker::new();
        let high = make_usage(8500, 10000);
        tracker.evaluate(&high);
        assert!(tracker.warned_warning);

        let low = make_usage(7000, 10000);
        tracker.reset_if_below_threshold(&low);
        assert!(!tracker.warned_warning);
    }

    #[test]
    fn tracker_reset_below_90_clears_critical() {
        let mut tracker = ContextWarningTracker::new();
        let high = make_usage(9500, 10000);
        tracker.evaluate(&high);
        assert!(tracker.warned_critical);

        let medium = make_usage(8500, 10000);
        tracker.reset_if_below_threshold(&medium);
        assert!(!tracker.warned_critical);
        // Warning flag should remain since 85% >= 80%
        assert!(tracker.warned_warning);
    }

    /// Regression test for Issue #202: compute_compact_params must return
    /// Some when token/message thresholds are exceeded, regardless of
    /// interactive mode. The fix ensures flush_session calls
    /// compact_with_hooks before the non-interactive early return.
    #[test]
    fn compute_compact_params_triggers_when_budget_exceeded() {
        use crate::session::SessionRecord;
        use std::path::PathBuf;

        let mut session = SessionRecord::new(PathBuf::from("/tmp/issue202"));
        // Push enough messages to exceed message-count threshold (default = 64)
        for i in 0..70 {
            session.push_message(crate::session::SessionMessage::new(
                crate::session::MessageRole::User,
                format!("msg_{i:03}"),
                format!("content {i}"),
            ));
        }
        // With default auto_compact_threshold (64), 70 messages should trigger
        let params = compute_compact_params(&session, 128_000, None);
        assert!(
            params.is_some(),
            "compute_compact_params should trigger when message count exceeds threshold"
        );
    }

    // --- SessionStats tests (Issue #206 B-1) ---

    #[test]
    fn session_stats_new_has_start_time() {
        let stats = SessionStats::new();
        assert!(stats.session_start.is_some());
        assert_eq!(stats.total_turns, 0);
        assert!(stats.tool_calls.is_empty());
    }

    #[test]
    fn session_stats_record_tool_call() {
        let mut stats = SessionStats::new();
        stats.record_tool_call("file.read");
        stats.record_tool_call("file.read");
        stats.record_tool_call("file.edit");
        assert_eq!(stats.tool_calls["file.read"], 2);
        assert_eq!(stats.tool_calls["file.edit"], 1);
        assert_eq!(stats.total_tool_calls(), 3);
    }

    #[test]
    fn session_stats_record_file_change() {
        let mut stats = SessionStats::new();
        stats.record_file_change(10, 3);
        stats.record_file_change(5, 2);
        assert_eq!(stats.lines_added, 15);
        assert_eq!(stats.lines_deleted, 5);
    }

    #[test]
    fn session_stats_record_turn() {
        let mut stats = SessionStats::new();
        stats.record_turn();
        stats.record_turn();
        assert_eq!(stats.total_turns, 2);
    }

    #[test]
    fn session_stats_record_compact() {
        let mut stats = SessionStats::new();
        stats.record_compact(false);
        stats.record_compact(true);
        stats.record_compact(true);
        assert_eq!(stats.compact_count, 3);
        assert_eq!(stats.sidecar_count, 2);
    }

    #[test]
    fn session_stats_tool_calls_summary() {
        let mut stats = SessionStats::new();
        stats.record_tool_call("file.read");
        stats.record_tool_call("file.read");
        stats.record_tool_call("file.read");
        stats.record_tool_call("file.edit");
        let summary = stats.tool_calls_summary();
        // file.read has higher count, should come first
        assert!(summary.starts_with("file.read x3"));
        assert!(summary.contains("file.edit"));
    }

    // --- count_diff_lines tests (Issue #206 B-1) ---

    #[test]
    fn count_diff_lines_basic() {
        let diff = "\
--- a/foo.rs
+++ b/foo.rs
@@ -1,3 +1,4 @@
 unchanged
-removed line
+added line 1
+added line 2
";
        let (added, deleted) = count_diff_lines(diff);
        assert_eq!(added, 2);
        assert_eq!(deleted, 1);
    }

    #[test]
    fn count_diff_lines_empty() {
        let (added, deleted) = count_diff_lines("");
        assert_eq!(added, 0);
        assert_eq!(deleted, 0);
    }

    #[test]
    fn count_diff_lines_no_changes() {
        let diff = "\
--- a/foo.rs
+++ b/foo.rs
@@ -1,2 +1,2 @@
 same line
 another same line
";
        let (added, deleted) = count_diff_lines(diff);
        assert_eq!(added, 0);
        assert_eq!(deleted, 0);
    }

    // --- format_tool_counts tests (Issue #206 B-1) ---

    #[test]
    fn format_tool_counts_multiple() {
        let counts = vec![
            ("file.read".to_string(), 3u32),
            ("file.edit".to_string(), 1),
        ];
        let result = format_tool_counts(counts.into_iter());
        assert_eq!(result, "file.read x3, file.edit");
    }

    #[test]
    fn format_tool_counts_empty() {
        let counts: Vec<(String, u32)> = vec![];
        let result = format_tool_counts(counts.into_iter());
        assert_eq!(result, "");
    }

    #[test]
    fn format_tool_counts_single() {
        let counts = vec![("shell.exec".to_string(), 5u32)];
        let result = format_tool_counts(counts.into_iter());
        assert_eq!(result, "shell.exec x5");
    }

    // --- Issue #208: effective_token_budget logic ---

    /// Regression test for Issue #208: when context_budget is set, the
    /// effective token budget must use min(context_window, context_budget),
    /// NOT the raw context_window.
    #[test]
    fn effective_token_budget_uses_context_budget() {
        // Simulates the logic in App::effective_token_budget
        let context_window: u32 = 262_144;
        let context_budget: Option<u32> = Some(32_768);
        let effective = match context_budget {
            Some(budget) => context_window.min(budget) as usize,
            None => context_window as usize,
        };
        assert_eq!(effective, 32_768);
    }

    #[test]
    fn effective_token_budget_falls_back_to_context_window() {
        let context_window: u32 = 262_144;
        let context_budget: Option<u32> = None;
        let effective = match context_budget {
            Some(budget) => context_window.min(budget) as usize,
            None => context_window as usize,
        };
        assert_eq!(effective, 262_144);
    }
}
