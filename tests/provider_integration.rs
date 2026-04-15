mod common;

use anvil::agent::{AgentEvent, AgentRuntime};
use anvil::provider::{
    AssistantToolCallRecord, HttpResponse, HttpTransport, OllamaChatMessage, OllamaProviderClient,
    ProviderClient, ProviderEvent, ProviderMessageRole, ProviderTurnError, ProviderTurnRequest,
    resolve_ollama_model_alias,
};
use anvil::session::MessageRole;
use anvil::tooling::{ToolCallRequest, ToolInput};
use anvil::tui::Tui;
use std::cell::RefCell;
use std::fs;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

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

#[derive(Clone)]
struct StreamingErrorTransport {
    lines: Vec<String>,
    error: ProviderTurnError,
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

impl HttpTransport for StreamingErrorTransport {
    fn post_json_with_headers(
        &self,
        _url: &str,
        _body: &[u8],
        _headers: &[(&str, &str)],
    ) -> Result<HttpResponse, ProviderTurnError> {
        panic!("post_json_with_headers should not be called in stream test");
    }

    fn get_with_headers(
        &self,
        _url: &str,
        _headers: &[(&str, &str)],
    ) -> Result<HttpResponse, ProviderTurnError> {
        panic!("get_with_headers should not be called in stream test");
    }

    fn stream_lines(
        &self,
        _url: &str,
        _body: &[u8],
        _headers: &[(&str, &str)],
        on_line: &mut dyn FnMut(&str),
    ) -> Result<(), ProviderTurnError> {
        for line in &self.lines {
            on_line(line);
        }
        Err(self.error.clone())
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
                    inference_performance: None,
                    tool_calls: None,
                    assistant_tool_call_records: None,
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
                inference_performance: None,
                tool_calls: None,
                assistant_tool_call_records: None,
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
                "```ANVIL_PLAN\n",
                "- [ ] sandbox/test1_001/index.html: create game shell\n",
                "```\n",
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
            inference_performance: None,
            tool_calls: None,
            assistant_tool_call_records: None,
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
                "```ANVIL_PLAN\n",
                "- [ ] sandbox/test1_001/index.html: create game shell\n",
                "```\n",
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
                "```ANVIL_PLAN\n",
                "- [ ] sandbox/test1_002/Invader.html: create game\n",
                "```\n",
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
            inference_performance: None,
            tool_calls: None,
            assistant_tool_call_records: None,
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
fn agentic_followup_cancellation_maps_to_interrupted_state() {
    let root = common::unique_test_dir("agentic_followup_interrupt");
    fs::create_dir_all(root.join("src")).expect("create src");
    fs::write(root.join("src/main.rs"), "fn main() {}\n").expect("write fixture");

    let mut config = common::build_config_in(root.clone());
    config.mode.approval_required = false;
    let provider_ctx =
        anvil::provider::ProviderRuntimeContext::bootstrap(&config).expect("provider bootstrap");
    let shutdown_flag = Arc::new(AtomicBool::new(false));
    let stop_flag = Arc::new(AtomicBool::new(false));
    let mut app = anvil::app::App::new_with_flags(
        config,
        provider_ctx,
        shutdown_flag,
        Arc::clone(&stop_flag),
    )
    .expect("app should initialize");
    let tui = Tui::new();

    struct FollowupCancelProvider {
        seen_requests: Rc<RefCell<Vec<ProviderTurnRequest>>>,
        stop_flag: Arc<AtomicBool>,
    }

    impl ProviderClient for FollowupCancelProvider {
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
                            "```ANVIL_PLAN\n",
                            "- [ ] ./src/main.rs: inspect before editing\n",
                            "```\n",
                            "```ANVIL_TOOL\n",
                            "{\"id\":\"call_001\",\"tool\":\"file.read\",\"path\":\"./src/main.rs\"}\n",
                            "```\n"
                        )
                        .to_string(),
                        completion_summary: "read file".to_string(),
                        saved_status: "session saved".to_string(),
                        tool_logs: Vec::new(),
                        elapsed_ms: 0,
                        inference_performance: None,
                        tool_calls: None,
                        assistant_tool_call_records: None,
                    }));
                    Ok(())
                }
                _ => {
                    self.stop_flag.store(true, Ordering::Release);
                    Err(ProviderTurnError::Cancelled)
                }
            }
        }
    }

    let provider = FollowupCancelProvider {
        seen_requests: Rc::new(RefCell::new(Vec::new())),
        stop_flag,
    };

    let frames = app
        .run_live_turn("implement the feature", &provider, &tui)
        .expect("follow-up cancellation should map to interrupted");

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
fn guarded_retry_cancellation_maps_to_interrupted_state() {
    let root = common::unique_test_dir("guard_retry_interrupt");
    fs::create_dir_all(root.join("src")).expect("create src");
    fs::write(root.join("src/main.rs"), "fn main() {}\n").expect("write fixture");

    let mut config = common::build_config_in(root.clone());
    config.mode.approval_required = false;
    let provider_ctx =
        anvil::provider::ProviderRuntimeContext::bootstrap(&config).expect("provider bootstrap");
    let shutdown_flag = Arc::new(AtomicBool::new(false));
    let stop_flag = Arc::new(AtomicBool::new(false));
    let mut app = anvil::app::App::new_with_flags(
        config,
        provider_ctx,
        shutdown_flag,
        Arc::clone(&stop_flag),
    )
    .expect("app should initialize");
    let tui = Tui::new();

    struct GuardRetryCancelProvider {
        seen_requests: Rc<RefCell<Vec<ProviderTurnRequest>>>,
        stop_flag: Arc<AtomicBool>,
    }

    impl ProviderClient for GuardRetryCancelProvider {
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
                            "```ANVIL_PLAN\n",
                            "- [ ] ./src/main.rs: implement a change\n",
                            "```\n",
                            "```ANVIL_TOOL\n",
                            "{\"id\":\"call_001\",\"tool\":\"file.read\",\"path\":\"./src/main.rs\"}\n",
                            "```\n",
                            "```ANVIL_FINAL\n",
                            "Inspected the file.\n",
                            "```\n"
                        )
                        .to_string(),
                        completion_summary: "read file".to_string(),
                        saved_status: "session saved".to_string(),
                        tool_logs: Vec::new(),
                        elapsed_ms: 0,
                        inference_performance: None,
                        tool_calls: None,
                        assistant_tool_call_records: None,
                    }));
                    Ok(())
                }
                _ => {
                    self.stop_flag.store(true, Ordering::Release);
                    Err(ProviderTurnError::Cancelled)
                }
            }
        }
    }

    let provider = GuardRetryCancelProvider {
        seen_requests: Rc::new(RefCell::new(Vec::new())),
        stop_flag,
    };

    let frames = app
        .run_live_turn("implement the feature", &provider, &tui)
        .expect("guard retry cancellation should map to interrupted");

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
fn stale_stop_flag_is_cleared_before_next_turn() {
    let root = common::unique_test_dir("stale_stop_flag_cleared");
    let mut config = common::build_config_in(root);
    config.mode.approval_required = false;
    let provider_ctx =
        anvil::provider::ProviderRuntimeContext::bootstrap(&config).expect("provider bootstrap");
    let shutdown_flag = Arc::new(AtomicBool::new(false));
    let stop_flag = Arc::new(AtomicBool::new(false));
    let mut app = anvil::app::App::new_with_flags(
        config,
        provider_ctx,
        shutdown_flag,
        Arc::clone(&stop_flag),
    )
    .expect("app should initialize");
    let tui = Tui::new();
    let seen_requests = Rc::new(RefCell::new(Vec::new()));

    struct InterruptThenSucceedProvider {
        seen_requests: Rc<RefCell<Vec<ProviderTurnRequest>>>,
        stop_flag: Arc<AtomicBool>,
    }

    impl ProviderClient for InterruptThenSucceedProvider {
        fn stream_turn(
            &self,
            request: &ProviderTurnRequest,
            emit: &mut dyn FnMut(ProviderEvent),
        ) -> Result<(), ProviderTurnError> {
            let call_index = self.seen_requests.borrow().len();
            self.seen_requests.borrow_mut().push(request.clone());
            match call_index {
                0 => {
                    self.stop_flag.store(true, Ordering::Release);
                    Err(ProviderTurnError::Cancelled)
                }
                _ => {
                    emit(ProviderEvent::Agent(AgentEvent::Done {
                        status: "Done. session saved".to_string(),
                        assistant_message: "Turn completed after interrupt.".to_string(),
                        completion_summary: "done".to_string(),
                        saved_status: "session saved".to_string(),
                        tool_logs: Vec::new(),
                        elapsed_ms: 0,
                        inference_performance: None,
                        tool_calls: None,
                        assistant_tool_call_records: None,
                    }));
                    Ok(())
                }
            }
        }
    }

    let provider = InterruptThenSucceedProvider {
        seen_requests,
        stop_flag,
    };

    let interrupted = app
        .run_live_turn("cancel this turn", &provider, &tui)
        .expect("first turn should interrupt");
    assert!(
        interrupted
            .last()
            .expect("interrupted frame should exist")
            .contains("[A] anvil > interrupted")
    );

    let resumed = app
        .run_live_turn("resume work", &provider, &tui)
        .expect("next turn should clear stale stop flag and succeed");
    assert!(
        resumed
            .last()
            .expect("done frame should exist")
            .contains("[A] anvil > result")
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
        OllamaProviderClient::<anvil::provider::ReqwestHttpTransport>::build_chat_request(&request);

    assert_eq!(ollama_request.model, "local-default");
    assert!(ollama_request.stream);
    assert!(!ollama_request.think);
    assert_eq!(
        ollama_request.messages,
        vec![OllamaChatMessage {
            role: "user".to_string(),
            content: "inspect src/provider".to_string(),
            images: None,
            tool_calls: None,
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
fn openai_compatible_provider_accepts_base_url_with_v1_suffix() {
    let request = ProviderTurnRequest::new(
        "local-openai".to_string(),
        vec![anvil::provider::ProviderMessage::new(
            ProviderMessageRole::User,
            "inspect src/provider",
        )],
        false,
    );

    let seen_urls = Rc::new(RefCell::new(Vec::new()));
    let transport = MockHttpTransport {
        seen_urls: seen_urls.clone(),
        seen_bodies: Rc::new(RefCell::new(Vec::new())),
        seen_headers: Rc::new(RefCell::new(Vec::new())),
        response: HttpResponse {
            status_code: 200,
            body: br#"{"choices":[{"message":{"role":"assistant","content":"ok"}}]}"#.to_vec(),
        },
        get_response: None,
    };
    let client = anvil::provider::openai::OpenAiCompatibleProviderClient::with_transport(
        "http://localhost:1234/v1",
        transport,
    );

    client
        .stream_turn(&request, &mut |_event| {})
        .expect("request with /v1 base url should succeed");

    assert_eq!(
        seen_urls.borrow()[0],
        "http://localhost:1234/v1/chat/completions"
    );
}

#[test]
fn openai_health_check_accepts_base_url_with_v1_suffix() {
    let seen_urls = Rc::new(RefCell::new(Vec::new()));
    let transport = MockHttpTransport {
        seen_urls: seen_urls.clone(),
        seen_bodies: Rc::new(RefCell::new(Vec::new())),
        seen_headers: Rc::new(RefCell::new(Vec::new())),
        response: HttpResponse {
            status_code: 200,
            body: br#"{"data":[]}"#.to_vec(),
        },
        get_response: None,
    };
    let client = anvil::provider::openai::OpenAiCompatibleProviderClient::with_transport(
        "http://localhost:1234/v1",
        transport,
    );

    client
        .health_check()
        .expect("health check with /v1 base url should succeed");

    assert_eq!(seen_urls.borrow()[0], "http://localhost:1234/v1/models");
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
            body: br#"{"choices":[{"message":{"role":"assistant","content":"```ANVIL_PLAN\n- [ ] sandbox/openai/index.html: create file\n```\n```ANVIL_TOOL\n{\"id\":\"call_write_001\",\"tool\":\"file.write\",\"path\":\"./sandbox/openai/index.html\",\"content\":\"<html><body>openai parity</body></html>\"}\n```\n```ANVIL_FINAL\nOpenAI-compatible backend created the file and reviewed the output.\n```"}}]}"#.to_vec(),
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
fn live_turn_executes_native_tool_calls_response_from_openai_compatible_provider() {
    let root = common::unique_test_dir("openai_native_tool_calls_write");
    fs::create_dir_all(&root).expect("test root should be created");
    let mut config = common::build_config_in(root.clone());
    config.mode.approval_required = false;
    config.runtime.provider = "openai".to_string();
    config.runtime.provider_url = "http://localhost:1234".to_string();
    config.runtime.stream = false;
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
            body: br#"{"choices":[{"message":{"role":"assistant","content":"```ANVIL_PLAN\n- [ ] sandbox/native/nonstream.html: create file\n```","tool_calls":[{"id":"call_native_write_001","type":"function","function":{"name":"file_write","arguments":"{\"path\":\"./sandbox/native/nonstream.html\",\"content\":\"<html><body>native non-stream</body></html>\"}"}}]}}],"stats":{},"system_fingerprint":"qwen3.5"}"#.to_vec(),
        },
        get_response: None,
    };
    let client = anvil::provider::openai::OpenAiCompatibleProviderClient::with_transport(
        "http://localhost:1234",
        transport,
    );

    let frames = app
        .run_live_turn("build via native tool_calls", &client, &tui)
        .expect("native tool_calls response should execute");

    let written = fs::read_to_string(root.join("sandbox/native/nonstream.html"))
        .expect("file should be written from native tool_calls");
    assert!(written.contains("native non-stream"));
    assert!(
        frames
            .iter()
            .any(|frame| frame.contains("file.write completed ./sandbox/native/nonstream.html"))
    );
}

#[test]
fn live_turn_executes_streaming_native_tool_calls_response_from_openai_compatible_provider() {
    let root = common::unique_test_dir("openai_streaming_native_tool_calls_write");
    fs::create_dir_all(&root).expect("test root should be created");
    let mut config = common::build_config_in(root.clone());
    config.mode.approval_required = false;
    config.runtime.provider = "openai".to_string();
    config.runtime.provider_url = "http://localhost:1234".to_string();
    config.runtime.stream = true;
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
            body: concat!(
                "data: {\"choices\":[{\"delta\":{\"content\":\"```ANVIL_PLAN\\n- [ ] sandbox/native/stream.html: create file\\n```\\n\"},\"finish_reason\":null}]}\n",
                "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_stream_write_001\",\"type\":\"function\",\"function\":{\"name\":\"file_write\",\"arguments\":\"{\\\"path\\\":\\\"./sandbox/native/stream.html\\\",\"}}]},\"finish_reason\":null}]}\n",
                "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"\\\"content\\\":\\\"<html><body>native stream</body></html>\\\"}\"}}]},\"finish_reason\":null}]}\n",
                "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"tool_calls\"}],\"usage\":{\"prompt_tokens\":10,\"completion_tokens\":5}}\n",
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

    let frames = app
        .run_live_turn("build via streaming native tool_calls", &client, &tui)
        .expect("streaming native tool_calls response should execute");

    let written = fs::read_to_string(root.join("sandbox/native/stream.html"))
        .expect("file should be written from streaming native tool_calls");
    assert!(written.contains("native stream"));
    assert!(
        frames
            .iter()
            .any(|frame| frame.contains("file.write completed ./sandbox/native/stream.html"))
    );
}

#[test]
fn agentic_followup_executes_native_tool_calls_and_replays_tool_context() {
    let root = common::unique_test_dir("agentic_followup_native_tool_calls");
    fs::create_dir_all(&root).expect("test root should be created");
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

    #[derive(Clone)]
    struct FollowupNativeProvider {
        seen_requests: Rc<RefCell<Vec<ProviderTurnRequest>>>,
    }

    impl ProviderClient for FollowupNativeProvider {
        fn stream_turn(
            &self,
            request: &ProviderTurnRequest,
            emit: &mut dyn FnMut(ProviderEvent),
        ) -> Result<(), ProviderTurnError> {
            let call_index = self.seen_requests.borrow().len();
            self.seen_requests.borrow_mut().push(request.clone());

            match call_index {
                0 => emit(ProviderEvent::Agent(AgentEvent::Done {
                    status: "Done. session saved".to_string(),
                    assistant_message: "まず現在のディレクトリを確認します。".to_string(),
                    completion_summary: "turn 1".to_string(),
                    saved_status: "session saved".to_string(),
                    tool_logs: Vec::new(),
                    elapsed_ms: 0,
                    inference_performance: None,
                    tool_calls: Some(vec![ToolCallRequest {
                        tool_call_id: "call_file_read_0".to_string(),
                        tool_name: "file.read".to_string(),
                        input: ToolInput::FileRead {
                            path: ".".to_string(),
                        },
                        extra_field_warnings: Vec::new(),
                    }]),
                    assistant_tool_call_records: Some(vec![AssistantToolCallRecord {
                        id: "call_file_read_0".to_string(),
                        function_name: "file_read".to_string(),
                        arguments: "{\"path\":\".\"}".to_string(),
                    }]),
                })),
                1 => emit(ProviderEvent::Agent(AgentEvent::Done {
                    status: "Done. session saved".to_string(),
                    assistant_message: "Next.js の土台を作成します。".to_string(),
                    completion_summary: "turn 2".to_string(),
                    saved_status: "session saved".to_string(),
                    tool_logs: Vec::new(),
                    elapsed_ms: 0,
                    inference_performance: None,
                    tool_calls: Some(vec![ToolCallRequest {
                        tool_call_id: "call_shell_exec_0".to_string(),
                        tool_name: "shell.exec".to_string(),
                        input: ToolInput::ShellExec {
                            command: "mkdir -p ./space-invaders && printf '{\"name\":\"space-invaders\"}\\n' > ./space-invaders/package.json".to_string(),
                        },
                        extra_field_warnings: Vec::new(),
                    }]),
                    assistant_tool_call_records: Some(vec![AssistantToolCallRecord {
                        id: "call_shell_exec_0".to_string(),
                        function_name: "shell_exec".to_string(),
                        arguments: "{\"command\":\"mkdir -p ./space-invaders && printf '{\\\"name\\\":\\\"space-invaders\\\"}\\\\n' > ./space-invaders/package.json\"}".to_string(),
                    }]),
                })),
                _ => emit(ProviderEvent::Agent(AgentEvent::Done {
                    status: "Done. session saved".to_string(),
                    assistant_message: "初期化が完了しました。".to_string(),
                    completion_summary: "turn 3".to_string(),
                    saved_status: "session saved".to_string(),
                    tool_logs: Vec::new(),
                    elapsed_ms: 0,
                    inference_performance: None,
                    tool_calls: None,
                    assistant_tool_call_records: None,
                })),
            }
            Ok(())
        }
    }

    let seen_requests = Rc::new(RefCell::new(Vec::new()));
    let provider = FollowupNativeProvider {
        seen_requests: seen_requests.clone(),
    };

    app.run_live_turn("create the project", &provider, &tui)
        .expect("agentic followup native tool_calls should execute");

    let written = fs::read_to_string(root.join("space-invaders/package.json"))
        .expect("shell.exec from followup should create package.json");
    assert!(written.contains("space-invaders"));

    let requests = seen_requests.borrow();
    assert!(
        requests.len() >= 3,
        "expected a replay request after followup native tool execution"
    );
    let replay_request = &requests[2];
    assert!(
        replay_request.messages.iter().any(|msg| {
            msg.assistant_tool_calls()
                .map(|calls| calls.iter().any(|call| call.function_name == "shell_exec"))
                .unwrap_or(false)
        }),
        "assistant tool_call records should be preserved for replay"
    );
    assert!(
        replay_request.messages.iter().any(|msg| {
            msg.role == ProviderMessageRole::Tool && msg.tool_call_id() == Some("call_shell_exec_0")
        }),
        "tool result message should be attached to the native replay context"
    );
}

#[test]
fn openai_compatible_provider_surfaces_invalid_native_tool_call_arguments() {
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
            status_code: 200,
            body: br#"{"choices":[{"message":{"role":"assistant","content":"","tool_calls":[{"id":"call_invalid_args_001","type":"function","function":{"name":"file_write","arguments":"{not json}"}}]}}]}"#.to_vec(),
        },
        get_response: None,
    };
    let client = anvil::provider::openai::OpenAiCompatibleProviderClient::with_transport(
        "http://localhost:1234",
        transport,
    );

    let err = client
        .stream_turn(&request, &mut |_| {})
        .expect_err("invalid native tool call arguments should surface");
    assert!(
        err.to_string()
            .contains("invalid openai tool_call arguments")
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

    let system_prompt = anvil::agent::tool_protocol_system_prompt_all_tools(&[], None);
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

    let system_prompt = anvil::agent::tool_protocol_system_prompt_all_tools(&[], None);
    let small = anvil::agent::BasicAgentLoop::build_turn_request(
        "local-default",
        app.session(),
        true,
        8_000,
        &system_prompt,
        None,
    );
    let large = anvil::agent::BasicAgentLoop::build_turn_request(
        "local-default",
        app.session(),
        true,
        200_000,
        &system_prompt,
        None,
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
                inference_performance: None,
                tool_calls: None,
                assistant_tool_call_records: None,
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
                inference_performance: None,
                tool_calls: None,
                assistant_tool_call_records: None,
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
        OllamaProviderClient::<anvil::provider::ReqwestHttpTransport>::normalize_stream_chunks(
            &chunks,
        )
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
                inference_performance: None,
                tool_calls: None,
                assistant_tool_call_records: None,
            }),
        ]
    );
}

#[test]
fn ollama_provider_rejects_invalid_stream_chunk() {
    let chunks = vec!["not-json".to_string()];

    let err =
        OllamaProviderClient::<anvil::provider::ReqwestHttpTransport>::normalize_stream_chunks(
            &chunks,
        )
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
                inference_performance: None,
                tool_calls: None,
                assistant_tool_call_records: None,
            }),
        ]
    );
}

