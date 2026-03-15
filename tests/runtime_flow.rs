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
    assert!(app.has_pending_runtime_events());
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
    assert!(matches!(
        app.run_runtime_turn("new request before approval", &runtime, &tui),
        Err(anvil::app::AppError::PendingApprovalRequired)
    ));

    let resumed = app
        .approve_and_continue(&runtime, &tui)
        .expect("runtime turn should resume after approval");

    assert_eq!(app.state_machine().snapshot().state, RuntimeState::Done);
    assert!(!app.has_pending_runtime_events());
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
        app.session().event_log.last().copied(),
        Some(AppEvent::SessionNormalizedAfterInterrupt)
    );

    let ready = app.reset_to_ready().expect("reset should succeed");
    assert_eq!(ready.state, RuntimeState::Ready);
}

#[test]
fn runtime_turn_can_deny_approval_and_return_to_ready() {
    let mut app = common::build_app();
    let tui = Tui::new();
    let runtime = AgentRuntime::from_script(AgentRuntimeScript::new(vec![
        AgentEvent::Thinking {
            status: "Thinking. model=local-default".to_string(),
            plan_items: vec!["prepare write".to_string()],
            active_index: Some(0),
            reasoning_summary: vec!["write needs confirmation".to_string()],
            elapsed_ms: 100,
        },
        AgentEvent::ApprovalRequested {
            status: "Awaiting approval for 1 tool call".to_string(),
            tool_name: "Write".to_string(),
            summary: "Update ANVIL.md".to_string(),
            risk: "Confirm".to_string(),
            tool_call_id: "call_deny_001".to_string(),
            elapsed_ms: 220,
        },
        AgentEvent::Done {
            status: "Done. session saved".to_string(),
            assistant_message: "this should not be emitted".to_string(),
            completion_summary: "unexpected completion".to_string(),
            saved_status: "session saved".to_string(),
            tool_logs: Vec::new(),
            elapsed_ms: 400,
        },
    ]));

    let _ = app
        .run_runtime_turn("deny this write", &runtime, &tui)
        .expect("runtime turn should pause for approval");

    let denied = app
        .deny_and_abort(&tui)
        .expect("deny should return to ready");

    assert_eq!(app.state_machine().snapshot().state, RuntimeState::Ready);
    assert!(!app.has_pending_runtime_events());
    assert!(
        denied
            .last()
            .expect("ready frame should exist")
            .contains("Approval denied")
    );
}

#[test]
fn runtime_turn_supports_multiple_approvals_in_one_turn() {
    let mut app = common::build_app();
    let tui = Tui::new();
    let runtime = AgentRuntime::from_script(AgentRuntimeScript::new(vec![
        AgentEvent::Thinking {
            status: "Thinking. model=local-default".to_string(),
            plan_items: vec![
                "prepare first write".to_string(),
                "prepare second write".to_string(),
            ],
            active_index: Some(0),
            reasoning_summary: vec!["two writes require confirmation".to_string()],
            elapsed_ms: 100,
        },
        AgentEvent::ApprovalRequested {
            status: "Awaiting approval for 1 tool call".to_string(),
            tool_name: "Write".to_string(),
            summary: "Update src/app/mod.rs".to_string(),
            risk: "Confirm".to_string(),
            tool_call_id: "call_multi_001".to_string(),
            elapsed_ms: 180,
        },
        AgentEvent::Working {
            status: "Working on first tool execution".to_string(),
            plan_items: vec![
                "prepare first write".to_string(),
                "prepare second write".to_string(),
            ],
            active_index: Some(0),
            tool_logs: vec![(
                "Write".to_string(),
                "update".to_string(),
                "src/app/mod.rs".to_string(),
            )],
            elapsed_ms: 260,
        },
        AgentEvent::Thinking {
            status: "Thinking after first approval".to_string(),
            plan_items: vec![
                "prepare first write".to_string(),
                "prepare second write".to_string(),
            ],
            active_index: Some(1),
            reasoning_summary: vec!["second write still needs confirmation".to_string()],
            elapsed_ms: 300,
        },
        AgentEvent::ApprovalRequested {
            status: "Awaiting approval for 1 tool call".to_string(),
            tool_name: "Write".to_string(),
            summary: "Update src/session/mod.rs".to_string(),
            risk: "Confirm".to_string(),
            tool_call_id: "call_multi_002".to_string(),
            elapsed_ms: 320,
        },
        AgentEvent::Working {
            status: "Working on second tool execution".to_string(),
            plan_items: vec![
                "prepare first write".to_string(),
                "prepare second write".to_string(),
            ],
            active_index: Some(1),
            tool_logs: vec![(
                "Write".to_string(),
                "update".to_string(),
                "src/session/mod.rs".to_string(),
            )],
            elapsed_ms: 420,
        },
        AgentEvent::Done {
            status: "Done. session saved".to_string(),
            assistant_message: "both approvals were processed".to_string(),
            completion_summary: "Completed the requested writes.".to_string(),
            saved_status: "session saved".to_string(),
            tool_logs: vec![(
                "Write".to_string(),
                "update".to_string(),
                "src/session/mod.rs".to_string(),
            )],
            elapsed_ms: 640,
        },
    ]));

    let _ = app
        .run_runtime_turn("apply two writes", &runtime, &tui)
        .expect("first approval pause should succeed");
    assert_eq!(
        app.state_machine().snapshot().state,
        RuntimeState::AwaitingApproval
    );
    assert!(app.has_pending_runtime_events());

    let resumed_once = app
        .approve_and_continue(&runtime, &tui)
        .expect("second approval pause should succeed");
    assert!(
        resumed_once
            .iter()
            .any(|frame| frame.contains("call_multi_002"))
    );
    assert_eq!(
        app.state_machine().snapshot().state,
        RuntimeState::AwaitingApproval
    );
    assert!(app.has_pending_runtime_events());

    let resumed_twice = app
        .approve_and_continue(&runtime, &tui)
        .expect("final completion should succeed");
    assert_eq!(app.state_machine().snapshot().state, RuntimeState::Done);
    assert!(!app.has_pending_runtime_events());
    assert!(
        resumed_twice
            .last()
            .expect("done frame should exist")
            .contains("both approvals were processed")
    );
}

