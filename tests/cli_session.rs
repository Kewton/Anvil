mod common;

use anvil::agent::AgentEvent;
use anvil::app::SessionControl;
use anvil::contracts::RuntimeState;
use anvil::provider::{ProviderClient, ProviderEvent, ProviderTurnError, ProviderTurnRequest};
use anvil::tui::Tui;
use std::cell::RefCell;
use std::fs;
use std::io::Cursor;
use std::rc::Rc;

#[derive(Clone)]
struct RecordingProvider {
    seen_requests: Rc<RefCell<Vec<ProviderTurnRequest>>>,
    events: Vec<ProviderEvent>,
}

impl ProviderClient for RecordingProvider {
    fn stream_turn(
        &self,
        request: &ProviderTurnRequest,
        emit: &mut dyn FnMut(ProviderEvent),
    ) -> Result<(), ProviderTurnError> {
        let call_index = self.seen_requests.borrow().len();
        self.seen_requests.borrow_mut().push(request.clone());

        // Agentic follow-up: return plain Done to terminate the loop
        if call_index > 0 {
            emit(ProviderEvent::Agent(AgentEvent::Done {
                status: "Done. session saved".to_string(),
                assistant_message: "Agentic follow-up completed.".to_string(),
                completion_summary: "Follow-up turn finished.".to_string(),
                saved_status: "session saved".to_string(),
                tool_logs: Vec::new(),
                elapsed_ms: 0,
                inference_performance: None,
            }));
            return Ok(());
        }

        for event in self.events.clone() {
            emit(event);
        }
        Ok(())
    }
}

#[test]
fn slash_commands_support_help_status_reset_and_exit() {
    let mut app = common::build_app();
    let tui = Tui::new();
    let provider = RecordingProvider {
        seen_requests: Rc::new(RefCell::new(Vec::new())),
        events: Vec::new(),
    };

    let startup = app.startup_console(&tui).expect("startup should render");
    let help = app
        .handle_cli_line("/help", &provider, &tui)
        .expect("help should render");
    let status = app
        .handle_cli_line("/status", &provider, &tui)
        .expect("status should render");
    let plan = app
        .handle_cli_line("/plan", &provider, &tui)
        .expect("plan should render");
    let plan_add = app
        .handle_cli_line("/plan-add inspect repo layout", &provider, &tui)
        .expect("plan add should render");
    let plan_focus = app
        .handle_cli_line("/plan-focus 1", &provider, &tui)
        .expect("plan focus should render");
    let plan_clear = app
        .handle_cli_line("/plan-clear", &provider, &tui)
        .expect("plan clear should render");
    let repo_find = app
        .handle_cli_line("/repo-find Cargo.toml", &provider, &tui)
        .expect("repo find should render");
    let timeline = app
        .handle_cli_line("/timeline", &provider, &tui)
        .expect("timeline should render");
    let compact = app
        .handle_cli_line("/compact", &provider, &tui)
        .expect("compact should render");
    let checkpoint = app
        .handle_cli_line("/checkpoint lock current plan", &provider, &tui)
        .expect("checkpoint should render");
    let model = app
        .handle_cli_line("/model", &provider, &tui)
        .expect("model should render");
    let provider_status = app
        .handle_cli_line("/provider", &provider, &tui)
        .expect("provider should render");
    let reset = app
        .handle_cli_line("/reset", &provider, &tui)
        .expect("reset should render");
    let exit = app
        .handle_cli_line("/exit", &provider, &tui)
        .expect("exit should be accepted");

    assert!(startup.contains("Ask for a task, or use /help, /model, /plan, /status"));
    assert!(help.frames.last().expect("help frame").contains("/approve"));
    assert!(
        status
            .frames
            .last()
            .expect("status frame")
            .contains("model:local-default")
    );
    assert!(
        plan.frames
            .last()
            .expect("plan frame")
            .contains("[A] anvil > plan")
    );
    assert!(
        plan_add
            .frames
            .last()
            .expect("plan add frame")
            .contains("inspect repo layout")
    );
    assert!(
        plan_focus
            .frames
            .last()
            .expect("plan focus frame")
            .contains("* 1. inspect repo layout")
    );
    assert!(
        plan_clear
            .frames
            .last()
            .expect("plan clear frame")
            .contains("no active plan")
    );
    assert!(
        repo_find
            .frames
            .last()
            .expect("repo-find frame")
            .contains("[A] anvil > repo-find Cargo.toml")
    );
    assert!(
        timeline
            .frames
            .last()
            .expect("timeline frame")
            .contains("[A] anvil > timeline")
    );
    assert!(
        compact
            .frames
            .last()
            .expect("compact frame")
            .contains("nothing to compact")
    );
    assert!(
        checkpoint
            .frames
            .last()
            .expect("checkpoint frame")
            .contains("checkpoint saved")
    );
    assert!(
        model
            .frames
            .last()
            .expect("model frame")
            .contains("current model: local-default")
    );
    assert!(
        provider_status
            .frames
            .last()
            .expect("provider frame")
            .contains("provider: ollama")
    );
    assert_eq!(app.state_machine().snapshot().state, RuntimeState::Ready);
    assert!(
        reset
            .frames
            .last()
            .expect("reset frame")
            .contains("Ready for the next task")
    );
    assert_eq!(exit.control, SessionControl::Exit);
}

