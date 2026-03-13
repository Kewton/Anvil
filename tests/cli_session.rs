mod common;

use anvil::agent::AgentEvent;
use anvil::app::SessionControl;
use anvil::contracts::RuntimeState;
use anvil::provider::{ProviderClient, ProviderEvent, ProviderTurnError, ProviderTurnRequest};
use anvil::tui::Tui;
use std::cell::RefCell;
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
        self.seen_requests.borrow_mut().push(request.clone());
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
    let model = app
        .handle_cli_line("/model", &provider, &tui)
        .expect("model should render");
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
        model
            .frames
            .last()
            .expect("model frame")
            .contains("current model: local-default")
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
        })],
    };

    let first = app
        .handle_cli_line("inspect app bootstrap", &provider, &tui)
        .expect("first prompt should run");
    let second = app
        .handle_cli_line("now summarize config behavior", &provider, &tui)
        .expect("follow-up prompt should run");

    assert_eq!(first.control, SessionControl::Continue);
    assert!(
        first
            .frames
            .last()
            .expect("first frame")
            .contains("provider-backed answer")
    );
    assert!(
        second
            .frames
            .last()
            .expect("second frame")
            .contains("provider-backed answer")
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

    let approved = app
        .handle_cli_line("/approve", &provider, &tui)
        .expect("approve should continue");
    assert!(
        approved
            .frames
            .last()
            .expect("approved frame")
            .contains("approved write completed")
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

    assert!(help.contains("/help"));
    assert!(help.contains("show available commands"));
    assert!(help.contains("/approve"));
    assert!(help.contains("continue the pending approved tool call"));
    assert!(help.contains("/exit"));
    assert!(help.contains("exit the session"));
}