#[test]
fn runtime_turn_supports_working_back_to_thinking_before_done() {
    let mut app = common::build_app();
    let tui = Tui::new();
    let runtime = AgentRuntime::from_script(AgentRuntimeScript::new(vec![
        AgentEvent::Thinking {
            status: "Thinking. model=local-default".to_string(),
            plan_items: vec!["inspect".to_string(), "summarize".to_string()],
            active_index: Some(0),
            reasoning_summary: vec!["starting analysis".to_string()],
            elapsed_ms: 80,
        },
        AgentEvent::Working {
            status: "Working on repository scan".to_string(),
            plan_items: vec!["inspect".to_string(), "summarize".to_string()],
            active_index: Some(0),
            tool_logs: vec![(
                "Read".to_string(),
                "open".to_string(),
                "src/app/mod.rs".to_string(),
            )],
            elapsed_ms: 160,
        },
        AgentEvent::Thinking {
            status: "Thinking after tool results".to_string(),
            plan_items: vec!["inspect".to_string(), "summarize".to_string()],
            active_index: Some(1),
            reasoning_summary: vec!["tool output is sufficient".to_string()],
            elapsed_ms: 240,
        },
        AgentEvent::Done {
            status: "Done. session saved".to_string(),
            assistant_message: "analysis resumed after tool execution".to_string(),
            completion_summary: "Summarized the repository scan.".to_string(),
            saved_status: "session saved".to_string(),
            tool_logs: Vec::new(),
            elapsed_ms: 360,
        },
    ]));

    let frames = app
        .run_runtime_turn("scan and summarize", &runtime, &tui)
        .expect("runtime turn should complete");

    assert_eq!(app.state_machine().snapshot().state, RuntimeState::Done);
    assert!(
        frames
            .iter()
            .any(|frame| frame.contains("Thinking after tool results"))
    );
}

#[test]
fn runtime_turn_can_fail_into_error_state() {
    let mut app = common::build_app();
    let tui = Tui::new();
    let runtime = AgentRuntime::from_script(AgentRuntimeScript::new(vec![
        AgentEvent::Thinking {
            status: "Thinking. model=local-default".to_string(),
            plan_items: vec!["inspect runtime".to_string()],
            active_index: Some(0),
            reasoning_summary: vec!["provider output malformed".to_string()],
            elapsed_ms: 70,
        },
        AgentEvent::Failed {
            status: "Error. runtime turn failed".to_string(),
            error_summary: "provider emitted malformed tool call".to_string(),
            recommended_actions: vec!["retry turn".to_string(), "inspect provider".to_string()],
            elapsed_ms: 140,
        },
    ]));

    let frames = app
        .run_runtime_turn("trigger failure", &runtime, &tui)
        .expect("runtime turn should reach error state");

    assert_eq!(app.state_machine().snapshot().state, RuntimeState::Error);
    assert!(
        frames
            .last()
            .expect("error frame should exist")
            .contains("[A] anvil > error")
    );
}

#[test]
fn pending_approval_survives_app_reload() {
    let root = common::unique_test_dir("pending_reload");
    let mut app = common::build_app_in(root.clone());
    let tui = Tui::new();
    let runtime = AgentRuntime::from_script(AgentRuntimeScript::new(vec![
        AgentEvent::Thinking {
            status: "Thinking. model=local-default".to_string(),
            plan_items: vec!["prepare write".to_string()],
            active_index: Some(0),
            reasoning_summary: vec!["write needs confirmation".to_string()],
            elapsed_ms: 100,
        },
        AgentEvent::ApprovalRequested {
            status: "Awaiting approval for 1 tool call".to_string(),
            tool_name: "Write".to_string(),
            summary: "Update src/app/mod.rs".to_string(),
            risk: "Confirm".to_string(),
            tool_call_id: "call_resume_001".to_string(),
            elapsed_ms: 200,
        },
        AgentEvent::Working {
            status: "Working on approved tool execution".to_string(),
            plan_items: vec!["prepare write".to_string()],
            active_index: Some(0),
            tool_logs: vec![(
                "Write".to_string(),
                "update".to_string(),
                "src/app/mod.rs".to_string(),
            )],
            elapsed_ms: 260,
        },
        AgentEvent::Done {
            status: "Done. session saved".to_string(),
            assistant_message: "approval resumed after reload".to_string(),
            completion_summary: "Resumed the pending approval path.".to_string(),
            saved_status: "session saved".to_string(),
            tool_logs: Vec::new(),
            elapsed_ms: 320,
        },
    ]));

    let _ = app
        .run_runtime_turn("persist pending approval", &runtime, &tui)
        .expect("runtime turn should pause");
    assert!(app.has_pending_runtime_events());

    let mut reloaded = common::build_app_in(root);
    assert!(reloaded.has_pending_runtime_events());

    let resumed = reloaded
        .approve_and_continue(&runtime, &tui)
        .expect("reloaded app should resume");
    assert_eq!(
        reloaded.state_machine().snapshot().state,
        RuntimeState::Done
    );
    assert!(
        resumed
            .last()
            .expect("done frame should exist")
            .contains("approval resumed after reload")
    );
}
