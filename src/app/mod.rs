/// Core application orchestrator.
///
/// [`App`] owns the session, state machine, tool registry and config,
/// coordinating turns between the user, the LLM provider, and the tool
/// executor.
pub mod mock;
pub mod render;

use crate::agent::BasicAgentLoop;
use crate::agent::{AgentEvent, AgentRuntime, PendingTurnState, StructuredAssistantResponse};
use crate::config::EffectiveConfig;
use crate::contracts::{
    AppEvent, AppStateSnapshot, ConsoleRenderContext, RuntimeState, ToolLogView,
};
use crate::extensions::{ExtensionLoadError, ExtensionRegistry, SlashCommandAction};
use crate::provider::{
    ProviderBootstrapError, ProviderClient, ProviderErrorKind, ProviderErrorRecord, ProviderEvent,
    ProviderRuntimeContext, ProviderTurnError, build_local_provider_client,
};
use crate::retrieval::{RepositoryIndex, default_cache_path, render_retrieval_result};
use crate::session::{
    MessageRole, MessageStatus, SessionError, SessionMessage, SessionRecord, SessionStore,
    new_assistant_message, new_user_message,
};
use crate::state::{StateMachine, StateTransition};
use crate::tooling::{
    LocalToolExecutor, ToolExecutionError, ToolExecutionPolicy, ToolExecutionResult, ToolRegistry,
    ToolRuntimeError,
};
use crate::tui::Tui;
use std::fmt::{Display, Formatter};
use std::io::{self, BufRead, Write};

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
        let session = session_store.load_or_create(&config.paths.cwd)?;
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

        match BasicAgentLoop::run_turn(provider_client, &request) {
            Ok(provider_events) => {
                let mut frames = Vec::new();
                let mut token_buffer = String::new();
                let mut last_rendered_len = 0usize;
                for (index, event) in provider_events.iter().enumerate() {
                    match event {
                        ProviderEvent::Agent(agent_event) => {
                            if let Some(structured_frames) =
                                self.handle_structured_done(agent_event, tui)?
                            {
                                frames.extend(structured_frames);
                            } else {
                                let snapshot = self.apply_agent_event(agent_event)?;
                                frames.push(self.render_console(tui)?);
                                if snapshot.state == RuntimeState::AwaitingApproval {
                                    let remaining_events = provider_events[index + 1..]
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
                        ProviderEvent::TokenDelta(delta) => {
                            token_buffer.push_str(delta);
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
                                )?);
                                break;
                            }
                            if render::should_render_stream_progress(
                                &token_buffer,
                                delta,
                                last_rendered_len,
                            ) {
                                frames.push(self.render_token_delta_frame(&token_buffer, tui)?);
                                last_rendered_len = token_buffer.len();
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
        }
    }

    pub fn approve_and_continue(
        &mut self,
        _runtime: &AgentRuntime,
        tui: &Tui,
    ) -> Result<Vec<String>, AppError> {
        let pending_turn = self.pending_turn.take().ok_or(AppError::NoPendingApproval)?;

        self.session.clear_pending_turn();
        self.persist_session(AppEvent::SessionSaved)?;
        self.execute_runtime_events(&pending_turn.remaining_events, tui)
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
        self.session_store.save(&self.session)?;
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

        Ok(frames)
    }

    fn handle_structured_done(
        &mut self,
        event: &AgentEvent,
        tui: &Tui,
    ) -> Result<Option<Vec<String>>, AppError> {
        let AgentEvent::Done {
            status,
            assistant_message,
            completion_summary: _,
            saved_status,
            tool_logs: _,
            elapsed_ms,
        } = event
        else {
            return Ok(None);
        };

        let structured = BasicAgentLoop::parse_structured_response(assistant_message)
            .map_err(AppError::ToolExecution)?;
        if structured.tool_calls.is_empty() {
            return Ok(None);
        }

        Ok(Some(self.complete_structured_response(
            structured,
            status,
            saved_status,
            *elapsed_ms,
            tui,
        )?))
    }

    fn complete_structured_response(
        &mut self,
        structured: StructuredAssistantResponse,
        status: &str,
        saved_status: &str,
        elapsed_ms: u128,
        tui: &Tui,
    ) -> Result<Vec<String>, AppError> {
        let results = self.execute_structured_tool_calls(&structured)?;
        let tool_log_views: Vec<ToolLogView> = results
            .iter()
            .map(ToolExecutionResult::to_tool_log_view)
            .collect();

        let working = AppStateSnapshot::new(RuntimeState::Working)
            .with_status("Executing tool plan".to_string())
            .with_tool_logs(tool_log_views.clone())
            .with_elapsed_ms(elapsed_ms)
            .with_context_usage(
                self.session.estimated_token_count(),
                self.config.runtime.context_window,
            );
        let _ = self.apply_transition(working, StateTransition::StartWorking)?;

        self.record_assistant_output(self.next_message_id("assistant"), structured.final_response)?;
        let done = AppStateSnapshot::new(RuntimeState::Done)
            .with_status(status.to_string())
            .with_tool_logs(tool_log_views)
            .with_completion_summary(
                format!("Executed {} tool call(s). {}", results.len(), saved_status),
                saved_status.to_string(),
            )
            .with_elapsed_ms(elapsed_ms)
            .with_context_usage(
                self.session.estimated_token_count(),
                self.config.runtime.context_window,
            );
        let _ = self.apply_transition(done, StateTransition::Finish)?;

        Ok(vec![self.render_console(tui)?])
    }

    fn execute_structured_tool_calls(
        &mut self,
        structured: &StructuredAssistantResponse,
    ) -> Result<Vec<ToolExecutionResult>, AppError> {
        let executor = LocalToolExecutor::new(self.config.paths.cwd.clone());
        let mut results = Vec::new();
        for call in &structured.tool_calls {
            let validated = self.tools.validate(call.clone()).map_err(|err| {
                AppError::ToolExecution(format!("tool validation failed: {err:?}"))
            })?;
            let request = validated
                .approve()
                .into_execution_request(ToolExecutionPolicy {
                    approval_required: self.config.mode.approval_required,
                    allow_restricted: false,
                    plan_mode: false,
                    plan_scope_granted: true,
                })
                .map_err(map_tool_execution_error)?;
            let result = executor.execute(request).map_err(map_tool_runtime_error)?;
            self.session.push_message(
                SessionMessage::new(
                    MessageRole::Tool,
                    "tool",
                    format!("{} {}", result.tool_name, result.summary),
                )
                .with_id(self.next_message_id("tool")),
            );
            results.push(result);
        }
        self.persist_session(AppEvent::SessionSaved)?;
        Ok(results)
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

    fn render_token_delta_frame(
        &mut self,
        token_buffer: &str,
        tui: &Tui,
    ) -> Result<String, AppError> {
        let visible = render::recent_stream_excerpt(token_buffer, 400);
        let snapshot = AppStateSnapshot::new(RuntimeState::Thinking)
            .with_status(format!("Streaming. model={}", self.config.runtime.model))
            .with_reasoning_summary(vec![visible])
            .with_context_usage(
                self.session.estimated_token_count(),
                self.config.runtime.context_window,
            );
        self.apply_transition(snapshot, StateTransition::StartThinking)?;
        self.render_console(tui)
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
        self.persist_session(AppEvent::SessionSaved)
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
        self.persist_session(AppEvent::SessionSaved)
    }

    fn clear_pending_turn(&mut self) -> Result<(), AppError> {
        if self.pending_turn.is_none() && !self.session.has_pending_turn() {
            return Ok(());
        }
        self.pending_turn = None;
        self.session.clear_pending_turn();
        self.persist_session(AppEvent::SessionSaved)
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
            _ => CliTurnOutput {
                frames: vec![format!(
                    "Unknown command: {command}\nTry /help for available commands."
                )],
                control: SessionControl::Continue,
            },
        };

        Ok(output)
    }

    fn add_plan_item(&mut self, item: String) -> Result<String, AppError> {
        let mut items = self
            .state_machine
            .snapshot()
            .plan
            .as_ref()
            .map(|plan| plan.items.clone())
            .unwrap_or_default();
        items.push(item);
        let active_index = self
            .state_machine
            .snapshot()
            .plan
            .as_ref()
            .and_then(|plan| plan.active_index)
            .or(Some(0));
        self.update_plan_snapshot(items, active_index, AppEvent::PlanItemAdded)?;
        Ok(render::render_plan_frame(self.state_machine.snapshot()))
    }

    fn focus_plan_item(&mut self, index: usize) -> Result<String, AppError> {
        let items = self
            .state_machine
            .snapshot()
            .plan
            .as_ref()
            .map(|plan| plan.items.clone())
            .unwrap_or_default();
        if items.is_empty() {
            return Ok("[A] anvil > plan\n  no active plan".to_string());
        }
        let active_index = Some(index.min(items.len().saturating_sub(1)));
        self.update_plan_snapshot(items, active_index, AppEvent::PlanFocusChanged)?;
        Ok(render::render_plan_frame(self.state_machine.snapshot()))
    }

    fn clear_plan_items(&mut self) -> Result<String, AppError> {
        self.update_plan_snapshot(Vec::new(), None, AppEvent::PlanCleared)?;
        Ok(render::render_plan_frame(self.state_machine.snapshot()))
    }

    fn update_plan_snapshot(
        &mut self,
        items: Vec<String>,
        active_index: Option<usize>,
        event: AppEvent,
    ) -> Result<(), AppError> {
        let mut snapshot = self.state_machine.snapshot().clone();
        snapshot.plan = if items.is_empty() {
            None
        } else {
            Some(crate::contracts::PlanView { items, active_index })
        };
        snapshot.last_event = Some(event);
        self.state_machine.replace_snapshot(snapshot.clone());
        self.session.set_last_snapshot(snapshot);
        self.persist_session(event)?;
        self.persist_session(AppEvent::SessionSaved)
    }

    fn repo_find(&self, query: &str) -> Result<String, AppError> {
        let cache_path = default_cache_path(&self.config.paths.state_dir);
        let index = RepositoryIndex::load_or_build(&self.config.paths.cwd, &cache_path)
            .map_err(|err| AppError::ToolExecution(err.to_string()))?;
        let result = index.search(query, 5);
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

fn standard_tool_registry() -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    registry.register_standard_tools();
    registry
}

fn map_tool_execution_error(error: ToolExecutionError) -> AppError {
    AppError::ToolExecution(match error {
        ToolExecutionError::ApprovalRequired(call_id) => {
            format!("tool approval required for {call_id}")
        }
        ToolExecutionError::RestrictedTool(tool) => format!("restricted tool blocked: {tool}"),
        ToolExecutionError::PlanModeBlocked(tool) => format!("tool blocked in plan mode: {tool}"),
        ToolExecutionError::PlanModeScopeRequired(tool) => {
            format!("tool scope required in plan mode: {tool}")
        }
    })
}

fn map_tool_runtime_error(error: ToolRuntimeError) -> AppError {
    AppError::ToolExecution(error.to_string())
}

/// Drive the interactive CLI session loop.
///
/// Reads lines from `input`, dispatches them through [`App::handle_cli_line`],
/// and writes rendered frames to `output` until the user exits.
pub fn run_session_loop<C: ProviderClient, R: BufRead, W: Write>(
    app: &mut App,
    provider_client: &C,
    tui: &Tui,
    mut input: R,
    output: &mut W,
) -> Result<(), AppError> {
    loop {
        write!(output, "{}", cli_prompt())
            .map_err(|err| AppError::Session(SessionError::SessionWriteFailed(err)))?;
        output
            .flush()
            .map_err(|err| AppError::Session(SessionError::SessionWriteFailed(err)))?;

        let mut line = String::new();
        let read = input
            .read_line(&mut line)
            .map_err(|err| AppError::Session(SessionError::SessionReadFailed(err)))?;
        if read == 0 {
            break;
        }

        let turn = app.handle_cli_line(&line, provider_client, tui)?;
        for frame in turn.frames {
            writeln!(output, "{frame}")
                .map_err(|err| AppError::Session(SessionError::SessionWriteFailed(err)))?;
        }
        if turn.control == SessionControl::Exit {
            break;
        }
    }

    Ok(())
}

/// Application entry point.
pub fn run() -> Result<(), AppError> {
    let config = EffectiveConfig::load()?;
    let provider = ProviderRuntimeContext::bootstrap(&config)?;
    let provider_client = build_local_provider_client(&config)?;
    let mut app = App::new(config, provider)?;
    let tui = Tui::new();
    println!("{}", app.startup_console(&tui)?);

    if !app.config.mode.interactive {
        return Ok(());
    }

    let stdin = io::stdin();
    let mut stdout = io::stdout();
    run_session_loop(&mut app, &provider_client, &tui, stdin.lock(), &mut stdout)?;

    Ok(())
}
