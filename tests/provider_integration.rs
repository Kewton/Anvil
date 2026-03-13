mod common;

use anvil::agent::AgentEvent;
use anvil::provider::{
    OllamaChatMessage, OllamaProviderClient, ProviderClient, ProviderMessageRole,
    ProviderTurnError, ProviderTurnRequest, ProviderTurnResponse,
};
use anvil::tui::Tui;
use std::cell::RefCell;
use std::rc::Rc;

#[derive(Clone)]
struct RecordingProvider {
    seen_requests: Rc<RefCell<Vec<ProviderTurnRequest>>>,
    response: Result<ProviderTurnResponse, ProviderTurnError>,
}

impl ProviderClient for RecordingProvider {
    fn perform_turn(
        &self,
        request: &ProviderTurnRequest,
    ) -> Result<ProviderTurnResponse, ProviderTurnError> {
        self.seen_requests.borrow_mut().push(request.clone());
        self.response.clone()
    }
}

#[test]
fn live_turn_hands_session_messages_to_provider_and_renders_done() {
    let mut app = common::build_app();
    let tui = Tui::new();
    let seen_requests = Rc::new(RefCell::new(Vec::new()));
    let provider = RecordingProvider {
        seen_requests: seen_requests.clone(),
        response: Ok(ProviderTurnResponse::new(vec![
            AgentEvent::Thinking {
                status: "Thinking. model=local-default".to_string(),
                plan_items: vec!["inspect".to_string(), "answer".to_string()],
                active_index: Some(0),
                reasoning_summary: vec!["using provider-backed runtime".to_string()],
                elapsed_ms: 50,
            },
            AgentEvent::Done {
                status: "Done. session saved".to_string(),
                assistant_message: "provider-backed turn completed".to_string(),
                completion_summary: "Provider turn finished successfully.".to_string(),
                saved_status: "session saved".to_string(),
                tool_logs: Vec::new(),
                elapsed_ms: 120,
            },
        ])),
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
        response: Err(ProviderTurnError::Cancelled),
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
