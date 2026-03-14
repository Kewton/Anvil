mod common;

use anvil::agent::{AgentEvent, AgentRuntime};
use anvil::provider::{
    HttpTransport, OllamaChatMessage, OllamaProviderClient, ProviderClient, ProviderEvent,
    ProviderMessageRole, ProviderTurnError, ProviderTurnRequest,
};
use anvil::tui::Tui;
use std::cell::RefCell;
use std::fs;
use std::rc::Rc;

#[derive(Clone)]
struct RecordingProvider {
    seen_requests: Rc<RefCell<Vec<ProviderTurnRequest>>>,
    events: Vec<ProviderEvent>,
    error: Option<ProviderTurnError>,
}

#[derive(Clone)]
struct MockHttpTransport {
    seen_authority: Rc<RefCell<Vec<String>>>,
    seen_paths: Rc<RefCell<Vec<String>>>,
    seen_bodies: Rc<RefCell<Vec<Vec<u8>>>>,
    response: Vec<u8>,
}

impl HttpTransport for MockHttpTransport {
    fn post_json(
        &self,
        authority: &str,
        _host: &str,
        _port: u16,
        path: &str,
        body: &[u8],
    ) -> Result<Vec<u8>, ProviderTurnError> {
        self.seen_authority.borrow_mut().push(authority.to_string());
        self.seen_paths.borrow_mut().push(path.to_string());
        self.seen_bodies.borrow_mut().push(body.to_vec());
        Ok(self.response.clone())
    }
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
    assert_eq!(request.messages.len(), 4);
    assert_eq!(request.messages[0].role, ProviderMessageRole::System);
    assert_eq!(request.messages[1].role, ProviderMessageRole::User);
    assert_eq!(request.messages[2].role, ProviderMessageRole::Assistant);
    assert_eq!(request.messages[3].content, "current task");
    assert!(
        frames
            .last()
            .expect("done frame should exist")
            .contains("provider-backed turn completed")
    );
    assert!(request.messages[0].content.contains("ANVIL_TOOL"));
}

#[test]
fn live_turn_executes_structured_file_write_response_without_approval() {
    let root = common::unique_test_dir("structured_write");
    let mut config = common::build_config_in(root.clone());
    config.mode.approval_required = false;
    let provider_ctx =
        anvil::provider::ProviderRuntimeContext::bootstrap(&config).expect("provider bootstrap");
    let mut app = anvil::app::App::new(config, provider_ctx).expect("app should initialize");
    let tui = Tui::new();
    let provider = RecordingProvider {
        seen_requests: Rc::new(RefCell::new(Vec::new())),
        events: vec![ProviderEvent::Agent(AgentEvent::Done {
            status: "Done. session saved".to_string(),
            assistant_message: concat!(
                "```ANVIL_TOOL\n",
                "{\"id\":\"call_write_001\",\"tool\":\"file.write\",\"path\":\"./sandbox/test1_001/index.html\",\"content\":\"<html><body>invaders</body></html>\"}\n",
                "```\n",
                "```ANVIL_FINAL\n",
                "Created the browser game shell in ./sandbox/test1_001 and reviewed the generated code for structure and launchability.\n",
                "```\n"
            )
            .to_string(),
            completion_summary: "Provider turn finished successfully.".to_string(),
            saved_status: "session saved".to_string(),
            tool_logs: Vec::new(),
            elapsed_ms: 120,
        })],
        error: None,
    };

    let frames = app
        .run_live_turn("build the game", &provider, &tui)
        .expect("structured response should execute");

    let written = fs::read_to_string(root.join("sandbox/test1_001/index.html"))
        .expect("file.write should materialize output");
    assert!(written.contains("invaders"));
    assert!(
        frames
            .iter()
            .any(|frame| frame.contains("[T] tool  > file.write"))
    );
    assert!(
        frames
            .last()
            .expect("done frame should exist")
            .contains("Created the browser game shell")
    );
}

#[test]
fn live_turn_executes_complete_structured_response_from_token_stream() {
    let root = common::unique_test_dir("structured_stream_write");
    let mut config = common::build_config_in(root.clone());
    config.mode.approval_required = false;
    let provider_ctx =
        anvil::provider::ProviderRuntimeContext::bootstrap(&config).expect("provider bootstrap");
    let mut app = anvil::app::App::new(config, provider_ctx).expect("app should initialize");
    let tui = Tui::new();
    let provider = RecordingProvider {
        seen_requests: Rc::new(RefCell::new(Vec::new())),
        events: vec![ProviderEvent::TokenDelta(
            concat!(
                "```ANVIL_TOOL\n",
                "{\"id\":\"call_write_001\",\"tool\":\"file.write\",\"path\":\"./sandbox/test1_001/index.html\",\"content\":\"<html><body>streamed invaders</body></html>\"}\n",
                "```\n",
                "```ANVIL_FINAL\n",
                "Streamed game output was created and reviewed.\n",
                "```\n"
            )
            .to_string(),
        )],
        error: None,
    };

    let frames = app
        .run_live_turn("build from stream", &provider, &tui)
        .expect("structured token stream should execute");

    let written = fs::read_to_string(root.join("sandbox/test1_001/index.html"))
        .expect("streamed file.write should materialize output");
    assert!(written.contains("streamed invaders"));
    assert!(
        frames
            .last()
            .expect("done frame should exist")
            .contains("Streamed game output was created")
    );
}