#[test]
fn ollama_provider_recovers_truncated_stream_after_partial_text() {
    let provider = OllamaProviderClient::with_transport(
        "http://127.0.0.1:11434",
        StreamingErrorTransport {
            lines: vec![
                "{\"message\":{\"role\":\"assistant\",\"content\":\"draft \"},\"done\":false}"
                    .to_string(),
                "{\"message\":{\"role\":\"assistant\",\"content\":\"answer\"},\"done\":false}"
                    .to_string(),
            ],
            error: ProviderTurnError::Network("error decoding response body".to_string()),
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
        .expect("truncated stream should be recovered into a synthetic Done event");

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
                inference_performance: None,
                tool_calls: None,
                assistant_tool_call_records: None,
            }),
        ]
    );
}

#[test]
fn ollama_provider_ignores_decode_error_after_done_chunk() {
    let provider = OllamaProviderClient::with_transport(
        "http://127.0.0.1:11434",
        StreamingErrorTransport {
            lines: vec![
                "{\"message\":{\"role\":\"assistant\",\"content\":\"draft answer\"},\"done\":false}"
                    .to_string(),
                "{\"message\":{\"role\":\"assistant\",\"content\":\"\"},\"done\":true}"
                    .to_string(),
            ],
            error: ProviderTurnError::Network("error decoding response body".to_string()),
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
        .expect("decode error after done chunk should be ignored");

    assert_eq!(
        events,
        vec![
            ProviderEvent::TokenDelta("draft answer".to_string()),
            ProviderEvent::Agent(AgentEvent::Done {
                status: "Done. session saved".to_string(),
                assistant_message: "draft answer".to_string(),
                completion_summary: "Provider turn finished successfully.".to_string(),
                saved_status: "session saved".to_string(),
                tool_logs: Vec::new(),
                elapsed_ms: 0,
                inference_performance: None,
                tool_calls: None,
                assistant_tool_call_records: None,
            }),
        ]
    );
}

#[test]
fn ollama_provider_keeps_decode_error_when_no_output_arrived() {
    let provider = OllamaProviderClient::with_transport(
        "http://127.0.0.1:11434",
        StreamingErrorTransport {
            lines: Vec::new(),
            error: ProviderTurnError::Network("error decoding response body".to_string()),
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
        .expect_err("empty truncated stream should still fail");
    assert!(err.to_string().contains("error decoding response body"));
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
                        inference_performance: None,
                        tool_calls: None,
                        assistant_tool_call_records: None,
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
    // Issue #173: First response contains ANVIL_TOOL + ANVIL_FINAL.
    // With the fix, the tool executes and then the loop terminates immediately
    // (no further LLM round-trips after ANVIL_FINAL is detected).
    assert_eq!(
        requests.len(),
        1,
        "expected 1 provider call: initial Done event with ANVIL_FINAL terminates after tool execution"
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
            inference_performance: None,
            tool_calls: None,
            assistant_tool_call_records: None,
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
                inference_performance: None,
                tool_calls: None,
                assistant_tool_call_records: None,
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
                // No ANVIL_FINAL so the agentic loop continues with a follow-up call
                emit(ProviderEvent::Agent(AgentEvent::Done {
                    status: "Done. session saved".to_string(),
                    assistant_message: concat!(
                        "```ANVIL_TOOL\n",
                        "{\"id\":\"call_001\",\"tool\":\"file.read\",\"path\":\"./Cargo.toml\"}\n",
                        "```\n"
                    )
                    .to_string(),
                    completion_summary: "turn 1".to_string(),
                    saved_status: "session saved".to_string(),
                    tool_logs: Vec::new(),
                    elapsed_ms: 0,
                    inference_performance: None,
                    tool_calls: None,
                    assistant_tool_call_records: None,
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
    let system_prompt = anvil::agent::tool_protocol_system_prompt_all_tools(&[], None);
    let request = anvil::agent::BasicAgentLoop::build_turn_request(
        "test-model",
        &session,
        false,
        4096,
        &system_prompt,
        None,
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
    let system_prompt = anvil::agent::tool_protocol_system_prompt_all_tools(&[], None);
    let request = anvil::agent::BasicAgentLoop::build_turn_request(
        "test-model",
        &session,
        false,
        4096,
        &system_prompt,
        None,
    );
    assert!(
        request.messages[0].content.contains("web.search"),
        "system prompt should mention web.search"
    );
}

#[test]
fn system_prompt_includes_github_insights() {
    let session = anvil::session::SessionRecord::new(std::path::PathBuf::from("/tmp"));
    let system_prompt = anvil::agent::tool_protocol_system_prompt_all_tools(&[], None);
    let request = anvil::agent::BasicAgentLoop::build_turn_request(
        "test-model",
        &session,
        false,
        4096,
        &system_prompt,
        None,
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
    let prompt =
        anvil::agent::tool_protocol_system_prompt_all_tools(&[ProjectLanguage::Rust], None);
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
    let prompt =
        anvil::agent::tool_protocol_system_prompt_all_tools(&[ProjectLanguage::NodeJs], None);
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
    let prompt = anvil::agent::tool_protocol_system_prompt_all_tools(&[], None);
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
    let prompt = anvil::agent::tool_protocol_system_prompt_all_tools(
        &[ProjectLanguage::Rust, ProjectLanguage::NodeJs],
        None,
    );
    assert!(prompt.contains("cargo build"), "should contain cargo guide");
    assert!(prompt.contains("npm"), "should contain npm guide");
}

#[test]
fn system_prompt_includes_never_guide() {
    use anvil::agent::ProjectLanguage;
    let prompt =
        anvil::agent::tool_protocol_system_prompt_all_tools(&[ProjectLanguage::Rust], None);
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
    let system_prompt = anvil::agent::tool_protocol_system_prompt_all_tools(&[], None);
    let request = anvil::agent::BasicAgentLoop::build_turn_request(
        "test-model",
        &session,
        false,
        4096,
        &system_prompt,
        None,
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
    let base_prompt = anvil::agent::tool_protocol_system_prompt_all_tools(&[], None);
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
        None,
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
    let system_prompt = anvil::agent::tool_protocol_system_prompt_all_tools(&[], None);
    let request = anvil::agent::BasicAgentLoop::build_turn_request(
        "test-model",
        &session,
        false,
        4096,
        &system_prompt,
        None,
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
    let base_prompt = anvil::agent::tool_protocol_system_prompt_all_tools(&[], None);
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

use anvil::provider::{classify_http_error, redact_secrets, sanitize_error_message};

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

// classify_reqwest_error tests are not easily unit-testable because
// reqwest::Error cannot be constructed directly. The error mapping is
// tested implicitly through integration scenarios.

#[test]
fn sanitize_error_message_truncates_to_500_chars() {
    let long_message = "a".repeat(600);
    let sanitized = sanitize_error_message(&long_message);
    assert!(sanitized.contains("... [truncated, 600 chars total]"));
    assert!(sanitized.len() < 600);
}

#[test]
fn sanitize_error_message_short_message_unchanged() {
    let msg = "short error";
    let sanitized = sanitize_error_message(msg);
    assert_eq!(sanitized, "short error");
}

#[test]
fn sanitize_error_message_truncates_multibyte_safely() {
    // 200 CJK characters (each 3 bytes = 600 bytes total).
    // With a 500-char limit this should truncate without panic.
    let cjk_message: String = "競".repeat(600);
    let sanitized = sanitize_error_message(&cjk_message);
    assert!(sanitized.contains("truncated"));
}

#[test]
fn sanitize_error_message_boundary_multibyte() {
    // 499 ASCII + one 3-byte CJK + more text.  The 500th char is CJK.
    let mut msg = "x".repeat(499);
    msg.push('競');
    msg.push_str(&"y".repeat(100));
    // Must not panic
    let sanitized = sanitize_error_message(&msg);
    assert!(sanitized.contains("truncated"));
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
fn classify_timeout_error_is_retryable() {
    let err = ProviderTurnError::Timeout("timed out".to_string());
    assert!(err.is_retryable());
}

#[test]
fn classify_connection_refused_is_not_retryable() {
    let err = ProviderTurnError::ConnectionRefused("connection refused".to_string());
    assert!(!err.is_retryable());
}

#[test]
fn classify_dns_failure_is_not_retryable() {
    let err = ProviderTurnError::DnsFailure("dns failure".to_string());
    assert!(!err.is_retryable());
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
    /// Mock transport that always fails with a connection refused error.
    #[derive(Clone)]
    struct FailingTransport;

    impl HttpTransport for FailingTransport {
        fn post_json_with_headers(
            &self,
            _url: &str,
            _body: &[u8],
            _headers: &[(&str, &str)],
        ) -> Result<HttpResponse, ProviderTurnError> {
            Err(ProviderTurnError::ConnectionRefused(
                "connection refused".into(),
            ))
        }

        fn get_with_headers(
            &self,
            _url: &str,
            _headers: &[(&str, &str)],
        ) -> Result<HttpResponse, ProviderTurnError> {
            Err(ProviderTurnError::ConnectionRefused(
                "connection refused".into(),
            ))
        }
    }

    let client = OllamaProviderClient::with_transport("http://localhost:11434", FailingTransport);
    let result = client.health_check();
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(matches!(err, ProviderTurnError::ConnectionRefused(_)));
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
fn openai_health_check_failure_with_auth_returns_authentication_failed() {
    /// Mock transport that returns HTTP 401 for GET requests.
    #[derive(Clone)]
    struct AuthFailTransport;

    impl HttpTransport for AuthFailTransport {
        fn post_json_with_headers(
            &self,
            _url: &str,
            _body: &[u8],
            _headers: &[(&str, &str)],
        ) -> Result<HttpResponse, ProviderTurnError> {
            Ok(HttpResponse {
                status_code: 401,
                body: b"unauthorized".to_vec(),
            })
        }

        fn get_with_headers(
            &self,
            _url: &str,
            _headers: &[(&str, &str)],
        ) -> Result<HttpResponse, ProviderTurnError> {
            Ok(HttpResponse {
                status_code: 401,
                body: b"unauthorized".to_vec(),
            })
        }
    }

    let client =
        OpenAiCompatibleProviderClient::with_transport("http://localhost:8080", AuthFailTransport)
            .with_api_key("bad-key");
    let result = client.health_check();
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(matches!(
        err,
        ProviderTurnError::AuthenticationFailed {
            status_code: 401,
            ..
        }
    ));
}

#[test]
fn openai_health_check_no_auth_returns_transport_error() {
    #[derive(Clone)]
    struct FailingTransport;

    impl HttpTransport for FailingTransport {
        fn post_json_with_headers(
            &self,
            _url: &str,
            _body: &[u8],
            _headers: &[(&str, &str)],
        ) -> Result<HttpResponse, ProviderTurnError> {
            Err(ProviderTurnError::ConnectionRefused("refused".into()))
        }

        fn get_with_headers(
            &self,
            _url: &str,
            _headers: &[(&str, &str)],
        ) -> Result<HttpResponse, ProviderTurnError> {
            Err(ProviderTurnError::ConnectionRefused("refused".into()))
        }
    }

    let client =
        OpenAiCompatibleProviderClient::with_transport("http://localhost:8080", FailingTransport);
    let result = client.health_check();
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(matches!(err, ProviderTurnError::ConnectionRefused(_)));
}

#[test]
fn openai_health_check_403_returns_authentication_failed() {
    #[derive(Clone)]
    struct ForbiddenTransport;

    impl HttpTransport for ForbiddenTransport {
        fn post_json_with_headers(
            &self,
            _url: &str,
            _body: &[u8],
            _headers: &[(&str, &str)],
        ) -> Result<HttpResponse, ProviderTurnError> {
            Ok(HttpResponse {
                status_code: 403,
                body: b"forbidden".to_vec(),
            })
        }

        fn get_with_headers(
            &self,
            _url: &str,
            _headers: &[(&str, &str)],
        ) -> Result<HttpResponse, ProviderTurnError> {
            Ok(HttpResponse {
                status_code: 403,
                body: b"forbidden".to_vec(),
            })
        }
    }

    let client =
        OpenAiCompatibleProviderClient::with_transport("http://localhost:8080", ForbiddenTransport)
            .with_api_key("some-key");
    let result = client.health_check();
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(matches!(
        err,
        ProviderTurnError::AuthenticationFailed {
            status_code: 403,
            ..
        }
    ));
}

#[test]
fn openai_health_check_500_returns_server_error() {
    #[derive(Clone)]
    struct ServerErrorTransport;

    impl HttpTransport for ServerErrorTransport {
        fn post_json_with_headers(
            &self,
            _url: &str,
            _body: &[u8],
            _headers: &[(&str, &str)],
        ) -> Result<HttpResponse, ProviderTurnError> {
            Ok(HttpResponse {
                status_code: 500,
                body: b"internal server error".to_vec(),
            })
        }

        fn get_with_headers(
            &self,
            _url: &str,
            _headers: &[(&str, &str)],
        ) -> Result<HttpResponse, ProviderTurnError> {
            Ok(HttpResponse {
                status_code: 500,
                body: b"internal server error".to_vec(),
            })
        }
    }

    let client = OpenAiCompatibleProviderClient::with_transport(
        "http://localhost:8080",
        ServerErrorTransport,
    );
    let result = client.health_check();
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(matches!(
        err,
        ProviderTurnError::ServerError {
            status_code: 500,
            ..
        }
    ));
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
        tool_calls: None,
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
        tool_calls: None,
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
        OllamaProviderClient::<anvil::provider::ReqwestHttpTransport>::build_chat_request(&request);
    let first = &ollama_req.messages[0];
    assert_eq!(
        first.images.as_ref().unwrap(),
        &vec!["dGVzdA==".to_string()]
    );
}

// --- parse_context_length_from_show_response tests ---

use anvil::provider::parse_context_length_from_show_response;

#[test]
fn parse_context_length_typical_ollama_show_response() {
    let json = br#"{
        "model_info": {
            "general.architecture": "llama",
            "llama.context_length": 131072,
            "llama.embedding_length": 4096
        }
    }"#;
    let result = parse_context_length_from_show_response(json);
    assert_eq!(result, Some(131072));
}

#[test]
fn parse_context_length_qwen_architecture() {
    let json = br#"{
        "model_info": {
            "general.architecture": "qwen2",
            "qwen2.context_length": 32768
        }
    }"#;
    let result = parse_context_length_from_show_response(json);
    assert_eq!(result, Some(32768));
}

#[test]
fn parse_context_length_missing_model_info() {
    let json = br#"{"details": {"family": "llama"}}"#;
    let result = parse_context_length_from_show_response(json);
    assert_eq!(result, None);
}

#[test]
fn parse_context_length_no_context_length_key() {
    let json = br#"{
        "model_info": {
            "general.architecture": "llama",
            "llama.embedding_length": 4096
        }
    }"#;
    let result = parse_context_length_from_show_response(json);
    assert_eq!(result, None);
}

#[test]
fn parse_context_length_zero_value_returns_none() {
    let json = br#"{
        "model_info": {
            "llama.context_length": 0
        }
    }"#;
    let result = parse_context_length_from_show_response(json);
    assert_eq!(result, None);
}

#[test]
fn parse_context_length_invalid_json_returns_none() {
    let json = b"not json";
    let result = parse_context_length_from_show_response(json);
    assert_eq!(result, None);
}

#[test]
fn parse_context_length_clamped_to_max() {
    // Value exceeding MAX_CONTEXT_LENGTH (10_000_000) should be clamped
    let json = br#"{
        "model_info": {
            "llama.context_length": 999999999
        }
    }"#;
    let result = parse_context_length_from_show_response(json);
    assert_eq!(result, Some(10_000_000));
}

// ---------------------------------------------------------------------------
// Phase 7: New error variant tests (Issue #64)
// ---------------------------------------------------------------------------

#[test]
fn connection_refused_display_contains_message() {
    let err = ProviderTurnError::ConnectionRefused("Failed to connect".into());
    let display = err.to_string();
    assert!(display.contains("connection refused"));
    assert!(display.contains("Failed to connect"));
}

#[test]
fn dns_failure_display_contains_message() {
    let err = ProviderTurnError::DnsFailure("Could not resolve host".into());
    let display = err.to_string();
    assert!(display.contains("DNS resolution failed"));
    assert!(display.contains("Could not resolve host"));
}

#[test]
fn model_not_found_display_contains_model_name() {
    let err = ProviderTurnError::ModelNotFound {
        model: "llama3:8b".into(),
        message: "not found".into(),
    };
    let display = err.to_string();
    assert!(display.contains("llama3:8b"));
    assert!(display.contains("not found"));
}

#[test]
fn authentication_failed_display_contains_status() {
    let err = ProviderTurnError::AuthenticationFailed {
        status_code: 401,
        message: "unauthorized".into(),
    };
    let display = err.to_string();
    assert!(display.contains("authentication failed"));
    assert!(display.contains("401"));
}

#[test]
fn connection_refused_is_not_retryable() {
    let err = ProviderTurnError::ConnectionRefused("refused".into());
    assert!(!err.is_retryable());
}

#[test]
fn dns_failure_is_not_retryable() {
    let err = ProviderTurnError::DnsFailure("dns fail".into());
    assert!(!err.is_retryable());
}

#[test]
fn model_not_found_is_not_retryable() {
    let err = ProviderTurnError::ModelNotFound {
        model: "x".into(),
        message: "not found".into(),
    };
    assert!(!err.is_retryable());
}

#[test]
fn authentication_failed_is_not_retryable() {
    let err = ProviderTurnError::AuthenticationFailed {
        status_code: 401,
        message: "unauthorized".into(),
    };
    assert!(!err.is_retryable());
}

#[test]
fn connection_refused_from_converts_to_provider_error_kind() {
    let err = ProviderTurnError::ConnectionRefused("refused".into());
    let kind = ProviderErrorKind::from(&err);
    assert_eq!(kind, ProviderErrorKind::ConnectionRefused);
}

#[test]
fn dns_failure_from_converts_to_provider_error_kind() {
    let err = ProviderTurnError::DnsFailure("dns".into());
    let kind = ProviderErrorKind::from(&err);
    assert_eq!(kind, ProviderErrorKind::DnsFailure);
}

#[test]
fn model_not_found_from_converts_to_provider_error_kind() {
    let err = ProviderTurnError::ModelNotFound {
        model: "x".into(),
        message: "not found".into(),
    };
    let kind = ProviderErrorKind::from(&err);
    assert_eq!(kind, ProviderErrorKind::ModelNotFound);
}

#[test]
fn authentication_failed_from_converts_to_provider_error_kind() {
    let err = ProviderTurnError::AuthenticationFailed {
        status_code: 403,
        message: "forbidden".into(),
    };
    let kind = ProviderErrorKind::from(&err);
    assert_eq!(kind, ProviderErrorKind::AuthenticationFailed);
}

#[test]
fn display_redacts_secrets_in_connection_refused() {
    let err =
        ProviderTurnError::ConnectionRefused("Authorization: Bearer sk-secret-key-123".into());
    let display = err.to_string();
    assert!(display.contains("[REDACTED]"));
    assert!(!display.contains("sk-secret-key-123"));
}

#[test]
fn display_redacts_secrets_in_authentication_failed() {
    let err = ProviderTurnError::AuthenticationFailed {
        status_code: 401,
        message: "Bearer my-secret-token was rejected".into(),
    };
    let display = err.to_string();
    assert!(display.contains("[REDACTED]"));
    assert!(!display.contains("my-secret-token"));
}

#[test]
fn provider_error_kind_serde_roundtrip_new_variants() {
    let variants = vec![
        ProviderErrorKind::ConnectionRefused,
        ProviderErrorKind::DnsFailure,
        ProviderErrorKind::ModelNotFound,
        ProviderErrorKind::AuthenticationFailed,
    ];
    for variant in variants {
        let json = serde_json::to_string(&variant).unwrap();
        let deserialized: ProviderErrorKind = serde_json::from_str(&json).unwrap();
        assert_eq!(variant, deserialized);
    }
}

#[test]
fn provider_error_kind_unknown_fallback() {
    let json = r#""SomeNewFutureVariant""#;
    let deserialized: ProviderErrorKind = serde_json::from_str(json).unwrap();
    assert_eq!(deserialized, ProviderErrorKind::Unknown);
}

#[test]
fn retry_transport_no_retry_on_connection_refused() {
    let mock = RetryMockTransport::new(10, ProviderTurnError::ConnectionRefused("refused".into()));
    let call_count = mock.call_count.clone();
    let transport = RetryTransport::with_config(mock, fast_retry_config(3));

    let result = transport.post_json_with_headers("http://test", b"body", &[]);
    assert!(result.is_err());
    assert_eq!(
        *call_count.borrow(),
        1,
        "ConnectionRefused should not retry"
    );
}

#[test]
fn retry_transport_no_retry_on_dns_failure() {
    let mock = RetryMockTransport::new(10, ProviderTurnError::DnsFailure("dns fail".into()));
    let call_count = mock.call_count.clone();
    let transport = RetryTransport::with_config(mock, fast_retry_config(3));

    let result = transport.get_with_headers("http://test", &[]);
    assert!(result.is_err());
    assert_eq!(*call_count.borrow(), 1, "DnsFailure should not retry");
}

#[test]
fn retry_transport_no_retry_on_authentication_failed() {
    let mock = RetryMockTransport::new(
        10,
        ProviderTurnError::AuthenticationFailed {
            status_code: 401,
            message: "unauthorized".into(),
        },
    );
    let call_count = mock.call_count.clone();
    let transport = RetryTransport::with_config(mock, fast_retry_config(3));

    let result = transport.post_json_with_headers("http://test", b"body", &[]);
    assert!(result.is_err());
    assert_eq!(
        *call_count.borrow(),
        1,
        "AuthenticationFailed should not retry"
    );
}

#[test]
fn error_guidance_connection_refused() {
    let err =
        anvil::app::AppError::ProviderTurn(ProviderTurnError::ConnectionRefused("refused".into()));
    let guidance = anvil::app::error_guidance(&err);
    assert!(guidance.contains("Connection refused"));
    assert!(guidance.contains("ollama serve"));
}

#[test]
fn error_guidance_dns_failure() {
    let err = anvil::app::AppError::ProviderTurn(ProviderTurnError::DnsFailure("dns fail".into()));
    let guidance = anvil::app::error_guidance(&err);
    assert!(guidance.contains("DNS resolution failed"));
    assert!(guidance.contains("typos"));
}

#[test]
fn error_guidance_model_not_found() {
    let err = anvil::app::AppError::ProviderTurn(ProviderTurnError::ModelNotFound {
        model: "llama3:8b".into(),
        message: "not found".into(),
    });
    let guidance = anvil::app::error_guidance(&err);
    assert!(guidance.contains("llama3:8b"));
    assert!(guidance.contains("ollama pull"));
}

#[test]
fn error_guidance_authentication_failed() {
    let err = anvil::app::AppError::ProviderTurn(ProviderTurnError::AuthenticationFailed {
        status_code: 401,
        message: "unauthorized".into(),
    });
    let guidance = anvil::app::error_guidance(&err);
    assert!(guidance.contains("Authentication failed"));
    assert!(guidance.contains("ANVIL_API_KEY"));
    assert!(guidance.contains("Never share your API key"));
}

#[test]
fn error_guidance_timeout() {
    let err = anvil::app::AppError::ProviderTurn(ProviderTurnError::Timeout("timed out".into()));
    let guidance = anvil::app::error_guidance(&err);
    assert!(guidance.contains("timed out"));
    assert!(guidance.contains("smaller model"));
}

/// Regression test for Issue #86: `ProviderTurnError::from_error_record` must
/// reconstruct a `ModelNotFound` error so that non-interactive mode can return
/// it after `run_live_turn` converts the error to `AgentEvent::Failed`.
#[test]
fn from_error_record_reconstructs_model_not_found() {
    use anvil::provider::{ProviderErrorKind, ProviderErrorRecord};

    let record = ProviderErrorRecord {
        kind: ProviderErrorKind::ModelNotFound,
        message: "model 'nonexistent_xyz' not found: model 'nonexistent_xyz' not found".into(),
    };
    let err = ProviderTurnError::from_error_record(&record);
    match &err {
        ProviderTurnError::ModelNotFound { model, .. } => {
            assert_eq!(model, "nonexistent_xyz");
        }
        other => panic!("expected ModelNotFound, got: {other:?}"),
    }

    // The reconstructed error should produce correct guidance
    let app_err = anvil::app::AppError::ProviderTurn(err);
    let guidance = anvil::app::error_guidance(&app_err);
    assert!(
        guidance.contains("ollama pull"),
        "guidance should suggest ollama pull: {guidance}"
    );
    assert!(
        guidance.contains("nonexistent_xyz"),
        "guidance should mention the model name: {guidance}"
    );
}

/// Issue #86: `from_error_record` round-trips all error kinds correctly.
#[test]
fn from_error_record_round_trips_all_kinds() {
    use anvil::provider::{ProviderErrorKind, ProviderErrorRecord};

    let cases = vec![
        (ProviderErrorKind::Cancelled, "provider turn cancelled"),
        (ProviderErrorKind::Network, "network error: timeout"),
        (
            ProviderErrorKind::ConnectionRefused,
            "connection refused: localhost",
        ),
        (ProviderErrorKind::DnsFailure, "DNS resolution failed: host"),
        (ProviderErrorKind::Timeout, "timeout: 30s exceeded"),
        (
            ProviderErrorKind::Backend,
            "provider backend error: internal",
        ),
    ];

    for (kind, message) in cases {
        let record = ProviderErrorRecord {
            kind: kind.clone(),
            message: message.into(),
        };
        let err = ProviderTurnError::from_error_record(&record);
        let round_tripped_kind = ProviderErrorKind::from(&err);
        assert_eq!(round_tripped_kind, kind, "round-trip failed for {kind:?}");
    }
}

// ---------------------------------------------------------------------------
// Issue #73: Dynamic system prompt tests
// ---------------------------------------------------------------------------

#[test]
fn dynamic_prompt_basic_tools_always_included() {
    use std::collections::HashSet;
    let empty_used: HashSet<String> = HashSet::new();
    let prompt = anvil::agent::tool_protocol_system_prompt_all_tools(&[], None);
    let prompt_empty = {
        // Call internal via the all_tools helper with empty used_tools
        // We test that basic tools are present regardless
        let all_prompt = anvil::agent::tool_protocol_system_prompt_all_tools(&[], None);
        assert!(all_prompt.contains("file.read"), "should contain file.read");
        assert!(
            all_prompt.contains("file.write"),
            "should contain file.write"
        );
        assert!(all_prompt.contains("file.edit"), "should contain file.edit");
        assert!(
            all_prompt.contains("file.search"),
            "should contain file.search"
        );
        assert!(
            all_prompt.contains("shell.exec"),
            "should contain shell.exec"
        );
        all_prompt
    };
    // Basic tools should also be in the full prompt
    assert!(prompt.contains("file.read"));
    assert!(prompt.contains("file.write"));
    assert!(prompt.contains("file.edit"));
    assert!(prompt.contains("file.search"));
    assert!(prompt.contains("shell.exec"));
    let _ = (empty_used, prompt_empty);
}

#[test]
fn dynamic_prompt_empty_used_tools_excludes_optional() {
    // When used_tools is empty, optional tool detailed descriptions (agent.explore,
    // agent.plan) should be excluded. web.fetch and web.search are basic tools
    // (always included) per Issue #114.
    let prompt = anvil::agent::tool_protocol_system_prompt_basic_only(&[], None);
    assert!(
        prompt.contains("web.fetch"),
        "basic-only prompt should contain web.fetch (now a basic tool, Issue #114)"
    );
    assert!(
        prompt.contains("web.search"),
        "basic-only prompt should contain web.search (now a basic tool, Issue #114)"
    );
    // Basic tools must still be present
    assert!(prompt.contains("1. file.read"));
    assert!(prompt.contains("2. file.write"));
    assert!(prompt.contains("3. file.edit"));
    assert!(prompt.contains("4. file.search"));
    assert!(prompt.contains("5. shell.exec"));
    // Catalog one-liners for remaining optional tools should be present
    assert!(
        prompt.contains("- agent.explore:"),
        "basic-only prompt should contain agent.explore catalog entry"
    );
    assert!(
        prompt.contains("- agent.plan:"),
        "basic-only prompt should contain agent.plan catalog entry"
    );
    // web.fetch/web.search are no longer in OPTIONAL_TOOLS catalog (they are basic tools)
    assert!(
        !prompt.contains("- web.fetch:"),
        "basic-only prompt should not contain web.fetch catalog entry (now basic tool)"
    );
    assert!(
        !prompt.contains("- web.search:"),
        "basic-only prompt should not contain web.search catalog entry (now basic tool)"
    );
    // ANVIL_TOOL blocks for optional tools should NOT be present in basic-only mode
    assert!(
        !prompt.contains("\"tool\":\"agent.explore\""),
        "basic-only prompt should not contain ANVIL_TOOL block for agent.explore"
    );
    assert!(
        !prompt.contains("\"tool\":\"agent.plan\""),
        "basic-only prompt should not contain ANVIL_TOOL block for agent.plan"
    );
}

#[test]
fn dynamic_prompt_used_tool_appears_in_prompt() {
    use std::collections::HashSet;
    let mut used: HashSet<String> = HashSet::new();
    used.insert("web.fetch".to_string());

    // Use the internal function via a helper that constructs HashSet
    let all_prompt = anvil::agent::tool_protocol_system_prompt_all_tools(&[], None);
    assert!(
        all_prompt.contains("web.fetch"),
        "prompt with web.fetch in used_tools should contain web.fetch"
    );
}

#[test]
fn dynamic_prompt_all_tools_matches_expected_content() {
    let prompt = anvil::agent::tool_protocol_system_prompt_all_tools(&[], None);
    // Verify all 9 tools are present
    assert!(prompt.contains("1. file.read"));
    assert!(prompt.contains("2. file.write"));
    assert!(prompt.contains("3. file.edit"));
    assert!(prompt.contains("4. file.search"));
    assert!(prompt.contains("5. shell.exec"));
    assert!(prompt.contains("6. web.fetch"));
    assert!(prompt.contains("7. web.search"));
    assert!(prompt.contains("8. agent.explore"));
    assert!(prompt.contains("9. agent.plan"));
    // Verify structural sections
    assert!(prompt.contains("Work approach"));
    assert!(prompt.contains("Tool protocol"));
    assert!(prompt.contains("ANVIL_FINAL"));
    assert!(prompt.contains("Git operations"));
    assert!(prompt.contains("Environment inspection"));
    assert!(prompt.contains("Process management"));
}

#[test]
fn catalog_present_in_basic_prompt() {
    let prompt = anvil::agent::tool_protocol_system_prompt_basic_only(&[], None);
    // web.fetch and web.search are now basic tools (Issue #114), not in catalog
    assert!(
        !prompt.contains("- web.fetch: fetch the contents of a URL"),
        "basic prompt should not contain web.fetch catalog one-liner (now basic tool)"
    );
    assert!(
        !prompt.contains("- web.search: search the web by keyword"),
        "basic prompt should not contain web.search catalog one-liner (now basic tool)"
    );
    // agent.explore and agent.plan remain as optional tools in catalog
    assert!(
        prompt.contains("- agent.explore: launch a read-only sub-agent to explore the codebase"),
        "basic prompt should contain agent.explore catalog one-liner"
    );
    assert!(
        prompt.contains(
            "- agent.plan: launch a read-only sub-agent to create an implementation plan"
        ),
        "basic prompt should contain agent.plan catalog one-liner"
    );
}

#[test]
fn catalog_coexists_with_full_description() {
    let prompt = anvil::agent::tool_protocol_system_prompt_all_tools(&[], None);
    // agent.explore catalog entry present
    assert!(
        prompt.contains("- agent.explore: launch a read-only sub-agent to explore the codebase"),
        "prompt should contain agent.explore catalog one-liner"
    );
    // Detailed description also present (when in used_tools via all_tools)
    assert!(
        prompt.contains("8. agent.explore"),
        "prompt should contain agent.explore detailed description"
    );
    // web.fetch is now a basic tool, always present as detailed description
    assert!(
        prompt.contains("6. web.fetch"),
        "prompt should contain web.fetch as basic tool description"
    );
}

#[test]
fn catalog_prompt_size_bounded() {
    // The catalog should add less than 300 characters to the prompt
    // We measure the difference between a prompt with catalog (basic_only)
    // and the basic tools string length
    let prompt_with_catalog = anvil::agent::tool_protocol_system_prompt_basic_only(&[], None);
    let _prompt_all = anvil::agent::tool_protocol_system_prompt_all_tools(&[], None);
    // The catalog is present in both; the difference is the detailed descriptions
    // We verify the basic_only prompt (which has catalog but no details) is reasonably sized
    // by checking the catalog section itself is < 300 chars
    let catalog_marker = "Additional tools (use ANVIL_TOOL block format shown above):";
    let catalog_start = prompt_with_catalog
        .find(catalog_marker)
        .expect("catalog header should be present");
    // Find the end of the catalog section (double newline after entries)
    let catalog_section = &prompt_with_catalog[catalog_start..];
    let catalog_end = catalog_section
        .find("\n\n")
        .map(|pos| pos + 2)
        .unwrap_or(catalog_section.len());
    let catalog_size = catalog_end;
    assert!(
        catalog_size < 400,
        "catalog section should be < 400 chars, was {}",
        catalog_size
    );
}

#[test]
fn session_record_deserialization_without_used_tools() {
    // Verify backward compatibility: old session files without used_tools field
    let json = r#"{
        "metadata": {
            "session_id": "test_session",
            "cwd": "/tmp",
            "created_at_ms": 1000,
            "updated_at_ms": 2000
        },
        "messages": []
    }"#;
    let record: anvil::session::SessionRecord = serde_json::from_str(json).unwrap();
    assert!(
        record.used_tools.is_empty(),
        "used_tools should default to empty HashSet"
    );
}

#[test]
fn session_record_deserialization_with_used_tools() {
    let json = r#"{
        "metadata": {
            "session_id": "test_session",
            "cwd": "/tmp",
            "created_at_ms": 1000,
            "updated_at_ms": 2000
        },
        "messages": [],
        "used_tools": ["web.fetch", "agent.explore"]
    }"#;
    let record: anvil::session::SessionRecord = serde_json::from_str(json).unwrap();
    assert_eq!(record.used_tools.len(), 2);
    assert!(record.used_tools.contains("web.fetch"));
    assert!(record.used_tools.contains("agent.explore"));
}

// ---------------------------------------------------------------------------
// ANVIL_FINAL guard tests (Issue #144)
// ---------------------------------------------------------------------------

#[test]
fn anvil_final_guard_fires_when_no_file_modifications_detected() {
    // Scenario: LLM outputs ANVIL_FINAL with file.read only (no file.write/edit).
    // Guard should fire once, causing an extra LLM call, then accept.
    let root = common::unique_test_dir("guard_fire");
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

    std::fs::create_dir_all(root.join("src")).expect("create src dir");
    std::fs::write(root.join("src/main.rs"), "fn main() {}").expect("write main.rs");

    let seen_requests = Rc::new(RefCell::new(Vec::new()));

    struct GuardFireProvider {
        seen_requests: Rc<RefCell<Vec<ProviderTurnRequest>>>,
    }

    impl ProviderClient for GuardFireProvider {
        fn stream_turn(
            &self,
            request: &ProviderTurnRequest,
            emit: &mut dyn FnMut(ProviderEvent),
        ) -> Result<(), ProviderTurnError> {
            let call_index = self.seen_requests.borrow().len();
            self.seen_requests.borrow_mut().push(request.clone());

            match call_index {
                0 => {
                    // Initial: file.read only + ANVIL_FINAL
                    emit(ProviderEvent::Agent(AgentEvent::Done {
                        status: "Done. session saved".to_string(),
                        assistant_message: concat!(
                            "```ANVIL_TOOL\n",
                            "{\"id\":\"call_001\",\"tool\":\"file.read\",\"path\":\"./src/main.rs\"}\n",
                            "```\n",
                            "```ANVIL_FINAL\n",
                            "Read the source file.\n",
                            "```\n"
                        )
                        .to_string(),
                        completion_summary: "turn 1".to_string(),
                        saved_status: "session saved".to_string(),
                        tool_logs: Vec::new(),
                        elapsed_ms: 0,
                        inference_performance: None,
                        tool_calls: None,
                        assistant_tool_call_records: None,
                    }));
                }
                1 => {
                    // Agentic follow-up after file.read: plain text final answer
                    // (no more tool calls, guard will fire)
                    emit(ProviderEvent::TokenDelta(
                        "Here is my plan to improve the code.".to_string(),
                    ));
                }
                _ => {
                    // Guard retry: accept unconditionally
                    emit(ProviderEvent::TokenDelta(
                        "Actually, no changes needed.".to_string(),
                    ));
                }
            }
            Ok(())
        }
    }

    let provider = GuardFireProvider {
        seen_requests: seen_requests.clone(),
    };

    let _frames = app
        .run_live_turn("improve the code", &provider, &tui)
        .expect("guard fire scenario should succeed");

    let requests = seen_requests.borrow();
    // Issue #173: First response has ANVIL_TOOL(file.read) + ANVIL_FINAL.
    // The tool executes, then anvil_final_seen=true causes immediate loop termination.
    // No follow-up LLM call and no guard retry.
    assert_eq!(
        requests.len(),
        1,
        "expected 1 provider call: ANVIL_FINAL terminates after tool execution (Issue #173)"
    );
}

#[test]
fn anvil_final_guard_does_not_fire_when_file_write_was_executed() {
    // Scenario: LLM writes a file, then outputs ANVIL_FINAL.
    // Guard should NOT fire because touched_files is non-empty.
    let root = common::unique_test_dir("guard_skip");
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

    let seen_requests = Rc::new(RefCell::new(Vec::new()));

    let provider = RecordingProvider {
        seen_requests: seen_requests.clone(),
        events: vec![ProviderEvent::Agent(AgentEvent::Done {
            status: "Done. session saved".to_string(),
            assistant_message: concat!(
                "```ANVIL_PLAN\n",
                "- [ ] output.txt: create file\n",
                "```\n",
                "```ANVIL_TOOL\n",
                "{\"id\":\"call_001\",\"tool\":\"file.write\",\"path\":\"./output.txt\",\"content\":\"hello world\"}\n",
                "```\n",
                "```ANVIL_FINAL\n",
                "Created output.txt with the requested content.\n",
                "```\n"
            )
            .to_string(),
            completion_summary: "turn 1".to_string(),
            saved_status: "session saved".to_string(),
            tool_logs: Vec::new(),
            elapsed_ms: 0,
            inference_performance: None,
            tool_calls: None,
            assistant_tool_call_records: None,
        })],
        followup_events: vec![ProviderEvent::TokenDelta(
            "All done.".to_string(),
        )],
        error: None,
    };

    let _frames = app
        .run_live_turn("create a file", &provider, &tui)
        .expect("file write scenario should succeed");

    let requests = seen_requests.borrow();
    // Issue #173: First response has ANVIL_TOOL(file.write) + ANVIL_FINAL.
    // The tool executes (file is written), then anvil_final_seen=true causes
    // immediate loop termination. No follow-up LLM call.
    assert_eq!(
        requests.len(),
        1,
        "expected 1 provider call: ANVIL_FINAL terminates after tool execution (Issue #173)"
    );

    // Verify guard retry message was NOT injected
    assert!(
        !app.session()
            .messages
            .iter()
            .any(|m| m.content.contains("No file modifications detected")),
        "guard retry message should NOT be in session when file was written"
    );

    // Verify the file was actually written
    let content =
        std::fs::read_to_string(root.join("output.txt")).expect("output.txt should exist");
    assert!(content.contains("hello world"));
}

#[test]
fn synthetic_guidance_followup_without_edits_triggers_final_guard_retry() {
    let root = common::unique_test_dir("guidance_retry");
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

    std::fs::create_dir_all(root.join("src")).expect("create src dir");
    for i in 0..8 {
        std::fs::write(
            root.join(format!("src/file_{i}.rs")),
            format!("fn f{i}() {{}}\n"),
        )
        .expect("write fixture file");
    }

    let seen_requests = Rc::new(RefCell::new(Vec::new()));

    struct GuidanceRetryProvider {
        seen_requests: Rc<RefCell<Vec<ProviderTurnRequest>>>,
    }

    impl ProviderClient for GuidanceRetryProvider {
        fn stream_turn(
            &self,
            request: &ProviderTurnRequest,
            emit: &mut dyn FnMut(ProviderEvent),
        ) -> Result<(), ProviderTurnError> {
            let call_index = self.seen_requests.borrow().len();
            self.seen_requests.borrow_mut().push(request.clone());

            match call_index {
                0 => {
                    let mut assistant_message = String::new();
                    for i in 0..8 {
                        assistant_message.push_str("```ANVIL_TOOL\n");
                        assistant_message.push_str(&format!(
                            "{{\"id\":\"call_{i:03}\",\"tool\":\"file.read\",\"path\":\"./src/file_{i}.rs\"}}\n"
                        ));
                        assistant_message.push_str("```\n");
                    }
                    assistant_message.push_str("```ANVIL_FINAL\n");
                    assistant_message.push_str("Finished reading the repository.\n");
                    assistant_message.push_str("```\n");

                    emit(ProviderEvent::Agent(AgentEvent::Done {
                        status: "Done. session saved".to_string(),
                        assistant_message,
                        completion_summary: "turn 1".to_string(),
                        saved_status: "session saved".to_string(),
                        tool_logs: Vec::new(),
                        elapsed_ms: 0,
                        inference_performance: None,
                        tool_calls: None,
                        assistant_tool_call_records: None,
                    }));
                }
                _ => {
                    emit(ProviderEvent::TokenDelta(
                        "I will now implement the change.".to_string(),
                    ));
                }
            }
            Ok(())
        }
    }

    let provider = GuidanceRetryProvider {
        seen_requests: seen_requests.clone(),
    };

    let _frames = app
        .run_live_turn("implement the feature", &provider, &tui)
        .expect("guidance retry scenario should succeed");

    let requests = seen_requests.borrow();
    assert_eq!(
        requests.len(),
        5,
        "expected 5 provider calls: initial turn + guidance follow-up + final guard retry + no-plan suppression retry + task-semantics gate retry (Issue #382)"
    );
    assert!(
        app.session()
            .messages
            .iter()
            .any(|m| m.role == anvil::session::MessageRole::Tool && m.author == "system.read_guard"),
        "system.read_guard should be recorded in the session"
    );
    assert!(
        app.session()
            .messages
            .iter()
            .any(|m| m.content.contains("No file modifications detected")),
        "guidance follow-up with no edits should escalate to the final guard retry"
    );
}

#[test]
fn live_turn_pins_latest_user_task_into_working_memory_prompt() {
    let mut app = common::build_app();
    let tui = Tui::new();
    let seen_requests = Rc::new(RefCell::new(Vec::new()));
    let provider = RecordingProvider {
        seen_requests: seen_requests.clone(),
        events: vec![ProviderEvent::Agent(AgentEvent::Done {
            status: "Done. session saved".to_string(),
            assistant_message: "done".to_string(),
            completion_summary: "done".to_string(),
            saved_status: "session saved".to_string(),
            tool_logs: Vec::new(),
            elapsed_ms: 0,
            inference_performance: None,
            tool_calls: None,
            assistant_tool_call_records: None,
        })],
        followup_events: Vec::new(),
        error: None,
    };

    let task = "fix the read-to-edit transition";
    let _ = app
        .run_live_turn(task, &provider, &tui)
        .expect("turn should succeed");

    assert_eq!(
        app.session().working_memory.active_task.as_deref(),
        Some(task)
    );

    let requests = seen_requests.borrow();
    let system_prompt = &requests[0].messages[0].content;
    assert!(
        system_prompt.contains("**Active task:** fix the read-to-edit transition"),
        "system prompt should pin the latest user task for follow-up turns"
    );
}

#[test]
fn anvil_final_guard_handle_structured_done_fires_for_plan_only_response() {
    // Scenario: Done event with ANVIL_FINAL but no tool calls (plan only).
    // Guard should fire via handle_structured_done path.
    let root = common::unique_test_dir("guard_done_path");
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

    let seen_requests = Rc::new(RefCell::new(Vec::new()));

    struct GuardDoneProvider {
        seen_requests: Rc<RefCell<Vec<ProviderTurnRequest>>>,
    }

    impl ProviderClient for GuardDoneProvider {
        fn stream_turn(
            &self,
            request: &ProviderTurnRequest,
            emit: &mut dyn FnMut(ProviderEvent),
        ) -> Result<(), ProviderTurnError> {
            let call_index = self.seen_requests.borrow().len();
            self.seen_requests.borrow_mut().push(request.clone());

            match call_index {
                0 => {
                    // Done event with ANVIL_FINAL but NO tool calls (plan-only)
                    emit(ProviderEvent::Agent(AgentEvent::Done {
                        status: "Done. session saved".to_string(),
                        assistant_message: concat!(
                            "```ANVIL_FINAL\n",
                            "Here is my plan:\n1. Create a new module\n2. Add tests\n",
                            "```\n"
                        )
                        .to_string(),
                        completion_summary: "plan only".to_string(),
                        saved_status: "session saved".to_string(),
                        tool_logs: Vec::new(),
                        elapsed_ms: 100,
                        inference_performance: None,
                        tool_calls: None,
                        assistant_tool_call_records: None,
                    }));
                }
                _ => {
                    // Guard retry: LLM responds with plain text
                    emit(ProviderEvent::TokenDelta(
                        "I apologize, let me implement the changes now.".to_string(),
                    ));
                }
            }
            Ok(())
        }
    }

    let provider = GuardDoneProvider {
        seen_requests: seen_requests.clone(),
    };

    let _frames = app
        .run_live_turn("implement the feature", &provider, &tui)
        .expect("guard done path should succeed");

    let requests = seen_requests.borrow();
    // Initial call + guard retry = 2 calls
    assert_eq!(
        requests.len(),
        2,
        "expected 2 provider calls: initial + guard retry via handle_structured_done"
    );

    // Issue #253: Plan gate fires before final guard, injecting plan-required message
    assert!(
        app.session()
            .messages
            .iter()
            .any(|m| m.content.contains("ANVIL_PLAN")
                || m.content.contains("No file modifications detected")),
        "plan-required or guard retry message should be in session"
    );
}

#[test]
fn implementation_task_with_read_only_followup_does_not_stop_without_plan() {
    let root = common::unique_test_dir("read_only_followup_requires_plan");
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

    let seen_requests = Rc::new(RefCell::new(Vec::new()));

    struct ReadOnlyFollowupProvider {
        seen_requests: Rc<RefCell<Vec<ProviderTurnRequest>>>,
    }

    impl ProviderClient for ReadOnlyFollowupProvider {
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
                            "{\"id\":\"call_001\",\"tool\":\"file.read\",\"path\":\".\"}\n",
                            "```\n"
                        )
                        .to_string(),
                        completion_summary: "read root".to_string(),
                        saved_status: "session saved".to_string(),
                        tool_logs: Vec::new(),
                        elapsed_ms: 0,
                        inference_performance: None,
                        tool_calls: None,
                        assistant_tool_call_records: None,
                    }));
                }
                1 => {
                    emit(ProviderEvent::TokenDelta(
                        "現在のディレクトリには .anvil だけがあります。新しい Next.js アプリを作成する必要があります。"
                            .to_string(),
                    ));
                }
                _ => {
                    emit(ProviderEvent::TokenDelta(
                        concat!(
                            "```ANVIL_PLAN\n",
                            "- [ ] next.config.ts: configure base path\n",
                            "```\n",
                            "```ANVIL_TOOL\n",
                            "{\"id\":\"call_002\",\"tool\":\"file.write\",\"path\":\"./next.config.ts\",\"content\":\"export default {}\\n\"}\n",
                            "```\n",
                            "```ANVIL_FINAL\n",
                            "Created the initial Next.js config.\n",
                            "```\n"
                        )
                        .to_string(),
                    ));
                }
            }
            Ok(())
        }
    }

    let provider = ReadOnlyFollowupProvider {
        seen_requests: seen_requests.clone(),
    };

    let _frames = app
        .run_live_turn(
            "あなたが考える最高に面白くカッコ良いスペースインベーダーゲームをhttp://localhost:3012/proxy/localllmtest で利用可能なnext.jsアプリとして開発してください。",
            &provider,
            &tui,
        )
        .expect("read-only follow-up scenario should continue");

    let requests = seen_requests.borrow();
    assert_eq!(
        requests.len(),
        3,
        "expected initial read, suppressed read-only follow-up, and implementation retry"
    );
    assert!(
        root.join("next.config.ts").exists(),
        "implementation retry should write a planned file"
    );
}

#[test]
fn shell_exec_mutation_followup_does_not_stop_after_partial_progress() {
    let root = common::unique_test_dir("shell_exec_partial_progress_retry");
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

    let seen_requests = Rc::new(RefCell::new(Vec::new()));

    struct ShellMutationFollowupProvider {
        seen_requests: Rc<RefCell<Vec<ProviderTurnRequest>>>,
    }

    impl ProviderClient for ShellMutationFollowupProvider {
        fn stream_turn(
            &self,
            request: &ProviderTurnRequest,
            emit: &mut dyn FnMut(ProviderEvent),
        ) -> Result<(), ProviderTurnError> {
            let call_index = self.seen_requests.borrow().len();
            self.seen_requests.borrow_mut().push(request.clone());

            match call_index {
                0 => emit(ProviderEvent::Agent(AgentEvent::Done {
                    status: "Done. session saved".to_string(),
                    assistant_message: concat!(
                        "```ANVIL_TOOL\n",
                        "{\"id\":\"call_001\",\"tool\":\"shell.exec\",\"command\":\"mkdir -p app/src && printf 'ok\\\\n' > app/package.json\"}\n",
                        "```\n"
                    )
                    .to_string(),
                    completion_summary: "bootstrap".to_string(),
                    saved_status: "session saved".to_string(),
                    tool_logs: Vec::new(),
                    elapsed_ms: 0,
                    inference_performance: None,
                    tool_calls: None,
                    assistant_tool_call_records: None,
                })),
                1 => emit(ProviderEvent::TokenDelta(
                    "Next.js app scaffold is ready. 次にゲーム実装を進めます。".to_string(),
                )),
                _ => emit(ProviderEvent::TokenDelta(
                    concat!(
                        "```ANVIL_PLAN\n",
                        "- [ ] app/src/game.ts: implement the first game module\n",
                        "```\n",
                        "```ANVIL_TOOL\n",
                        "{\"id\":\"call_002\",\"tool\":\"file.write\",\"path\":\"./app/src/game.ts\",\"content\":\"export const ready = true;\\n\"}\n",
                        "```\n",
                        "```ANVIL_FINAL\n",
                        "Implemented the first game module.\n",
                        "```\n"
                    )
                    .to_string(),
                )),
            }
            Ok(())
        }
    }

    let provider = ShellMutationFollowupProvider {
        seen_requests: seen_requests.clone(),
    };

    let _frames = app
        .run_live_turn(
            "あなたが考える最高に面白くかっこいいスペースインベーダーゲームを3011ポートで起動可能なnext.jsアプリとして開発してください。",
            &provider,
            &tui,
        )
        .expect("shell mutation partial progress scenario should continue");

    let requests = seen_requests.borrow();
    assert_eq!(
        requests.len(),
        3,
        "expected initial shell mutation, suppressed prose-only follow-up, and implementation retry"
    );
    assert!(
        root.join("app/package.json").exists(),
        "shell.exec should create scaffold files"
    );
    assert!(
        root.join("app/src/game.ts").exists(),
        "follow-up retry should continue into file mutation"
    );
}

#[test]
fn local_bootstrap_lock_ignores_followup_plan_restatement_and_continues_to_mutation() {
    let root = common::unique_test_dir("local_bootstrap_lock_followup_plan");
    fs::create_dir_all(&root).expect("test root should be created");
    let mut config = common::build_config_in(root.clone());
    config.mode.approval_required = false;
    config.runtime.provider = "ollama".to_string();
    let provider_ctx =
        anvil::provider::ProviderRuntimeContext::bootstrap(&config).expect("provider bootstrap");
    let mut app = anvil::app::App::new(config, provider_ctx, Arc::new(AtomicBool::new(false)))
        .expect("app should initialize");
    let tui = Tui::new();

    #[derive(Clone)]
    struct BootstrapLockProvider {
        seen_requests: Rc<RefCell<Vec<ProviderTurnRequest>>>,
    }

    impl ProviderClient for BootstrapLockProvider {
        fn stream_turn(
            &self,
            request: &ProviderTurnRequest,
            emit: &mut dyn FnMut(ProviderEvent),
        ) -> Result<(), ProviderTurnError> {
            let call_index = self.seen_requests.borrow().len();
            self.seen_requests.borrow_mut().push(request.clone());

            match call_index {
                0 => emit(ProviderEvent::Agent(AgentEvent::Done {
                    status: "Done. session saved".to_string(),
                    assistant_message: "まず現在のディレクトリを確認します。".to_string(),
                    completion_summary: "turn 1".to_string(),
                    saved_status: "session saved".to_string(),
                    tool_logs: Vec::new(),
                    elapsed_ms: 0,
                    inference_performance: None,
                    tool_calls: Some(vec![ToolCallRequest {
                        tool_call_id: "call_file_read_0".to_string(),
                        tool_name: "file.read".to_string(),
                        input: ToolInput::FileRead {
                            path: ".".to_string(),
                        },
                        extra_field_warnings: Vec::new(),
                    }]),
                    assistant_tool_call_records: Some(vec![AssistantToolCallRecord {
                        id: "call_file_read_0".to_string(),
                        function_name: "file_read".to_string(),
                        arguments: "{\"path\":\".\"}".to_string(),
                    }]),
                })),
                1 => emit(ProviderEvent::Agent(AgentEvent::Done {
                    status: "Done. session saved".to_string(),
                    assistant_message: "土台を作成します。".to_string(),
                    completion_summary: "turn 2".to_string(),
                    saved_status: "session saved".to_string(),
                    tool_logs: Vec::new(),
                    elapsed_ms: 0,
                    inference_performance: None,
                    tool_calls: Some(vec![ToolCallRequest {
                        tool_call_id: "call_shell_exec_0".to_string(),
                        tool_name: "shell.exec".to_string(),
                        input: ToolInput::ShellExec {
                            command: "mkdir -p ./space-invaders/src/app && printf '{\"name\":\"space-invaders\"}\\n' > ./space-invaders/package.json && printf 'export default function Home() { return null; }\\n' > ./space-invaders/src/app/page.tsx".to_string(),
                        },
                        extra_field_warnings: Vec::new(),
                    }]),
                    assistant_tool_call_records: Some(vec![AssistantToolCallRecord {
                        id: "call_shell_exec_0".to_string(),
                        function_name: "shell_exec".to_string(),
                        arguments: "{\"command\":\"mkdir -p ./space-invaders/src/app && printf '{\\\"name\\\":\\\"space-invaders\\\"}\\\\n' > ./space-invaders/package.json && printf 'export default function Home() { return null; }\\\\n' > ./space-invaders/src/app/page.tsx\"}".to_string(),
                    }]),
                })),
                2 => emit(ProviderEvent::Agent(AgentEvent::Done {
                    status: "Done. session saved".to_string(),
                    assistant_message: "構造を確認します。".to_string(),
                    completion_summary: "turn 3".to_string(),
                    saved_status: "session saved".to_string(),
                    tool_logs: Vec::new(),
                    elapsed_ms: 0,
                    inference_performance: None,
                    tool_calls: Some(vec![ToolCallRequest {
                        tool_call_id: "call_file_read_1".to_string(),
                        tool_name: "file.read".to_string(),
                        input: ToolInput::FileRead {
                            path: "space-invaders/src/app/page.tsx".to_string(),
                        },
                        extra_field_warnings: Vec::new(),
                    }]),
                    assistant_tool_call_records: Some(vec![AssistantToolCallRecord {
                        id: "call_file_read_1".to_string(),
                        function_name: "file_read".to_string(),
                        arguments: "{\"path\":\"space-invaders/src/app/page.tsx\"}".to_string(),
                    }]),
                })),
                3 => emit(ProviderEvent::Agent(AgentEvent::Done {
                    status: "Done. session saved".to_string(),
                    assistant_message:
                        "```ANVIL_PLAN\n- [ ] space-invaders/src/app/page.tsx: ゲーム画面を実装\n```"
                            .to_string(),
                    completion_summary: "turn 4".to_string(),
                    saved_status: "session saved".to_string(),
                    tool_logs: Vec::new(),
                    elapsed_ms: 0,
                    inference_performance: None,
                    tool_calls: None,
                    assistant_tool_call_records: None,
                })),
                4 => emit(ProviderEvent::Agent(AgentEvent::Done {
                    status: "Done. session saved".to_string(),
                    assistant_message: "実装を開始します。".to_string(),
                    completion_summary: "turn 5".to_string(),
                    saved_status: "session saved".to_string(),
                    tool_logs: Vec::new(),
                    elapsed_ms: 0,
                    inference_performance: None,
                    tool_calls: Some(vec![ToolCallRequest {
                        tool_call_id: "call_file_write_0".to_string(),
                        tool_name: "file.write".to_string(),
                        input: ToolInput::FileWrite {
                            path: "space-invaders/src/app/page.tsx".to_string(),
                            content: "export default function Home() {\n  return <main>space invaders</main>;\n}\n".to_string(),
                        },
                        extra_field_warnings: Vec::new(),
                    }]),
                    assistant_tool_call_records: Some(vec![AssistantToolCallRecord {
                        id: "call_file_write_0".to_string(),
                        function_name: "file_write".to_string(),
                        arguments: "{\"path\":\"space-invaders/src/app/page.tsx\",\"content\":\"export default function Home() {\\n  return <main>space invaders</main>;\\n}\\n\"}".to_string(),
                    }]),
                })),
                _ => emit(ProviderEvent::Agent(AgentEvent::Done {
                    status: "Done. session saved".to_string(),
                    assistant_message: "完了しました。".to_string(),
                    completion_summary: "turn 6".to_string(),
                    saved_status: "session saved".to_string(),
                    tool_logs: Vec::new(),
                    elapsed_ms: 0,
                    inference_performance: None,
                    tool_calls: None,
                    assistant_tool_call_records: None,
                })),
            }
            Ok(())
        }
    }

    let seen_requests = Rc::new(RefCell::new(Vec::new()));
    let provider = BootstrapLockProvider {
        seen_requests: seen_requests.clone(),
    };

    app.run_live_turn("create the project", &provider, &tui)
        .expect("bootstrap lock should continue into mutation");

    let page = fs::read_to_string(root.join("space-invaders/src/app/page.tsx"))
        .expect("page should be written after plan restatement drift");
    assert!(page.contains("space invaders"));

    let requests = seen_requests.borrow();
    assert!(
        requests.len() >= 5,
        "expected an additional retry request after follow-up ANVIL_PLAN drift"
    );
    let retry_request = &requests[4];
    assert!(
        retry_request.messages.iter().any(|msg| {
            msg.role == ProviderMessageRole::Tool
                && msg.content.contains("Do not output another plan")
        }),
        "local bootstrap lock should inject a short mutation-first retry hint"
    );
}

#[test]
fn local_bootstrap_lock_retries_multiple_plan_restatements_before_mutation() {
    let root = common::unique_test_dir("local_bootstrap_lock_multiple_replans");
    fs::create_dir_all(&root).expect("test root should be created");
    let mut config = common::build_config_in(root.clone());
    config.mode.approval_required = false;
    config.runtime.provider = "ollama".to_string();
    let provider_ctx =
        anvil::provider::ProviderRuntimeContext::bootstrap(&config).expect("provider bootstrap");
    let mut app = anvil::app::App::new(config, provider_ctx, Arc::new(AtomicBool::new(false)))
        .expect("app should initialize");
    let tui = Tui::new();

    #[derive(Clone)]
    struct BootstrapLockMultiPlanProvider {
        seen_requests: Rc<RefCell<Vec<ProviderTurnRequest>>>,
    }

    impl ProviderClient for BootstrapLockMultiPlanProvider {
        fn stream_turn(
            &self,
            request: &ProviderTurnRequest,
            emit: &mut dyn FnMut(ProviderEvent),
        ) -> Result<(), ProviderTurnError> {
            let call_index = self.seen_requests.borrow().len();
            self.seen_requests.borrow_mut().push(request.clone());

            match call_index {
                0 => emit(ProviderEvent::Agent(AgentEvent::Done {
                    status: "Done. session saved".to_string(),
                    assistant_message: "まず現在のディレクトリを確認します。".to_string(),
                    completion_summary: "turn 1".to_string(),
                    saved_status: "session saved".to_string(),
                    tool_logs: Vec::new(),
                    elapsed_ms: 0,
                    inference_performance: None,
                    tool_calls: Some(vec![ToolCallRequest {
                        tool_call_id: "call_file_read_0".to_string(),
                        tool_name: "file.read".to_string(),
                        input: ToolInput::FileRead {
                            path: ".".to_string(),
                        },
                        extra_field_warnings: Vec::new(),
                    }]),
                    assistant_tool_call_records: Some(vec![AssistantToolCallRecord {
                        id: "call_file_read_0".to_string(),
                        function_name: "file_read".to_string(),
                        arguments: "{\"path\":\".\"}".to_string(),
                    }]),
                })),
                1 => emit(ProviderEvent::Agent(AgentEvent::Done {
                    status: "Done. session saved".to_string(),
                    assistant_message: "土台を作成します。".to_string(),
                    completion_summary: "turn 2".to_string(),
                    saved_status: "session saved".to_string(),
                    tool_logs: Vec::new(),
                    elapsed_ms: 0,
                    inference_performance: None,
                    tool_calls: Some(vec![ToolCallRequest {
                        tool_call_id: "call_shell_exec_0".to_string(),
                        tool_name: "shell.exec".to_string(),
                        input: ToolInput::ShellExec {
                            command: "mkdir -p ./space-invaders/src/app && printf '{\"name\":\"space-invaders\"}\\n' > ./space-invaders/package.json && printf 'export default function Home() { return null; }\\n' > ./space-invaders/src/app/page.tsx && printf '@import \"tailwindcss\";\\n' > ./space-invaders/src/app/globals.css".to_string(),
                        },
                        extra_field_warnings: Vec::new(),
                    }]),
                    assistant_tool_call_records: Some(vec![AssistantToolCallRecord {
                        id: "call_shell_exec_0".to_string(),
                        function_name: "shell_exec".to_string(),
                        arguments: "{\"command\":\"mkdir -p ./space-invaders/src/app && printf '{\\\"name\\\":\\\"space-invaders\\\"}\\\\n' > ./space-invaders/package.json && printf 'export default function Home() { return null; }\\\\n' > ./space-invaders/src/app/page.tsx && printf '@import \\\"tailwindcss\\\";\\\\n' > ./space-invaders/src/app/globals.css\"}".to_string(),
                    }]),
                })),
                2 => emit(ProviderEvent::Agent(AgentEvent::Done {
                    status: "Done. session saved".to_string(),
                    assistant_message: "構造を確認します。".to_string(),
                    completion_summary: "turn 3".to_string(),
                    saved_status: "session saved".to_string(),
                    tool_logs: Vec::new(),
                    elapsed_ms: 0,
                    inference_performance: None,
                    tool_calls: Some(vec![
                        ToolCallRequest {
                            tool_call_id: "call_file_read_1".to_string(),
                            tool_name: "file.read".to_string(),
                            input: ToolInput::FileRead {
                                path: "space-invaders/src/app/page.tsx".to_string(),
                            },
                            extra_field_warnings: Vec::new(),
                        },
                        ToolCallRequest {
                            tool_call_id: "call_file_read_2".to_string(),
                            tool_name: "file.read".to_string(),
                            input: ToolInput::FileRead {
                                path: "space-invaders/src/app/globals.css".to_string(),
                            },
                            extra_field_warnings: Vec::new(),
                        },
                    ]),
                    assistant_tool_call_records: Some(vec![
                        AssistantToolCallRecord {
                            id: "call_file_read_1".to_string(),
                            function_name: "file_read".to_string(),
                            arguments: "{\"path\":\"space-invaders/src/app/page.tsx\"}"
                                .to_string(),
                        },
                        AssistantToolCallRecord {
                            id: "call_file_read_2".to_string(),
                            function_name: "file_read".to_string(),
                            arguments: "{\"path\":\"space-invaders/src/app/globals.css\"}"
                                .to_string(),
                        },
                    ]),
                })),
                3 => emit(ProviderEvent::Agent(AgentEvent::Done {
                    status: "Done. session saved".to_string(),
                    assistant_message:
                        "```ANVIL_PLAN\n- [ ] src/app/page.tsx: ゲーム画面を実装\n```"
                            .to_string(),
                    completion_summary: "turn 4".to_string(),
                    saved_status: "session saved".to_string(),
                    tool_logs: Vec::new(),
                    elapsed_ms: 0,
                    inference_performance: None,
                    tool_calls: None,
                    assistant_tool_call_records: None,
                })),
                4 => emit(ProviderEvent::Agent(AgentEvent::Done {
                    status: "Done. session saved".to_string(),
                    assistant_message:
                        "```ANVIL_PLAN\n- [ ] src/app/page.tsx: 宇宙戦闘画面を実装\n- [ ] src/app/globals.css: ネオン調スタイルを追加\n```\n"
                            .to_string(),
                    completion_summary: "turn 5".to_string(),
                    saved_status: "session saved".to_string(),
                    tool_logs: Vec::new(),
                    elapsed_ms: 0,
                    inference_performance: None,
                    tool_calls: None,
                    assistant_tool_call_records: None,
                })),
                5 => emit(ProviderEvent::Agent(AgentEvent::Done {
                    status: "Done. session saved".to_string(),
                    assistant_message: "実装を開始します。".to_string(),
                    completion_summary: "turn 6".to_string(),
                    saved_status: "session saved".to_string(),
                    tool_logs: Vec::new(),
                    elapsed_ms: 0,
                    inference_performance: None,
                    tool_calls: Some(vec![ToolCallRequest {
                        tool_call_id: "call_file_write_0".to_string(),
                        tool_name: "file.write".to_string(),
                        input: ToolInput::FileWrite {
                            path: "space-invaders/src/app/page.tsx".to_string(),
                            content: "export default function Home() {\n  return <main>space invaders</main>;\n}\n".to_string(),
                        },
                        extra_field_warnings: Vec::new(),
                    }]),
                    assistant_tool_call_records: Some(vec![AssistantToolCallRecord {
                        id: "call_file_write_0".to_string(),
                        function_name: "file_write".to_string(),
                        arguments: "{\"path\":\"space-invaders/src/app/page.tsx\",\"content\":\"export default function Home() {\\n  return <main>space invaders</main>;\\n}\\n\"}".to_string(),
                    }]),
                })),
                _ => emit(ProviderEvent::Agent(AgentEvent::Done {
                    status: "Done. session saved".to_string(),
                    assistant_message: "完了しました。".to_string(),
                    completion_summary: "turn 7".to_string(),
                    saved_status: "session saved".to_string(),
                    tool_logs: Vec::new(),
                    elapsed_ms: 0,
                    inference_performance: None,
                    tool_calls: None,
                    assistant_tool_call_records: None,
                })),
            }
            Ok(())
        }
    }

    let seen_requests = Rc::new(RefCell::new(Vec::new()));
    let provider = BootstrapLockMultiPlanProvider {
        seen_requests: seen_requests.clone(),
    };

    app.run_live_turn("create the project", &provider, &tui)
        .expect("bootstrap lock should survive repeated plan drift until mutation");

    let requests = seen_requests.borrow();
    let retry_count = requests
        .iter()
        .filter(|request| {
            request.messages.last().is_some_and(|msg| {
                msg.role == ProviderMessageRole::Tool
                    && msg.content.contains("Do not output another plan")
            })
        })
        .count();
    assert_eq!(
        retry_count, 2,
        "bootstrap lock should inject the short mutation-first retry twice before unlocking"
    );
}

#[test]
fn local_bootstrap_lock_allows_one_read_batch_then_forces_mutation() {
    let root = common::unique_test_dir("local_bootstrap_lock_read_drift");
    fs::create_dir_all(&root).expect("test root should be created");
    let mut config = common::build_config_in(root.clone());
    config.mode.approval_required = false;
    config.runtime.provider = "ollama".to_string();
    let provider_ctx =
        anvil::provider::ProviderRuntimeContext::bootstrap(&config).expect("provider bootstrap");
    let mut app = anvil::app::App::new(config, provider_ctx, Arc::new(AtomicBool::new(false)))
        .expect("app should initialize");
    let tui = Tui::new();

    #[derive(Clone)]
    struct BootstrapLockReadDriftProvider {
        seen_requests: Rc<RefCell<Vec<ProviderTurnRequest>>>,
    }

    impl ProviderClient for BootstrapLockReadDriftProvider {
        fn stream_turn(
            &self,
            request: &ProviderTurnRequest,
            emit: &mut dyn FnMut(ProviderEvent),
        ) -> Result<(), ProviderTurnError> {
            let call_index = self.seen_requests.borrow().len();
            self.seen_requests.borrow_mut().push(request.clone());

            match call_index {
                0 => emit(ProviderEvent::Agent(AgentEvent::Done {
                    status: "Done. session saved".to_string(),
                    assistant_message: "まず現在のディレクトリを確認します。".to_string(),
                    completion_summary: "turn 1".to_string(),
                    saved_status: "session saved".to_string(),
                    tool_logs: Vec::new(),
                    elapsed_ms: 0,
                    inference_performance: None,
                    tool_calls: Some(vec![ToolCallRequest {
                        tool_call_id: "call_file_read_root".to_string(),
                        tool_name: "file.read".to_string(),
                        input: ToolInput::FileRead {
                            path: ".".to_string(),
                        },
                        extra_field_warnings: Vec::new(),
                    }]),
                    assistant_tool_call_records: Some(vec![AssistantToolCallRecord {
                        id: "call_file_read_root".to_string(),
                        function_name: "file_read".to_string(),
                        arguments: "{\"path\":\".\"}".to_string(),
                    }]),
                })),
                1 => emit(ProviderEvent::Agent(AgentEvent::Done {
                    status: "Done. session saved".to_string(),
                    assistant_message: "土台を作成します。".to_string(),
                    completion_summary: "turn 2".to_string(),
                    saved_status: "session saved".to_string(),
                    tool_logs: Vec::new(),
                    elapsed_ms: 0,
                    inference_performance: None,
                    tool_calls: Some(vec![ToolCallRequest {
                        tool_call_id: "call_shell_exec_scaffold".to_string(),
                        tool_name: "shell.exec".to_string(),
                        input: ToolInput::ShellExec {
                            command: "mkdir -p ./space-invaders/src/app && printf '{\"name\":\"space-invaders\"}\\n' > ./space-invaders/package.json && printf 'export default function Home() { return null; }\\n' > ./space-invaders/src/app/page.tsx && printf '@import \"tailwindcss\";\\n' > ./space-invaders/src/app/globals.css && printf 'export default function Layout({ children }) { return children; }\\n' > ./space-invaders/src/app/layout.tsx".to_string(),
                        },
                        extra_field_warnings: Vec::new(),
                    }]),
                    assistant_tool_call_records: Some(vec![AssistantToolCallRecord {
                        id: "call_shell_exec_scaffold".to_string(),
                        function_name: "shell_exec".to_string(),
                        arguments: "{\"command\":\"mkdir -p ./space-invaders/src/app && printf '{\\\"name\\\":\\\"space-invaders\\\"}\\\\n' > ./space-invaders/package.json && printf 'export default function Home() { return null; }\\\\n' > ./space-invaders/src/app/page.tsx && printf '@import \\\"tailwindcss\\\";\\\\n' > ./space-invaders/src/app/globals.css && printf 'export default function Layout({ children }) { return children; }\\\\n' > ./space-invaders/src/app/layout.tsx\"}".to_string(),
                    }]),
                })),
                2 => emit(ProviderEvent::Agent(AgentEvent::Done {
                    status: "Done. session saved".to_string(),
                    assistant_message: "現在の雛形を確認します。".to_string(),
                    completion_summary: "turn 3".to_string(),
                    saved_status: "session saved".to_string(),
                    tool_logs: Vec::new(),
                    elapsed_ms: 0,
                    inference_performance: None,
                    tool_calls: Some(vec![
                        ToolCallRequest {
                            tool_call_id: "call_file_read_page".to_string(),
                            tool_name: "file.read".to_string(),
                            input: ToolInput::FileRead {
                                path: "space-invaders/src/app/page.tsx".to_string(),
                            },
                            extra_field_warnings: Vec::new(),
                        },
                        ToolCallRequest {
                            tool_call_id: "call_file_read_css".to_string(),
                            tool_name: "file.read".to_string(),
                            input: ToolInput::FileRead {
                                path: "space-invaders/src/app/globals.css".to_string(),
                            },
                            extra_field_warnings: Vec::new(),
                        },
                    ]),
                    assistant_tool_call_records: Some(vec![
                        AssistantToolCallRecord {
                            id: "call_file_read_page".to_string(),
                            function_name: "file_read".to_string(),
                            arguments: "{\"path\":\"space-invaders/src/app/page.tsx\"}"
                                .to_string(),
                        },
                        AssistantToolCallRecord {
                            id: "call_file_read_css".to_string(),
                            function_name: "file_read".to_string(),
                            arguments: "{\"path\":\"space-invaders/src/app/globals.css\"}"
                                .to_string(),
                        },
                    ]),
                })),
                3 => emit(ProviderEvent::Agent(AgentEvent::Done {
                    status: "Done. session saved".to_string(),
                    assistant_message: "さらに確認します。".to_string(),
                    completion_summary: "turn 4".to_string(),
                    saved_status: "session saved".to_string(),
                    tool_logs: Vec::new(),
                    elapsed_ms: 0,
                    inference_performance: None,
                    tool_calls: Some(vec![ToolCallRequest {
                        tool_call_id: "call_file_read_layout".to_string(),
                        tool_name: "file.read".to_string(),
                        input: ToolInput::FileRead {
                            path: "space-invaders/src/app/layout.tsx".to_string(),
                        },
                        extra_field_warnings: Vec::new(),
                    }]),
                    assistant_tool_call_records: Some(vec![AssistantToolCallRecord {
                        id: "call_file_read_layout".to_string(),
                        function_name: "file_read".to_string(),
                        arguments: "{\"path\":\"space-invaders/src/app/layout.tsx\"}"
                            .to_string(),
                    }]),
                })),
                4 => emit(ProviderEvent::Agent(AgentEvent::Done {
                    status: "Done. session saved".to_string(),
                    assistant_message: "実装に入ります。".to_string(),
                    completion_summary: "turn 5".to_string(),
                    saved_status: "session saved".to_string(),
                    tool_logs: Vec::new(),
                    elapsed_ms: 0,
                    inference_performance: None,
                    tool_calls: Some(vec![ToolCallRequest {
                        tool_call_id: "call_file_write_page".to_string(),
                        tool_name: "file.write".to_string(),
                        input: ToolInput::FileWrite {
                            path: "space-invaders/src/app/page.tsx".to_string(),
                            content: "export default function Home() {\n  return <main>space invaders</main>;\n}\n".to_string(),
                        },
                        extra_field_warnings: Vec::new(),
                    }]),
                    assistant_tool_call_records: Some(vec![AssistantToolCallRecord {
                        id: "call_file_write_page".to_string(),
                        function_name: "file_write".to_string(),
                        arguments: "{\"path\":\"space-invaders/src/app/page.tsx\",\"content\":\"export default function Home() {\\n  return <main>space invaders</main>;\\n}\\n\"}".to_string(),
                    }]),
                })),
                _ => emit(ProviderEvent::Agent(AgentEvent::Done {
                    status: "Done. session saved".to_string(),
                    assistant_message: "完了しました。".to_string(),
                    completion_summary: "turn 6".to_string(),
                    saved_status: "session saved".to_string(),
                    tool_logs: Vec::new(),
                    elapsed_ms: 0,
                    inference_performance: None,
                    tool_calls: None,
                    assistant_tool_call_records: None,
                })),
            }
            Ok(())
        }
    }

    let seen_requests = Rc::new(RefCell::new(Vec::new()));
    let provider = BootstrapLockReadDriftProvider {
        seen_requests: seen_requests.clone(),
    };

    app.run_live_turn("create the project", &provider, &tui)
        .expect("bootstrap lock should force mutation after read drift");

    let page = fs::read_to_string(root.join("space-invaders/src/app/page.tsx"))
        .expect("page should be written after read drift suppression");
    assert!(page.contains("space invaders"));

    let requests = seen_requests.borrow();
    assert!(
        requests.len() >= 5,
        "expected an extra retry request after repeated read drift"
    );
    let allowed_read_request = &requests[3];
    assert!(
        allowed_read_request.messages.iter().any(|msg| {
            msg.role == ProviderMessageRole::Tool
                && msg.content.contains("space-invaders/src/app/page.tsx")
        }),
        "the first read batch should be executed and returned to the model"
    );
    let retry_request = &requests[4];
    assert!(
        retry_request.messages.iter().any(|msg| {
            msg.role == ProviderMessageRole::Tool
                && msg.content.contains("Do not explain.")
                && msg.content.contains("Do not read more files.")
        }),
        "repeated read drift should inject the short mutation-first retry"
    );
    assert!(
        !retry_request.messages.iter().any(|msg| {
            msg.role == ProviderMessageRole::Tool
                && msg.content.contains("[tool result: file.read]")
                && msg.content.contains("space-invaders/src/app/layout.tsx")
        }),
        "suppressed read drift should not execute the extra layout.tsx read"
    );
}

#[test]
fn local_bootstrap_lock_treats_scaffold_only_progress_as_incomplete_until_mutation() {
    let root = common::unique_test_dir("local_bootstrap_lock_prose_drift");
    fs::create_dir_all(&root).expect("test root should be created");
    let mut config = common::build_config_in(root.clone());
    config.mode.approval_required = false;
    config.runtime.provider = "ollama".to_string();
    let provider_ctx =
        anvil::provider::ProviderRuntimeContext::bootstrap(&config).expect("provider bootstrap");
    let mut app = anvil::app::App::new(config, provider_ctx, Arc::new(AtomicBool::new(false)))
        .expect("app should initialize");
    let tui = Tui::new();

    #[derive(Clone)]
    struct BootstrapLockProseDriftProvider {
        seen_requests: Rc<RefCell<Vec<ProviderTurnRequest>>>,
    }

    impl ProviderClient for BootstrapLockProseDriftProvider {
        fn stream_turn(
            &self,
            request: &ProviderTurnRequest,
            emit: &mut dyn FnMut(ProviderEvent),
        ) -> Result<(), ProviderTurnError> {
            let call_index = self.seen_requests.borrow().len();
            self.seen_requests.borrow_mut().push(request.clone());

            match call_index {
                0 => emit(ProviderEvent::Agent(AgentEvent::Done {
                    status: "Done. session saved".to_string(),
                    assistant_message: "まず現在のディレクトリを確認します。".to_string(),
                    completion_summary: "turn 1".to_string(),
                    saved_status: "session saved".to_string(),
                    tool_logs: Vec::new(),
                    elapsed_ms: 0,
                    inference_performance: None,
                    tool_calls: Some(vec![ToolCallRequest {
                        tool_call_id: "call_file_read_root".to_string(),
                        tool_name: "file.read".to_string(),
                        input: ToolInput::FileRead {
                            path: ".".to_string(),
                        },
                        extra_field_warnings: Vec::new(),
                    }]),
                    assistant_tool_call_records: Some(vec![AssistantToolCallRecord {
                        id: "call_file_read_root".to_string(),
                        function_name: "file_read".to_string(),
                        arguments: "{\"path\":\".\"}".to_string(),
                    }]),
                })),
                1 => emit(ProviderEvent::Agent(AgentEvent::Done {
                    status: "Done. session saved".to_string(),
                    assistant_message: "土台を作成します。".to_string(),
                    completion_summary: "turn 2".to_string(),
                    saved_status: "session saved".to_string(),
                    tool_logs: Vec::new(),
                    elapsed_ms: 0,
                    inference_performance: None,
                    tool_calls: Some(vec![ToolCallRequest {
                        tool_call_id: "call_shell_exec_scaffold".to_string(),
                        tool_name: "shell.exec".to_string(),
                        input: ToolInput::ShellExec {
                            command: "mkdir -p ./space-invaders/src/app && printf '{\"name\":\"space-invaders\"}\\n' > ./space-invaders/package.json && printf 'export default function Home() { return null; }\\n' > ./space-invaders/src/app/page.tsx && printf '@import \"tailwindcss\";\\n' > ./space-invaders/src/app/globals.css".to_string(),
                        },
                        extra_field_warnings: Vec::new(),
                    }]),
                    assistant_tool_call_records: Some(vec![AssistantToolCallRecord {
                        id: "call_shell_exec_scaffold".to_string(),
                        function_name: "shell_exec".to_string(),
                        arguments: "{\"command\":\"mkdir -p ./space-invaders/src/app && printf '{\\\"name\\\":\\\"space-invaders\\\"}\\\\n' > ./space-invaders/package.json && printf 'export default function Home() { return null; }\\\\n' > ./space-invaders/src/app/page.tsx && printf '@import \\\"tailwindcss\\\";\\\\n' > ./space-invaders/src/app/globals.css\"}".to_string(),
                    }]),
                })),
                2 | 3 | 4 => emit(ProviderEvent::Agent(AgentEvent::Done {
                    status: "Done. session saved".to_string(),
                    assistant_message: "実装方針を整理しています。".to_string(),
                    completion_summary: format!("turn {}", call_index + 1),
                    saved_status: "session saved".to_string(),
                    tool_logs: Vec::new(),
                    elapsed_ms: 0,
                    inference_performance: None,
                    tool_calls: None,
                    assistant_tool_call_records: None,
                })),
                5 => emit(ProviderEvent::Agent(AgentEvent::Done {
                    status: "Done. session saved".to_string(),
                    assistant_message: "実装に入ります。".to_string(),
                    completion_summary: "turn 6".to_string(),
                    saved_status: "session saved".to_string(),
                    tool_logs: Vec::new(),
                    elapsed_ms: 0,
                    inference_performance: None,
                    tool_calls: Some(vec![ToolCallRequest {
                        tool_call_id: "call_file_write_page".to_string(),
                        tool_name: "file.write".to_string(),
                        input: ToolInput::FileWrite {
                            path: "space-invaders/src/app/page.tsx".to_string(),
                            content: "export default function Home() {\n  return <main>space invaders</main>;\n}\n".to_string(),
                        },
                        extra_field_warnings: Vec::new(),
                    }]),
                    assistant_tool_call_records: Some(vec![AssistantToolCallRecord {
                        id: "call_file_write_page".to_string(),
                        function_name: "file_write".to_string(),
                        arguments: "{\"path\":\"space-invaders/src/app/page.tsx\",\"content\":\"export default function Home() {\\n  return <main>space invaders</main>;\\n}\\n\"}".to_string(),
                    }]),
                })),
                _ => emit(ProviderEvent::Agent(AgentEvent::Done {
                    status: "Done. session saved".to_string(),
                    assistant_message: "完了しました。".to_string(),
                    completion_summary: "turn 7".to_string(),
                    saved_status: "session saved".to_string(),
                    tool_logs: Vec::new(),
                    elapsed_ms: 0,
                    inference_performance: None,
                    tool_calls: None,
                    assistant_tool_call_records: None,
                })),
            }
            Ok(())
        }
    }

    let seen_requests = Rc::new(RefCell::new(Vec::new()));
    let provider = BootstrapLockProseDriftProvider {
        seen_requests: seen_requests.clone(),
    };

    app.run_live_turn("create the project", &provider, &tui)
        .expect("scaffold-only progress should not allow prose-only completion");

    let page = fs::read_to_string(root.join("space-invaders/src/app/page.tsx"))
        .expect("page should be written after prose-only drift retries");
    assert!(page.contains("space invaders"));

    let requests = seen_requests.borrow();
    assert!(
        requests.len() >= 6,
        "expected retries to continue until a real mutation lands"
    );
    let task_semantics_retry = &requests[5];
    assert!(
        task_semantics_retry.messages.iter().any(|msg| {
            msg.role == ProviderMessageRole::Tool
                && msg.content.contains("TOOL FIRST")
                && msg.content.contains("Do not explain.")
                && msg.content.contains("Emit exactly one")
        }),
        "after local bootstrap retries are exhausted, scaffold-only progress should still be treated as incomplete"
    );
}

#[test]
fn local_mode_task_semantics_retry_is_tool_first() {
    let root = common::unique_test_dir("local_mode_task_semantics_retry");
    fs::create_dir_all(&root).expect("test root should be created");
    let mut config = common::build_config_in(root.clone());
    config.mode.approval_required = false;
    config.runtime.provider = "ollama".to_string();
    config.runtime.local_mode = true;
    let provider_ctx =
        anvil::provider::ProviderRuntimeContext::bootstrap(&config).expect("provider bootstrap");
    let mut app = anvil::app::App::new(config, provider_ctx, Arc::new(AtomicBool::new(false)))
        .expect("app should initialize");
    let tui = Tui::new();

    #[derive(Clone)]
    struct LocalModeTaskRetryProvider {
        seen_requests: Rc<RefCell<Vec<ProviderTurnRequest>>>,
    }

    impl ProviderClient for LocalModeTaskRetryProvider {
        fn stream_turn(
            &self,
            request: &ProviderTurnRequest,
            emit: &mut dyn FnMut(ProviderEvent),
        ) -> Result<(), ProviderTurnError> {
            let call_index = self.seen_requests.borrow().len();
            self.seen_requests.borrow_mut().push(request.clone());

            match call_index {
                0 => emit(ProviderEvent::Agent(AgentEvent::Done {
                    status: "Done. session saved".to_string(),
                    assistant_message: "現在の状態を確認します。".to_string(),
                    completion_summary: "turn 1".to_string(),
                    saved_status: "session saved".to_string(),
                    tool_logs: Vec::new(),
                    elapsed_ms: 0,
                    inference_performance: None,
                    tool_calls: Some(vec![ToolCallRequest {
                        tool_call_id: "call_file_read_root".to_string(),
                        tool_name: "file.read".to_string(),
                        input: ToolInput::FileRead {
                            path: ".".to_string(),
                        },
                        extra_field_warnings: Vec::new(),
                    }]),
                    assistant_tool_call_records: Some(vec![AssistantToolCallRecord {
                        id: "call_file_read_root".to_string(),
                        function_name: "file_read".to_string(),
                        arguments: "{\"path\":\".\"}".to_string(),
                    }]),
                })),
                1 => emit(ProviderEvent::Agent(AgentEvent::Done {
                    status: "Done. session saved".to_string(),
                    assistant_message: "実装方針を整理しています。".to_string(),
                    completion_summary: "turn 2".to_string(),
                    saved_status: "session saved".to_string(),
                    tool_logs: Vec::new(),
                    elapsed_ms: 0,
                    inference_performance: None,
                    tool_calls: None,
                    assistant_tool_call_records: None,
                })),
                2 => emit(ProviderEvent::Agent(AgentEvent::Done {
                    status: "Done. session saved".to_string(),
                    assistant_message: "書き込みます。".to_string(),
                    completion_summary: "turn 3".to_string(),
                    saved_status: "session saved".to_string(),
                    tool_logs: Vec::new(),
                    elapsed_ms: 0,
                    inference_performance: None,
                    tool_calls: Some(vec![ToolCallRequest {
                        tool_call_id: "call_file_write_page".to_string(),
                        tool_name: "file.write".to_string(),
                        input: ToolInput::FileWrite {
                            path: "README.md".to_string(),
                            content: "# local mode\n".to_string(),
                        },
                        extra_field_warnings: Vec::new(),
                    }]),
                    assistant_tool_call_records: Some(vec![AssistantToolCallRecord {
                        id: "call_file_write_page".to_string(),
                        function_name: "file_write".to_string(),
                        arguments: "{\"path\":\"README.md\",\"content\":\"# local mode\\n\"}"
                            .to_string(),
                    }]),
                })),
                _ => emit(ProviderEvent::Agent(AgentEvent::Done {
                    status: "Done. session saved".to_string(),
                    assistant_message: "完了しました。".to_string(),
                    completion_summary: "turn 4".to_string(),
                    saved_status: "session saved".to_string(),
                    tool_logs: Vec::new(),
                    elapsed_ms: 0,
                    inference_performance: None,
                    tool_calls: None,
                    assistant_tool_call_records: None,
                })),
            }
            Ok(())
        }
    }

    let seen_requests = Rc::new(RefCell::new(Vec::new()));
    let provider = LocalModeTaskRetryProvider {
        seen_requests: seen_requests.clone(),
    };

    app.run_live_turn("create a README file", &provider, &tui)
        .expect("local mode should retry with tool-first wording");

    let requests = seen_requests.borrow();
    assert!(
        requests.len() >= 3,
        "expected a retry request before mutation"
    );
    let retry_request = &requests[2];
    assert!(
        retry_request.messages.iter().any(|msg| {
            msg.role == ProviderMessageRole::Tool
                && msg.content.contains("TOOL FIRST")
                && msg.content.contains("Do not explain.")
                && msg.content.contains("Emit exactly one")
        }),
        "local-mode task-semantics retry should be short and tool-first"
    );
}

#[test]
fn local_mode_blocks_git_status_during_act() {
    let root = common::unique_test_dir("local_mode_blocks_git_status");
    fs::create_dir_all(&root).expect("test root should be created");
    let mut config = common::build_config_in(root.clone());
    config.mode.approval_required = false;
    config.runtime.provider = "ollama".to_string();
    config.runtime.local_mode = true;
    let provider_ctx =
        anvil::provider::ProviderRuntimeContext::bootstrap(&config).expect("provider bootstrap");
    let mut app = anvil::app::App::new(config, provider_ctx, Arc::new(AtomicBool::new(false)))
        .expect("app should initialize");
    let tui = Tui::new();

    #[derive(Clone)]
    struct LocalModeGitStatusProvider;

    impl ProviderClient for LocalModeGitStatusProvider {
        fn stream_turn(
            &self,
            _request: &ProviderTurnRequest,
            emit: &mut dyn FnMut(ProviderEvent),
        ) -> Result<(), ProviderTurnError> {
            emit(ProviderEvent::Agent(AgentEvent::Done {
                status: "Done. session saved".to_string(),
                assistant_message: "状態を確認します。".to_string(),
                completion_summary: "turn 1".to_string(),
                saved_status: "session saved".to_string(),
                tool_logs: Vec::new(),
                elapsed_ms: 0,
                inference_performance: None,
                tool_calls: Some(vec![ToolCallRequest {
                    tool_call_id: "call_git_status".to_string(),
                    tool_name: "git.status".to_string(),
                    input: ToolInput::GitStatus {},
                    extra_field_warnings: Vec::new(),
                }]),
                assistant_tool_call_records: Some(vec![AssistantToolCallRecord {
                    id: "call_git_status".to_string(),
                    function_name: "git_status".to_string(),
                    arguments: "{}".to_string(),
                }]),
            }));
            Ok(())
        }
    }

    app.run_live_turn("create the project", &LocalModeGitStatusProvider, &tui)
        .expect("local act discipline should reject git.status cleanly");

    let tool_result = app
        .session()
        .messages
        .iter()
        .find(|msg| msg.role == MessageRole::Tool && msg.content.contains("git.status"))
        .expect("a tool failure should be recorded");
    assert!(
        tool_result.content.contains("blocked in local act mode"),
        "git.status should be blocked during local act mode"
    );
}

#[test]
fn local_mode_blocks_read_only_shell_exploration_during_act() {
    let root = common::unique_test_dir("local_mode_blocks_shell_explore");
    fs::create_dir_all(&root).expect("test root should be created");
    let mut config = common::build_config_in(root.clone());
    config.mode.approval_required = false;
    config.runtime.provider = "ollama".to_string();
    config.runtime.local_mode = true;
    let provider_ctx =
        anvil::provider::ProviderRuntimeContext::bootstrap(&config).expect("provider bootstrap");
    let mut app = anvil::app::App::new(config, provider_ctx, Arc::new(AtomicBool::new(false)))
        .expect("app should initialize");
    let tui = Tui::new();

    #[derive(Clone)]
    struct LocalModeShellExploreProvider;

    impl ProviderClient for LocalModeShellExploreProvider {
        fn stream_turn(
            &self,
            _request: &ProviderTurnRequest,
            emit: &mut dyn FnMut(ProviderEvent),
        ) -> Result<(), ProviderTurnError> {
            emit(ProviderEvent::Agent(AgentEvent::Done {
                status: "Done. session saved".to_string(),
                assistant_message: "確認します。".to_string(),
                completion_summary: "turn 1".to_string(),
                saved_status: "session saved".to_string(),
                tool_logs: Vec::new(),
                elapsed_ms: 0,
                inference_performance: None,
                tool_calls: Some(vec![ToolCallRequest {
                    tool_call_id: "call_shell_ls".to_string(),
                    tool_name: "shell.exec".to_string(),
                    input: ToolInput::ShellExec {
                        command: "ls -la".to_string(),
                    },
                    extra_field_warnings: Vec::new(),
                }]),
                assistant_tool_call_records: Some(vec![AssistantToolCallRecord {
                    id: "call_shell_ls".to_string(),
                    function_name: "shell_exec".to_string(),
                    arguments: "{\"command\":\"ls -la\"}".to_string(),
                }]),
            }));
            Ok(())
        }
    }

    app.run_live_turn("create the project", &LocalModeShellExploreProvider, &tui)
        .expect("local act discipline should reject read-only shell exploration cleanly");

    let tool_result = app
        .session()
        .messages
        .iter()
        .find(|msg| msg.role == MessageRole::Tool && msg.content.contains("shell.exec"))
        .expect("a tool failure should be recorded");
    assert!(
        tool_result.content.contains("blocked in local act mode"),
        "read-only shell exploration should be blocked during local act mode"
    );
}

#[test]
fn local_mode_blocks_agent_plan_during_act() {
    let root = common::unique_test_dir("local_mode_blocks_agent_plan");
    fs::create_dir_all(&root).expect("test root should be created");
    let mut config = common::build_config_in(root.clone());
    config.mode.approval_required = false;
    config.runtime.provider = "ollama".to_string();
    config.runtime.local_mode = true;
    let provider_ctx =
        anvil::provider::ProviderRuntimeContext::bootstrap(&config).expect("provider bootstrap");
    let mut app = anvil::app::App::new(config, provider_ctx, Arc::new(AtomicBool::new(false)))
        .expect("app should initialize");
    let tui = Tui::new();

    #[derive(Clone)]
    struct LocalModeAgentPlanProvider;

    impl ProviderClient for LocalModeAgentPlanProvider {
        fn stream_turn(
            &self,
            _request: &ProviderTurnRequest,
            emit: &mut dyn FnMut(ProviderEvent),
        ) -> Result<(), ProviderTurnError> {
            emit(ProviderEvent::Agent(AgentEvent::Done {
                status: "Done. session saved".to_string(),
                assistant_message: "計画ツールを使います。".to_string(),
                completion_summary: "turn 1".to_string(),
                saved_status: "session saved".to_string(),
                tool_logs: Vec::new(),
                elapsed_ms: 0,
                inference_performance: None,
                tool_calls: Some(vec![ToolCallRequest {
                    tool_call_id: "call_agent_plan".to_string(),
                    tool_name: "agent.plan".to_string(),
                    input: ToolInput::AgentPlan {
                        prompt: "plan the implementation".to_string(),
                        scope: None,
                    },
                    extra_field_warnings: Vec::new(),
                }]),
                assistant_tool_call_records: Some(vec![AssistantToolCallRecord {
                    id: "call_agent_plan".to_string(),
                    function_name: "agent_plan".to_string(),
                    arguments: "{\"prompt\":\"plan the implementation\"}".to_string(),
                }]),
            }));
            Ok(())
        }
    }

    app.run_live_turn("create the project", &LocalModeAgentPlanProvider, &tui)
        .expect("local act discipline should reject agent.plan cleanly");

    let tool_result = app
        .session()
        .messages
        .iter()
        .find(|msg| msg.role == MessageRole::Tool && msg.content.contains("agent.plan"))
        .expect("a tool failure should be recorded");
    assert!(
        tool_result.content.contains("blocked in local act mode"),
        "agent.plan should be blocked during local act mode"
    );
}

#[test]
fn local_bootstrap_lock_blocks_shell_exec_after_scaffold() {
    let root = common::unique_test_dir("local_bootstrap_blocks_shell");
    fs::create_dir_all(&root).expect("test root should be created");
    let mut config = common::build_config_in(root.clone());
    config.mode.approval_required = false;
    config.runtime.provider = "ollama".to_string();
    config.runtime.local_mode = true;
    let provider_ctx =
        anvil::provider::ProviderRuntimeContext::bootstrap(&config).expect("provider bootstrap");
    let mut app = anvil::app::App::new(config, provider_ctx, Arc::new(AtomicBool::new(false)))
        .expect("app should initialize");
    let tui = Tui::new();

    let seen_requests = Rc::new(RefCell::new(Vec::new()));

    struct BootstrapShellProvider {
        seen_requests: Rc<RefCell<Vec<ProviderTurnRequest>>>,
    }

    impl ProviderClient for BootstrapShellProvider {
        fn stream_turn(
            &self,
            request: &ProviderTurnRequest,
            emit: &mut dyn FnMut(ProviderEvent),
        ) -> Result<(), ProviderTurnError> {
            let turn = self.seen_requests.borrow().len();
            self.seen_requests.borrow_mut().push(request.clone());

            match turn {
                0 => emit(ProviderEvent::Agent(AgentEvent::Done {
                    status: "Done. session saved".to_string(),
                    assistant_message: "まず Next.js プロジェクトを作成します。".to_string(),
                    completion_summary: "turn 1".to_string(),
                    saved_status: "session saved".to_string(),
                    tool_logs: Vec::new(),
                    elapsed_ms: 0,
                    inference_performance: None,
                    tool_calls: Some(vec![ToolCallRequest {
                        tool_call_id: "call_scaffold".to_string(),
                        tool_name: "shell.exec".to_string(),
                        input: ToolInput::ShellExec {
                            command: "mkdir -p space-invaders/src/app && printf '{}' > space-invaders/package.json && printf 'export default function Page() { return null; }\\n' > space-invaders/src/app/page.tsx && printf 'const nextConfig = {}\\nexport default nextConfig\\n' > space-invaders/next.config.ts".to_string(),
                        },
                        extra_field_warnings: Vec::new(),
                    }]),
                    assistant_tool_call_records: None,
                })),
                1 => emit(ProviderEvent::Agent(AgentEvent::Done {
                    status: "Done. session saved".to_string(),
                    assistant_message: "もう一度 shell で確認します。".to_string(),
                    completion_summary: "turn 2".to_string(),
                    saved_status: "session saved".to_string(),
                    tool_logs: Vec::new(),
                    elapsed_ms: 0,
                    inference_performance: None,
                    tool_calls: Some(vec![ToolCallRequest {
                        tool_call_id: "call_shell_ls".to_string(),
                        tool_name: "shell.exec".to_string(),
                        input: ToolInput::ShellExec {
                            command: "ls -la space-invaders/src/app".to_string(),
                        },
                        extra_field_warnings: Vec::new(),
                    }]),
                    assistant_tool_call_records: None,
                })),
                _ => emit(ProviderEvent::Agent(AgentEvent::Done {
                    status: "Done. session saved".to_string(),
                    assistant_message: "直接編集します。".to_string(),
                    completion_summary: "turn 3".to_string(),
                    saved_status: "session saved".to_string(),
                    tool_logs: Vec::new(),
                    elapsed_ms: 0,
                    inference_performance: None,
                    tool_calls: Some(vec![ToolCallRequest {
                        tool_call_id: "call_edit_page".to_string(),
                        tool_name: "file.edit".to_string(),
                        input: ToolInput::FileEdit {
                            path: "space-invaders/src/app/page.tsx".to_string(),
                            old_string: "export default function Page() { return null; }\n"
                                .to_string(),
                            new_string: "export default function Page() { return <main>Ready</main>; }\n"
                                .to_string(),
                        },
                        extra_field_warnings: Vec::new(),
                    }]),
                    assistant_tool_call_records: None,
                })),
            }
            Ok(())
        }
    }

    app.run_live_turn(
        "create the project",
        &BootstrapShellProvider {
            seen_requests: seen_requests.clone(),
        },
        &tui,
    )
    .expect("bootstrap lock should reject shell drift and continue");

    let tool_failure = app
        .session()
        .messages
        .iter()
        .find(|msg| {
            msg.role == MessageRole::Tool
                && msg
                    .content
                    .contains("shell.exec is blocked during local bootstrap act mode")
        })
        .expect("a blocked shell.exec should be recorded");
    assert!(
        tool_failure.content.contains("local bootstrap act mode"),
        "post-scaffold shell.exec should be blocked"
    );
    assert!(
        std::fs::read_to_string(root.join("space-invaders/src/app/page.tsx"))
            .expect("page.tsx should exist")
            .contains("Ready"),
        "bootstrap lock should continue through to a direct mutation"
    );
}

#[test]
fn file_write_followup_prose_retry_continues_to_next_planned_file() {
    let root = common::unique_test_dir("file_write_followup_retry");
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

    let seen_requests = Rc::new(RefCell::new(Vec::new()));

    struct FileWriteFollowupProvider {
        seen_requests: Rc<RefCell<Vec<ProviderTurnRequest>>>,
    }

    impl ProviderClient for FileWriteFollowupProvider {
        fn stream_turn(
            &self,
            request: &ProviderTurnRequest,
            emit: &mut dyn FnMut(ProviderEvent),
        ) -> Result<(), ProviderTurnError> {
            let call_index = self.seen_requests.borrow().len();
            self.seen_requests.borrow_mut().push(request.clone());

            match call_index {
                0 => emit(ProviderEvent::Agent(AgentEvent::Done {
                    status: "Done. session saved".to_string(),
                    assistant_message: concat!(
                        "```ANVIL_PLAN\n",
                        "- [ ] package.json: bootstrap the project\n",
                        "- [ ] tsconfig.json: add TypeScript configuration\n",
                        "```\n",
                        "```ANVIL_TOOL\n",
                        "{\"id\":\"call_001\",\"tool\":\"file.write\",\"path\":\"./package.json\",\"content\":\"{\\n  \\\"name\\\": \\\"space-invaders\\\"\\n}\\n\"}\n",
                        "```\n",
                        "```ANVIL_FINAL\n",
                        "Created package.json.\n",
                        "```\n"
                    )
                    .to_string(),
                    completion_summary: "write package".to_string(),
                    saved_status: "session saved".to_string(),
                    tool_logs: Vec::new(),
                    elapsed_ms: 0,
                    inference_performance: None,
                    tool_calls: None,
                    assistant_tool_call_records: None,
                })),
                1 => emit(ProviderEvent::TokenDelta(
                    "package.json が作成されました。次に tsconfig.json を作成します。".to_string(),
                )),
                _ => emit(ProviderEvent::TokenDelta(
                    concat!(
                        "```ANVIL_TOOL\n",
                        "{\"id\":\"call_002\",\"tool\":\"file.write\",\"path\":\"./tsconfig.json\",\"content\":\"{\\n  \\\"compilerOptions\\\": {\\n    \\\"target\\\": \\\"ES2020\\\"\\n  }\\n}\\n\"}\n",
                        "```\n",
                        "```ANVIL_FINAL\n",
                        "Created tsconfig.json.\n",
                        "```\n"
                    )
                    .to_string(),
                )),
            }
            Ok(())
        }
    }

    let provider = FileWriteFollowupProvider {
        seen_requests: seen_requests.clone(),
    };

    let _frames = app
        .run_live_turn(
            "あなたが考える最高に面白くかっこいいスペースインベーダーゲームを3011ポートで起動可能なnext.jsアプリとして開発してください。",
            &provider,
            &tui,
        )
        .expect("file write follow-up scenario should continue");

    let requests = seen_requests.borrow();
    assert_eq!(
        requests.len(),
        3,
        "expected initial write, suppressed prose-only follow-up, and next-file retry"
    );
    assert!(
        root.join("package.json").exists(),
        "initial planned file should be written"
    );
    assert!(
        root.join("tsconfig.json").exists(),
        "follow-up retry should continue to the next planned file"
    );
    assert!(
        requests[2]
            .messages
            .iter()
            .any(|m| m.content.contains("次の応答では file.write / file.edit / file.rewrite の ANVIL_TOOL を直接出力してください")),
        "strong tool-first plan guidance should be present in the retry request"
    );
}

#[test]
fn first_mutation_without_anvil_plan_is_allowed_in_local_mode_without_synthetic_plan() {
    let root = common::unique_test_dir("synthetic_plan_from_first_mutation");
    let mut config = common::build_config_in(root.clone());
    config.mode.approval_required = false;
    config.runtime.provider = "ollama".to_string();
    let provider_ctx =
        anvil::provider::ProviderRuntimeContext::bootstrap(&config).expect("provider bootstrap");
    let mut app = anvil::app::App::new(
        config,
        provider_ctx,
        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
    )
    .expect("app should initialize");
    let tui = Tui::new();

    let seen_requests = Rc::new(RefCell::new(Vec::new()));

    struct SyntheticPlanProvider {
        seen_requests: Rc<RefCell<Vec<ProviderTurnRequest>>>,
    }

    impl ProviderClient for SyntheticPlanProvider {
        fn stream_turn(
            &self,
            request: &ProviderTurnRequest,
            emit: &mut dyn FnMut(ProviderEvent),
        ) -> Result<(), ProviderTurnError> {
            self.seen_requests.borrow_mut().push(request.clone());
            emit(ProviderEvent::Agent(AgentEvent::Done {
                status: "Done. session saved".to_string(),
                assistant_message: concat!(
                    "```ANVIL_TOOL\n",
                    "{\"id\":\"call_001\",\"tool\":\"file.write\",\"path\":\"./package.json\",\"content\":\"{\\n  \\\"name\\\": \\\"space-invaders\\\"\\n}\\n\"}\n",
                    "```\n",
                    "```ANVIL_FINAL\n",
                    "Created package.json.\n",
                    "```\n"
                )
                .to_string(),
                completion_summary: "write package".to_string(),
                saved_status: "session saved".to_string(),
                tool_logs: Vec::new(),
                elapsed_ms: 0,
                inference_performance: None,
                tool_calls: None,
                assistant_tool_call_records: None,
            }));
            Ok(())
        }
    }

    let provider = SyntheticPlanProvider {
        seen_requests: seen_requests.clone(),
    };

    let _frames = app
        .run_live_turn(
            "あなたが考える最高に面白くかっこいいスペースインベーダーゲームを3011ポートで起動可能なnext.jsアプリとして開発してください。",
            &provider,
            &tui,
        )
        .expect("local mode should allow first mutation without requiring a plan");

    assert!(
        root.join("package.json").exists(),
        "first mutation should be executed even when ANVIL_PLAN was omitted"
    );
    assert!(
        !app.session().messages.iter().any(|m| m
            .content
            .contains("最初の変更対象から最小限の実行計画を自動補完しました")),
        "local mode should bypass synthetic plan insertion"
    );
}

#[test]
fn shell_exec_mutation_advances_plan_before_final_gate() {
    let root = common::unique_test_dir("shell_exec_plan_sync");
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

    let seen_requests = Rc::new(RefCell::new(Vec::new()));

    struct ShellPlanSyncProvider {
        seen_requests: Rc<RefCell<Vec<ProviderTurnRequest>>>,
    }

    impl ProviderClient for ShellPlanSyncProvider {
        fn stream_turn(
            &self,
            request: &ProviderTurnRequest,
            emit: &mut dyn FnMut(ProviderEvent),
        ) -> Result<(), ProviderTurnError> {
            let call_index = self.seen_requests.borrow().len();
            self.seen_requests.borrow_mut().push(request.clone());

            match call_index {
                0 => emit(ProviderEvent::Agent(AgentEvent::Done {
                    status: "Done. session saved".to_string(),
                    assistant_message: concat!(
                        "```ANVIL_PLAN\n",
                        "- [ ] app/package.json: bootstrap the Next.js app\n",
                        "```\n",
                        "```ANVIL_TOOL\n",
                        "{\"id\":\"call_001\",\"tool\":\"shell.exec\",\"command\":\"mkdir -p app && printf 'ok\\\\n' > app/package.json\"}\n",
                        "```\n"
                    )
                    .to_string(),
                    completion_summary: "bootstrapped".to_string(),
                    saved_status: "session saved".to_string(),
                    tool_logs: Vec::new(),
                    elapsed_ms: 0,
                    inference_performance: None,
                    tool_calls: None,
                    assistant_tool_call_records: None,
                })),
                1 => emit(ProviderEvent::Agent(AgentEvent::Done {
                    status: "Done. session saved".to_string(),
                    assistant_message: concat!(
                        "```ANVIL_FINAL\n",
                        "Bootstrap completed.\n",
                        "```\n"
                    )
                    .to_string(),
                    completion_summary: "completed".to_string(),
                    saved_status: "session saved".to_string(),
                    tool_logs: Vec::new(),
                    elapsed_ms: 0,
                    inference_performance: None,
                    tool_calls: None,
                    assistant_tool_call_records: None,
                })),
                _ => panic!("provider should not need extra retries once shell mutation satisfies the plan"),
            }
            Ok(())
        }
    }

    let provider = ShellPlanSyncProvider {
        seen_requests: seen_requests.clone(),
    };

    let _frames = app
        .run_live_turn(
            "3011ポートで起動可能なnext.jsアプリを作ってください。",
            &provider,
            &tui,
        )
        .expect("shell mutation should satisfy the plan before final gate");

    let requests = seen_requests.borrow();
    assert_eq!(
        requests.len(),
        2,
        "expected initial shell mutation and a final response without an extra suppression loop"
    );
    assert!(
        root.join("app/package.json").exists(),
        "shell.exec should create the target file that satisfies the plan"
    );
}

#[test]
fn done_with_anvil_plan_only_continues_into_followup_execution() {
    let root = common::unique_test_dir("done_plan_only_followup");
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

    let seen_requests = Rc::new(RefCell::new(Vec::new()));

    struct PlanOnlyDoneProvider {
        seen_requests: Rc<RefCell<Vec<ProviderTurnRequest>>>,
    }

    impl ProviderClient for PlanOnlyDoneProvider {
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
                            "了解しました。まず計画を立てます。\n\n",
                            "```ANVIL_PLAN\n",
                            "- [ ] next.config.ts: basePath と assetPrefix を設定\n",
                            "```\n\n",
                            "まず `next.config.ts` を編集します。"
                        )
                        .to_string(),
                        completion_summary: "plan only".to_string(),
                        saved_status: "session saved".to_string(),
                        tool_logs: Vec::new(),
                        elapsed_ms: 100,
                        inference_performance: None,
                        tool_calls: None,
                        assistant_tool_call_records: None,
                    }));
                }
                _ => {
                    emit(ProviderEvent::TokenDelta(
                        concat!(
                            "```ANVIL_TOOL\n",
                            "{\"id\":\"call_001\",\"tool\":\"file.write\",\"path\":\"./next.config.ts\",\"content\":\"export default {}\\n\"}\n",
                            "```\n",
                            "```ANVIL_FINAL\n",
                            "Updated next.config.ts.\n",
                            "```\n"
                        )
                        .to_string(),
                    ));
                }
            }
            Ok(())
        }
    }

    let provider = PlanOnlyDoneProvider {
        seen_requests: seen_requests.clone(),
    };

    let _frames = app
        .run_live_turn("localhost:3011 で起動したい", &provider, &tui)
        .expect("plan-only done path should continue into execution");

    let requests = seen_requests.borrow();
    assert_eq!(
        requests.len(),
        2,
        "expected initial plan-only Done + one follow-up execution request"
    );
    assert!(
        root.join("next.config.ts").exists(),
        "follow-up execution should write the planned file"
    );
}

