use crate::contracts::{AppEvent, AppStateSnapshot, RuntimeState};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StateTransition {
    StartThinking,
    RequestApproval,
    StartWorking,
    Interrupt,
    Finish,
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

impl StateMachine {
    pub fn new() -> Self {
        Self {
            snapshot: AppStateSnapshot::new(RuntimeState::Ready),
        }
    }

    pub fn snapshot(&self) -> &AppStateSnapshot {
        &self.snapshot
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
            StateTransition::Interrupt => RuntimeState::Interrupted,
            StateTransition::Finish => RuntimeState::Done,
            StateTransition::ResetToReady => RuntimeState::Ready,
        };

        debug_assert_eq!(snapshot.state, expected);
        let from = self.snapshot.state;
        let to = snapshot.state;

        let valid = matches!(
            (from, transition, to),
            (RuntimeState::Ready, StateTransition::ResetToReady, RuntimeState::Ready)
                | (RuntimeState::Ready, StateTransition::StartThinking, RuntimeState::Thinking)
                | (
                    RuntimeState::Thinking,
                    StateTransition::RequestApproval,
                    RuntimeState::AwaitingApproval
                )
                | (RuntimeState::Thinking, StateTransition::StartWorking, RuntimeState::Working)
                | (RuntimeState::Thinking, StateTransition::Interrupt, RuntimeState::Interrupted)
                | (RuntimeState::Thinking, StateTransition::Finish, RuntimeState::Done)
                | (
                    RuntimeState::AwaitingApproval,
                    StateTransition::StartWorking,
                    RuntimeState::Working
                )
                | (
                    RuntimeState::AwaitingApproval,
                    StateTransition::ResetToReady,
                    RuntimeState::Ready
                )
                | (
                    RuntimeState::AwaitingApproval,
                    StateTransition::Interrupt,
                    RuntimeState::Interrupted
                )
                | (RuntimeState::Working, StateTransition::Interrupt, RuntimeState::Interrupted)
                | (RuntimeState::Working, StateTransition::Finish, RuntimeState::Done)
                | (RuntimeState::Interrupted, StateTransition::ResetToReady, RuntimeState::Ready)
                | (RuntimeState::Done, StateTransition::ResetToReady, RuntimeState::Ready)
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