#[test]
fn regular_input_runs_live_turn_and_supports_follow_up_in_same_session() {
    let root = common::unique_test_dir("cli_follow_up");
    let mut app = common::build_app_in(root);
    let tui = Tui::new();
    let seen_requests = Rc::new(RefCell::new(Vec::new()));
    let provider = RecordingProvider {
        seen_requests: seen_requests.clone(),
        events: vec![ProviderEvent::Agent(AgentEvent::Done {
            status: "Done. session saved".to_string(),
            assistant_message: "provider-backed answer".to_string(),
            completion_summary: "Completed the requested task.".to_string(),
            saved_status: "session saved".to_string(),
            tool_logs: Vec::new(),
            elapsed_ms: 120,
            inference_performance: None,
        })],
    };

    let first = app
        .handle_cli_line("inspect app bootstrap", &provider, &tui)
        .expect("first prompt should run");
    let _second = app
        .handle_cli_line("now summarize config behavior", &provider, &tui)
        .expect("follow-up prompt should run");

    assert_eq!(first.control, SessionControl::Continue);
    // Assistant messages are excluded from frame rendering (streamed to stderr,
    // Issue #1). The Done frame shows the completion_summary instead.
    assert!(
        first
            .frames
            .last()
            .expect("first frame")
            .contains("[A] anvil > result"),
        "done frame should contain result section"
    );
    // The assistant message should be stored in session for LLM context.
    assert!(
        app.session()
            .messages
            .iter()
            .any(|m| m.content == "provider-backed answer"),
        "assistant message should be in session history"
    );
    assert_eq!(seen_requests.borrow().len(), 2);
    assert!(
        seen_requests.borrow()[1]
            .messages
            .iter()
            .any(|message| message.content == "inspect app bootstrap")
    );
    assert!(
        app.session()
            .messages
            .iter()
            .any(|message| message.content == "now summarize config behavior")
    );
}