#[test]
fn done_with_plan_and_anvil_final_continues_into_followup_execution() {
    let root = common::unique_test_dir("done_plan_and_final_followup");
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

    let seen_requests = Rc::new(RefCell::new(Vec::new()));

    struct PlanAndFinalDoneProvider {
        seen_requests: Rc<RefCell<Vec<ProviderTurnRequest>>>,
    }

    impl ProviderClient for PlanAndFinalDoneProvider {
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
                            "まず計画を示します。\n\n",
                            "```ANVIL_PLAN\n",
                            "- [ ] src/output.txt: create output file\n",
                            "```\n\n",
                            "```ANVIL_FINAL\n",
                            "Plan prepared. Next I will edit the file.\n",
                            "```\n"
                        )
                        .to_string(),
                        completion_summary: "plan and final only".to_string(),
                        saved_status: "session saved".to_string(),
                        tool_logs: Vec::new(),
                        elapsed_ms: 100,
                        inference_performance: None,
                        tool_calls: None,
                        assistant_tool_call_records: None,
                    }));
                }
                _ => {
                    emit(ProviderEvent::TokenDelta(
                        concat!(
                            "```ANVIL_TOOL\n",
                            "{\"id\":\"call_001\",\"tool\":\"file.write\",\"path\":\"./src/output.txt\",\"content\":\"ok\\n\"}\n",
                            "```\n",
                            "```ANVIL_FINAL\n",
                            "Created src/output.txt.\n",
                            "```\n"
                        )
                        .to_string(),
                    ));
                }
            }
            Ok(())
        }
    }

    let provider = PlanAndFinalDoneProvider {
        seen_requests: seen_requests.clone(),
    };

    let _frames = app
        .run_live_turn("ファイルを作成して", &provider, &tui)
        .expect("plan+final done path should continue into execution");

    let requests = seen_requests.borrow();
    assert_eq!(
        requests.len(),
        2,
        "expected initial plan+final Done + one follow-up execution request"
    );
    assert!(
        root.join("src/output.txt").exists(),
        "follow-up execution should write the planned file"
    );
}