#[test]
fn structured_response_parser_repairs_malformed_file_write_block() {
    let response = anvil::agent::BasicAgentLoop::parse_structured_response(concat!(
        "```ANVIL_TOOL\n",
        "{\"id\":\"call_write_001\",\"tool\":\"file.write\",\"path\":\"./sandbox/test1_002/Invader.html\",\"content\":\"<html>\n",
        "<body>\n",
        "<button aria-label=\"left\">LEFT</button>\n",
        "</body>\n",
        "</html>\"}\n",
        "```\n",
        "```ANVIL_FINAL\n",
        "Created the game and reviewed the output.\n",
        "```\n"
    ))
    .expect("parser should repair malformed file.write JSON");

    assert_eq!(response.tool_calls.len(), 1);
    match &response.tool_calls[0].input {
        anvil::tooling::ToolInput::FileWrite { path, content } => {
            assert_eq!(path, "./sandbox/test1_002/Invader.html");
            assert!(content.contains("aria-label=\"left\""));
            assert!(content.contains("<button"));
        }
        other => panic!("unexpected tool input: {other:?}"),
    }
}

#[test]
fn live_turn_executes_malformed_structured_file_write_response() {
    let root = common::unique_test_dir("malformed_structured_write");
    let mut config = common::build_config_in(root.clone());
    config.mode.approval_required = false;
    let provider_ctx =
        anvil::provider::ProviderRuntimeContext::bootstrap(&config).expect("provider bootstrap");
    let mut app = anvil::app::App::new(config, provider_ctx).expect("app should initialize");
    let tui = Tui::new();
    let provider = RecordingProvider {
        seen_requests: Rc::new(RefCell::new(Vec::new())),
        events: vec![ProviderEvent::Agent(AgentEvent::Done {
            status: "Done. session saved".to_string(),
            assistant_message: concat!(
                "```ANVIL_TOOL\n",
                "{\"id\":\"call_write_001\",\"tool\":\"file.write\",\"path\":\"./sandbox/test1_002/Invader.html\",\"content\":\"<html>\n",
                "<body>\n",
                "<button aria-label=\"left\">LEFT</button>\n",
                "</body>\n",
                "</html>\"}\n",
                "```\n",
                "```ANVIL_FINAL\n",
                "Created the browser game and reviewed the generated code.\n",
                "```\n"
            )
            .to_string(),
            completion_summary: "Provider turn finished successfully.".to_string(),
            saved_status: "session saved".to_string(),
            tool_logs: Vec::new(),
            elapsed_ms: 120,
        })],
        error: None,
    };

    let frames = app
        .run_live_turn("build the game", &provider, &tui)
        .expect("malformed structured response should execute");

    let written = fs::read_to_string(root.join("sandbox/test1_002/Invader.html"))
        .expect("file.write should materialize output");
    assert!(written.contains("aria-label=\"left\""));
    assert!(
        frames
            .iter()
            .any(|frame| frame.contains("file.write completed ./sandbox/test1_002/Invader.html"))
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

    let ollama_request =
        OllamaProviderClient::<anvil::provider::TcpHttpTransport>::build_chat_request(&request);

    assert_eq!(ollama_request.model, "local-default");
    assert!(ollama_request.stream);
    assert!(!ollama_request.think);
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

    assert!(frames.iter().any(|frame| frame.contains("drafting ")));
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

#[test]
fn ollama_provider_normalizes_ndjson_stream_to_provider_events() {
    let chunks = vec![
        r#"{"message":{"role":"assistant","content":"draft "},"done":false}"#.to_string(),
        r#"{"message":{"role":"assistant","content":"answer"},"done":false}"#.to_string(),
        r#"{"message":{"role":"assistant","content":""},"done":true}"#.to_string(),
    ];

    let events =
        OllamaProviderClient::<anvil::provider::TcpHttpTransport>::normalize_stream_chunks(&chunks)
            .expect("ollama stream should normalize");

    assert_eq!(
        events,
        vec![
            ProviderEvent::TokenDelta("draft ".to_string()),
            ProviderEvent::TokenDelta("answer".to_string()),
            ProviderEvent::Agent(AgentEvent::Done {
                status: "Done. session saved".to_string(),
                assistant_message: "draft answer".to_string(),
                completion_summary: "Provider turn finished successfully.".to_string(),
                saved_status: "session saved".to_string(),
                tool_logs: Vec::new(),
                elapsed_ms: 0,
            }),
        ]
    );
}

#[test]
fn ollama_provider_rejects_invalid_stream_chunk() {
    let chunks = vec!["not-json".to_string()];

    let err =
        OllamaProviderClient::<anvil::provider::TcpHttpTransport>::normalize_stream_chunks(&chunks)
            .expect_err("invalid ollama chunk should fail");

    assert!(err.to_string().contains("invalid ollama response"));
}

#[test]
fn ollama_provider_stream_turn_posts_chat_request_and_normalizes_chunked_response() {
    let seen_authority = Rc::new(RefCell::new(Vec::new()));
    let seen_paths = Rc::new(RefCell::new(Vec::new()));
    let seen_bodies = Rc::new(RefCell::new(Vec::new()));
    let body = concat!(
        "{\"message\":{\"role\":\"assistant\",\"content\":\"draft \"},\"done\":false}\n",
        "{\"message\":{\"role\":\"assistant\",\"content\":\"answer\"},\"done\":false}\n",
        "{\"message\":{\"role\":\"assistant\",\"content\":\"\"},\"done\":true}\n"
    );
    let response = format!(
        "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nContent-Type: application/x-ndjson\r\nConnection: close\r\n\r\n{:X}\r\n{}\r\n0\r\n\r\n",
        body.len(),
        body
    )
    .into_bytes();
    let provider = OllamaProviderClient::with_transport(
        "http://127.0.0.1:11434",
        MockHttpTransport {
            seen_authority: seen_authority.clone(),
            seen_paths: seen_paths.clone(),
            seen_bodies: seen_bodies.clone(),
            response,
        },
    );
    let request = ProviderTurnRequest::new(
        "local-default".to_string(),
        vec![anvil::provider::ProviderMessage::new(
            ProviderMessageRole::User,
            "inspect src/provider",
        )],
        true,
    );
    let mut events = Vec::new();

    provider
        .stream_turn(&request, &mut |event| events.push(event))
        .expect("provider should normalize chunked response");
    let bodies = seen_bodies.borrow();
    let body_text = String::from_utf8(bodies[0].clone()).expect("body should be utf8");
    assert_eq!(seen_authority.borrow().as_slice(), ["127.0.0.1:11434"]);
    assert_eq!(seen_paths.borrow().as_slice(), ["/api/chat"]);
    assert!(body_text.contains("\"model\":\"local-default\""));
    assert!(body_text.contains("\"content\":\"inspect src/provider\""));

    assert_eq!(
        events,
        vec![
            ProviderEvent::TokenDelta("draft ".to_string()),
            ProviderEvent::TokenDelta("answer".to_string()),
            ProviderEvent::Agent(AgentEvent::Done {
                status: "Done. session saved".to_string(),
                assistant_message: "draft answer".to_string(),
                completion_summary: "Provider turn finished successfully.".to_string(),
                saved_status: "session saved".to_string(),
                tool_logs: Vec::new(),
                elapsed_ms: 0,
            }),
        ]
    );
}

#[test]
fn ollama_provider_accepts_dechunked_body_even_if_chunked_header_remains() {
    let body = concat!(
        "{\"message\":{\"role\":\"assistant\",\"content\":\"draft \"},\"done\":false}\n",
        "{\"message\":{\"role\":\"assistant\",\"content\":\"answer\"},\"done\":false}\n",
        "{\"message\":{\"role\":\"assistant\",\"content\":\"\"},\"done\":true}\n"
    );
    let response = format!(
        "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nContent-Type: application/x-ndjson\r\nConnection: close\r\n\r\n{}",
        body
    )
    .into_bytes();
    let provider = OllamaProviderClient::with_transport(
        "http://127.0.0.1:11434",
        MockHttpTransport {
            seen_authority: Rc::new(RefCell::new(Vec::new())),
            seen_paths: Rc::new(RefCell::new(Vec::new())),
            seen_bodies: Rc::new(RefCell::new(Vec::new())),
            response,
        },
    );
    let request = ProviderTurnRequest::new(
        "local-default".to_string(),
        vec![anvil::provider::ProviderMessage::new(
            ProviderMessageRole::User,
            "inspect src/provider",
        )],
        true,
    );
    let mut events = Vec::new();

    provider
        .stream_turn(&request, &mut |event| events.push(event))
        .expect("provider should accept curl-style dechunked body");

    assert!(
        events
            .iter()
            .any(|event| matches!(event, ProviderEvent::TokenDelta(_)))
    );
}

#[test]
fn ollama_provider_surfaces_non_success_status_as_backend_error() {
    let provider = OllamaProviderClient::with_transport(
        "http://127.0.0.1:11434",
        MockHttpTransport {
            seen_authority: Rc::new(RefCell::new(Vec::new())),
            seen_paths: Rc::new(RefCell::new(Vec::new())),
            seen_bodies: Rc::new(RefCell::new(Vec::new())),
            response: b"HTTP/1.1 500 Internal Server Error\r\nContent-Length: 11\r\nConnection: close\r\n\r\nollama down".to_vec(),
        },
    );
    let request = ProviderTurnRequest::new(
        "local-default".to_string(),
        vec![anvil::provider::ProviderMessage::new(
            ProviderMessageRole::User,
            "inspect src/provider",
        )],
        true,
    );
    let err = provider
        .stream_turn(&request, &mut |_event| {})
        .expect_err("non-success status should fail");

    assert!(err.to_string().contains("ollama request failed"));
    assert!(err.to_string().contains("500"));
}
