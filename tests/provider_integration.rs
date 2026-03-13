mod common;

use anvil::agent::{AgentEvent, AgentRuntime};
use anvil::provider::{
    OllamaChatMessage, OllamaProviderClient, ProviderClient, ProviderEvent, ProviderMessageRole,
    ProviderTurnError, ProviderTurnRequest,
};
use anvil::tui::Tui;
use std::cell::RefCell;
use std::rc::Rc;

#[derive(Clone)]
struct RecordingProvider {
    seen_requests: Rc<RefCell<Vec<ProviderTurnRequest>>>,
    events: Vec<ProviderEvent>,
    error: Option<ProviderTurnError>,
}

impl ProviderClient for RecordingProvider {
    fn stream_turn(
        &self,
        request: &ProviderTurnRequest,
        emit: &mut dyn FnMut(ProviderEvent),
    ) -> Result<(), ProviderTurnError> {
        self.seen_requests.borrow_mut().push(request.clone());
        for event in self.events.clone() {
            emit(event);
        }
        self.error.clone().map_or(Ok(()), Err)
    }
}

#[test]
fn live_turn_hands_session_messages_to_provider_and_renders_done() {
    let mut app = common::build_app();
    let tui = Tui::new();
    let seen_requests = Rc::new(RefCell::new(Vec::new()));
    let provider = RecordingProvider {
        seen_requests: seen_requests.clone(),
        events: vec![
            ProviderEvent::Agent(AgentEvent::Thinking {
                status: "Thinking. model=local-default".to_string(),
                plan_items: vec!["inspect".to_string(), "answer".to_string()],
                active_index: Some(0),
                reasoning_summary: vec!["using provider-backed runtime".to_string()],
                elapsed_ms: 50,
            }),
            ProviderEvent::Agent(AgentEvent::Done {
                status: "Done. session saved".to_string(),
                assistant_message: "provider-backed turn completed".to_string(),
                completion_summary: "Provider turn finished successfully.".to_string(),
                saved_status: "session saved".to_string(),
                tool_logs: Vec::new(),
                elapsed_ms: 120,
            }),
        ],
        error: None,
    };

    app.record_user_input("msg_prev_user", "previous task")
        .expect("history should persist");
    app.record_assistant_output("msg_prev_assistant", "previous answer")
        .expect("history should persist");

    let frames = app
        .run_live_turn("current task", &provider, &tui)
        .expect("live turn should succeed");

    let requests = seen_requests.borrow();
    let request = requests.last().expect("provider request should exist");
    assert_eq!(request.model, "local-default");
    assert_eq!(request.messages.len(), 3);
    assert_eq!(request.messages[0].role, ProviderMessageRole::User);
    assert_eq!(request.messages[1].role, ProviderMessageRole::Assistant);
    assert_eq!(request.messages[2].content, "current task");
    assert!(
        frames
            .last()
            .expect("done frame should exist")
            .contains("provider-backed turn completed")
    );
}

#[test]
fn live_turn_maps_provider_cancellation_to_interrupted_state() {
    let mut app = common::build_app();
    let tui = Tui::new();
    let provider = RecordingProvider {
        seen_requests: Rc::new(RefCell::new(Vec::new())),
        events: Vec::new(),
        error: Some(ProviderTurnError::Cancelled),
    };

    let frames = app
        .run_live_turn("cancel this turn", &provider, &tui)
        .expect("cancelled provider turn should map to interrupted");

    assert_eq!(
        app.state_machine().snapshot().state,
        anvil::contracts::RuntimeState::Interrupted
    );
    assert!(
        frames
            .last()
            .expect("interrupted frame should exist")
            .contains("[A] anvil > interrupted")
    );
}

#[test]
fn ollama_provider_builds_chat_request_shape() {
    let request = ProviderTurnRequest::new(
        "local-default".to_string(),
        vec![anvil::provider::ProviderMessage::new(
            ProviderMessageRole::User,
            "inspect src/provider",
        )],
        true,
    );

    let ollama_request = OllamaProviderClient::build_chat_request(&request);

    assert_eq!(ollama_request.model, "local-default");
    assert!(ollama_request.stream);
    assert_eq!(
        ollama_request.messages,
        vec![OllamaChatMessage {
            role: "user".to_string(),
            content: "inspect src/provider".to_string(),
        }]
    );
}

#[test]
fn basic_agent_loop_applies_context_shaping_limit() {
    let mut app = common::build_app();
    app.record_user_input("msg_001", "u1").expect("persist");
    app.record_assistant_output("msg_002", "a1")
        .expect("persist");
    app.record_user_input("msg_003", "u2").expect("persist");
    app.record_assistant_output("msg_004", "a2")
        .expect("persist");
    app.record_user_input("msg_005", "u3").expect("persist");

    let request = anvil::agent::BasicAgentLoop::build_turn_request_with_limit(
        "local-default",
        app.session(),
        true,
        3,
    );

    assert_eq!(request.messages.len(), 3);
    assert_eq!(request.messages[0].content, "u2");
    assert_eq!(request.messages[2].content, "u3");
}