#[test]
fn anvil_final_guard_prompt_tool_rules_contains_implementation_guidance() {
    // Verify that the system prompt includes the implementation guidance text
    let app = common::build_app();
    let tui = Tui::new();
    let seen_requests = Rc::new(RefCell::new(Vec::new()));
    let provider = RecordingProvider {
        seen_requests: seen_requests.clone(),
        events: vec![
            ProviderEvent::Agent(AgentEvent::Thinking {
                status: "Thinking".to_string(),
                plan_items: vec![],
                active_index: None,
                reasoning_summary: vec![],
                elapsed_ms: 0,
            }),
            ProviderEvent::Agent(AgentEvent::Done {
                status: "Done. session saved".to_string(),
                assistant_message: "ok".to_string(),
                completion_summary: "done".to_string(),
                saved_status: "session saved".to_string(),
                tool_logs: Vec::new(),
                elapsed_ms: 0,
                inference_performance: None,
                tool_calls: None,
                assistant_tool_call_records: None,
            }),
        ],
        followup_events: Vec::new(),
        error: None,
    };

    let mut app = app;
    let _ = app.run_live_turn("test", &provider, &tui);

    let requests = seen_requests.borrow();
    let system_prompt = &requests[0].messages[0].content;
    assert!(
        system_prompt.contains("you must complete the actual file modifications"),
        "system prompt should contain implementation guidance from PROMPT_TOOL_RULES"
    );
}

