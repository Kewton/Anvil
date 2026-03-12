use crate::agent::{AgentEvent, AgentRuntime};
use crate::config::EffectiveConfig;
use crate::contracts::{
    AppEvent, AppStateSnapshot, ConsoleRenderContext, RuntimeState, ToolLogView,
};
use crate::provider::{ProviderBootstrapError, ProviderRuntimeContext};
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
    pending_runtime_events: Vec<AgentEvent>,
}

#[derive(Debug)]
pub enum AppError {
    Config(crate::config::ConfigError),
    ProviderBootstrap(ProviderBootstrapError),
    Session(SessionError),
    StateTransition(crate::state::StateTransitionError),
    NoPendingApproval,
}

impl Display for AppError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Config(err) => write!(f, "{err}"),
            Self::ProviderBootstrap(err) => write!(f, "{err}"),
            Self::Session(err) => write!(f, "{err}"),
            Self::StateTransition(err) => write!(f, "{err}"),
            Self::NoPendingApproval => write!(f, "no pending approval to continue"),
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

        Ok(Self {
            config,
            provider,
            state_machine: StateMachine::new(),
            session_store,
            session,
            pending_runtime_events: Vec::new(),
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
        let user_input = user_input.into();
        self.pending_runtime_events.clear();
        self.record_user_input(self.next_message_id("user"), user_input)?;
        self.execute_runtime_events(runtime.events(), tui)
    }

    pub fn approve_and_continue(
        &mut self,
        _runtime: &AgentRuntime,
        tui: &Tui,
    ) -> Result<Vec<String>, AppError> {
        if self.pending_runtime_events.is_empty() {
            return Err(AppError::NoPendingApproval);
        }

        let remaining = std::mem::take(&mut self.pending_runtime_events);
        self.execute_runtime_events(&remaining, tui)
    }

    pub fn reset_to_ready(&mut self) -> Result<AppStateSnapshot, AppError> {
        let snapshot = AppStateSnapshot::new(RuntimeState::Ready)
            .with_status("Ready for the next task".to_string())
            .with_context_usage(
                self.session.estimated_token_count(),
                self.config.runtime.context_window,
            );
        self.apply_transition(snapshot, StateTransition::ResetToReady)
    }

    pub fn mock_thinking_snapshot(&mut self) -> Result<AppStateSnapshot, AppError> {
        let snapshot = AppStateSnapshot::new(RuntimeState::Thinking)
            .with_status(format!("Thinking. model={}", self.config.runtime.model))
            .with_plan(
                vec![
                    "inspect repository structure".to_string(),
                    "map runtime and tool flow".to_string(),
                    "summarize constraints".to_string(),
                ],
                Some(1),
            )
            .with_reasoning_summary(vec![
                "main startup wiring is the current focus".to_string(),
                "provider integration is not enabled yet".to_string(),
            ])
            .with_elapsed_ms(240)
            .with_context_usage(
                self.session.estimated_token_count(),
                self.config.runtime.context_window,
            );
        self.apply_transition(snapshot, StateTransition::StartThinking)
    }

    pub fn mock_approval_snapshot(&mut self) -> Result<AppStateSnapshot, AppError> {
        let snapshot = AppStateSnapshot::new(RuntimeState::AwaitingApproval)
            .with_status("Awaiting approval for 1 tool call".to_string())
            .with_approval(
                "Write".to_string(),
                "Create workspace/anvil-notes.md".to_string(),
                "Confirm".to_string(),
                "call_001".to_string(),
            )
            .with_elapsed_ms(510)
            .with_context_usage(
                self.session.estimated_token_count(),
                self.config.runtime.context_window,
            );
        self.apply_transition(snapshot, StateTransition::RequestApproval)
    }

    pub fn mock_interrupted_snapshot(&mut self) -> Result<AppStateSnapshot, AppError> {
        let snapshot = AppStateSnapshot::new(RuntimeState::Interrupted)
            .with_status("Interrupted safely".to_string())
            .with_interrupt(
                "provider turn".to_string(),
                "session preserved".to_string(),
                vec!["resume work".to_string(), "inspect status".to_string()],
            )
            .with_elapsed_ms(820)
            .with_context_usage(
                self.session.estimated_token_count(),
                self.config.runtime.context_window,
            );
        self.session.normalize_interrupted_turn("provider turn");
        self.apply_transition(snapshot, StateTransition::Interrupt)?;
        self.persist_session(AppEvent::SessionNormalizedAfterInterrupt)?;
        Ok(self.state_machine.snapshot().clone())
    }

    pub fn mock_working_snapshot(&mut self) -> Result<AppStateSnapshot, AppError> {
        let snapshot = AppStateSnapshot::new(RuntimeState::Working)
            .with_status("Working on tool execution".to_string())
            .with_plan(
                vec![
                    "inspect repository structure".to_string(),
                    "execute reads and summarize findings".to_string(),
                    "prepare next action".to_string(),
                ],
                Some(1),
            )
            .with_tool_logs(vec![
                ToolLogView {
                    tool_name: "Read".to_string(),
                    action: "open".to_string(),
                    target: "src/app/mod.rs".to_string(),
                },
                ToolLogView {
                    tool_name: "Grep".to_string(),
                    action: "search".to_string(),
                    target: "StateTransition".to_string(),
                },
            ])
            .with_elapsed_ms(1_240)
            .with_context_usage(
                self.session.estimated_token_count(),
                self.config.runtime.context_window,
            );
        self.apply_transition(snapshot, StateTransition::StartWorking)
    }

    pub fn mock_done_snapshot(&mut self) -> Result<AppStateSnapshot, AppError> {
        self.record_assistant_output(
            "msg_assistant_done",
            "調査結果を整理しました。local-first の強みを保ちつつ、runtime と tui の責務を分離できます。",
        )?;
        let snapshot = AppStateSnapshot::new(RuntimeState::Done)
            .with_status("Done. session saved".to_string())
            .with_tool_logs(vec![
                ToolLogView {
                    tool_name: "Read".to_string(),
                    action: "open".to_string(),
                    target: "src/app/mod.rs".to_string(),
                },
                ToolLogView {
                    tool_name: "Write".to_string(),
                    action: "update".to_string(),
                    target: "workspace/work-plan.md".to_string(),
                },
            ])
            .with_completion_summary(
                "Updated the state and session foundation and saved the session.",
                "session saved",
            )
            .with_elapsed_ms(3_120)
            .with_context_usage(
                self.session.estimated_token_count(),
                self.config.runtime.context_window,
            );
        self.apply_transition(snapshot, StateTransition::Finish)
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
        self.pending_runtime_events.clear();

        for (index, event) in events.iter().enumerate() {
            let snapshot = self.apply_agent_event(event)?;
            frames.push(self.render_console(tui)?);

            if snapshot.state == RuntimeState::AwaitingApproval {
                self.pending_runtime_events = events[index + 1..].to_vec();
                break;
            }
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

pub fn run() -> Result<(), AppError> {
    let config = EffectiveConfig::load()?;
    let provider = ProviderRuntimeContext::bootstrap(&config)?;
    let mut app = App::new(config, provider)?;
    let tui = Tui::new();
    let _ = app.initial_snapshot()?;

    println!("{}", app.render_console(&tui)?);
    Ok(())
}