#[test]
fn basic_agent_loop_derives_context_budget_from_context_window() {
    let mut app = common::build_app();
    for index in 0..20 {
        app.record_user_input(format!("msg_u_{index:02}"), "1234567890".repeat(50))
            .expect("persist");
    }

    let small = anvil::agent::BasicAgentLoop::build_turn_request(
        "local-default",
        app.session(),
        true,
        1_000,
    );
    let large = anvil::agent::BasicAgentLoop::build_turn_request(
        "local-default",
        app.session(),
        true,
        200_000,
    );

    assert!(small.messages.len() < large.messages.len());
}

#[test]
fn live_turn_records_provider_backend_error_detail_in_session() {
    let mut app = common::build_app();
    let tui = Tui::new();
    let provider = RecordingProvider {
        seen_requests: Rc::new(RefCell::new(Vec::new())),
        events: Vec::new(),
        error: Some(ProviderTurnError::Backend("socket closed".to_string())),
    };

    let frames = app
        .run_live_turn("trigger backend error", &provider, &tui)
        .expect("backend error should map to error state");

    assert_eq!(
        app.state_machine().snapshot().state,
        anvil::contracts::RuntimeState::Error
    );
    assert!(
        frames
            .last()
            .expect("error frame")
            .contains("[A] anvil > error")
    );
    assert!(
        app.session()
            .provider_errors
            .last()
            .expect("provider detail should exist")
            .message
            .contains("socket closed")
    );
}

#[test]
fn live_turn_surfaces_token_delta_progress() {
    let mut app = common::build_app();
    let tui = Tui::new();
    let provider = RecordingProvider {
        seen_requests: Rc::new(RefCell::new(Vec::new())),
        events: vec![
            ProviderEvent::TokenDelta("drafting ".to_string()),
            ProviderEvent::TokenDelta("response".to_string()),
            ProviderEvent::Agent(AgentEvent::Done {
                status: "Done. session saved".to_string(),
                assistant_message: "stream finished".to_string(),
                completion_summary: "Streaming completed.".to_string(),
                saved_status: "session saved".to_string(),
                tool_logs: Vec::new(),
                elapsed_ms: 90,
            }),
        ],
        error: None,
    };

    let frames = app
        .run_live_turn("stream this", &provider, &tui)
        .expect("live turn should succeed");

    assert!(
        frames
            .iter()
            .any(|frame| frame.contains("drafting response"))
    );
}

#[test]
fn live_turn_can_pause_for_provider_approval_and_resume() {
    let mut app = common::build_app();
    let tui = Tui::new();
    let provider = RecordingProvider {
        seen_requests: Rc::new(RefCell::new(Vec::new())),
        events: vec![
            ProviderEvent::Agent(AgentEvent::Thinking {
                status: "Thinking. model=local-default".to_string(),
                plan_items: vec!["prepare write".to_string()],
                active_index: Some(0),
                reasoning_summary: vec!["approval needed".to_string()],
                elapsed_ms: 40,
            }),
            ProviderEvent::Agent(AgentEvent::ApprovalRequested {
                status: "Awaiting approval for 1 tool call".to_string(),
                tool_name: "Write".to_string(),
                summary: "Update src/provider/mod.rs".to_string(),
                risk: "Confirm".to_string(),
                tool_call_id: "call_live_001".to_string(),
                elapsed_ms: 70,
            }),
            ProviderEvent::Agent(AgentEvent::Working {
                status: "Working on approved tool execution".to_string(),
                plan_items: vec!["prepare write".to_string()],
                active_index: Some(0),
                tool_logs: vec![(
                    "Write".to_string(),
                    "update".to_string(),
                    "src/provider/mod.rs".to_string(),
                )],
                elapsed_ms: 90,
            }),
            ProviderEvent::Agent(AgentEvent::Done {
                status: "Done. session saved".to_string(),
                assistant_message: "live approval resumed".to_string(),
                completion_summary: "Approval flow completed.".to_string(),
                saved_status: "session saved".to_string(),
                tool_logs: Vec::new(),
                elapsed_ms: 120,
            }),
        ],
        error: None,
    };

    let frames = app
        .run_live_turn("approve this", &provider, &tui)
        .expect("provider-backed approval should pause");

    assert!(app.has_pending_runtime_events());
    assert!(
        frames
            .iter()
            .any(|frame| frame.contains("[A] anvil > approval"))
    );

    let resumed = app
        .approve_and_continue(&AgentRuntime::new(), &tui)
        .expect("approval should resume");

    assert!(
        resumed
            .last()
            .expect("done frame should exist")
            .contains("live approval resumed")
    );
}
