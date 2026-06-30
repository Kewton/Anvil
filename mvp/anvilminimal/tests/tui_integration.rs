use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use anvilminimal::config::{Action, Config, Provider};
use anvilminimal::minimal_loop::loop_run::run_session_with_required_paths_with_ui;
use anvilminimal::planner::{generate_step_plan_with_ui, run_ultra_plan_with_ui};
use anvilminimal::providers::{AssistantReply, ChatClient};
use anvilminimal::state::{ConversationMessage, SessionSnapshot, ToolCall};
use anvilminimal::tools::registry::ToolSpec;
use anvilminimal::tui::markdown::TerminalMarkdownRenderer;
use anvilminimal::tui::status::UiStatus;
use anvilminimal::tui::{InteractionUi, OutputRenderer, UiGuard};
use serde_json::json;

fn config(root: PathBuf) -> Config {
    Config {
        workspace_root: root.clone(),
        state_dir: root.join("state"),
        eval_events_path: None,
        completion_contract_path: None,
        yes: true,
        offline: false,
        context_budget: 1000,
        model: "m".to_string(),
        provider: Provider::Ollama,
        planner_model: "pm".to_string(),
        planner_provider: Provider::Gemini,
        ollama_host: "http://localhost:11434".to_string(),
        num_predict: 100,
        max_iterations: 4,
        chat_timeout_secs: 1,
        chat_retries: 1,
        resume: None,
        fresh_session: false,
        no_footer: false,
        profile: "generic".to_string(),
        style: "default".to_string(),
        action: Action::Repl,
    }
}

struct FakeClient {
    label: &'static str,
    replies: Vec<AssistantReply>,
    requests: Vec<Vec<ConversationMessage>>,
}

impl FakeClient {
    fn new(label: &'static str, replies: Vec<AssistantReply>) -> Self {
        Self {
            label,
            replies,
            requests: Vec::new(),
        }
    }
}

impl ChatClient for FakeClient {
    fn label(&self) -> &str {
        self.label
    }

    fn supports_native_tools(&self, _model: &str) -> bool {
        true
    }

    fn chat(
        &mut self,
        _model: &str,
        messages: &[ConversationMessage],
        _tools: &[ToolSpec],
        _native_tools_enabled: bool,
    ) -> anyhow::Result<AssistantReply> {
        self.requests.push(messages.to_vec());
        Ok(self.replies.remove(0))
    }
}

#[derive(Default)]
struct FakeUi {
    events: Mutex<Vec<String>>,
    interrupted: AtomicBool,
}

impl FakeUi {
    fn events(&self) -> Vec<String> {
        self.events.lock().unwrap().clone()
    }
}

impl InteractionUi for FakeUi {
    fn before_model_call(&self, label: &str) -> UiGuard {
        self.events.lock().unwrap().push(format!("model:{label}"));
        UiGuard::noop()
    }

    fn before_tool_call(&self, name: &str) -> UiGuard {
        self.events.lock().unwrap().push(format!("tool:{name}"));
        UiGuard::noop()
    }

    fn publish_status(&self, status: UiStatus) {
        self.events
            .lock()
            .unwrap()
            .push(format!("status:{}:{}", status.provider, status.model));
    }

    fn interrupted(&self) -> bool {
        self.interrupted.load(Ordering::SeqCst)
    }
}

#[test]
fn tui_integration_records_model_and_tool_boundaries() {
    let dir = tempfile::tempdir().unwrap();
    let mut client = FakeClient::new(
        "fake",
        vec![
            AssistantReply {
                content: String::new(),
                tool_calls: vec![ToolCall::new(
                    "Write",
                    json!({"path":"a.txt","content":"ok"}),
                )],
                prompt_tokens: Some(10),
                completion_tokens: Some(2),
            },
            AssistantReply::text("done"),
        ],
    );
    let ui = FakeUi::default();
    let mut session = SessionSnapshot::new();
    let result = run_session_with_required_paths_with_ui(
        &mut client,
        &mut session,
        "create a.txt",
        &["a.txt".to_string()],
        &config(dir.path().to_path_buf()),
        &ui,
    )
    .unwrap();
    assert_eq!(result, "required artifacts satisfied: a.txt");
    let events = ui.events();
    assert!(events.iter().any(|event| event == "model:fake m"));
    assert!(events.iter().any(|event| event == "tool:Write"));
    assert!(events.iter().any(|event| event == "status:fake:m"));
}

#[test]
fn tui_markdown_raw_session_storage() {
    let dir = tempfile::tempdir().unwrap();
    let raw = "<think>secret</think># done";
    let mut client = FakeClient::new("fake", vec![AssistantReply::text(raw)]);
    let ui = FakeUi::default();
    let mut session = SessionSnapshot::new();
    let reply = run_session_with_required_paths_with_ui(
        &mut client,
        &mut session,
        "answer",
        &[],
        &config(dir.path().to_path_buf()),
        &ui,
    )
    .unwrap();
    assert_eq!(reply, raw);
    assert_eq!(session.messages[1].content, raw);
    let rendered = TerminalMarkdownRenderer::new(false, true).render_to_string(raw);
    assert_eq!(rendered, "done");
    assert!(!rendered.contains("secret"));
}

