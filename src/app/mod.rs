pub mod mock;

use crate::agent::BasicAgentLoop;
use crate::agent::{AgentEvent, AgentRuntime, PendingTurnState};
use crate::config::EffectiveConfig;
use crate::contracts::{
    AppEvent, AppStateSnapshot, ConsoleRenderContext, RuntimeState, ToolLogView,
};
use crate::provider::{
    ProviderBootstrapError, ProviderClient, ProviderErrorKind, ProviderErrorRecord, ProviderEvent,
    ProviderRuntimeContext, ProviderTurnError, build_local_provider_client,
};
use crate::session::{
    MessageRole, MessageStatus, SessionError, SessionMessage, SessionRecord, SessionStore,
    new_assistant_message, new_user_message,
};
use crate::state::{StateMachine, StateTransition};
use crate::tui::Tui;
use std::fmt::{Display, Formatter};
use std::io::{self, BufRead, Write};

const MAX_CONSOLE_MESSAGES: usize = 5;

pub struct App {
    config: EffectiveConfig,
    provider: ProviderRuntimeContext,
    state_machine: StateMachine,
    session_store: SessionStore,
    session: SessionRecord,
    pending_turn: Option<PendingTurnState>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionControl {
    Continue,
    Exit,
}

pub struct CliTurnOutput {
    pub frames: Vec<String>,
    pub control: SessionControl,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlashCommandAction {
    Help,
    Status,
    Plan,
    Model,
    Approve,
    Deny,
    Reset,
    Exit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlashCommandSpec {
    pub name: &'static str,
    pub description: &'static str,
    pub action: SlashCommandAction,
}

const SLASH_COMMANDS: [SlashCommandSpec; 8] = [
    SlashCommandSpec {
        name: "/help",
        description: "show available commands",
        action: SlashCommandAction::Help,
    },
    SlashCommandSpec {
        name: "/status",
        description: "show the current console state",
        action: SlashCommandAction::Status,
    },
    SlashCommandSpec {
        name: "/plan",
        description: "show the current plan and active step",
        action: SlashCommandAction::Plan,
    },
    SlashCommandSpec {
        name: "/model",
        description: "show the current model context",
        action: SlashCommandAction::Model,
    },
    SlashCommandSpec {
        name: "/approve",
        description: "continue the pending approved tool call",
        action: SlashCommandAction::Approve,
    },
    SlashCommandSpec {
        name: "/deny",
        description: "reject the pending tool call",
        action: SlashCommandAction::Deny,
    },
    SlashCommandSpec {
        name: "/reset",
        description: "return to Ready",
        action: SlashCommandAction::Reset,
    },
    SlashCommandSpec {
        name: "/exit",
        description: "exit the session",
        action: SlashCommandAction::Exit,
    },
];

#[derive(Debug)]
pub enum AppError {
    Config(crate::config::ConfigError),
    ProviderBootstrap(ProviderBootstrapError),
    Session(SessionError),
    ProviderTurn(ProviderTurnError),
    StateTransition(crate::state::StateTransitionError),
    NoPendingApproval,
    PendingApprovalRequired,
}

impl Display for AppError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Config(err) => write!(f, "{err}"),
            Self::ProviderBootstrap(err) => write!(f, "{err}"),
            Self::Session(err) => write!(f, "{err}"),
            Self::ProviderTurn(err) => write!(f, "{err}"),
            Self::StateTransition(err) => write!(f, "{err}"),
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

        Ok(Self {
            config,
            provider,
            state_machine: StateMachine::from_snapshot(initial_state_snapshot),
            session_store,
            pending_turn: session.pending_turn.clone(),
            session,
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
            render_resume_header(&self.config),
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
            self.provider.capabilities.streaming,
            self.config.runtime.context_window,
        );

        match BasicAgentLoop::run_turn(provider_client, &request) {
            Ok(provider_events) => {
                let mut frames = Vec::new();
                let mut token_buffer = String::new();
                for (index, event) in provider_events.iter().enumerate() {
                    match event {
                        ProviderEvent::Agent(agent_event) => {
                            let snapshot = self.apply_agent_event(&agent_event)?;
                            frames.push(self.render_console(tui)?);
                            if snapshot.state == RuntimeState::AwaitingApproval {
                                let remaining_events = provider_events[index + 1..]
                                    .iter()
                                    .filter_map(|event| match event {
                                        ProviderEvent::Agent(agent_event) => {
                                            Some(agent_event.clone())
                                        }
                                        ProviderEvent::TokenDelta(_) => None,
                                    })
                                    .collect::<Vec<_>>();
                                self.set_pending_turn(PendingTurnState {
                                    waiting_tool_call_id: approval_tool_call_id(agent_event),
                                    remaining_events,
                                })?;
                                break;
                            }
                        }
                        ProviderEvent::TokenDelta(delta) => {
                            token_buffer.push_str(delta);
                            frames.push(self.render_token_delta_frame(&token_buffer, tui)?);
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
        let Some(pending_turn) = self.pending_turn.clone() else {
            return Err(AppError::NoPendingApproval);
        };

        self.clear_pending_turn()?;
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
                    waiting_tool_call_id: approval_tool_call_id(event),
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
                    .with_tool_logs(build_tool_logs(tool_logs))
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
                    .with_tool_logs(build_tool_logs(tool_logs))
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
        let snapshot = AppStateSnapshot::new(RuntimeState::Thinking)
            .with_status(format!("Streaming. model={}", self.config.runtime.model))
            .with_reasoning_summary(vec![token_buffer.to_string()])
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
            return self.handle_slash_command(trimmed, tui);
        }

        match self.run_live_turn(trimmed, provider_client, tui) {
            Ok(frames) => Ok(CliTurnOutput {
                frames,
                control: SessionControl::Continue,
            }),
            Err(AppError::PendingApprovalRequired) => Ok(CliTurnOutput {
                frames: vec![render_pending_approval_frame()],
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
        tui: &Tui,
    ) -> Result<CliTurnOutput, AppError> {
        let output = match find_slash_command(command).map(|spec| spec.action) {
            Some(SlashCommandAction::Help) => CliTurnOutput {
                frames: vec![render_help_frame()],
                control: SessionControl::Continue,
            },
            Some(SlashCommandAction::Status) => CliTurnOutput {
                frames: vec![self.render_console(tui)?],
                control: SessionControl::Continue,
            },
            Some(SlashCommandAction::Plan) => CliTurnOutput {
                frames: vec![render_plan_frame(self.state_machine.snapshot())],
                control: SessionControl::Continue,
            },
            Some(SlashCommandAction::Model) => CliTurnOutput {
                frames: vec![render_model_frame(&self.config)],
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
            _ => CliTurnOutput {
                frames: vec![format!(
                    "Unknown command: {command}\nTry /help for available commands."
                )],
                control: SessionControl::Continue,
            },
        };

        Ok(output)
    }
}

fn build_tool_logs(logs: &[(String, String, String)]) -> Vec<ToolLogView> {
    logs.iter()
        .map(|(tool_name, action, target)| ToolLogView {
            tool_name: tool_name.clone(),
            action: action.clone(),
            target: target.clone(),
        })
        .collect()
}

fn approval_tool_call_id(event: &AgentEvent) -> String {
    match event {
        AgentEvent::ApprovalRequested { tool_call_id, .. } => tool_call_id.clone(),
        _ => "pending_approval".to_string(),
    }
}

pub fn render_help_frame() -> String {
    let mut lines = vec!["Anvil slash commands".to_string(), String::new()];
    for spec in slash_commands() {
        lines.push(format!("{:<10} {}", spec.name, spec.description));
    }
    lines.join("\n")
}

fn render_plan_frame(snapshot: &AppStateSnapshot) -> String {
    let mut lines = vec!["[A] anvil > plan".to_string()];
    if let Some(plan) = &snapshot.plan {
        for (index, item) in plan.items.iter().enumerate() {
            let marker = if plan.active_index == Some(index) {
                "*"
            } else {
                "-"
            };
            lines.push(format!("  {marker} {}. {}", index + 1, item));
        }
    } else {
        lines.push("  no active plan".to_string());
    }
    lines.join("\n")
}

fn render_model_frame(config: &EffectiveConfig) -> String {
    format!(
        "[A] anvil > current model: {}\n  provider: {}\n  context window: {}",
        config.runtime.model, config.runtime.provider, config.runtime.context_window
    )
}

fn render_resume_header(config: &EffectiveConfig) -> String {
    [
        "  --------------------------------------------------------------".to_string(),
        "  Resuming existing session".to_string(),
        format!("  Model   : {}", config.runtime.model),
        format!("  Context : {}k", config.runtime.context_window / 1_000),
        format!("  Project : {}", config.paths.cwd.display()),
        "  --------------------------------------------------------------".to_string(),
    ]
    .join("\n")
}

pub fn cli_prompt() -> &'static str {
    "[U] you > "
}

pub fn slash_commands() -> &'static [SlashCommandSpec] {
    &SLASH_COMMANDS
}

fn find_slash_command(command: &str) -> Option<SlashCommandSpec> {
    slash_commands()
        .iter()
        .copied()
        .find(|spec| spec.name == command || (spec.name == "/exit" && command == "/quit"))
}

fn render_pending_approval_frame() -> String {
    "[A] anvil > resolve the pending approval before starting a new turn\n  use /approve or /deny"
        .to_string()
}

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
