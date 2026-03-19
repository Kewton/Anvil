/// Core application orchestrator.
///
/// [`App`] owns the session, state machine, tool registry and config,
/// coordinating turns between the user, the LLM provider, and the tool
/// executor.
pub mod agentic;
pub mod cli;
pub mod mock;
pub mod plan;
pub mod policy;
pub mod render;

use crate::agent::BasicAgentLoop;
use crate::agent::{AgentEvent, AgentRuntime, PendingTurnState, ProjectLanguage};
use crate::config::EffectiveConfig;
use crate::contracts::{
    AppEvent, AppStateSnapshot, ConsoleRenderContext, ContextUsageView, ContextWarningLevel,
    RuntimeState,
};
use crate::extensions::{ExtensionLoadError, ExtensionRegistry, SlashCommandAction};
use crate::provider::{
    ProviderBootstrapError, ProviderClient, ProviderErrorKind, ProviderErrorRecord, ProviderEvent,
    ProviderRuntimeContext, ProviderTurnError,
};
use crate::retrieval::{RepositoryIndex, default_cache_path, render_retrieval_result};
use crate::session::{
    MessageRole, MessageStatus, SessionError, SessionMessage, SessionRecord, SessionStore,
    new_assistant_message, new_user_message,
};
use crate::spinner::Spinner;
use crate::state::{StateMachine, StateTransition};
use crate::tooling::{
    ExecutionClass, ExecutionMode, PermissionClass, PlanModePolicy, RollbackPolicy, ToolKind,
    ToolRegistry, ToolSpec,
};
use crate::tui::Tui;
use std::fmt::{Display, Formatter};
use std::io::{self, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

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

/// Central application state.
pub struct App {
    config: EffectiveConfig,
    provider: ProviderRuntimeContext,
    state_machine: StateMachine,
    session_store: SessionStore,
    session: SessionRecord,
    extensions: ExtensionRegistry,
    tools: ToolRegistry,
    system_prompt: String,
    shutdown_flag: Arc<AtomicBool>,
    warning_tracker: ContextWarningTracker,
    /// Hooks engine. `None` when hooks.json is absent or initialization failed.
    /// Declared before mcp_manager to maintain Drop order (DR3-007).
    hooks_engine: Option<crate::hooks::HooksEngine>,
    /// MCP server manager. `None` when mcp.json is absent or initialization failed.
    /// [D2-010] Declared last so it is dropped last (Drop order = declaration order).
    mcp_manager: Option<crate::mcp::McpManager>,
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
        let mut tools = standard_tool_registry();
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
        let base_prompt = crate::agent::tool_protocol_system_prompt(
            &detected_languages,
            mcp_descriptions.as_deref(),
        );
        let mut system_prompt = match config.project_instructions() {
            Some(instructions) => format!(
                "{}\n\n## Project instructions (from ANVIL.md)\n{}",
                base_prompt, instructions
            ),
            None => base_prompt,
        };

        // Offline mode: append note to system prompt and warn about shell.exec
        if config.mode.offline {
            system_prompt.push_str(
                "\n\nNote: Offline mode is active. web.fetch and web.search are unavailable. Do not use shell.exec to make network requests (curl, wget, etc.). Use local tools only."
            );
            eprintln!(
                "Warning: shell.exec can still access the network in offline mode. For full network isolation, use OS/firewall-level controls."
            );
        }

        Ok(Self {
            tools,
            config,
            provider,
            state_machine: StateMachine::from_snapshot(initial_state_snapshot),
            session_store,
            session,
            extensions,
            system_prompt,
            shutdown_flag,
            warning_tracker: ContextWarningTracker::new(),
            hooks_engine,
            mcp_manager,
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
            self.config.runtime.model,
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
                self.config.runtime.context_window,
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
    /// For auto: checks should_compact() first, keep_recent = threshold/2
    /// For manual: unconditional, keep_recent = 8
    fn compact_with_hooks(&mut self, trigger: &str) -> bool {
        if trigger == "auto" && !self.session.should_compact() {
            return false;
        }

        // Run PreCompact hook (soft-fail)
        if let Some(ref engine) = self.hooks_engine {
            let event = crate::hooks::PreCompactEvent {
                hook_point: "PreCompact",
                session_id: self.session.metadata.session_id.clone(),
                trigger: trigger.to_string(),
                message_count: self.session.messages.len(),
            };
            if let Err(err) = engine.run_pre_compact(event) {
                tracing::warn!("PreCompact hook error: {err}");
            }
        }

        let keep_recent = if trigger == "auto" {
            self.session.auto_compact_threshold / 2
        } else {
            8
        };
        self.session.compact_history(keep_recent)
    }

    /// Get a clone of the shutdown flag for injection into sub-components.
    pub(crate) fn shutdown_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.shutdown_flag)
    }

    pub(crate) fn session_mut(&mut self) -> &mut SessionRecord {
        &mut self.session
    }

    pub fn render_console(&self, tui: &Tui) -> Result<String, AppError> {
        Ok(tui.render_console(&self.build_console_render_context()))
    }

    pub fn startup_console(&mut self, tui: &Tui) -> Result<String, AppError> {
        if self.session.message_count() == 0 && self.session.last_snapshot.is_none() {
            let snapshot = self.initial_snapshot()?;
            return Ok(tui.render_startup(&self.config, &snapshot));
        }

        Ok(format!(
            "{}\n{}",
            render::render_resume_header(&self.config),
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
        self.session
            .push_message(new_user_message(message_id, content));
        self.persist_session(AppEvent::SessionSaved)?;
        Ok(())
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
        self.record_user_input(self.next_message_id("user"), user_input)?;
        self.begin_live_turn_state()?;

        let request = BasicAgentLoop::build_turn_request(
            self.config.runtime.model.clone(),
            &self.session,
            self.provider.capabilities.streaming && self.config.runtime.stream,
            self.config.runtime.context_window,
            &self.system_prompt,
        );

        // Phase 1: Collect events from provider with spinner + streaming output.
        let mut spinner_opt = Some(Spinner::start(
            format!("Thinking. model={}", self.config.runtime.model),
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
                                    BasicAgentLoop::parse_structured_response(&token_buffer)
                                        .map_err(AppError::ToolExecution)?;
                                frames.extend(self.complete_structured_response(
                                    structured,
                                    "Done. session saved",
                                    "session saved",
                                    0,
                                    None,
                                    tui,
                                    provider_client,
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
            self.config.runtime.context_window,
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
    /// In non-interactive mode, skip disk persistence entirely.
    fn flush_session(&mut self) -> Result<(), AppError> {
        if !self.config.mode.interactive {
            return Ok(());
        }
        self.compact_with_hooks("auto");
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
                        crate::tooling::diff::generate_diff_preview(
                            &self.config.paths.cwd,
                            &tc.input,
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
            &self.config.runtime.model,
            self.config.runtime.max_console_messages,
            true,
        )
    }

    fn build_startup_render_context(&self) -> ConsoleRenderContext {
        // On startup/resume, include assistant messages so the user can see
        // the conversation history from the previous session.
        self.session.console_render_context(
            self.state_machine.snapshot(),
            &self.config.runtime.model,
            self.config.runtime.max_console_messages,
            false,
        )
    }

    fn next_message_id(&self, prefix: &str) -> String {
        format!("{prefix}_{:04}", self.session.message_count() + 1)
    }

    fn begin_live_turn_state(&mut self) -> Result<(), AppError> {
        let snapshot = AppStateSnapshot::new(RuntimeState::Thinking)
            .with_status(format!("Thinking. model={}", self.config.runtime.model))
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
            Some(SlashCommandAction::Model) => CliTurnOutput {
                frames: vec![render::render_model_frame(&self.config)],
                control: SessionControl::Continue,
            },
            Some(SlashCommandAction::Provider) => CliTurnOutput {
                frames: vec![render::render_provider_frame(&self.config, &self.provider)],
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
        let result = index.search(query, 5);
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

    fn compact_session_history(&mut self) -> Result<String, AppError> {
        let changed = self.compact_with_hooks("manual");
        if changed {
            self.persist_session(AppEvent::SessionSaved)?;
            let usage = ContextUsageView {
                estimated_tokens: self.session.estimated_token_count(),
                max_tokens: self.config.runtime.context_window,
            };
            self.warning_tracker.reset_if_below_threshold(&usage);
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
            "  Valid keys: provider, model, provider_url, context_window, stream\n",
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

fn standard_tool_registry() -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    registry.register_standard_tools();
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
}