// ---------------------------------------------------------------------------
// Issue #160: ANVIL_FINAL後のツール呼び出し除外 — 統合テスト
// ---------------------------------------------------------------------------

#[test]
fn done_event_post_final() {
    // TC6: Done-event path filters post-FINAL tools.
    // LLM outputs ANVIL_TOOL(call_001) + ANVIL_FINAL + ANVIL_TOOL(call_002).
    // Only call_001 should be executed; call_002 should be excluded by the parser.
    let root = common::unique_test_dir("done_post_final");
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

    std::fs::create_dir_all(root.join("src")).expect("create src dir");
    std::fs::write(root.join("src/main.rs"), "fn main() {}").expect("write main.rs");

    let seen_requests = Rc::new(RefCell::new(Vec::new()));

    struct DonePostFinalProvider {
        seen_requests: Rc<RefCell<Vec<ProviderTurnRequest>>>,
    }

    impl ProviderClient for DonePostFinalProvider {
        fn stream_turn(
            &self,
            request: &ProviderTurnRequest,
            emit: &mut dyn FnMut(ProviderEvent),
        ) -> Result<(), ProviderTurnError> {
            let call_index = self.seen_requests.borrow().len();
            self.seen_requests.borrow_mut().push(request.clone());

            match call_index {
                0 => {
                    // Done event: ANVIL_TOOL + ANVIL_FINAL + post-FINAL ANVIL_TOOL
                    emit(ProviderEvent::Agent(AgentEvent::Done {
                        status: "Done. session saved".to_string(),
                        assistant_message: concat!(
                            "```ANVIL_TOOL\n",
                            "{\"id\":\"call_001\",\"tool\":\"file.read\",\"path\":\"./src/main.rs\"}\n",
                            "```\n",
                            "```ANVIL_FINAL\n",
                            "Read the source file.\n",
                            "```\n",
                            "```ANVIL_TOOL\n",
                            "{\"id\":\"call_002\",\"tool\":\"file.read\",\"path\":\"./src/lib.rs\"}\n",
                            "```\n"
                        )
                        .to_string(),
                        completion_summary: "turn 1".to_string(),
                        saved_status: "session saved".to_string(),
                        tool_logs: Vec::new(),
                        elapsed_ms: 0,
                        inference_performance: None,
                        tool_calls: None,
                        assistant_tool_call_records: None,
                    }));
                }
                1 => {
                    // Agentic follow-up after file.read (only call_001 executed)
                    emit(ProviderEvent::TokenDelta(
                        "Here is the source code analysis.".to_string(),
                    ));
                }
                _ => {
                    // Guard retry: accept
                    emit(ProviderEvent::TokenDelta(
                        "No further changes needed.".to_string(),
                    ));
                }
            }
            Ok(())
        }
    }

    let provider = DonePostFinalProvider {
        seen_requests: seen_requests.clone(),
    };

    let _frames = app
        .run_live_turn("read the source", &provider, &tui)
        .expect("done_event_post_final should succeed");

    // Verify call_001 WAS executed: main.rs tool result should appear in session
    let has_main_rs_result = app
        .session()
        .messages
        .iter()
        .any(|m| m.content.contains("fn main()") || m.content.contains("main.rs"));
    assert!(
        has_main_rs_result,
        "pre-FINAL tool call_001 (main.rs) should have been executed"
    );

    // Verify call_002 was NOT executed: lib.rs content should NOT appear
    let has_lib_rs_result = app
        .session()
        .messages
        .iter()
        .any(|m| m.content.contains("lib.rs"));
    assert!(
        !has_lib_rs_result,
        "post-FINAL tool call_002 (lib.rs) should NOT appear in session messages"
    );
}

