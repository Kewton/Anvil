mod common;

use anvil::agent::{AgentEvent, AgentRuntime};
use anvil::provider::{
    HttpResponse, HttpTransport, OllamaChatMessage, OllamaProviderClient, ProviderClient,
    ProviderEvent, ProviderMessageRole, ProviderTurnError, ProviderTurnRequest,
    resolve_ollama_model_alias,
};
use anvil::tui::Tui;
use std::cell::RefCell;
use std::fs;
use std::rc::Rc;

type HeaderLog = Rc<RefCell<Vec<Vec<(String, String)>>>>;

/// Mock provider that supports multi-turn agentic loops.
///
/// On the first `stream_turn` call it emits `events`.  On subsequent calls
/// (triggered by the agentic follow-up) it emits `followup_events`, which
/// defaults to a simple Done with empty text (no tool calls) so the loop
/// terminates.
#[derive(Clone)]
struct RecordingProvider {
    seen_requests: Rc<RefCell<Vec<ProviderTurnRequest>>>,
    events: Vec<ProviderEvent>,
    followup_events: Vec<ProviderEvent>,
    error: Option<ProviderTurnError>,
}

#[derive(Clone)]
struct MockHttpTransport {
    seen_urls: Rc<RefCell<Vec<String>>>,
    seen_bodies: Rc<RefCell<Vec<Vec<u8>>>>,
    seen_headers: HeaderLog,
    response: HttpResponse,
    get_response: Option<HttpResponse>,
}

impl HttpTransport for MockHttpTransport {
    fn post_json_with_headers(
        &self,
        url: &str,
        body: &[u8],
        headers: &[(&str, &str)],
    ) -> Result<HttpResponse, ProviderTurnError> {
        self.seen_urls.borrow_mut().push(url.to_string());
        self.seen_bodies.borrow_mut().push(body.to_vec());
        self.seen_headers.borrow_mut().push(
            headers
                .iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                .collect(),
        );
        Ok(self.response.clone())
    }

    fn get_with_headers(
        &self,
        url: &str,
        headers: &[(&str, &str)],
    ) -> Result<HttpResponse, ProviderTurnError> {
        self.seen_urls.borrow_mut().push(url.to_string());
        self.seen_headers.borrow_mut().push(
            headers
                .iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                .collect(),
        );
        Ok(self
            .get_response
            .clone()
            .unwrap_or_else(|| self.response.clone()))
    }
}

impl ProviderClient for RecordingProvider {
    fn stream_turn(
        &self,
        request: &ProviderTurnRequest,
        emit: &mut dyn FnMut(ProviderEvent),
    ) -> Result<(), ProviderTurnError> {
        let call_index = self.seen_requests.borrow().len();
        self.seen_requests.borrow_mut().push(request.clone());

        // For agentic follow-up calls: if followup_events is configured, use
        // those.  Otherwise emit a plain-text Done (no tool calls) so the
        // agentic loop terminates.
        if call_index > 0 {
            if !self.followup_events.is_empty() {
                for event in self.followup_events.clone() {
                    emit(event);
                }
            } else {
                emit(ProviderEvent::Agent(AgentEvent::Done {
                    status: "Done. session saved".to_string(),
                    assistant_message: "Agentic follow-up completed.".to_string(),
                    completion_summary: "Follow-up turn finished.".to_string(),
                    saved_status: "session saved".to_string(),
                    tool_logs: Vec::new(),
                    elapsed_ms: 0,
                }));
            }
            return self.error.clone().map_or(Ok(()), Err);
        }

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
        followup_events: Vec::new(),
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
    // Assistant message is excluded from frame rendering (streamed to stderr,
    // Issue #1). The Done frame shows result/completion_summary instead.
    assert!(
        frames
            .last()
            .expect("done frame should exist")
            .contains("[A] anvil > result"),
        "done frame should contain result section"
    );
    assert!(
        app.session()
            .messages
            .iter()
            .any(|m| m.content == "provider-backed turn completed"),
        "assistant message should be in session history"
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
    let mut app = anvil::app::App::new(
        config,
        provider_ctx,
        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
    )
    .expect("app should initialize");
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
        followup_events: Vec::new(),
        error: None,
    };

    let frames = app
        .run_live_turn("build the game", &provider, &tui)
        .expect("structured response should execute");

    let written = fs::read_to_string(root.join("sandbox/test1_001/index.html"))
        .expect("file.write should materialize output");
    assert!(written.contains("invaders"));
    // Intermediate Thinking/Working frames are no longer emitted to avoid
    // duplicate output (Issue #1).  Only the final Done frame is returned,
    // which contains tool_logs and completion_summary.
    assert!(
        frames
            .iter()
            .any(|frame| frame.contains("[T] tool  > file.write")),
        "done frame should contain tool log for file.write"
    );
    assert!(
        frames
            .last()
            .expect("done frame should exist")
            .contains("Executed"),
        "done frame should contain execution summary"
    );
}

#[test]
fn ollama_model_alias_resolution_prefers_unique_installed_prefix_match() {
    let resolved = resolve_ollama_model_alias(
        "qwen3.5:35b",
        &[
            "qwen3.5:35b-a3b-q8_0".to_string(),
            "qwen3.5:27b-q8_0".to_string(),
        ],
    );
    assert_eq!(resolved, "qwen3.5:35b-a3b-q8_0");
}

#[test]
fn live_turn_executes_complete_structured_response_from_token_stream() {
    let root = common::unique_test_dir("structured_stream_write");
    let mut config = common::build_config_in(root.clone());
    config.mode.approval_required = false;
    let provider_ctx =
        anvil::provider::ProviderRuntimeContext::bootstrap(&config).expect("provider bootstrap");
    let mut app = anvil::app::App::new(
        config,
        provider_ctx,
        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
    )
    .expect("app should initialize");
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
        followup_events: Vec::new(),
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
            .contains("Executed")
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
    let mut app = anvil::app::App::new(
        config,
        provider_ctx,
        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
    )
    .expect("app should initialize");
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
        followup_events: Vec::new(),
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
        followup_events: Vec::new(),
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
            images: None,
        }]
    );
}

#[test]
fn openai_compatible_provider_maps_response_into_done_event() {
    let request = ProviderTurnRequest::new(
        "local-openai".to_string(),
        vec![anvil::provider::ProviderMessage::new(
            ProviderMessageRole::User,
            "inspect src/provider",
        )],
        true,
    );

    let seen_urls = Rc::new(RefCell::new(Vec::new()));
    let transport = MockHttpTransport {
        seen_urls: seen_urls.clone(),
        seen_bodies: Rc::new(RefCell::new(Vec::new())),
        seen_headers: Rc::new(RefCell::new(Vec::new())),
        response: HttpResponse {
            status_code: 200,
            body: br#"{"choices":[{"message":{"role":"assistant","content":"openai-compatible answer"}}]}"#
                .to_vec(),
        },
        get_response: None,
    };
    let client = anvil::provider::openai::OpenAiCompatibleProviderClient::with_transport(
        "http://localhost:1234",
        transport,
    );

    let mut events = Vec::new();
    client
        .stream_turn(&request, &mut |event| events.push(event))
        .expect("openai-compatible turn should succeed");

    assert_eq!(
        seen_urls.borrow()[0],
        "http://localhost:1234/v1/chat/completions"
    );
    assert!(events.iter().any(
        |event| matches!(event, ProviderEvent::TokenDelta(delta) if delta == "openai-compatible answer")
    ));
    assert!(events.iter().any(
        |event| matches!(event, ProviderEvent::Agent(AgentEvent::Done { assistant_message, .. }) if assistant_message == "openai-compatible answer")
    ));
}

#[test]
fn openai_compatible_provider_parses_sse_streams() {
    let request = ProviderTurnRequest::new(
        "local-openai".to_string(),
        vec![anvil::provider::ProviderMessage::new(
            ProviderMessageRole::User,
            "inspect src/provider",
        )],
        true,
    );

    let transport = MockHttpTransport {
        seen_urls: Rc::new(RefCell::new(Vec::new())),
        seen_bodies: Rc::new(RefCell::new(Vec::new())),
        seen_headers: Rc::new(RefCell::new(Vec::new())),
        response: HttpResponse {
            status_code: 200,
            body: concat!(
                "data: {\"choices\":[{\"delta\":{\"content\":\"draft \"},\"finish_reason\":null}]}\n",
                "data: {\"choices\":[{\"delta\":{\"content\":\"answer\"},\"finish_reason\":\"stop\"}]}\n",
                "data: [DONE]\n"
            )
            .as_bytes()
            .to_vec(),
        },
        get_response: None,
    };
    let client = anvil::provider::openai::OpenAiCompatibleProviderClient::with_transport(
        "http://localhost:1234",
        transport,
    );

    let mut events = Vec::new();
    client
        .stream_turn(&request, &mut |event| events.push(event))
        .expect("openai-compatible stream should succeed");

    assert!(
        events
            .iter()
            .any(|event| matches!(event, ProviderEvent::TokenDelta(delta) if delta == "draft "))
    );
    assert!(events.iter().any(
        |event| matches!(event, ProviderEvent::Agent(AgentEvent::Done { assistant_message, .. }) if assistant_message == "draft answer")
    ));
}