#[test]
fn interrupt_boundaries_stop_before_model_call() {
    let dir = tempfile::tempdir().unwrap();
    let mut client = FakeClient::new("fake", vec![AssistantReply::text("unused")]);
    let mut session = SessionSnapshot::new();
    let ui = FakeUi::default();
    ui.interrupted.store(true, Ordering::SeqCst);
    let err = run_session_with_required_paths_with_ui(
        &mut client,
        &mut session,
        "answer",
        &[],
        &config(dir.path().to_path_buf()),
        &ui,
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("interrupted by user"));
}

#[test]
fn planner_uses_ui_for_planner_model_call() {
    let dir = tempfile::tempdir().unwrap();
    let json = r#"{"goal":"test","steps":[{"id":"s1","kind":"report","instruction":"say done","expected_paths":[],"verify":[],"expected_result":"pass"}]}"#;
    let mut planner = FakeClient::new("planner", vec![AssistantReply::text(json)]);
    let ui = FakeUi::default();
    let plan =
        generate_step_plan_with_ui(&mut planner, "test", &config(dir.path().to_path_buf()), &ui)
            .unwrap();
    assert_eq!(plan.steps.len(), 1);
    assert!(
        ui.events()
            .iter()
            .any(|event| event == "model:planner planner pm")
    );
}

#[test]
fn tui_ultra_plan_run_smoke_fake_clients() {
    let dir = tempfile::tempdir().unwrap();
    let mut step_plan = anvilminimal::planner::step_plan::StepPlan::single("write app");
    step_plan.steps[0].kind = "implement".to_string();
    step_plan.steps[0]
        .expected_paths
        .push("app.txt".to_string());
    let step_json = serde_json::to_string(&step_plan).unwrap();
    let plan = anvilminimal::planner::ultra_plan::UltraPlan {
        goal: "build app".to_string(),
        profile: "generic".to_string(),
        style: "default".to_string(),
        intent: "create".to_string(),
        phases: vec![
            anvilminimal::planner::ultra_plan::UltraPhase {
                id: "p1".to_string(),
                prompt: "phase 1".to_string(),
            },
            anvilminimal::planner::ultra_plan::UltraPhase {
                id: "p2".to_string(),
                prompt: "phase 2".to_string(),
            },
        ],
    };
    let mut planner = FakeClient::new(
        "planner",
        vec![
            AssistantReply::text(step_json.clone()),
            AssistantReply::text(step_json),
        ],
    );
    let mut execution = FakeClient::new(
        "exec",
        vec![
            AssistantReply {
                content: String::new(),
                tool_calls: vec![ToolCall::new(
                    "Write",
                    json!({"path":"app.txt","content":"phase1"}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            },
            AssistantReply {
                content: String::new(),
                tool_calls: vec![ToolCall::new(
                    "Write",
                    json!({"path":"app.txt","content":"phase2"}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            },
        ],
    );
    let ui = FakeUi::default();
    let result = run_ultra_plan_with_ui(
        &mut planner,
        &mut execution,
        &plan,
        &config(dir.path().to_path_buf()),
        &ui,
    )
    .unwrap();
    assert_eq!(result, "ultra-plan-run complete: 2 phases");
    assert_eq!(planner.requests.len(), 2);
    assert_eq!(planner.requests[0].len(), planner.requests[1].len());
    assert_eq!(execution.requests.len(), 2);
    assert!(
        execution.requests[1].len() > execution.requests[0].len(),
        "phase 2 execution should reuse the ultra execution session"
    );
    let second_request = format!("{:?}", execution.requests[1]);
    assert!(second_request.contains("phase1"));
}

#[test]
fn tui_slash_failure_records_run_events_and_failure_stage() {
    let dir = tempfile::tempdir().unwrap();
    let events_path = dir.path().join(".anvil/runs/test/events.jsonl");
    let mut cfg = config(dir.path().to_path_buf());
    cfg.eval_events_path = Some(events_path.clone());
    let mut planner = FakeClient::new("planner", Vec::new());
    let mut execution = FakeClient::new("exec", Vec::new());
    let ui = FakeUi::default();
    let err = anvilminimal::tui::slash::handle_command(
        "/unknown-command test",
        &cfg,
        &mut planner,
        &mut execution,
        &ui,
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("unknown slash command"));
    let events = std::fs::read_to_string(&events_path).unwrap();
    assert!(events.contains("\"event\":\"tui_command_start\""));
    assert!(events.contains("\"event\":\"tui_command_stop\""));
    assert!(events.contains("\"event\":\"loop_stop\""));
    assert!(events.contains("\"failure_kind\":\"tui_command_failed\""));
    assert!(events.contains("\"lifecycle_stage\":\"tui_command\""));
    let summary =
        std::fs::read_to_string(events_path.parent().unwrap().join("summary.md")).unwrap();
    assert!(summary.contains("TUI command failed"));
}

#[test]
fn plain_renderer_keeps_raw_output() {
    let renderer = anvilminimal::tui::markdown::PlainRenderer;
    renderer
        .render_assistant("<think>secret</think>raw")
        .expect("plain renderer writes");
}
