pub mod mock;

use crate::agent::BasicAgentLoop;
use crate::agent::{AgentEvent, AgentRuntime, PendingTurnState};
use crate::config::EffectiveConfig;
use crate::contracts::{
    AppEvent, AppStateSnapshot, ConsoleRenderContext, RuntimeState, ToolLogView,
};
use crate::provider::{
    ProviderBootstrapError, ProviderClient, ProviderRuntimeContext, ProviderTurnError,
};
use crate::session::{
    MessageStatus, SessionError, SessionRecord, SessionStore, new_assistant_message,
    new_user_message,
};
use crate::state::{StateMachine, StateTransition};
use crate::tui::Tui;
use std::fmt::{Display, Formatter};

const MAX_CONSOLE_MESSAGES: usize = 5;

pub struct App {
    config: EffectiveConfig,
    provider: ProviderRuntimeContext,
    state_machine: StateMachine,
    session_store: SessionStore,
    session: SessionRecord,
    pending_turn: Option<PendingTurnState>,
}

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
        let mut frames = vec![self.render_live_thinking_frame(tui)?];

        let request = BasicAgentLoop::build_turn_request(
            self.config.runtime.model.clone(),
            &self.session,
            self.provider.capabilities.streaming,
        );

        match BasicAgentLoop::run_turn(provider_client, &request) {
            Ok(response) => {
                frames.extend(self.execute_runtime_events(&response.events, tui)?);
                Ok(frames)
            }
            Err(ProviderTurnError::Cancelled) => {
                frames.extend(self.execute_runtime_events(
                    &[AgentEvent::Interrupted {
                        status: "Interrupted safely".to_string(),
                        interrupted_what: "provider turn".to_string(),
                        saved_status: "session preserved".to_string(),
                        next_actions: vec!["resume work".to_string(), "inspect status".to_string()],
                        elapsed_ms: 0,
                    }],
                    tui,
                )?);
                Ok(frames)
            }
            Err(ProviderTurnError::Backend(message)) => {
                frames.extend(self.execute_runtime_events(
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
                )?);
                Ok(frames)
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

    fn render_live_thinking_frame(&mut self, tui: &Tui) -> Result<String, AppError> {
        let snapshot = AppStateSnapshot::new(RuntimeState::Thinking)
            .with_status(format!("Thinking. model={}", self.config.runtime.model))
            .with_elapsed_ms(0)
            .with_context_usage(
                self.session.estimated_token_count(),
                self.config.runtime.context_window,
            );
        self.apply_transition(snapshot, StateTransition::StartThinking)?;
        self.render_console(tui)
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

    pub fn has_pending_runtime_events(&self) -> bool {
        self.pending_turn.is_some()
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

pub fn run() -> Result<(), AppError> {
    let config = EffectiveConfig::load()?;
    let provider = ProviderRuntimeContext::bootstrap(&config)?;
    let mut app = App::new(config, provider)?;
    let tui = Tui::new();
    let _ = app.initial_snapshot()?;

    println!("{}", app.render_console(&tui)?);
    Ok(())
}