#[test]
fn openai_compatible_provider_normalizes_error_message() {
    let request = ProviderTurnRequest::new(
        "local-openai".to_string(),
        vec![anvil::provider::ProviderMessage::new(
            ProviderMessageRole::User,
            "inspect src/provider",
        )],
        false,
    );

    let transport = MockHttpTransport {
        seen_urls: Rc::new(RefCell::new(Vec::new())),
        seen_bodies: Rc::new(RefCell::new(Vec::new())),
        seen_headers: Rc::new(RefCell::new(Vec::new())),
        response: HttpResponse {
            status_code: 401,
            body: br#"{"error":{"message":"invalid api key"}}"#.to_vec(),
        },
        get_response: None,
    };
    let client = anvil::provider::openai::OpenAiCompatibleProviderClient::with_transport(
        "http://localhost:1234",
        transport,
    );

    let err = client
        .stream_turn(&request, &mut |_| {})
        .expect_err("error body should be normalized");

    assert!(err.to_string().contains("invalid api key"));
}

#[test]
fn openai_compatible_provider_forwards_authorization_header() {
    let request = ProviderTurnRequest::new(
        "local-openai".to_string(),
        vec![anvil::provider::ProviderMessage::new(
            ProviderMessageRole::User,
            "inspect src/provider",
        )],
        false,
    );
    let seen_headers = Rc::new(RefCell::new(Vec::new()));
    let transport = MockHttpTransport {
        seen_urls: Rc::new(RefCell::new(Vec::new())),
        seen_bodies: Rc::new(RefCell::new(Vec::new())),
        seen_headers: seen_headers.clone(),
        response: HttpResponse {
            status_code: 200,
            body: br#"{"choices":[{"message":{"role":"assistant","content":"ok"}}]}"#.to_vec(),
        },
        get_response: None,
    };
    let client = anvil::provider::openai::OpenAiCompatibleProviderClient::with_transport(
        "http://localhost:1234",
        transport,
    )
    .with_api_key("Bearer test-key");

    client
        .stream_turn(&request, &mut |_event| {})
        .expect("authorized request should succeed");

    let recorded = seen_headers.borrow();
    assert!(
        recorded[0]
            .iter()
            .any(|(name, value)| name == "Authorization" && value == "Bearer test-key")
    );
}