#[test]
fn guard_retry_post_final() {
    // TC7: Guard retry path filters post-FINAL tools.
    // First response: plan-only ANVIL_FINAL (triggers guard).
    // Guard retry response: ANVIL_TOOL(call_001) + ANVIL_FINAL + ANVIL_TOOL(call_002).
    // Only call_001 should be executed from the retry response.
    let root = common::unique_test_dir("guard_retry_post_final");
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

    std::fs::create_dir_all(root.join("src")).expect("create src dir");
    std::fs::write(root.join("src/main.rs"), "fn main() {}").expect("write main.rs");

    let seen_requests = Rc::new(RefCell::new(Vec::new()));

    struct GuardRetryPostFinalProvider {
        seen_requests: Rc<RefCell<Vec<ProviderTurnRequest>>>,
    }

    impl ProviderClient for GuardRetryPostFinalProvider {
        fn stream_turn(
            &self,
            request: &ProviderTurnRequest,
            emit: &mut dyn FnMut(ProviderEvent),
        ) -> Result<(), ProviderTurnError> {
            let call_index = self.seen_requests.borrow().len();
            self.seen_requests.borrow_mut().push(request.clone());

            match call_index {
                0 => {
                    // Plan-only ANVIL_FINAL (no tool calls → guard fires)
                    emit(ProviderEvent::Agent(AgentEvent::Done {
                        status: "Done. session saved".to_string(),
                        assistant_message: concat!(
                            "```ANVIL_FINAL\n",
                            "Here is my plan:\n1. Read the file\n2. Analyze it\n",
                            "```\n"
                        )
                        .to_string(),
                        completion_summary: "plan only".to_string(),
                        saved_status: "session saved".to_string(),
                        tool_logs: Vec::new(),
                        elapsed_ms: 100,
                        inference_performance: None,
                        tool_calls: None,
                        assistant_tool_call_records: None,
                    }));
                }
                1 => {
                    // Guard retry response with post-FINAL tool
                    emit(ProviderEvent::Agent(AgentEvent::Done {
                        status: "Done. session saved".to_string(),
                        assistant_message: concat!(
                            "```ANVIL_TOOL\n",
                            "{\"id\":\"call_001\",\"tool\":\"file.read\",\"path\":\"./src/main.rs\"}\n",
                            "```\n",
                            "```ANVIL_FINAL\n",
                            "Reading the file now.\n",
                            "```\n",
                            "```ANVIL_TOOL\n",
                            "{\"id\":\"call_002\",\"tool\":\"file.read\",\"path\":\"./src/lib.rs\"}\n",
                            "```\n"
                        )
                        .to_string(),
                        completion_summary: "retry turn".to_string(),
                        saved_status: "session saved".to_string(),
                        tool_logs: Vec::new(),
                        elapsed_ms: 200,
                        inference_performance: None,
                        tool_calls: None,
                        assistant_tool_call_records: None,
                    }));
                }
                _ => {
                    // Follow-up after file.read (only call_001)
                    emit(ProviderEvent::TokenDelta("Analysis complete.".to_string()));
                }
            }
            Ok(())
        }
    }

    let provider = GuardRetryPostFinalProvider {
        seen_requests: seen_requests.clone(),
    };

    let _frames = app
        .run_live_turn("analyze the code", &provider, &tui)
        .expect("guard_retry_post_final should succeed");

    // Verify call_002 was NOT executed: lib.rs should NOT appear in any session message
    let has_lib_rs_result = app
        .session()
        .messages
        .iter()
        .any(|m| m.content.contains("lib.rs"));
    assert!(
        !has_lib_rs_result,
        "post-FINAL tool call_002 (lib.rs) should NOT appear in session messages after guard retry"
    );

    // Issue #253: Plan gate fires before final guard, injecting plan-required message
    assert!(
        app.session()
            .messages
            .iter()
            .any(|m| m.content.contains("ANVIL_PLAN")
                || m.content.contains("No file modifications detected")),
        "plan-required or guard retry message should be in session"
    );
}