#[test]
fn slash_approve_and_deny_resolve_pending_tool_approval() {
    let mut app = common::build_app();
    let tui = Tui::new();
    let provider = RecordingProvider {
        seen_requests: Rc::new(RefCell::new(Vec::new())),
        events: vec![
            ProviderEvent::Agent(AgentEvent::ApprovalRequested {
                status: "Awaiting approval for 1 tool call".to_string(),
                tool_name: "Write".to_string(),
                summary: "Update workspace/work-plan.md".to_string(),
                risk: "Confirm".to_string(),
                tool_call_id: "call_approve_001".to_string(),
                elapsed_ms: 100,
            }),
            ProviderEvent::Agent(AgentEvent::Done {
                status: "Done. session saved".to_string(),
                assistant_message: "approved write completed".to_string(),
                completion_summary: "Write completed.".to_string(),
                saved_status: "session saved".to_string(),
                tool_logs: Vec::new(),
                elapsed_ms: 240,
                inference_performance: None,
            }),
        ],
    };

    let paused = app
        .handle_cli_line("apply the update", &provider, &tui)
        .expect("turn should pause");
    assert!(
        paused
            .frames
            .last()
            .expect("paused frame")
            .contains("[A] anvil > approval")
    );
    let blocked = app
        .handle_cli_line("new task before resolving approval", &provider, &tui)
        .expect("pending approval should become a user-facing frame");
    assert!(
        blocked
            .frames
            .last()
            .expect("blocked frame")
            .contains("resolve the pending approval")
    );
    assert!(
        blocked
            .frames
            .last()
            .expect("blocked frame")
            .contains("call_approve_001")
    );

    let approved = app
        .handle_cli_line("/approve", &provider, &tui)
        .expect("approve should continue");
    // Assistant message is excluded from frame rendering (streamed to stderr,
    // Issue #1). The Done frame shows result/completion_summary instead.
    assert!(
        approved
            .frames
            .last()
            .expect("approved frame")
            .contains("[A] anvil > result"),
        "approved done frame should contain result section"
    );
    assert!(
        app.session()
            .messages
            .iter()
            .any(|m| m.content == "approved write completed"),
        "assistant message should be in session history"
    );

    let provider_deny = RecordingProvider {
        seen_requests: Rc::new(RefCell::new(Vec::new())),
        events: vec![ProviderEvent::Agent(AgentEvent::ApprovalRequested {
            status: "Awaiting approval for 1 tool call".to_string(),
            tool_name: "Write".to_string(),
            summary: "Update workspace/work-plan.md".to_string(),
            risk: "Confirm".to_string(),
            tool_call_id: "call_deny_001".to_string(),
            elapsed_ms: 100,
        })],
    };
    let _ = app
        .handle_cli_line("queue another update", &provider_deny, &tui)
        .expect("second turn should pause");
    let denied = app
        .handle_cli_line("/deny", &provider_deny, &tui)
        .expect("deny should resolve");

    assert!(
        denied
            .frames
            .last()
            .expect("denied frame")
            .contains("Approval denied")
    );
    assert_eq!(app.state_machine().snapshot().state, RuntimeState::Ready);
}

#[test]
fn startup_console_resumes_existing_session_history() {
    let root = common::unique_test_dir("cli_resume");
    let tui = Tui::new();
    let provider = RecordingProvider {
        seen_requests: Rc::new(RefCell::new(Vec::new())),
        events: vec![ProviderEvent::Agent(AgentEvent::Done {
            status: "Done. session saved".to_string(),
            assistant_message: "initial answer".to_string(),
            completion_summary: "Initial task completed.".to_string(),
            saved_status: "session saved".to_string(),
            tool_logs: Vec::new(),
            elapsed_ms: 100,
            inference_performance: None,
        })],
    };
    let mut first = common::build_app_in(root.clone());
    let _ = first
        .handle_cli_line("first task", &provider, &tui)
        .expect("first turn should run");

    let mut resumed = common::build_app_in(root);
    let startup = resumed.startup_console(&tui).expect("resume should render");

    assert!(startup.contains("Model   : local-default"));
    assert!(startup.contains("Project :"));
    assert!(startup.contains("[U] you > first task"));
    assert!(startup.contains("[A] anvil > initial answer"));
}

#[test]
fn startup_console_can_ignore_saved_history_in_fresh_session_mode() {
    let root = common::unique_test_dir("cli_fresh_session");
    let mut first = common::build_app_in(root.clone());
    first
        .record_user_input("msg_001", "old task")
        .expect("history should persist");
    first
        .record_assistant_output("msg_002", "old answer")
        .expect("history should persist");

    let mut config = common::build_config_in(root);
    config.mode.fresh_session = true;
    let provider = anvil::provider::ProviderRuntimeContext::bootstrap(&config)
        .expect("provider should bootstrap");
    let mut fresh = anvil::app::App::new(
        config,
        provider,
        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
    )
    .expect("app should initialize");
    let tui = Tui::new();

    let startup = fresh.startup_console(&tui).expect("startup should render");

    assert!(startup.contains("Ask for a task"));
    assert!(!startup.contains("old answer"));
    assert!(!startup.contains("[U] you > old task"));
}

