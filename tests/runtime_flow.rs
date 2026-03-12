mod common;

use anvil::agent::{AgentEvent, AgentRuntime, AgentRuntimeScript};
use anvil::contracts::{AppEvent, RuntimeState};
use anvil::tui::Tui;

#[test]
fn runtime_turn_pauses_for_single_tool_call_approval_and_resumes_to_done() {
    let mut app = common::build_app();
    let tui = Tui::new();
    let runtime = AgentRuntime::from_script(AgentRuntimeScript::new(vec![
        AgentEvent::Thinking {
            status: "Thinking. model=local-default".to_string(),
            plan_items: vec![
                "inspect repository structure".to_string(),
                "edit session model".to_string(),
            ],
            active_index: Some(1),
            reasoning_summary: vec!["approval is required before write".to_string()],
            elapsed_ms: 120,
        },
        AgentEvent::ApprovalRequested {
            status: "Awaiting approval for 1 tool call".to_string(),
            tool_name: "Write".to_string(),
            summary: "Update src/session/mod.rs".to_string(),
            risk: "Confirm".to_string(),
            tool_call_id: "call_001".to_string(),
            elapsed_ms: 260,
        },
        AgentEvent::Working {
            status: "Working on tool execution".to_string(),
            plan_items: vec![
                "inspect repository structure".to_string(),
                "edit session model".to_string(),
            ],
            active_index: Some(1),
            tool_logs: vec![(
                "Write".to_string(),
                "update".to_string(),
                "src/session/mod.rs".to_string(),
            )],
            elapsed_ms: 540,
        },
        AgentEvent::Done {
            status: "Done. session saved".to_string(),
            assistant_message: "session flow is now runtime-driven".to_string(),
            completion_summary: "Updated the session model and saved the session.".to_string(),
            saved_status: "session saved".to_string(),
            tool_logs: vec![(
                "Write".to_string(),
                "update".to_string(),
                "src/session/mod.rs".to_string(),
            )],
            elapsed_ms: 920,
        },
    ]));

    let paused = app
        .run_runtime_turn("wire runtime flow", &runtime, &tui)
        .expect("runtime turn should pause for approval");

    assert_eq!(
        app.state_machine().snapshot().state,
        RuntimeState::AwaitingApproval
    );
    assert!(
        paused
            .iter()
            .any(|frame| frame.contains("[A] anvil > approval"))
    );
    assert_eq!(
        app.session()
            .messages
            .last()
            .expect("message should exist")
            .content,
        "wire runtime flow"
    );

    let resumed = app
        .approve_and_continue(&runtime, &tui)
        .expect("runtime turn should resume after approval");

    assert_eq!(app.state_machine().snapshot().state, RuntimeState::Done);
    assert!(
        resumed
            .iter()
            .any(|frame| frame.contains("[A] anvil > result"))
    );
    assert!(
        resumed
            .last()
            .expect("done frame should exist")
            .contains("[A] anvil > session flow is now runtime-driven")
    );
}

#[test]
fn runtime_turn_can_interrupt_and_reset_to_ready() {
    let mut app = common::build_app();
    let tui = Tui::new();
    let runtime = AgentRuntime::from_script(AgentRuntimeScript::new(vec![
        AgentEvent::Thinking {
            status: "Thinking. model=local-default".to_string(),
            plan_items: vec!["inspect runtime".to_string()],
            active_index: Some(0),
            reasoning_summary: vec!["user requested stop".to_string()],
            elapsed_ms: 90,
        },
        AgentEvent::Interrupted {
            status: "Interrupted safely".to_string(),
            interrupted_what: "provider turn".to_string(),
            saved_status: "session preserved".to_string(),
            next_actions: vec!["resume work".to_string(), "inspect status".to_string()],
            elapsed_ms: 180,
        },
    ]));

    let frames = app
        .run_runtime_turn("stop after analysis", &runtime, &tui)
        .expect("runtime turn should complete with interruption");

    assert_eq!(
        app.state_machine().snapshot().state,
        RuntimeState::Interrupted
    );
    assert!(
        frames
            .iter()
            .any(|frame| frame.contains("[A] anvil > interrupted"))
    );
    assert_eq!(
        app.session().session_event,
        Some(AppEvent::SessionNormalizedAfterInterrupt)
    );

    let ready = app.reset_to_ready().expect("reset should succeed");
    assert_eq!(ready.state, RuntimeState::Ready);
}