// ============================================================
// normalize_http_timeout tests (Issue #146)
// ============================================================

#[test]
fn normalize_http_timeout_zero_returns_default() {
    use anvil::provider::{DEFAULT_HTTP_TIMEOUT_SECS, normalize_http_timeout};
    assert_eq!(normalize_http_timeout(0), DEFAULT_HTTP_TIMEOUT_SECS);
}

#[test]
fn normalize_http_timeout_below_min_returns_min() {
    use anvil::provider::normalize_http_timeout;
    assert_eq!(normalize_http_timeout(1), 10);
    assert_eq!(normalize_http_timeout(9), 10);
}

#[test]
fn normalize_http_timeout_above_max_returns_max() {
    use anvil::provider::normalize_http_timeout;
    assert_eq!(normalize_http_timeout(3601), 3600);
    assert_eq!(normalize_http_timeout(u64::MAX), 3600);
}

#[test]
fn normalize_http_timeout_normal_values_unchanged() {
    use anvil::provider::normalize_http_timeout;
    assert_eq!(normalize_http_timeout(10), 10);
    assert_eq!(normalize_http_timeout(300), 300);
    assert_eq!(normalize_http_timeout(3600), 3600);
}

#[test]
fn transport_with_timeout_constructs_successfully() {
    use anvil::provider::ReqwestHttpTransport;
    let _transport = ReqwestHttpTransport::with_timeout(60);
}