#[test]
fn regular_input_surfaces_tool_execution_logs_in_console() {
    let mut app = common::build_app();
    let tui = Tui::new();
    let provider = RecordingProvider {
        seen_requests: Rc::new(RefCell::new(Vec::new())),
        events: vec![
            ProviderEvent::Agent(AgentEvent::Working {
                status: "Working on repository inspection".to_string(),
                plan_items: vec!["read app".to_string(), "answer".to_string()],
                active_index: Some(0),
                tool_logs: vec![(
                    "Read".to_string(),
                    "open".to_string(),
                    "src/app/mod.rs".to_string(),
                )],
                elapsed_ms: 90,
            }),
            ProviderEvent::Agent(AgentEvent::Done {
                status: "Done. session saved".to_string(),
                assistant_message: "tool-backed answer".to_string(),
                completion_summary: "Inspected the app module.".to_string(),
                saved_status: "session saved".to_string(),
                tool_logs: vec![(
                    "Read".to_string(),
                    "open".to_string(),
                    "src/app/mod.rs".to_string(),
                )],
                elapsed_ms: 140,
                inference_performance: None,
            }),
        ],
    };

    let output = app
        .handle_cli_line("inspect the app module", &provider, &tui)
        .expect("turn should run");
    let joined = output.frames.join("\n");

    assert!(joined.contains("[T] tool  > Read"));
    assert!(joined.contains("src/app/mod.rs"));
}

#[test]
fn cli_prompt_matches_operator_console_identity() {
    assert_eq!(anvil::app::cli_prompt(), "[U] you > ");
}

#[test]
fn run_session_loop_uses_operator_prompt_and_exits_on_command() {
    let mut app = common::build_app();
    let tui = Tui::new();
    let provider = RecordingProvider {
        seen_requests: Rc::new(RefCell::new(Vec::new())),
        events: Vec::new(),
    };
    let input = Cursor::new("/exit\n");
    let mut output = Vec::new();

    anvil::app::run_session_loop(&mut app, &provider, &tui, input, &mut output)
        .expect("session loop should exit cleanly");

    let rendered = String::from_utf8(output).expect("output should be utf8");
    assert!(rendered.contains("[U] you > "));
    assert!(rendered.contains("Exiting Anvil."));
}

#[test]
fn help_frame_is_built_from_registered_slash_commands() {
    let help = anvil::app::render_help_frame();
    let commands = anvil::app::slash_commands();

    assert!(help.contains("/help"));
    assert!(help.contains("show available commands"));
    assert!(help.contains("/approve"));
    assert!(help.contains("continue the pending approved tool call"));
    assert!(help.contains("/exit"));
    assert!(help.contains("exit the session"));
    assert!(help.contains("/plan-add"));
    assert!(help.contains("/plan-focus"));
    assert!(help.contains("/plan-clear"));
    assert!(help.contains("/repo-find"));
    assert!(help.contains("/timeline"));
    assert!(help.contains("/compact"));
    assert!(help.contains("/checkpoint"));
    assert!(help.contains("/provider"));
    assert!(commands.iter().any(|spec| spec.name == "/plan"));
    assert!(commands.iter().any(|spec| spec.name == "/model"));
}

#[test]
fn custom_slash_commands_load_from_extension_file_and_run_live_turn() {
    let root = common::unique_test_dir("custom_slash_command");
    fs::create_dir_all(root.join(".anvil")).expect("extension dir should exist");
    fs::write(
        root.join(".anvil/slash-commands.json"),
        r#"{
  "commands": [
    {
      "name": "/invaders",
      "description": "build the browser invaders demo",
      "prompt": "Create the requested invader prototype in the sandbox."
    }
  ]
}"#,
    )
    .expect("custom slash command file should be written");

    let mut app = common::build_app_in(root);
    let tui = Tui::new();
    let seen_requests = Rc::new(RefCell::new(Vec::new()));
    let provider = RecordingProvider {
        seen_requests: seen_requests.clone(),
        events: vec![ProviderEvent::Agent(AgentEvent::Done {
            status: "Done. session saved".to_string(),
            assistant_message: "custom command completed".to_string(),
            completion_summary: "Custom slash command completed.".to_string(),
            saved_status: "session saved".to_string(),
            tool_logs: Vec::new(),
            elapsed_ms: 90,
            inference_performance: None,
        })],
    };

    let help = app
        .handle_cli_line("/help", &provider, &tui)
        .expect("help should include custom command");
    let invaders = app
        .handle_cli_line("/invaders", &provider, &tui)
        .expect("custom slash command should run");

    assert!(
        help.frames
            .last()
            .expect("help frame")
            .contains("/invaders")
    );
    // Assistant message is excluded from frame rendering (streamed to stderr,
    // Issue #1). The Done frame shows result/completion_summary instead.
    assert!(
        invaders
            .frames
            .last()
            .expect("custom command frame")
            .contains("[A] anvil > result"),
        "custom command done frame should contain result section"
    );
    assert!(
        app.session()
            .messages
            .iter()
            .any(|m| m.content == "custom command completed"),
        "assistant message should be in session history"
    );
    assert!(
        seen_requests.borrow()[0]
            .messages
            .iter()
            .any(|message| message.content
                == "Create the requested invader prototype in the sandbox.")
    );
}

