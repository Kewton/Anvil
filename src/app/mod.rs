use crate::config::EffectiveConfig;
use crate::contracts::{AppEvent, AppStateSnapshot, RuntimeState};
use crate::provider::{ProviderBootstrapError, ProviderRuntimeContext};
use crate::state::{StateMachine, StateTransition};
use crate::tui::Tui;
use std::fmt::{Display, Formatter};

pub struct App {
    config: EffectiveConfig,
    provider: ProviderRuntimeContext,
    state_machine: StateMachine,
}

#[derive(Debug)]
pub enum AppError {
    Config(crate::config::ConfigError),
    ProviderBootstrap(ProviderBootstrapError),
    StateTransition(crate::state::StateTransitionError),
}

impl Display for AppError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Config(err) => write!(f, "{err}"),
            Self::ProviderBootstrap(err) => write!(f, "{err}"),
            Self::StateTransition(err) => write!(f, "{err}"),
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

impl From<crate::state::StateTransitionError> for AppError {
    fn from(value: crate::state::StateTransitionError) -> Self {
        Self::StateTransition(value)
    }
}

impl App {
    pub fn new(config: EffectiveConfig, provider: ProviderRuntimeContext) -> Self {
        Self {
            config,
            provider,
            state_machine: StateMachine::new(),
        }
    }

    pub fn initial_snapshot(&mut self) -> Result<AppStateSnapshot, AppError> {
        let snapshot = self.build_initial_snapshot();
        self.state_machine
            .transition_to(snapshot.clone(), StateTransition::ResetToReady)?;
        Ok(snapshot)
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
    }

    pub fn state_machine(&self) -> &StateMachine {
        &self.state_machine
    }

    pub fn startup_events(&self) -> [AppEvent; 3] {
        [
            AppEvent::ConfigLoaded,
            AppEvent::ProviderBootstrapped,
            AppEvent::StartupCompleted,
        ]
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
            ]);
        self.state_machine
            .transition_to(snapshot.clone(), StateTransition::StartThinking)?;
        Ok(snapshot)
    }

    pub fn mock_approval_snapshot(&mut self) -> Result<AppStateSnapshot, AppError> {
        let snapshot = AppStateSnapshot::new(RuntimeState::AwaitingApproval)
            .with_status("Awaiting approval for 1 tool call".to_string())
            .with_approval(
                "Write".to_string(),
                "Create workspace/anvil-notes.md".to_string(),
                "Confirm".to_string(),
                "call_001".to_string(),
            );
        self.state_machine
            .transition_to(snapshot.clone(), StateTransition::RequestApproval)?;
        Ok(snapshot)
    }

    pub fn mock_interrupted_snapshot(&mut self) -> Result<AppStateSnapshot, AppError> {
        let snapshot = AppStateSnapshot::new(RuntimeState::Interrupted)
            .with_status("Interrupted safely".to_string())
            .with_interrupt(
                "provider turn".to_string(),
                "session preserved".to_string(),
                vec!["resume work".to_string(), "inspect status".to_string()],
            );
        self.state_machine
            .transition_to(snapshot.clone(), StateTransition::Interrupt)?;
        Ok(snapshot)
    }
}

pub fn run() -> Result<(), AppError> {
    let config = EffectiveConfig::load()?;
    let provider = ProviderRuntimeContext::bootstrap(&config)?;
    let mut app = App::new(config, provider);
    let tui = Tui::new();
    let snapshot = app.initial_snapshot()?;

    println!("{}", tui.render(&snapshot));
    Ok(())
}
