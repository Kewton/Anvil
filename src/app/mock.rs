use crate::app::{App, AppError};
use crate::contracts::{AppEvent, AppStateSnapshot, RuntimeState, ToolLogView};
use crate::state::StateTransition;

pub trait MockAppExt {
    fn mock_thinking_snapshot(&mut self) -> Result<AppStateSnapshot, AppError>;
    fn mock_approval_snapshot(&mut self) -> Result<AppStateSnapshot, AppError>;
    fn mock_interrupted_snapshot(&mut self) -> Result<AppStateSnapshot, AppError>;
    fn mock_working_snapshot(&mut self) -> Result<AppStateSnapshot, AppError>;
    fn mock_done_snapshot(&mut self) -> Result<AppStateSnapshot, AppError>;
}

impl MockAppExt for App {
    fn mock_thinking_snapshot(&mut self) -> Result<AppStateSnapshot, AppError> {
        let snapshot = AppStateSnapshot::new(RuntimeState::Thinking)
            .with_status(format!("Thinking. model={}", self.config().runtime.model))
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
                self.session().estimated_token_count(),
                self.config().runtime.context_window,
            );
        self.apply_transition_for_mock(snapshot, StateTransition::StartThinking)
    }

    fn mock_approval_snapshot(&mut self) -> Result<AppStateSnapshot, AppError> {
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
                self.session().estimated_token_count(),
                self.config().runtime.context_window,
            );
        self.apply_transition_for_mock(snapshot, StateTransition::RequestApproval)
    }

    fn mock_interrupted_snapshot(&mut self) -> Result<AppStateSnapshot, AppError> {
        let snapshot = AppStateSnapshot::new(RuntimeState::Interrupted)
            .with_status("Interrupted safely".to_string())
            .with_interrupt(
                "provider turn".to_string(),
                "session preserved".to_string(),
                vec!["resume work".to_string(), "inspect status".to_string()],
            )
            .with_elapsed_ms(820)
            .with_context_usage(
                self.session().estimated_token_count(),
                self.config().runtime.context_window,
            );
        self.session_mut()
            .normalize_interrupted_turn("provider turn");
        self.apply_transition_for_mock(snapshot, StateTransition::Interrupt)?;
        self.persist_session_event_for_mock(AppEvent::SessionNormalizedAfterInterrupt)?;
        Ok(self.state_machine().snapshot().clone())
    }

    fn mock_working_snapshot(&mut self) -> Result<AppStateSnapshot, AppError> {
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
                self.session().estimated_token_count(),
                self.config().runtime.context_window,
            );
        self.apply_transition_for_mock(snapshot, StateTransition::StartWorking)
    }

    fn mock_done_snapshot(&mut self) -> Result<AppStateSnapshot, AppError> {
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
            .with_inference_performance(crate::contracts::InferencePerformanceView {
                tokens_per_sec_tenths: Some(325),
                eval_tokens: Some(150),
                eval_duration_ms: Some(4615),
            })
            .with_context_usage(
                self.session().estimated_token_count(),
                self.config().runtime.context_window,
            );
        self.apply_transition_for_mock(snapshot, StateTransition::Finish)
    }
}