#[test]
fn plan_commands_record_typed_events_in_timeline() {
    let mut app = common::build_app();
    let tui = Tui::new();
    let provider = RecordingProvider {
        seen_requests: Rc::new(RefCell::new(Vec::new())),
        events: Vec::new(),
    };

    let _ = app
        .handle_cli_line("/plan-add inspect provider", &provider, &tui)
        .expect("plan add should work");
    let _ = app
        .handle_cli_line("/plan-focus 1", &provider, &tui)
        .expect("plan focus should work");
    let timeline = app
        .handle_cli_line("/timeline", &provider, &tui)
        .expect("timeline should render");

    let frame = timeline.frames.last().expect("timeline frame");
    assert!(frame.contains("PlanItemAdded"));
    assert!(frame.contains("PlanFocusChanged"));
    assert!(frame.contains("inspect provider"));
}

#[test]
fn compact_command_summarizes_older_messages() {
    let mut app = common::build_app();
    let tui = Tui::new();
    let provider = RecordingProvider {
        seen_requests: Rc::new(RefCell::new(Vec::new())),
        events: Vec::new(),
    };

    for index in 0..12 {
        let _ = app
            .handle_cli_line(&format!("message {index}"), &provider, &tui)
            .expect("message should be accepted");
    }

    let compact = app
        .handle_cli_line("/compact", &provider, &tui)
        .expect("compact should work");
    let timeline = app
        .handle_cli_line("/timeline", &provider, &tui)
        .expect("timeline should render");

    assert!(
        compact
            .frames
            .last()
            .expect("compact frame")
            .contains("compacted older session history")
    );
    let frame = timeline.frames.last().expect("timeline frame");
    assert!(frame.contains("SessionCompacted"));
    assert!(
        app.session().messages[0]
            .content
            .contains("[compacted session summary]")
    );
}

#[test]
fn repo_find_adds_retrieval_context_to_following_provider_turn() {
    let root = common::unique_test_dir("repo_find_context");
    fs::create_dir_all(root.join("src")).expect("src dir");
    fs::write(
        root.join("src/provider_notes.rs"),
        "pub fn provider_notes() { println!(\"provider diagnostics\"); }\n",
    )
    .expect("write file");

    let mut app = common::build_app_in(root);
    let tui = Tui::new();
    let seen_requests = Rc::new(RefCell::new(Vec::new()));
    let provider = RecordingProvider {
        seen_requests: seen_requests.clone(),
        events: vec![ProviderEvent::Agent(AgentEvent::Done {
            status: "Done. session saved".to_string(),
            assistant_message: "provider-backed answer".to_string(),
            completion_summary: "Completed the requested task.".to_string(),
            saved_status: "session saved".to_string(),
            tool_logs: Vec::new(),
            elapsed_ms: 120,
            inference_performance: None,
        })],
    };

    let _ = app
        .handle_cli_line("/repo-find provider_notes", &provider, &tui)
        .expect("repo find should work");
    let _ = app
        .handle_cli_line("summarize provider notes", &provider, &tui)
        .expect("follow-up should run");

    let borrowed = seen_requests.borrow();
    let request = borrowed.last().expect("request should exist");
    assert!(request.messages.iter().any(|message| {
        message.content.contains("[retrieval context]")
            && message.content.contains("src/provider_notes.rs")
    }));
}