#[test]
fn live_turn_executes_structured_response_from_openai_compatible_provider() {
    let root = common::unique_test_dir("openai_structured_write");
    let mut config = common::build_config_in(root.clone());
    config.mode.approval_required = false;
    config.runtime.provider = "openai".to_string();
    config.runtime.provider_url = "http://localhost:1234".to_string();
    let provider_ctx =
        anvil::provider::ProviderRuntimeContext::bootstrap(&config).expect("provider bootstrap");
    let mut app = anvil::app::App::new(
        config,
        provider_ctx,
        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
    )
    .expect("app should initialize");
    let tui = Tui::new();

    let transport = MockHttpTransport {
        seen_urls: Rc::new(RefCell::new(Vec::new())),
        seen_bodies: Rc::new(RefCell::new(Vec::new())),
        seen_headers: Rc::new(RefCell::new(Vec::new())),
        response: HttpResponse {
            status_code: 200,
            body: br#"{"choices":[{"message":{"role":"assistant","content":"```ANVIL_TOOL\n{\"id\":\"call_write_001\",\"tool\":\"file.write\",\"path\":\"./sandbox/openai/index.html\",\"content\":\"<html><body>openai parity</body></html>\"}\n```\n```ANVIL_FINAL\nOpenAI-compatible backend created the file and reviewed the output.\n```"}}]}"#.to_vec(),
        },
        get_response: None,
    };
    let client = anvil::provider::openai::OpenAiCompatibleProviderClient::with_transport(
        "http://localhost:1234",
        transport,
    );

    let frames = app
        .run_live_turn("build via openai-compatible provider", &client, &tui)
        .expect("structured response should execute");

    let written =
        fs::read_to_string(root.join("sandbox/openai/index.html")).expect("file should be written");
    assert!(written.contains("openai parity"));
    assert!(
        frames
            .iter()
            .any(|frame| frame.contains("file.write completed ./sandbox/openai/index.html"))
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

    let system_prompt = anvil::agent::tool_protocol_system_prompt(&[]);
    let request = anvil::agent::BasicAgentLoop::build_turn_request_with_limit(
        "local-default",
        app.session(),
        true,
        3,
        &system_prompt,
    );

    // System prompt is now injected as messages[0]
    assert_eq!(request.messages.len(), 4);
    assert_eq!(
        request.messages[0].role,
        anvil::provider::ProviderMessageRole::System
    );
    assert_eq!(request.messages[1].content, "u2");
    assert_eq!(request.messages[3].content, "u3");
}

#[test]
fn basic_agent_loop_derives_context_budget_from_context_window() {
    let mut app = common::build_app();
    for index in 0..20 {
        app.record_user_input(format!("msg_u_{index:02}"), "1234567890".repeat(50))
            .expect("persist");
    }

    let system_prompt = anvil::agent::tool_protocol_system_prompt(&[]);
    let small = anvil::agent::BasicAgentLoop::build_turn_request(
        "local-default",
        app.session(),
        true,
        8_000,
        &system_prompt,
    );
    let large = anvil::agent::BasicAgentLoop::build_turn_request(
        "local-default",
        app.session(),
        true,
        200_000,
        &system_prompt,
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
        followup_events: Vec::new(),
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
        followup_events: Vec::new(),
        error: None,
    };

    let frames = app
        .run_live_turn("stream this", &provider, &tui)
        .expect("live turn should succeed");

    // Token deltas are streamed to stderr in real-time. Assistant messages
    // are excluded from frame rendering to avoid duplicate output (Issue #1).
    assert!(
        frames
            .last()
            .expect("done frame should exist")
            .contains("[A] anvil > result"),
        "done frame should contain result section"
    );
    assert!(
        app.session()
            .messages
            .iter()
            .any(|m| m.content == "stream finished"),
        "assistant message should be in session history"
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
        followup_events: Vec::new(),
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

    // Assistant message is excluded from frame rendering (streamed to stderr,
    // Issue #1). The Done frame shows result/completion_summary instead.
    assert!(
        resumed
            .last()
            .expect("done frame should exist")
            .contains("[A] anvil > result"),
        "done frame should contain result section"
    );
    assert!(
        app.session()
            .messages
            .iter()
            .any(|m| m.content == "live approval resumed"),
        "assistant message should be in session history"
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
fn ollama_provider_stream_turn_posts_chat_request_and_normalizes_response() {
    let seen_urls = Rc::new(RefCell::new(Vec::new()));
    let seen_bodies = Rc::new(RefCell::new(Vec::new()));
    let body = concat!(
        "{\"message\":{\"role\":\"assistant\",\"content\":\"draft \"},\"done\":false}\n",
        "{\"message\":{\"role\":\"assistant\",\"content\":\"answer\"},\"done\":false}\n",
        "{\"message\":{\"role\":\"assistant\",\"content\":\"\"},\"done\":true}\n"
    );
    let provider = OllamaProviderClient::with_transport(
        "http://127.0.0.1:11434",
        MockHttpTransport {
            seen_urls: seen_urls.clone(),
            seen_bodies: seen_bodies.clone(),
            seen_headers: Rc::new(RefCell::new(Vec::new())),
            response: HttpResponse {
                status_code: 200,
                body: body.as_bytes().to_vec(),
            },
            get_response: None,
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
        .expect("provider should normalize response");
    let bodies = seen_bodies.borrow();
    let body_text = String::from_utf8(bodies[0].clone()).expect("body should be utf8");
    assert_eq!(
        seen_urls.borrow().as_slice(),
        ["http://127.0.0.1:11434/api/chat"]
    );
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
fn ollama_provider_surfaces_non_success_status_as_backend_error() {
    let provider = OllamaProviderClient::with_transport(
        "http://127.0.0.1:11434",
        MockHttpTransport {
            seen_urls: Rc::new(RefCell::new(Vec::new())),
            seen_bodies: Rc::new(RefCell::new(Vec::new())),
            seen_headers: Rc::new(RefCell::new(Vec::new())),
            response: HttpResponse {
                status_code: 500,
                body: b"ollama down".to_vec(),
            },
            get_response: None,
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

    assert!(
        matches!(
            err,
            ProviderTurnError::ServerError {
                status_code: 500,
                ..
            }
        ),
        "500 status should be classified as ServerError, got: {err:?}"
    );
}

// --- HttpTransport GET and header validation tests ---

#[test]
fn mock_transport_get_returns_configured_response() {
    let transport = MockHttpTransport {
        seen_urls: Rc::new(RefCell::new(Vec::new())),
        seen_bodies: Rc::new(RefCell::new(Vec::new())),
        seen_headers: Rc::new(RefCell::new(Vec::new())),
        response: HttpResponse {
            status_code: 200,
            body: b"post response".to_vec(),
        },
        get_response: Some(HttpResponse {
            status_code: 200,
            body: b"get response".to_vec(),
        }),
    };

    let result = transport.get("http://localhost/api/tags").unwrap();
    assert_eq!(result.status_code, 200);
    assert_eq!(result.body, b"get response");

    // Verify the URL was recorded
    let urls = transport.seen_urls.borrow();
    assert_eq!(urls.len(), 1);
    assert_eq!(urls[0], "http://localhost/api/tags");
}

#[test]
fn mock_transport_get_with_headers_records_headers() {
    let seen_headers: HeaderLog = Rc::new(RefCell::new(Vec::new()));
    let transport = MockHttpTransport {
        seen_urls: Rc::new(RefCell::new(Vec::new())),
        seen_bodies: Rc::new(RefCell::new(Vec::new())),
        seen_headers: seen_headers.clone(),
        response: HttpResponse {
            status_code: 200,
            body: b"ok".to_vec(),
        },
        get_response: None,
    };

    let result = transport
        .get_with_headers(
            "http://localhost/v1/models",
            &[("Authorization", "Bearer sk-test")],
        )
        .unwrap();
    assert_eq!(result.status_code, 200);

    let headers = seen_headers.borrow();
    assert_eq!(headers.len(), 1);
    assert_eq!(headers[0].len(), 1);
    assert_eq!(headers[0][0].0, "Authorization");
    assert_eq!(headers[0][0].1, "Bearer sk-test");
}

#[test]
fn mock_transport_get_falls_back_to_post_response_when_get_response_is_none() {
    let transport = MockHttpTransport {
        seen_urls: Rc::new(RefCell::new(Vec::new())),
        seen_bodies: Rc::new(RefCell::new(Vec::new())),
        seen_headers: Rc::new(RefCell::new(Vec::new())),
        response: HttpResponse {
            status_code: 200,
            body: b"fallback response".to_vec(),
        },
        get_response: None,
    };

    let result = transport.get("http://localhost/api/tags").unwrap();
    assert_eq!(result.body, b"fallback response");
}

#[test]
fn post_json_default_delegates_to_post_json_with_headers() {
    let seen_headers: HeaderLog = Rc::new(RefCell::new(Vec::new()));
    let transport = MockHttpTransport {
        seen_urls: Rc::new(RefCell::new(Vec::new())),
        seen_bodies: Rc::new(RefCell::new(Vec::new())),
        seen_headers: seen_headers.clone(),
        response: HttpResponse {
            status_code: 200,
            body: b"ok".to_vec(),
        },
        get_response: None,
    };

    // post_json should delegate to post_json_with_headers with empty headers
    let result = transport
        .post_json("http://localhost/api/chat", b"{}")
        .unwrap();
    assert_eq!(result.status_code, 200);

    let headers = seen_headers.borrow();
    assert_eq!(headers.len(), 1);
    // Default impl passes empty headers slice
    assert!(headers[0].is_empty());
}

#[test]
fn get_default_delegates_to_get_with_headers() {
    let seen_headers: HeaderLog = Rc::new(RefCell::new(Vec::new()));
    let transport = MockHttpTransport {
        seen_urls: Rc::new(RefCell::new(Vec::new())),
        seen_bodies: Rc::new(RefCell::new(Vec::new())),
        seen_headers: seen_headers.clone(),
        response: HttpResponse {
            status_code: 200,
            body: b"ok".to_vec(),
        },
        get_response: None,
    };

    // get() should delegate to get_with_headers with empty headers
    let result = transport.get("http://localhost/api/tags").unwrap();
    assert_eq!(result.status_code, 200);

    let headers = seen_headers.borrow();
    assert_eq!(headers.len(), 1);
    assert!(headers[0].is_empty());
}

// --- Agentic loop unit tests ---

#[test]
fn agentic_loop_multi_iteration_tool_calls_then_final_answer() {
    let root = common::unique_test_dir("agentic_multi_iter");
    let mut config = common::build_config_in(root.clone());
    config.mode.approval_required = false;
    let provider_ctx =
        anvil::provider::ProviderRuntimeContext::bootstrap(&config).expect("provider bootstrap");
    let mut app = anvil::app::App::new(
        config,
        provider_ctx,
        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
    )
    .expect("app should initialize");
    let tui = Tui::new();

    // Create files that the tool calls will read
    std::fs::create_dir_all(root.join("src")).expect("create src dir");
    std::fs::write(root.join("Cargo.toml"), "[package]\nname = \"test\"")
        .expect("write Cargo.toml");
    std::fs::write(root.join("src/lib.rs"), "// lib").expect("write lib.rs");

    let seen_requests = Rc::new(RefCell::new(Vec::new()));

    struct MultiIterProvider {
        seen_requests: Rc<RefCell<Vec<ProviderTurnRequest>>>,
    }

    impl ProviderClient for MultiIterProvider {
        fn stream_turn(
            &self,
            request: &ProviderTurnRequest,
            emit: &mut dyn FnMut(ProviderEvent),
        ) -> Result<(), ProviderTurnError> {
            let call_index = self.seen_requests.borrow().len();
            self.seen_requests.borrow_mut().push(request.clone());

            match call_index {
                0 => {
                    emit(ProviderEvent::Agent(AgentEvent::Done {
                        status: "Done. session saved".to_string(),
                        assistant_message: concat!(
                            "```ANVIL_TOOL\n",
                            "{\"id\":\"call_001\",\"tool\":\"file.read\",\"path\":\"./Cargo.toml\"}\n",
                            "```\n",
                            "```ANVIL_FINAL\n",
                            "Reading Cargo.toml first.\n",
                            "```\n"
                        )
                        .to_string(),
                        completion_summary: "turn 1".to_string(),
                        saved_status: "session saved".to_string(),
                        tool_logs: Vec::new(),
                        elapsed_ms: 0,
                    }));
                }
                1 => {
                    emit(ProviderEvent::TokenDelta(
                        concat!(
                            "```ANVIL_TOOL\n",
                            "{\"id\":\"call_002\",\"tool\":\"file.read\",\"path\":\"./src\"}\n",
                            "```\n",
                            "```ANVIL_FINAL\n",
                            "Now reading src directory.\n",
                            "```\n"
                        )
                        .to_string(),
                    ));
                }
                _ => {
                    emit(ProviderEvent::TokenDelta(
                        "All done! Found the files.".to_string(),
                    ));
                }
            }
            Ok(())
        }
    }

    let provider = MultiIterProvider {
        seen_requests: seen_requests.clone(),
    };

    let frames = app
        .run_live_turn("analyze the project", &provider, &tui)
        .expect("multi-iteration agentic loop should succeed");

    let requests = seen_requests.borrow();
    // Should have made 3 calls: initial + 2 follow-ups
    assert_eq!(
        requests.len(),
        3,
        "expected 3 provider calls for multi-iteration loop"
    );

    // Final frame should show Done state
    assert!(
        frames
            .last()
            .expect("done frame should exist")
            .contains("Executed"),
        "final frame should contain execution summary"
    );
}

#[test]
fn agentic_loop_tool_result_payload_included_in_session_messages() {
    let root = common::unique_test_dir("agentic_tool_result");
    let mut config = common::build_config_in(root.clone());
    config.mode.approval_required = false;
    let provider_ctx =
        anvil::provider::ProviderRuntimeContext::bootstrap(&config).expect("provider bootstrap");
    let mut app = anvil::app::App::new(
        config,
        provider_ctx,
        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
    )
    .expect("app should initialize");
    let tui = Tui::new();

    // Write a test file so file.read has something to return
    std::fs::create_dir_all(root.join("testdir")).expect("create testdir");
    std::fs::write(root.join("testdir/hello.txt"), "Hello World content").expect("write test file");

    let provider = RecordingProvider {
        seen_requests: Rc::new(RefCell::new(Vec::new())),
        events: vec![ProviderEvent::Agent(AgentEvent::Done {
            status: "Done. session saved".to_string(),
            assistant_message: concat!(
                "```ANVIL_TOOL\n",
                "{\"id\":\"call_001\",\"tool\":\"file.read\",\"path\":\"./testdir/hello.txt\"}\n",
                "```\n",
                "```ANVIL_FINAL\n",
                "Read the file.\n",
                "```\n"
            )
            .to_string(),
            completion_summary: "turn finished".to_string(),
            saved_status: "session saved".to_string(),
            tool_logs: Vec::new(),
            elapsed_ms: 0,
        })],
        followup_events: Vec::new(),
        error: None,
    };

    app.run_live_turn("read hello.txt", &provider, &tui)
        .expect("tool result test should succeed");

    // Check that session messages contain the tool result with actual file content
    let tool_messages: Vec<_> = app
        .session()
        .messages
        .iter()
        .filter(|m| m.role == anvil::session::MessageRole::Tool)
        .collect();

    assert!(
        !tool_messages.is_empty(),
        "should have at least one tool result message"
    );
    assert!(
        tool_messages[0]
            .content
            .contains("[tool result: file.read]"),
        "tool result should include tool name format"
    );
    assert!(
        tool_messages[0].content.contains("Hello World content"),
        "tool result should include actual file content"
    );
}

#[test]
fn agentic_loop_respects_max_iteration_limit() {
    let root = common::unique_test_dir("agentic_max_iter");
    let mut config = common::build_config_in(root.clone());
    config.mode.approval_required = false;
    let provider_ctx =
        anvil::provider::ProviderRuntimeContext::bootstrap(&config).expect("provider bootstrap");
    let mut app = anvil::app::App::new(
        config,
        provider_ctx,
        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
    )
    .expect("app should initialize");
    let tui = Tui::new();

    // Create file so file.read succeeds
    std::fs::create_dir_all(&root).expect("create root");
    std::fs::write(root.join("Cargo.toml"), "[package]\nname = \"test\"")
        .expect("write Cargo.toml");

    struct AlwaysToolCallProvider {
        call_count: Rc<RefCell<usize>>,
    }

    impl ProviderClient for AlwaysToolCallProvider {
        fn stream_turn(
            &self,
            _request: &ProviderTurnRequest,
            emit: &mut dyn FnMut(ProviderEvent),
        ) -> Result<(), ProviderTurnError> {
            let count = {
                let mut c = self.call_count.borrow_mut();
                *c += 1;
                *c
            };

            // Always return a file.read tool call
            emit(ProviderEvent::Agent(AgentEvent::Done {
                status: "Done. session saved".to_string(),
                assistant_message: format!(
                    concat!(
                        "```ANVIL_TOOL\n",
                        "{{\"id\":\"call_{count:03}\",\"tool\":\"file.read\",\"path\":\"./Cargo.toml\"}}\n",
                        "```\n",
                        "```ANVIL_FINAL\n",
                        "Iteration {count}.\n",
                        "```\n"
                    ),
                    count = count
                ),
                completion_summary: format!("iteration {count}"),
                saved_status: "session saved".to_string(),
                tool_logs: Vec::new(),
                elapsed_ms: 0,
            }));
            Ok(())
        }
    }

    let call_count = Rc::new(RefCell::new(0));
    let provider = AlwaysToolCallProvider {
        call_count: call_count.clone(),
    };

    let _frames = app
        .run_live_turn("infinite loop test", &provider, &tui)
        .expect("max iteration should complete without error");

    // MAX_AGENT_ITERATIONS is 10, plus the initial call = 11 total
    let total_calls = *call_count.borrow();
    assert!(
        total_calls <= 11,
        "should not exceed MAX_AGENT_ITERATIONS (10) + 1 initial call, got {total_calls}"
    );
}

#[test]
fn agentic_loop_error_during_followup_propagates() {
    let root = common::unique_test_dir("agentic_followup_error");
    let mut config = common::build_config_in(root.clone());
    config.mode.approval_required = false;
    let provider_ctx =
        anvil::provider::ProviderRuntimeContext::bootstrap(&config).expect("provider bootstrap");
    let mut app = anvil::app::App::new(
        config,
        provider_ctx,
        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
    )
    .expect("app should initialize");
    let tui = Tui::new();

    // Create file so file.read succeeds on the first iteration
    std::fs::create_dir_all(&root).expect("create root");
    std::fs::write(root.join("Cargo.toml"), "[package]\nname = \"test\"")
        .expect("write Cargo.toml");

    struct ErrorOnFollowupProvider {
        call_count: Rc<RefCell<usize>>,
    }

    impl ProviderClient for ErrorOnFollowupProvider {
        fn stream_turn(
            &self,
            _request: &ProviderTurnRequest,
            emit: &mut dyn FnMut(ProviderEvent),
        ) -> Result<(), ProviderTurnError> {
            let count = {
                let mut c = self.call_count.borrow_mut();
                let v = *c;
                *c += 1;
                v
            };

            if count == 0 {
                emit(ProviderEvent::Agent(AgentEvent::Done {
                    status: "Done. session saved".to_string(),
                    assistant_message: concat!(
                        "```ANVIL_TOOL\n",
                        "{\"id\":\"call_001\",\"tool\":\"file.read\",\"path\":\"./Cargo.toml\"}\n",
                        "```\n",
                        "```ANVIL_FINAL\n",
                        "Reading Cargo.toml.\n",
                        "```\n"
                    )
                    .to_string(),
                    completion_summary: "turn 1".to_string(),
                    saved_status: "session saved".to_string(),
                    tool_logs: Vec::new(),
                    elapsed_ms: 0,
                }));
                Ok(())
            } else {
                Err(ProviderTurnError::Backend(
                    "connection reset during follow-up".to_string(),
                ))
            }
        }
    }

    let provider = ErrorOnFollowupProvider {
        call_count: Rc::new(RefCell::new(0)),
    };
    let result = app.run_live_turn("trigger followup error", &provider, &tui);

    assert!(
        result.is_err(),
        "error during agentic follow-up should propagate"
    );
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("agentic follow-up failed"),
        "error message should indicate agentic follow-up failure, got: {err_msg}"
    );
}

// --- web.fetch agent protocol tests ---

#[test]
fn structured_response_parser_handles_web_fetch_tool_block() {
    let response = anvil::agent::BasicAgentLoop::parse_structured_response(concat!(
        "```ANVIL_TOOL\n",
        "{\"id\":\"call_fetch_001\",\"tool\":\"web.fetch\",\"url\":\"https://example.com\"}\n",
        "```\n",
        "```ANVIL_FINAL\n",
        "Fetched the page content.\n",
        "```\n"
    ))
    .expect("parser should handle web.fetch block");

    assert_eq!(response.tool_calls.len(), 1);
    assert_eq!(response.tool_calls[0].tool_name, "web.fetch");
    match &response.tool_calls[0].input {
        anvil::tooling::ToolInput::WebFetch { url } => {
            assert_eq!(url, "https://example.com");
        }
        other => panic!("unexpected tool input: {other:?}"),
    }
}

#[test]
fn structured_response_parser_repairs_web_fetch_block() {
    // Simulate malformed JSON that the repair path should handle
    let response = anvil::agent::BasicAgentLoop::parse_structured_response(concat!(
        "```ANVIL_TOOL\n",
        "{\"id\":\"call_fetch_002\",\"tool\":\"web.fetch\",\"url\":\"https://example.com/page\"\n",
        "```\n",
        "```ANVIL_FINAL\n",
        "Fetched the page.\n",
        "```\n"
    ))
    .expect("parser should repair web.fetch block");

    assert_eq!(response.tool_calls.len(), 1);
    match &response.tool_calls[0].input {
        anvil::tooling::ToolInput::WebFetch { url } => {
            assert_eq!(url, "https://example.com/page");
        }
        other => panic!("unexpected tool input: {other:?}"),
    }
}

#[test]
fn system_prompt_includes_web_fetch_tool() {
    let session = anvil::session::SessionRecord::new(std::path::PathBuf::from("/tmp"));
    let system_prompt = anvil::agent::tool_protocol_system_prompt(&[]);
    let request = anvil::agent::BasicAgentLoop::build_turn_request(
        "test-model",
        &session,
        false,
        4096,
        &system_prompt,
    );
    assert!(
        request.messages[0].content.contains("web.fetch"),
        "system prompt should mention web.fetch"
    );
}

// --- web.search agent protocol tests ---

#[test]
fn structured_response_parser_handles_web_search_tool_block() {
    let response = anvil::agent::BasicAgentLoop::parse_structured_response(concat!(
        "```ANVIL_TOOL\n",
        "{\"id\":\"call_search_001\",\"tool\":\"web.search\",\"query\":\"rust error handling\"}\n",
        "```\n",
        "```ANVIL_FINAL\n",
        "Searched for rust error handling.\n",
        "```\n"
    ))
    .expect("parser should handle web.search block");

    assert_eq!(response.tool_calls.len(), 1);
    assert_eq!(response.tool_calls[0].tool_name, "web.search");
    match &response.tool_calls[0].input {
        anvil::tooling::ToolInput::WebSearch { query } => {
            assert_eq!(query, "rust error handling");
        }
        other => panic!("unexpected tool input: {other:?}"),
    }
}

#[test]
fn structured_response_parser_rejects_web_search_missing_query() {
    let result = anvil::agent::BasicAgentLoop::parse_structured_response(concat!(
        "```ANVIL_TOOL\n",
        "{\"id\":\"call_search_002\",\"tool\":\"web.search\"}\n",
        "```\n",
        "```ANVIL_FINAL\n",
        "Done.\n",
        "```\n"
    ));

    assert!(result.is_err(), "missing query should fail");
    let err = result.unwrap_err();
    assert!(
        err.contains("missing query"),
        "error should mention missing query, got: {err}"
    );
}

#[test]
fn structured_response_parser_repairs_web_search_block() {
    // Simulate malformed JSON that the repair path should handle
    let response = anvil::agent::BasicAgentLoop::parse_structured_response(concat!(
        "```ANVIL_TOOL\n",
        "{\"id\":\"call_search_003\",\"tool\":\"web.search\",\"query\":\"serde derive\"\n",
        "```\n",
        "```ANVIL_FINAL\n",
        "Searched.\n",
        "```\n"
    ))
    .expect("parser should repair web.search block");

    assert_eq!(response.tool_calls.len(), 1);
    match &response.tool_calls[0].input {
        anvil::tooling::ToolInput::WebSearch { query } => {
            assert_eq!(query, "serde derive");
        }
        other => panic!("unexpected tool input: {other:?}"),
    }
}

#[test]
fn system_prompt_includes_web_search_tool() {
    let session = anvil::session::SessionRecord::new(std::path::PathBuf::from("/tmp"));
    let system_prompt = anvil::agent::tool_protocol_system_prompt(&[]);
    let request = anvil::agent::BasicAgentLoop::build_turn_request(
        "test-model",
        &session,
        false,
        4096,
        &system_prompt,
    );
    assert!(
        request.messages[0].content.contains("web.search"),
        "system prompt should mention web.search"
    );
}

#[test]
fn system_prompt_includes_github_insights() {
    let session = anvil::session::SessionRecord::new(std::path::PathBuf::from("/tmp"));
    let system_prompt = anvil::agent::tool_protocol_system_prompt(&[]);
    let request = anvil::agent::BasicAgentLoop::build_turn_request(
        "test-model",
        &session,
        false,
        4096,
        &system_prompt,
    );
    assert!(
        request.messages[0].content.contains("GitHub Insights"),
        "system prompt should mention GitHub Insights"
    );
    assert!(
        request.messages[0].content.contains("gh api"),
        "system prompt should mention gh api"
    );
}

// --- Phase 3: detect_project_languages tests ---

#[test]
fn detect_rust_project() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    std::fs::write(tmp.path().join("Cargo.toml"), "[package]\nname = \"test\"").expect("write");
    let languages = anvil::app::detect_project_languages(tmp.path());
    assert_eq!(languages, vec![anvil::agent::ProjectLanguage::Rust]);
}

#[test]
fn detect_nodejs_project() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    std::fs::write(tmp.path().join("package.json"), "{}").expect("write");
    let languages = anvil::app::detect_project_languages(tmp.path());
    assert_eq!(languages, vec![anvil::agent::ProjectLanguage::NodeJs]);
}

#[test]
fn detect_empty_project() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    let languages = anvil::app::detect_project_languages(tmp.path());
    assert!(languages.is_empty());
}

#[test]
fn detect_both_rust_and_nodejs() {
    let tmp = tempfile::tempdir().expect("tmpdir");
    std::fs::write(tmp.path().join("Cargo.toml"), "[package]").expect("write");
    std::fs::write(tmp.path().join("package.json"), "{}").expect("write");
    let languages = anvil::app::detect_project_languages(tmp.path());
    assert_eq!(
        languages,
        vec![
            anvil::agent::ProjectLanguage::Rust,
            anvil::agent::ProjectLanguage::NodeJs,
        ]
    );
}

// --- Phase 3: tool_protocol_system_prompt dynamic guide tests ---

#[test]
fn system_prompt_rust_includes_git_and_cargo() {
    use anvil::agent::ProjectLanguage;
    let prompt = anvil::agent::tool_protocol_system_prompt(&[ProjectLanguage::Rust]);
    assert!(
        prompt.contains("Git operations"),
        "should contain Git operations guide"
    );
    assert!(
        prompt.contains("cargo build"),
        "should contain cargo build guide"
    );
}

#[test]
fn system_prompt_nodejs_includes_git_and_npm_but_not_cargo() {
    use anvil::agent::ProjectLanguage;
    let prompt = anvil::agent::tool_protocol_system_prompt(&[ProjectLanguage::NodeJs]);
    assert!(
        prompt.contains("Git operations"),
        "should contain Git operations guide"
    );
    assert!(prompt.contains("npm"), "should contain npm guide");
    assert!(
        !prompt.contains("cargo build"),
        "should not contain cargo build guide"
    );
}

#[test]
fn system_prompt_empty_has_git_only() {
    let prompt = anvil::agent::tool_protocol_system_prompt(&[]);
    assert!(
        prompt.contains("Git operations"),
        "should contain Git operations guide"
    );
    assert!(
        !prompt.contains("cargo build"),
        "should not contain cargo build guide"
    );
    assert!(!prompt.contains("npm test"), "should not contain npm guide");
}

#[test]
fn system_prompt_both_languages_includes_both() {
    use anvil::agent::ProjectLanguage;
    let prompt = anvil::agent::tool_protocol_system_prompt(&[
        ProjectLanguage::Rust,
        ProjectLanguage::NodeJs,
    ]);
    assert!(prompt.contains("cargo build"), "should contain cargo guide");
    assert!(prompt.contains("npm"), "should contain npm guide");
}

#[test]
fn system_prompt_includes_never_guide() {
    use anvil::agent::ProjectLanguage;
    let prompt = anvil::agent::tool_protocol_system_prompt(&[ProjectLanguage::Rust]);
    assert!(
        prompt.contains("NEVER"),
        "should contain NEVER guide for dangerous operations"
    );
}

// --- file.edit agent protocol tests ---

#[test]
fn file_edit_anvil_tool_block_parses() {
    let response = anvil::agent::BasicAgentLoop::parse_structured_response(concat!(
        "```ANVIL_TOOL\n",
        "{\"id\":\"call_edit_001\",\"tool\":\"file.edit\",\"path\":\"./src/main.rs\",\"old_string\":\"fn main()\",\"new_string\":\"fn main() -> Result<()>\"}\n",
        "```\n",
        "```ANVIL_FINAL\n",
        "Edited the file.\n",
        "```\n"
    ))
    .expect("parser should handle file.edit block");

    assert_eq!(response.tool_calls.len(), 1);
    assert_eq!(response.tool_calls[0].tool_name, "file.edit");
    match &response.tool_calls[0].input {
        anvil::tooling::ToolInput::FileEdit {
            path,
            old_string,
            new_string,
        } => {
            assert_eq!(path, "./src/main.rs");
            assert_eq!(old_string, "fn main()");
            assert_eq!(new_string, "fn main() -> Result<()>");
        }
        other => panic!("unexpected tool input: {other:?}"),
    }
}

#[test]
fn system_prompt_includes_file_edit_tool() {
    let session = anvil::session::SessionRecord::new(std::path::PathBuf::from("/tmp"));
    let system_prompt = anvil::agent::tool_protocol_system_prompt(&[]);
    let request = anvil::agent::BasicAgentLoop::build_turn_request(
        "test-model",
        &session,
        false,
        4096,
        &system_prompt,
    );
    assert!(
        request.messages[0].content.contains("file.edit"),
        "system prompt should mention file.edit"
    );
}

// --- ANVIL.md project instructions tests ---

#[test]
fn system_prompt_includes_project_instructions() {
    let session = anvil::session::SessionRecord::new(std::path::PathBuf::from("/tmp"));
    let instructions = "Always use snake_case for function names.";
    let base_prompt = anvil::agent::tool_protocol_system_prompt(&[]);
    let system_prompt = format!(
        "{}\n\n## Project instructions (from ANVIL.md)\n{}",
        base_prompt, instructions
    );
    let request = anvil::agent::BasicAgentLoop::build_turn_request(
        "test-model",
        &session,
        false,
        4096,
        &system_prompt,
    );

    assert!(
        request.messages[0]
            .content
            .contains("Project instructions (from ANVIL.md)"),
        "system prompt should contain ANVIL.md header"
    );
    assert!(
        request.messages[0].content.contains(instructions),
        "system prompt should contain the project instructions"
    );
    assert!(
        request.messages[0].content.contains("You are Anvil"),
        "system prompt should still contain base prompt"
    );
}

#[test]
fn system_prompt_without_project_instructions() {
    let session = anvil::session::SessionRecord::new(std::path::PathBuf::from("/tmp"));
    let system_prompt = anvil::agent::tool_protocol_system_prompt(&[]);
    let request = anvil::agent::BasicAgentLoop::build_turn_request(
        "test-model",
        &session,
        false,
        4096,
        &system_prompt,
    );

    assert!(
        !request.messages[0]
            .content
            .contains("Project instructions (from ANVIL.md)"),
        "system prompt should NOT contain ANVIL.md header when None"
    );
    assert!(
        request.messages[0].content.contains("You are Anvil"),
        "system prompt should contain base prompt"
    );
}

#[test]
fn build_turn_request_with_limit_includes_system_prompt() {
    let mut app = common::build_app();
    app.record_user_input("msg_001", "u1").expect("persist");
    app.record_assistant_output("msg_002", "a1")
        .expect("persist");
    app.record_user_input("msg_003", "u2").expect("persist");

    let instructions = "Test project instructions.";
    let base_prompt = anvil::agent::tool_protocol_system_prompt(&[]);
    let system_prompt = format!(
        "{}\n\n## Project instructions (from ANVIL.md)\n{}",
        base_prompt, instructions
    );
    let request = anvil::agent::BasicAgentLoop::build_turn_request_with_limit(
        "local-default",
        app.session(),
        true,
        3,
        &system_prompt,
    );

    assert_eq!(request.messages.len(), 4);
    assert_eq!(
        request.messages[0].role,
        anvil::provider::ProviderMessageRole::System
    );
    assert!(
        request.messages[0].content.contains(instructions),
        "system prompt should contain project instructions"
    );
    assert!(
        request.messages[0].content.contains("You are Anvil"),
        "system prompt should contain base prompt"
    );
}

// ---------------------------------------------------------------------------
// Phase 1: ProviderTurnError expansion tests
// ---------------------------------------------------------------------------

#[test]
fn provider_turn_error_is_retryable_network() {
    let err = ProviderTurnError::Network("connection refused".to_string());
    assert!(err.is_retryable());
}

#[test]
fn provider_turn_error_is_retryable_server_error() {
    let err = ProviderTurnError::ServerError {
        status_code: 500,
        message: "internal server error".to_string(),
    };
    assert!(err.is_retryable());
}

#[test]
fn provider_turn_error_is_retryable_timeout() {
    let err = ProviderTurnError::Timeout("request timed out".to_string());
    assert!(err.is_retryable());
}

#[test]
fn provider_turn_error_not_retryable_cancelled() {
    let err = ProviderTurnError::Cancelled;
    assert!(!err.is_retryable());
}

#[test]
fn provider_turn_error_not_retryable_client_error() {
    let err = ProviderTurnError::ClientError {
        status_code: 401,
        message: "unauthorized".to_string(),
    };
    assert!(!err.is_retryable());
}

#[test]
fn provider_turn_error_not_retryable_parse() {
    let err = ProviderTurnError::Parse("invalid JSON".to_string());
    assert!(!err.is_retryable());
}

#[test]
fn provider_turn_error_not_retryable_backend() {
    let err = ProviderTurnError::Backend("unknown error".to_string());
    assert!(!err.is_retryable());
}

#[test]
fn provider_turn_error_display_network() {
    let err = ProviderTurnError::Network("connection refused".to_string());
    assert_eq!(err.to_string(), "network error: connection refused");
}

#[test]
fn provider_turn_error_display_server_error() {
    let err = ProviderTurnError::ServerError {
        status_code: 502,
        message: "bad gateway".to_string(),
    };
    assert_eq!(err.to_string(), "server error (502): bad gateway");
}

#[test]
fn provider_turn_error_display_client_error() {
    let err = ProviderTurnError::ClientError {
        status_code: 403,
        message: "forbidden".to_string(),
    };
    assert_eq!(err.to_string(), "client error (403): forbidden");
}

#[test]
fn provider_turn_error_display_timeout() {
    let err = ProviderTurnError::Timeout("timed out after 30s".to_string());
    assert_eq!(err.to_string(), "timeout: timed out after 30s");
}

#[test]
fn provider_turn_error_display_parse() {
    let err = ProviderTurnError::Parse("expected '{'".to_string());
    assert_eq!(err.to_string(), "parse error: expected '{'");
}

#[test]
fn provider_turn_error_display_cancelled() {
    let err = ProviderTurnError::Cancelled;
    assert_eq!(err.to_string(), "provider turn cancelled");
}

#[test]
fn provider_turn_error_display_backend() {
    let err = ProviderTurnError::Backend("something went wrong".to_string());
    assert_eq!(
        err.to_string(),
        "provider backend error: something went wrong"
    );
}

// ---------------------------------------------------------------------------
// Phase 1: ProviderErrorKind serde tests
// ---------------------------------------------------------------------------

use anvil::provider::ProviderErrorKind;

#[test]
fn provider_error_kind_serde_roundtrip_known_variants() {
    let variants = vec![
        ProviderErrorKind::Cancelled,
        ProviderErrorKind::Network,
        ProviderErrorKind::ServerError,
        ProviderErrorKind::ClientError,
        ProviderErrorKind::Timeout,
        ProviderErrorKind::Parse,
        ProviderErrorKind::Backend,
    ];
    for variant in variants {
        let json = serde_json::to_string(&variant).unwrap();
        let deserialized: ProviderErrorKind = serde_json::from_str(&json).unwrap();
        assert_eq!(variant, deserialized);
    }
}

#[test]
fn provider_error_kind_unknown_variant_fallback() {
    // A future variant name that doesn't exist should deserialize to Unknown.
    let json = r#""RateLimit""#;
    let deserialized: ProviderErrorKind = serde_json::from_str(json).unwrap();
    assert_eq!(deserialized, ProviderErrorKind::Unknown);
}

#[test]
fn provider_error_kind_unknown_variant_arbitrary_string() {
    let json = r#""SomethingCompletelyNew""#;
    let deserialized: ProviderErrorKind = serde_json::from_str(json).unwrap();
    assert_eq!(deserialized, ProviderErrorKind::Unknown);
}

// ---------------------------------------------------------------------------
// Phase 3: Error classification tests
// ---------------------------------------------------------------------------

use anvil::provider::{
    classify_curl_error, classify_http_error, redact_secrets, sanitize_error_message,
};

#[test]
fn classify_http_error_500_returns_server_error() {
    let err = classify_http_error(500, "internal server error");
    assert!(matches!(
        err,
        ProviderTurnError::ServerError {
            status_code: 500,
            ..
        }
    ));
}

#[test]
fn classify_http_error_502_returns_server_error() {
    let err = classify_http_error(502, "bad gateway");
    assert!(matches!(
        err,
        ProviderTurnError::ServerError {
            status_code: 502,
            ..
        }
    ));
}

#[test]
fn classify_http_error_401_returns_client_error() {
    let err = classify_http_error(401, "unauthorized");
    assert!(matches!(
        err,
        ProviderTurnError::ClientError {
            status_code: 401,
            ..
        }
    ));
}

#[test]
fn classify_http_error_404_returns_client_error() {
    let err = classify_http_error(404, "not found");
    assert!(matches!(
        err,
        ProviderTurnError::ClientError {
            status_code: 404,
            ..
        }
    ));
}

#[test]
fn classify_http_error_299_returns_backend() {
    let err = classify_http_error(299, "unexpected");
    assert!(matches!(err, ProviderTurnError::Backend(_)));
}

#[test]
fn classify_curl_error_exit_28_returns_timeout() {
    let err = classify_curl_error(28, "operation timed out");
    assert!(matches!(err, ProviderTurnError::Timeout(_)));
}

#[test]
fn classify_curl_error_exit_7_returns_network() {
    let err = classify_curl_error(7, "failed to connect");
    assert!(matches!(err, ProviderTurnError::Network(_)));
}

#[test]
fn classify_curl_error_exit_6_returns_network() {
    let err = classify_curl_error(6, "could not resolve host");
    assert!(matches!(err, ProviderTurnError::Network(_)));
}

#[test]
fn classify_curl_error_exit_other_returns_network() {
    let err = classify_curl_error(56, "recv failure");
    assert!(matches!(err, ProviderTurnError::Network(_)));
}

#[test]
fn sanitize_error_message_truncates_to_500_chars() {
    let long_message = "a".repeat(600);
    let sanitized = sanitize_error_message(&long_message);
    assert!(sanitized.contains("... [truncated, 600 bytes total]"));
    assert!(sanitized.len() < 600);
}

#[test]
fn sanitize_error_message_short_message_unchanged() {
    let msg = "short error";
    let sanitized = sanitize_error_message(msg);
    assert_eq!(sanitized, "short error");
}

#[test]
fn redact_secrets_authorization_header() {
    let msg = "Authorization: Bearer sk-1234567890abcdef";
    let redacted = redact_secrets(msg);
    assert!(redacted.contains("[REDACTED]"));
    assert!(!redacted.contains("sk-1234567890abcdef"));
}

#[test]
fn redact_secrets_bearer_token() {
    let msg = "error with Bearer my-secret-token in message";
    let redacted = redact_secrets(msg);
    assert!(redacted.contains("[REDACTED]"));
    assert!(!redacted.contains("my-secret-token"));
}

#[test]
fn redact_secrets_api_key() {
    let msg = "api_key: my-secret-key";
    let redacted = redact_secrets(msg);
    assert!(redacted.contains("[REDACTED]"));
    assert!(!redacted.contains("my-secret-key"));
}

#[test]
fn redact_secrets_no_secrets_unchanged() {
    let msg = "normal error message";
    let redacted = redact_secrets(msg);
    assert_eq!(redacted, "normal error message");
}

#[test]
fn classify_http_error_sanitizes_body_with_secrets() {
    let err = classify_http_error(500, "error with Authorization: Bearer sk-secret");
    if let ProviderTurnError::ServerError { message, .. } = err {
        assert!(message.contains("[REDACTED]"));
        assert!(!message.contains("sk-secret"));
    } else {
        panic!("expected ServerError");
    }
}

#[test]
fn classify_http_error_is_retryable_for_server_errors() {
    let err = classify_http_error(500, "internal error");
    assert!(err.is_retryable());
}

#[test]
fn classify_http_error_not_retryable_for_client_errors() {
    let err = classify_http_error(401, "unauthorized");
    assert!(!err.is_retryable());
}

#[test]
fn classify_curl_error_timeout_is_retryable() {
    let err = classify_curl_error(28, "timed out");
    assert!(err.is_retryable());
}

#[test]
fn classify_curl_error_network_is_retryable() {
    let err = classify_curl_error(7, "connection refused");
    assert!(err.is_retryable());
}

// ---------------------------------------------------------------------------
// Phase 4: RetryTransport tests
// ---------------------------------------------------------------------------

use anvil::provider::{RetryConfig, RetryTransport};

/// Mock transport that fails a configurable number of times before succeeding.
#[derive(Clone)]
struct RetryMockTransport {
    call_count: Rc<RefCell<usize>>,
    fail_count: usize,
    error: ProviderTurnError,
    response: HttpResponse,
    /// If set, stream_lines will invoke the callback before failing.
    invoke_callback_before_error: bool,
}

impl RetryMockTransport {
    fn new(fail_count: usize, error: ProviderTurnError) -> Self {
        Self {
            call_count: Rc::new(RefCell::new(0)),
            fail_count,
            error,
            response: HttpResponse {
                status_code: 200,
                body: b"ok".to_vec(),
            },
            invoke_callback_before_error: false,
        }
    }
}

impl HttpTransport for RetryMockTransport {
    fn post_json_with_headers(
        &self,
        _url: &str,
        _body: &[u8],
        _headers: &[(&str, &str)],
    ) -> Result<HttpResponse, ProviderTurnError> {
        let mut count = self.call_count.borrow_mut();
        *count += 1;
        if *count <= self.fail_count {
            Err(self.error.clone())
        } else {
            Ok(self.response.clone())
        }
    }

    fn get_with_headers(
        &self,
        _url: &str,
        _headers: &[(&str, &str)],
    ) -> Result<HttpResponse, ProviderTurnError> {
        let mut count = self.call_count.borrow_mut();
        *count += 1;
        if *count <= self.fail_count {
            Err(self.error.clone())
        } else {
            Ok(self.response.clone())
        }
    }

    fn stream_lines(
        &self,
        _url: &str,
        _body: &[u8],
        _headers: &[(&str, &str)],
        on_line: &mut dyn FnMut(&str),
    ) -> Result<(), ProviderTurnError> {
        let mut count = self.call_count.borrow_mut();
        *count += 1;
        if *count <= self.fail_count {
            if self.invoke_callback_before_error {
                on_line("partial data");
            }
            Err(self.error.clone())
        } else {
            on_line("success line");
            Ok(())
        }
    }
}

fn fast_retry_config(max_retries: u32) -> RetryConfig {
    RetryConfig {
        max_retries,
        base_delay_ms: 0,
        backoff_factor: 2,
        max_delay_ms: 0,
    }
}

#[test]
fn retry_transport_succeeds_on_second_attempt() {
    let mock = RetryMockTransport::new(1, ProviderTurnError::Network("fail".into()));
    let call_count = mock.call_count.clone();
    let transport = RetryTransport::with_config(mock, fast_retry_config(3));

    let result = transport.post_json_with_headers("http://test", b"body", &[]);
    assert!(result.is_ok());
    assert_eq!(*call_count.borrow(), 2);
}

#[test]
fn retry_transport_exhausts_max_retries() {
    // max_retries=3 means 4 total attempts (initial + 3 retries)
    let mock = RetryMockTransport::new(10, ProviderTurnError::Network("fail".into()));
    let call_count = mock.call_count.clone();
    let transport = RetryTransport::with_config(mock, fast_retry_config(3));

    let result = transport.post_json_with_headers("http://test", b"body", &[]);
    assert!(result.is_err());
    assert_eq!(*call_count.borrow(), 4);
}

#[test]
fn retry_transport_no_retry_on_client_error() {
    let mock = RetryMockTransport::new(
        10,
        ProviderTurnError::ClientError {
            status_code: 401,
            message: "unauthorized".into(),
        },
    );
    let call_count = mock.call_count.clone();
    let transport = RetryTransport::with_config(mock, fast_retry_config(3));

    let result = transport.post_json_with_headers("http://test", b"body", &[]);
    assert!(result.is_err());
    assert_eq!(*call_count.borrow(), 1);
}

#[test]
fn retry_transport_no_retry_on_parse_error() {
    let mock = RetryMockTransport::new(10, ProviderTurnError::Parse("bad json".into()));
    let call_count = mock.call_count.clone();
    let transport = RetryTransport::with_config(mock, fast_retry_config(3));

    let result = transport.get_with_headers("http://test", &[]);
    assert!(result.is_err());
    assert_eq!(*call_count.borrow(), 1);
}

#[test]
fn retry_transport_get_succeeds_on_second_attempt() {
    let mock = RetryMockTransport::new(1, ProviderTurnError::Timeout("slow".into()));
    let call_count = mock.call_count.clone();
    let transport = RetryTransport::with_config(mock, fast_retry_config(3));

    let result = transport.get_with_headers("http://test", &[]);
    assert!(result.is_ok());
    assert_eq!(*call_count.borrow(), 2);
}

#[test]
fn retry_transport_stream_lines_retries_on_connection_error() {
    let mock = RetryMockTransport::new(1, ProviderTurnError::Network("refused".into()));
    let call_count = mock.call_count.clone();
    let transport = RetryTransport::with_config(mock, fast_retry_config(3));

    let mut lines = Vec::new();
    let result = transport.stream_lines("http://test", b"body", &[], &mut |line| {
        lines.push(line.to_string());
    });
    assert!(result.is_ok());
    assert_eq!(*call_count.borrow(), 2);
    assert!(lines.contains(&"success line".to_string()));
}

#[test]
fn retry_transport_server_error_retries() {
    let mock = RetryMockTransport::new(
        2,
        ProviderTurnError::ServerError {
            status_code: 503,
            message: "service unavailable".into(),
        },
    );
    let call_count = mock.call_count.clone();
    let transport = RetryTransport::with_config(mock, fast_retry_config(3));

    let result = transport.get_with_headers("http://test", &[]);
    assert!(result.is_ok());
    assert_eq!(*call_count.borrow(), 3);
}

#[test]
fn retry_transport_stream_lines_no_retry_after_callback_invoked() {
    let mut mock = RetryMockTransport::new(10, ProviderTurnError::Network("mid-stream".into()));
    mock.invoke_callback_before_error = true;
    let call_count = mock.call_count.clone();
    let transport = RetryTransport::with_config(mock, fast_retry_config(3));

    let mut lines = Vec::new();
    let result = transport.stream_lines("http://test", b"body", &[], &mut |line| {
        lines.push(line.to_string());
    });
    assert!(result.is_err());
    // Only 1 call because callback was invoked, so guard prevents retry
    assert_eq!(*call_count.borrow(), 1);
}

// ---------------------------------------------------------------------------
// Phase 6: Health check tests
// ---------------------------------------------------------------------------

use anvil::provider::openai::OpenAiCompatibleProviderClient;

#[test]
fn ollama_health_check_success() {
    let mock = MockHttpTransport {
        seen_urls: Rc::new(RefCell::new(Vec::new())),
        seen_bodies: Rc::new(RefCell::new(Vec::new())),
        seen_headers: Rc::new(RefCell::new(Vec::new())),
        response: HttpResponse {
            status_code: 200,
            body: br#"{"models":[]}"#.to_vec(),
        },
        get_response: Some(HttpResponse {
            status_code: 200,
            body: br#"{"models":[]}"#.to_vec(),
        }),
    };
    let urls = mock.seen_urls.clone();
    let client = OllamaProviderClient::with_transport("http://localhost:11434", mock);
    let result = client.health_check();
    assert!(result.is_ok());
    let seen = urls.borrow();
    assert!(
        seen.iter().any(|u| u.contains("/api/tags")),
        "health check should hit /api/tags"
    );
}

#[test]
fn ollama_health_check_failure() {
    /// Mock transport that always fails with a network error.
    #[derive(Clone)]
    struct FailingTransport;

    impl HttpTransport for FailingTransport {
        fn post_json_with_headers(
            &self,
            _url: &str,
            _body: &[u8],
            _headers: &[(&str, &str)],
        ) -> Result<HttpResponse, ProviderTurnError> {
            Err(ProviderTurnError::Network("connection refused".into()))
        }

        fn get_with_headers(
            &self,
            _url: &str,
            _headers: &[(&str, &str)],
        ) -> Result<HttpResponse, ProviderTurnError> {
            Err(ProviderTurnError::Network("connection refused".into()))
        }
    }

    let client = OllamaProviderClient::with_transport("http://localhost:11434", FailingTransport);
    let result = client.health_check();
    assert!(result.is_err());
    let err_msg = result.unwrap_err();
    assert!(err_msg.contains("Ollamaに接続できません"));
    assert!(err_msg.contains("localhost:11434"));
}

#[test]
fn openai_health_check_success_with_auth() {
    let mock = MockHttpTransport {
        seen_urls: Rc::new(RefCell::new(Vec::new())),
        seen_bodies: Rc::new(RefCell::new(Vec::new())),
        seen_headers: Rc::new(RefCell::new(Vec::new())),
        response: HttpResponse {
            status_code: 200,
            body: br#"{"data":[]}"#.to_vec(),
        },
        get_response: Some(HttpResponse {
            status_code: 200,
            body: br#"{"data":[]}"#.to_vec(),
        }),
    };
    let urls = mock.seen_urls.clone();
    let headers = mock.seen_headers.clone();
    let client = OpenAiCompatibleProviderClient::with_transport("http://localhost:8080", mock)
        .with_api_key("test-key-123");
    let result = client.health_check();
    assert!(result.is_ok());
    let seen_urls = urls.borrow();
    assert!(
        seen_urls.iter().any(|u| u.contains("/v1/models")),
        "health check should hit /v1/models"
    );
    let seen_headers = headers.borrow();
    let last_headers = seen_headers.last().expect("should have headers");
    assert!(
        last_headers
            .iter()
            .any(|(k, v)| k == "Authorization" && v == "test-key-123"),
        "should send Authorization header with api_key"
    );
}

#[test]
fn openai_health_check_failure_with_auth_guidance() {
    #[derive(Clone)]
    struct FailingTransport;

    impl HttpTransport for FailingTransport {
        fn post_json_with_headers(
            &self,
            _url: &str,
            _body: &[u8],
            _headers: &[(&str, &str)],
        ) -> Result<HttpResponse, ProviderTurnError> {
            Err(ProviderTurnError::Network("connection refused".into()))
        }

        fn get_with_headers(
            &self,
            _url: &str,
            _headers: &[(&str, &str)],
        ) -> Result<HttpResponse, ProviderTurnError> {
            Err(ProviderTurnError::ClientError {
                status_code: 401,
                message: "unauthorized".into(),
            })
        }
    }

    let client =
        OpenAiCompatibleProviderClient::with_transport("http://localhost:8080", FailingTransport)
            .with_api_key("bad-key");
    let result = client.health_check();
    assert!(result.is_err());
    let err_msg = result.unwrap_err();
    assert!(err_msg.contains("OpenAI互換プロバイダーに接続できません"));
    assert!(err_msg.contains("認証情報の形式を確認してください"));
}

#[test]
fn openai_health_check_no_auth_no_guidance() {
    #[derive(Clone)]
    struct FailingTransport;

    impl HttpTransport for FailingTransport {
        fn post_json_with_headers(
            &self,
            _url: &str,
            _body: &[u8],
            _headers: &[(&str, &str)],
        ) -> Result<HttpResponse, ProviderTurnError> {
            Err(ProviderTurnError::Network("refused".into()))
        }

        fn get_with_headers(
            &self,
            _url: &str,
            _headers: &[(&str, &str)],
        ) -> Result<HttpResponse, ProviderTurnError> {
            Err(ProviderTurnError::Network("refused".into()))
        }
    }

    let client =
        OpenAiCompatibleProviderClient::with_transport("http://localhost:8080", FailingTransport);
    let result = client.health_check();
    assert!(result.is_err());
    let err_msg = result.unwrap_err();
    assert!(err_msg.contains("OpenAI互換プロバイダーに接続できません"));
    // No auth guidance when no api_key is set
    assert!(!err_msg.contains("認証情報の形式を確認してください"));
}

// -----------------------------------------------------------------------
// Phase 5.1: Ollama image serialization tests
// -----------------------------------------------------------------------

#[test]
fn ollama_chat_message_with_images_serializes_correctly() {
    let msg = OllamaChatMessage {
        role: "user".to_string(),
        content: "describe this image".to_string(),
        images: Some(vec!["aGVsbG8=".to_string()]),
    };
    let json = serde_json::to_value(&msg).unwrap();
    assert_eq!(json["role"], "user");
    assert_eq!(json["content"], "describe this image");
    assert_eq!(json["images"][0], "aGVsbG8=");
}

#[test]
fn ollama_chat_message_without_images_omits_images_key() {
    let msg = OllamaChatMessage {
        role: "user".to_string(),
        content: "hello".to_string(),
        images: None,
    };
    let json = serde_json::to_value(&msg).unwrap();
    assert!(json.get("images").is_none());
}

#[test]
fn ollama_build_chat_request_maps_provider_images() {
    use anvil::provider::{ImageContent, ProviderMessage, ProviderTurnRequest};

    let images = vec![ImageContent {
        base64: "dGVzdA==".to_string(),
        mime_type: "image/png".to_string(),
    }];
    let request = ProviderTurnRequest::new(
        "llava".to_string(),
        vec![ProviderMessage::new(ProviderMessageRole::User, "describe").with_images(images)],
        false,
    );
    let ollama_req =
        OllamaProviderClient::<anvil::provider::TcpHttpTransport>::build_chat_request(&request);
    let first = &ollama_req.messages[0];
    assert_eq!(
        first.images.as_ref().unwrap(),
        &vec!["dGVzdA==".to_string()]
    );
}
