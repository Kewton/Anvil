/// Core application orchestrator.
///
/// [`App`] owns the session, state machine, tool registry and config,
/// coordinating turns between the user, the LLM provider, and the tool
/// executor.
pub mod agentic;
pub mod cli;
pub mod mock;
pub mod plan;
pub mod render;

use crate::agent::BasicAgentLoop;
use crate::agent::{AgentEvent, AgentRuntime, PendingTurnState};
use crate::config::EffectiveConfig;
use crate::contracts::{
    AppEvent, AppStateSnapshot, ConsoleRenderContext, RuntimeState,
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
use crate::state::{StateMachine, StateTransition};
use crate::tooling::ToolRegistry;
use crate::spinner::Spinner;
use crate::tui::Tui;
use std::fmt::{Display, Formatter};
use std::io::{self, Write};

// Re-export render helpers that form the public API.
pub use render::{cli_prompt, render_help_frame, slash_commands};

const MAX_CONSOLE_MESSAGES: usize = 5;

/// Central application state.
pub struct App {
    config: EffectiveConfig,
    provider: ProviderRuntimeContext,
    state_machine: StateMachine,
    session_store: SessionStore,
    session: SessionRecord,
    pending_turn: Option<PendingTurnState>,
    extensions: ExtensionRegistry,
    tools: ToolRegistry,
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
    ) -> Result<Self, AppError> {
        let session_store = SessionStore::from_config(&config);
        let session = if config.mode.fresh_session {
            SessionRecord::new(config.paths.cwd.clone())
        } else {
            session_store.load_or_create(&config.paths.cwd)?
        };
        let initial_state_snapshot = session
            .last_snapshot
            .clone()
            .unwrap_or_else(|| AppStateSnapshot::new(RuntimeState::Ready));
        let extensions = ExtensionRegistry::load(&config.paths.cwd)?;

        Ok(Self {
            tools: standard_tool_registry(),
            config,
            provider,
            state_machine: StateMachine::from_snapshot(initial_state_snapshot),
            session_store,
            pending_turn: session.pending_turn.clone(),
            session,
            extensions,
        })
    }

    pub fn initial_snapshot(&mut self) -> Result<AppStateSnapshot, AppError> {
        let snapshot = self.build_initial_snapshot();
        self.apply_transition(snapshot, StateTransition::ResetToReady)
    }

    fn build_initial_snapshot(&self) -> AppStateSnapshot {
        AppStateSnapshot::new(RuntimeState::Ready)
            .with_event(AppEvent::StartupCompleted)
            .with_status(format!(
                "Ready. provider={} model={} stream={} tools={}",
                self.config.runtime.provider,
                self.config.runtime.model,
                self.provider.capabilities.streaming,
                self.provider.capabilities.tool_calling
            ))
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
            tui.render_console(&self.build_console_render_context())
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
        if self.pending_turn.is_some() {
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
        if self.pending_turn.is_some() {
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
        );

        // Phase 1: Collect events from provider with spinner + streaming output.
        let mut spinner_opt = Some(Spinner::start(format!(
            "Thinking. model={}",
            self.config.runtime.model
        )));

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
                        next_actions: vec![
                            "resume work".to_string(),
                            "inspect status".to_string(),
                        ],
                        elapsed_ms: 0,
                    }],
                    tui,
                )
            }
            Err(ProviderTurnError::Backend(message)) => {
                self.record_provider_error(ProviderTurnError::Backend(message.clone()))?;
                self.execute_runtime_events(
                    &[AgentEvent::Failed {
                        status: "Error. provider turn failed".to_string(),
                        error_summary: message,
                        recommended_actions: vec![
                            "retry turn".to_string(),
                            "inspect provider".to_string(),
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
        let pending_turn = self.pending_turn.take().ok_or(AppError::NoPendingApproval)?;

        self.session.clear_pending_turn();
        self.persist_session(AppEvent::SessionSaved)?;
        let result = self.execute_runtime_events(&pending_turn.remaining_events, tui);
        self.flush_session()?;
        result
    }

    pub fn deny_and_abort(&mut self, tui: &Tui) -> Result<Vec<String>, AppError> {
        if self.pending_turn.is_none() {
            return Err(AppError::NoPendingApproval);
        }

        self.clear_pending_turn()?;
        self.record_assistant_output(
            self.next_message_id("assistant"),
            "Approval denied. No tool was executed.",
        )?;
        let snapshot = AppStateSnapshot::new(RuntimeState::Ready)
            .with_status("Approval denied. Ready for the next task".to_string())
            .with_completion_summary("Approval denied. No tool was executed.", "no changes made")
            .with_context_usage(
                self.session.estimated_token_count(),
                self.config.runtime.context_window,
            );
        self.apply_transition(snapshot, StateTransition::ResetToReady)?;
        self.flush_session()?;
        Ok(vec![self.render_console(tui)?])
    }

    pub fn reset_to_ready(&mut self) -> Result<AppStateSnapshot, AppError> {
        self.clear_pending_turn()?;
        let snapshot = AppStateSnapshot::new(RuntimeState::Ready)
            .with_status("Ready for the next task".to_string())
            .with_context_usage(
                self.session.estimated_token_count(),
                self.config.runtime.context_window,
            );
        self.apply_transition(snapshot, StateTransition::ResetToReady)
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
    fn flush_session(&mut self) -> Result<(), AppError> {
        if self.session.dirty {
            self.session_store.save(&self.session)?;
            self.session.clear_dirty();
        }
        Ok(())
    }

    /// Immediately persist session to disk (for crash-safety critical paths).
    fn persist_session_immediate(&mut self, event: AppEvent) -> Result<(), AppError> {
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
                })?;
                break;
            }
        }

        if self.pending_turn.is_none() {
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
                    .with_elapsed_ms(*elapsed_ms)
                    .with_context_usage(
                        self.session.estimated_token_count(),
                        self.config.runtime.context_window,
                    );

                let transition = match self.state_machine.snapshot().state {
                    RuntimeState::Working => StateTransition::ResumeThinking,
                    _ => StateTransition::StartThinking,
                };

                self.apply_transition(snapshot, transition)
            }
            AgentEvent::ApprovalRequested {
                status,
                tool_name,
                summary,
                risk,
                tool_call_id,
                elapsed_ms,
            } => {
                let snapshot = AppStateSnapshot::new(RuntimeState::AwaitingApproval)
                    .with_status(status.clone())
                    .with_approval(
                        tool_name.clone(),
                        summary.clone(),
                        risk.clone(),
                        tool_call_id.clone(),
                    )
                    .with_elapsed_ms(*elapsed_ms)
                    .with_context_usage(
                        self.session.estimated_token_count(),
                        self.config.runtime.context_window,
                    );
                self.apply_transition(snapshot, StateTransition::RequestApproval)
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
                    .with_elapsed_ms(*elapsed_ms)
                    .with_context_usage(
                        self.session.estimated_token_count(),
                        self.config.runtime.context_window,
                    );
                self.apply_transition(snapshot, StateTransition::StartWorking)
            }
            AgentEvent::Done {
                status,
                assistant_message,
                completion_summary,
                saved_status,
                tool_logs,
                elapsed_ms,
            } => {
                self.record_assistant_output(self.next_message_id("assistant"), assistant_message)?;
                let snapshot = AppStateSnapshot::new(RuntimeState::Done)
                    .with_status(status.clone())
                    .with_tool_logs(render::build_tool_logs(tool_logs))
                    .with_completion_summary(completion_summary.clone(), saved_status.clone())
                    .with_elapsed_ms(*elapsed_ms)
                    .with_context_usage(
                        self.session.estimated_token_count(),
                        self.config.runtime.context_window,
                    );
                self.apply_transition(snapshot, StateTransition::Finish)
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
                    .with_elapsed_ms(*elapsed_ms)
                    .with_context_usage(
                        self.session.estimated_token_count(),
                        self.config.runtime.context_window,
                    );
                self.apply_transition(snapshot, StateTransition::Interrupt)?;
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
                    .with_elapsed_ms(*elapsed_ms)
                    .with_context_usage(
                        self.session.estimated_token_count(),
                        self.config.runtime.context_window,
                    );
                self.apply_transition(snapshot, StateTransition::Fail)
            }
        }
    }

    fn build_console_render_context(&self) -> ConsoleRenderContext {
        self.session.console_render_context(
            self.state_machine.snapshot(),
            &self.config.runtime.model,
            MAX_CONSOLE_MESSAGES,
        )
    }

    fn next_message_id(&self, prefix: &str) -> String {
        format!("{prefix}_{:04}", self.session.message_count() + 1)
    }

    fn begin_live_turn_state(&mut self) -> Result<(), AppError> {
        let snapshot = AppStateSnapshot::new(RuntimeState::Thinking)
            .with_status(format!("Thinking. model={}", self.config.runtime.model))
            .with_elapsed_ms(0)
            .with_context_usage(
                self.session.estimated_token_count(),
                self.config.runtime.context_window,
            );
        self.apply_transition(snapshot, StateTransition::StartThinking)?;
        Ok(())
    }

    fn record_provider_error(&mut self, error: ProviderTurnError) -> Result<(), AppError> {
        let (kind, message) = match error {
            ProviderTurnError::Cancelled => (
                ProviderErrorKind::Cancelled,
                "provider turn cancelled".to_string(),
            ),
            ProviderTurnError::Backend(message) => (ProviderErrorKind::Backend, message),
        };

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
        self.pending_turn.is_some()
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

        match self.run_live_turn(trimmed, provider_client, tui) {
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
        self.pending_turn = Some(pending_turn.clone());
        self.session.set_pending_turn(pending_turn);
        self.persist_session_immediate(AppEvent::SessionSaved)
    }

    fn clear_pending_turn(&mut self) -> Result<(), AppError> {
        if self.pending_turn.is_none() && !self.session.has_pending_turn() {
            return Ok(());
        }
        self.pending_turn = None;
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
                frames: vec![render::render_help_frame_for(self.extensions.slash_commands())],
                control: SessionControl::Continue,
            },
            Some(SlashCommandAction::Status) => CliTurnOutput {
                frames: vec![self.render_console(tui)?],
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
            Some(SlashCommandAction::Prompt(prompt)) => match self.run_live_turn(prompt, provider_client, tui) {
                Ok(frames) => CliTurnOutput {
                    frames,
                    control: SessionControl::Continue,
                },
                Err(AppError::PendingApprovalRequired) => CliTurnOutput {
                    frames: vec![render::render_pending_approval_frame(
                        self.state_machine.snapshot(),
                    )],
                    control: SessionControl::Continue,
                },
                Err(err) => return Err(err),
            },
            _ => {
                let suggestion = self.extensions.suggest_command(command);
                let msg = if let Some(suggested) = suggestion {
                    format!(
                        "Unknown command: {command}\nDid you mean: {suggested}?\nTry /help for available commands."
                    )
                } else {
                    format!(
                        "Unknown command: {command}\nTry /help for available commands."
                    )
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
        let changed = self.session.compact_history(8);
        if changed {
            self.persist_session(AppEvent::SessionSaved)?;
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
        ).to_string(),
        AppError::ProviderBootstrap(_) => concat!(
            "Hint: the LLM provider could not be reached\n",
            "  - Is Ollama running? Try: ollama serve\n",
            "  - Check provider URL with --provider-url\n",
            "  - For OpenAI-compatible backends: --provider openai --provider-url <url>\n",
            "  - Set API key with ANVIL_API_KEY if required"
        ).to_string(),
        AppError::Session(_) => concat!(
            "Hint: session file may be corrupted or inaccessible\n",
            "  - Try --fresh-session to start a new session\n",
            "  - Check file permissions in .anvil/sessions/"
        ).to_string(),
        AppError::Extension(_) => concat!(
            "Hint: failed to load custom slash commands\n",
            "  - Check .anvil/slash-commands.json for valid JSON\n",
            "  - Each entry needs: name, description, prompt"
        ).to_string(),
        AppError::ProviderTurn(_) => concat!(
            "Hint: the provider turn failed\n",
            "  - Check if the model is available: ollama list\n",
            "  - Network issues may cause transient failures — try again"
        ).to_string(),
        _ => String::new(),
    }
}

fn standard_tool_registry() -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    registry.register_standard_tools();
    registry
}

// Re-export CLI entry points from the cli module.
pub use cli::{run, run_session_loop};
