//! Explicit state machine governing application lifecycle.
//!
//! All state transitions are validated by [`StateMachine::transition_to`]
//! against a whitelist of legal (from, transition, to) triples.

use crate::contracts::{AppEvent, AppStateSnapshot, RuntimeState};

/// Named transitions between runtime states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StateTransition {
    StartThinking,
    RequestApproval,
    StartWorking,
    ResumeThinking,
    Interrupt,
    Finish,
    Fail,
    ResetToReady,
}

#[derive(Debug, Clone)]
pub struct StateMachine {
    snapshot: AppStateSnapshot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StateTransitionError {
    pub from: RuntimeState,
    pub to: RuntimeState,
    pub transition: StateTransition,
}

impl std::fmt::Display for StateTransitionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "invalid state transition: {:?} -> {:?} via {:?}",
            self.from, self.to, self.transition
        )
    }
}

impl std::error::Error for StateTransitionError {}

impl Default for StateMachine {
    fn default() -> Self {
        Self::new()
    }
}

impl StateMachine {
    pub fn new() -> Self {
        Self {
            snapshot: AppStateSnapshot::new(RuntimeState::Ready),
        }
    }

    pub fn from_snapshot(snapshot: AppStateSnapshot) -> Self {
        Self { snapshot }
    }

    pub fn snapshot(&self) -> &AppStateSnapshot {
        &self.snapshot
    }

    pub fn replace_snapshot(&mut self, snapshot: AppStateSnapshot) {
        self.snapshot = snapshot;
    }

    pub fn transition_to(
        &mut self,
        snapshot: AppStateSnapshot,
        transition: StateTransition,
    ) -> Result<(), StateTransitionError> {
        let expected = match transition {
            StateTransition::StartThinking => RuntimeState::Thinking,
            StateTransition::RequestApproval => RuntimeState::AwaitingApproval,
            StateTransition::StartWorking => RuntimeState::Working,
            StateTransition::ResumeThinking => RuntimeState::Thinking,
            StateTransition::Interrupt => RuntimeState::Interrupted,
            StateTransition::Finish => RuntimeState::Done,
            StateTransition::Fail => RuntimeState::Error,
            StateTransition::ResetToReady => RuntimeState::Ready,
        };

        debug_assert_eq!(snapshot.state, expected);
        let from = self.snapshot.state;
        let to = snapshot.state;

        let valid = matches!(
            (from, transition, to),
            (
                RuntimeState::Ready,
                StateTransition::ResetToReady,
                RuntimeState::Ready
            ) | (
                RuntimeState::Ready,
                StateTransition::StartThinking,
                RuntimeState::Thinking
            ) | (
                RuntimeState::Thinking,
                StateTransition::StartThinking,
                RuntimeState::Thinking
            ) | (
                RuntimeState::Thinking,
                StateTransition::RequestApproval,
                RuntimeState::AwaitingApproval
            ) | (
                RuntimeState::Thinking,
                StateTransition::StartWorking,
                RuntimeState::Working
            ) | (
                RuntimeState::Thinking,
                StateTransition::Interrupt,
                RuntimeState::Interrupted
            ) | (
                RuntimeState::Thinking,
                StateTransition::Finish,
                RuntimeState::Done
            ) | (
                RuntimeState::Done,
                StateTransition::StartThinking,
                RuntimeState::Thinking
            ) | (
                RuntimeState::Error,
                StateTransition::StartThinking,
                RuntimeState::Thinking
            ) | (
                RuntimeState::Interrupted,
                StateTransition::StartThinking,
                RuntimeState::Thinking
            ) | (
                RuntimeState::AwaitingApproval,
                StateTransition::StartWorking,
                RuntimeState::Working
            ) | (
                RuntimeState::AwaitingApproval,
                StateTransition::Finish,
                RuntimeState::Done
            ) | (
                RuntimeState::AwaitingApproval,
                StateTransition::ResetToReady,
                RuntimeState::Ready
            ) | (
                RuntimeState::AwaitingApproval,
                StateTransition::Interrupt,
                RuntimeState::Interrupted
            ) | (
                RuntimeState::AwaitingApproval,
                StateTransition::Fail,
                RuntimeState::Error
            ) | (
                RuntimeState::Working,
                StateTransition::ResumeThinking,
                RuntimeState::Thinking
            ) | (
                RuntimeState::Working,
                StateTransition::Interrupt,
                RuntimeState::Interrupted
            ) | (
                RuntimeState::Working,
                StateTransition::Finish,
                RuntimeState::Done
            ) | (
                RuntimeState::Working,
                StateTransition::Fail,
                RuntimeState::Error
            ) | (
                RuntimeState::Thinking,
                StateTransition::Fail,
                RuntimeState::Error
            ) | (
                RuntimeState::Thinking,
                StateTransition::ResetToReady,
                RuntimeState::Ready
            ) | (
                RuntimeState::Working,
                StateTransition::ResetToReady,
                RuntimeState::Ready
            ) | (
                RuntimeState::Interrupted,
                StateTransition::ResetToReady,
                RuntimeState::Ready
            ) | (
                RuntimeState::Done,
                StateTransition::ResetToReady,
                RuntimeState::Ready
            ) | (
                RuntimeState::Error,
                StateTransition::ResetToReady,
                RuntimeState::Ready
            )
        );

        if !valid {
            return Err(StateTransitionError {
                from,
                to,
                transition,
            });
        }

        self.snapshot = snapshot.with_event(AppEvent::StateChanged);
        Ok(())
    }
}