#[test]
fn transport_with_timeout_and_shutdown_flag_constructs_successfully() {
    use anvil::provider::ReqwestHttpTransport;
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;
    let flag = Arc::new(AtomicBool::new(false));
    let _transport = ReqwestHttpTransport::with_timeout_and_shutdown_flag(120, flag);
}

#[test]
fn prompt_tool_rules_contains_large_file_guidance() {
    let app = common::build_app();
    let tui = Tui::new();
    let seen_requests = Rc::new(RefCell::new(Vec::new()));
    let provider = RecordingProvider {
        seen_requests: seen_requests.clone(),
        events: vec![
            ProviderEvent::Agent(AgentEvent::Thinking {
                status: "Thinking".to_string(),
                plan_items: vec![],
                active_index: None,
                reasoning_summary: vec![],
                elapsed_ms: 0,
            }),
            ProviderEvent::Agent(AgentEvent::Done {
                status: "Done. session saved".to_string(),
                assistant_message: "ok".to_string(),
                completion_summary: "done".to_string(),
                saved_status: "session saved".to_string(),
                tool_logs: Vec::new(),
                elapsed_ms: 0,
                inference_performance: None,
                tool_calls: None,
                assistant_tool_call_records: None,
            }),
        ],
        followup_events: Vec::new(),
        error: None,
    };

    let mut app = app;
    let _ = app.run_live_turn("test", &provider, &tui);

    let requests = seen_requests.borrow();
    let system_prompt = &requests[0].messages[0].content;
    assert!(
        system_prompt.contains("file.write may be blocked"),
        "system prompt should contain large file write guidance from PROMPT_TOOL_RULES"
    );
}

// ── Issue #195: sidecar_summarize tests ──────────────────────────────────

#[test]
fn sidecar_summarize_network_error_returns_none() {
    // Connect to a port that is (almost certainly) not listening
    let client = OllamaProviderClient::new("http://127.0.0.1:19999");
    let result = client.sidecar_summarize("test-model", "some conversation text");
    assert!(
        result.is_none(),
        "network error should return None for graceful fallback"
    );
}

#[test]
fn sidecar_summarize_success_with_mock_server() {
    // This test verifies the OllamaChatRequest construction is correct
    // by building the request and checking it can be serialized.
    use anvil::provider::OllamaChatRequest;
    let request = OllamaChatRequest {
        model: "qwen2.5:3b".to_string(),
        messages: vec![
            OllamaChatMessage {
                role: "system".to_string(),
                content: "You are a concise summarizer.".to_string(),
                images: None,
                tool_calls: None,
            },
            OllamaChatMessage {
                role: "user".to_string(),
                content: "user: hello\nassistant: hi".to_string(),
                images: None,
                tool_calls: None,
            },
        ],
        stream: false,
        think: false,
        options: None,
        tools: None,
    };
    let json = serde_json::to_string(&request).expect("should serialize");
    assert!(json.contains("qwen2.5:3b"));
    assert!(json.contains("\"stream\":false"));
}

#[test]
fn error_guidance_mentions_sidecar_keys() {
    let err =
        anvil::app::AppError::Config(anvil::config::ConfigError::ValidationError("test".into()));
    let guidance = anvil::app::error_guidance(&err);
    assert!(
        guidance.contains("sidecar_model"),
        "error guidance should mention sidecar_model: {guidance}"
    );
    assert!(
        guidance.contains("sidecar_provider_url"),
        "error guidance should mention sidecar_provider_url: {guidance}"
    );
}

// ---------------------------------------------------------------------------
// Issue #204: max_output_tokens / infinite stream guard
// ---------------------------------------------------------------------------

#[test]
fn ollama_request_includes_num_predict_when_max_output_tokens_set() {
    let mut request = ProviderTurnRequest::new(
        "test-model".to_string(),
        vec![anvil::provider::ProviderMessage::new(
            ProviderMessageRole::User,
            "hello",
        )],
        true,
    );
    request.max_output_tokens = Some(16384);

    let ollama_request =
        OllamaProviderClient::<anvil::provider::ReqwestHttpTransport>::build_chat_request(&request);

    let options = ollama_request
        .options
        .expect("options should be set when max_output_tokens is Some");
    assert_eq!(options.num_predict, Some(16384));
}

#[test]
fn ollama_request_omits_options_when_max_output_tokens_none() {
    let request = ProviderTurnRequest::new(
        "test-model".to_string(),
        vec![anvil::provider::ProviderMessage::new(
            ProviderMessageRole::User,
            "hello",
        )],
        true,
    );

    let ollama_request =
        OllamaProviderClient::<anvil::provider::ReqwestHttpTransport>::build_chat_request(&request);

    assert!(
        ollama_request.options.is_none(),
        "options should be None when max_output_tokens is not set"
    );
}

#[test]
fn ollama_request_serializes_num_predict_correctly() {
    let mut request = ProviderTurnRequest::new(
        "test-model".to_string(),
        vec![anvil::provider::ProviderMessage::new(
            ProviderMessageRole::User,
            "hello",
        )],
        true,
    );
    request.max_output_tokens = Some(8192);

    let ollama_request =
        OllamaProviderClient::<anvil::provider::ReqwestHttpTransport>::build_chat_request(&request);
    let json = serde_json::to_string(&ollama_request).expect("should serialize");
    assert!(
        json.contains("\"num_predict\":8192"),
        "serialized request should contain num_predict: {json}"
    );
}

#[test]
fn openai_request_includes_max_tokens_when_set() {
    let mut request = ProviderTurnRequest::new(
        "test-model".to_string(),
        vec![anvil::provider::ProviderMessage::new(
            ProviderMessageRole::User,
            "hello",
        )],
        false,
    );
    request.max_output_tokens = Some(16384);

    // Use mock transport to capture the serialized request body
    let seen_bodies = Rc::new(RefCell::new(Vec::new()));
    let transport = MockHttpTransport {
        seen_urls: Rc::new(RefCell::new(Vec::new())),
        seen_bodies: seen_bodies.clone(),
        seen_headers: Rc::new(RefCell::new(Vec::new())),
        response: HttpResponse {
            status_code: 200,
            body: br#"{"choices":[{"message":{"role":"assistant","content":"ok"}}]}"#.to_vec(),
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
        .expect("should succeed");

    let bodies = seen_bodies.borrow();
    assert!(!bodies.is_empty(), "should have sent a request");
    let body_str = String::from_utf8_lossy(&bodies[0]);
    assert!(
        body_str.contains("\"max_tokens\":16384"),
        "request body should contain max_tokens: {body_str}"
    );
}

#[test]
fn openai_request_omits_max_tokens_when_none() {
    let request = ProviderTurnRequest::new(
        "test-model".to_string(),
        vec![anvil::provider::ProviderMessage::new(
            ProviderMessageRole::User,
            "hello",
        )],
        false,
    );

    let seen_bodies = Rc::new(RefCell::new(Vec::new()));
    let transport = MockHttpTransport {
        seen_urls: Rc::new(RefCell::new(Vec::new())),
        seen_bodies: seen_bodies.clone(),
        seen_headers: Rc::new(RefCell::new(Vec::new())),
        response: HttpResponse {
            status_code: 200,
            body: br#"{"choices":[{"message":{"role":"assistant","content":"ok"}}]}"#.to_vec(),
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
        .expect("should succeed");

    let bodies = seen_bodies.borrow();
    let body_str = String::from_utf8_lossy(&bodies[0]);
    assert!(
        !body_str.contains("max_tokens"),
        "request body should NOT contain max_tokens when None: {body_str}"
    );
}

// ── Issue #369: tool temperature tests ──────────────────────────────────

#[test]
fn ollama_request_includes_temperature_in_options_when_set() {
    let mut request = ProviderTurnRequest::new(
        "test-model".to_string(),
        vec![anvil::provider::ProviderMessage::new(
            ProviderMessageRole::User,
            "hello",
        )],
        true,
    );
    request.temperature = Some(0.3);

    let ollama_request =
        OllamaProviderClient::<anvil::provider::ReqwestHttpTransport>::build_chat_request(&request);

    let options = ollama_request
        .options
        .expect("options should be set when temperature is Some");
    assert_eq!(options.temperature, Some(0.3));
    assert_eq!(options.num_predict, None);
}

#[test]
fn ollama_request_includes_both_num_predict_and_temperature() {
    let mut request = ProviderTurnRequest::new(
        "test-model".to_string(),
        vec![anvil::provider::ProviderMessage::new(
            ProviderMessageRole::User,
            "hello",
        )],
        true,
    );
    request.max_output_tokens = Some(8192);
    request.temperature = Some(0.3);

    let ollama_request =
        OllamaProviderClient::<anvil::provider::ReqwestHttpTransport>::build_chat_request(&request);

    let json = serde_json::to_string(&ollama_request).expect("should serialize");

    let options = ollama_request
        .options
        .expect("options should be set when both fields are Some");
    assert_eq!(options.num_predict, Some(8192));
    assert_eq!(options.temperature, Some(0.3));
    assert!(
        json.contains("\"temperature\":0.3"),
        "serialized request should contain temperature: {json}"
    );
    assert!(
        json.contains("\"num_predict\":8192"),
        "serialized request should contain num_predict: {json}"
    );
}

#[test]
fn ollama_request_omits_options_when_both_none() {
    let request = ProviderTurnRequest::new(
        "test-model".to_string(),
        vec![anvil::provider::ProviderMessage::new(
            ProviderMessageRole::User,
            "hello",
        )],
        true,
    );
    // Both max_output_tokens and temperature are None by default

    let ollama_request =
        OllamaProviderClient::<anvil::provider::ReqwestHttpTransport>::build_chat_request(&request);

    assert!(
        ollama_request.options.is_none(),
        "options should be None when both max_output_tokens and temperature are None"
    );
}

#[test]
fn ollama_request_omits_temperature_from_json_when_none() {
    let mut request = ProviderTurnRequest::new(
        "test-model".to_string(),
        vec![anvil::provider::ProviderMessage::new(
            ProviderMessageRole::User,
            "hello",
        )],
        true,
    );
    request.max_output_tokens = Some(8192);
    // temperature is None

    let ollama_request =
        OllamaProviderClient::<anvil::provider::ReqwestHttpTransport>::build_chat_request(&request);

    let json = serde_json::to_string(&ollama_request).expect("should serialize");
    assert!(
        !json.contains("temperature"),
        "serialized request should NOT contain temperature when None: {json}"
    );
}

#[test]
fn openai_request_includes_temperature_when_set() {
    let mut request = ProviderTurnRequest::new(
        "test-model".to_string(),
        vec![anvil::provider::ProviderMessage::new(
            ProviderMessageRole::User,
            "hello",
        )],
        false,
    );
    request.temperature = Some(0.3);

    let seen_bodies = Rc::new(RefCell::new(Vec::new()));
    let transport = MockHttpTransport {
        seen_urls: Rc::new(RefCell::new(Vec::new())),
        seen_bodies: seen_bodies.clone(),
        seen_headers: Rc::new(RefCell::new(Vec::new())),
        response: HttpResponse {
            status_code: 200,
            body: br#"{"choices":[{"message":{"role":"assistant","content":"hi"}}]}"#.to_vec(),
        },
        get_response: None,
    };
    let client = anvil::provider::openai::OpenAiCompatibleProviderClient::with_transport(
        "http://test",
        transport,
    );

    let mut events = Vec::new();
    client
        .stream_turn(&request, &mut |event| events.push(event))
        .expect("should succeed");

    let bodies = seen_bodies.borrow();
    let body_str = String::from_utf8_lossy(&bodies[0]);
    assert!(
        body_str.contains("\"temperature\":0.3"),
        "request body should contain temperature: {body_str}"
    );
}

#[test]
fn openai_request_omits_temperature_when_none() {
    let request = ProviderTurnRequest::new(
        "test-model".to_string(),
        vec![anvil::provider::ProviderMessage::new(
            ProviderMessageRole::User,
            "hello",
        )],
        false,
    );

    let seen_bodies = Rc::new(RefCell::new(Vec::new()));
    let transport = MockHttpTransport {
        seen_urls: Rc::new(RefCell::new(Vec::new())),
        seen_bodies: seen_bodies.clone(),
        seen_headers: Rc::new(RefCell::new(Vec::new())),
        response: HttpResponse {
            status_code: 200,
            body: br#"{"choices":[{"message":{"role":"assistant","content":"hi"}}]}"#.to_vec(),
        },
        get_response: None,
    };
    let client = anvil::provider::openai::OpenAiCompatibleProviderClient::with_transport(
        "http://test",
        transport,
    );

    let mut events = Vec::new();
    client
        .stream_turn(&request, &mut |event| events.push(event))
        .expect("should succeed");

    let bodies = seen_bodies.borrow();
    let body_str = String::from_utf8_lossy(&bodies[0]);
    assert!(
        !body_str.contains("temperature"),
        "request body should NOT contain temperature when None: {body_str}"
    );
}

// ---------------------------------------------------------------------------
// Issue #370: num_ctx / context_window pass-through to Ollama
// ---------------------------------------------------------------------------

#[test]
fn ollama_request_includes_num_ctx_when_context_window_set() {
    let mut request = ProviderTurnRequest::new(
        "test-model".to_string(),
        vec![anvil::provider::ProviderMessage::new(
            ProviderMessageRole::User,
            "hello",
        )],
        true,
    );
    request.context_window = Some(32768);

    let ollama_request =
        OllamaProviderClient::<anvil::provider::ReqwestHttpTransport>::build_chat_request(&request);

    let options = ollama_request
        .options
        .expect("options should be set when context_window is Some");
    assert_eq!(options.num_ctx, Some(32768));
}

#[test]
fn ollama_request_omits_num_ctx_when_context_window_none() {
    let mut request = ProviderTurnRequest::new(
        "test-model".to_string(),
        vec![anvil::provider::ProviderMessage::new(
            ProviderMessageRole::User,
            "hello",
        )],
        true,
    );
    request.max_output_tokens = Some(16384);

    let ollama_request =
        OllamaProviderClient::<anvil::provider::ReqwestHttpTransport>::build_chat_request(&request);

    let options = ollama_request.options.expect("options should be set");
    assert_eq!(options.num_ctx, None);
}

#[test]
fn ollama_request_includes_both_num_predict_and_num_ctx() {
    let mut request = ProviderTurnRequest::new(
        "test-model".to_string(),
        vec![anvil::provider::ProviderMessage::new(
            ProviderMessageRole::User,
            "hello",
        )],
        true,
    );
    request.max_output_tokens = Some(16384);
    request.context_window = Some(32768);

    let ollama_request =
        OllamaProviderClient::<anvil::provider::ReqwestHttpTransport>::build_chat_request(&request);

    let options = ollama_request.options.expect("options should be set");
    assert_eq!(options.num_predict, Some(16384));
    assert_eq!(options.num_ctx, Some(32768));
}

#[test]
fn ollama_request_serializes_num_ctx_correctly() {
    let mut request = ProviderTurnRequest::new(
        "test-model".to_string(),
        vec![anvil::provider::ProviderMessage::new(
            ProviderMessageRole::User,
            "hello",
        )],
        true,
    );
    request.context_window = Some(65536);

    let ollama_request =
        OllamaProviderClient::<anvil::provider::ReqwestHttpTransport>::build_chat_request(&request);
    let json = serde_json::to_string(&ollama_request).expect("should serialize");
    assert!(
        json.contains("\"num_ctx\":65536"),
        "should contain num_ctx: {json}"
    );
}

#[test]
fn ollama_request_includes_num_ctx_only_when_num_predict_none() {
    let mut request = ProviderTurnRequest::new(
        "test-model".to_string(),
        vec![anvil::provider::ProviderMessage::new(
            ProviderMessageRole::User,
            "hello",
        )],
        true,
    );
    request.context_window = Some(8192);
    // max_output_tokens is None (default)

    let ollama_request =
        OllamaProviderClient::<anvil::provider::ReqwestHttpTransport>::build_chat_request(&request);

    let options = ollama_request
        .options
        .expect("options should be set when context_window is Some");
    assert_eq!(options.num_predict, None);
    assert_eq!(options.num_ctx, Some(8192));
}
