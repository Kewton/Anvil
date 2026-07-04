use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

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
        profile_explicit: false,
        profile_inference: None,
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

struct InterruptAfterUi {
    events: Mutex<Vec<String>>,
    checks: AtomicUsize,
    interrupt_after: usize,
}

impl InterruptAfterUi {
    fn new(interrupt_after: usize) -> Self {
        Self {
            events: Mutex::new(Vec::new()),
            checks: AtomicUsize::new(0),
            interrupt_after,
        }
    }
}

impl InteractionUi for InterruptAfterUi {
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
        self.checks.fetch_add(1, Ordering::SeqCst) >= self.interrupt_after
    }
}

fn tui_command_stop_events(text: &str) -> Vec<serde_json::Value> {
    text.lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter(|event| {
            event
                .get("event")
                .and_then(|value| value.as_str())
                .is_some_and(|name| name == "tui_command_stop")
        })
        .collect()
}

fn assert_exactly_one_tui_stop(text: &str, status: &str) {
    let stops = tui_command_stop_events(text);
    assert_eq!(stops.len(), 1, "{text}");
    assert_eq!(
        stops[0].get("status").and_then(|value| value.as_str()),
        Some(status),
        "{text}"
    );
}

fn assert_terminal_summary(summary: &str, status: &str) {
    assert!(
        summary.starts_with(&format!("{}\n", anvilminimal::build_info::summary_line())),
        "{summary}"
    );
    let expected = format!("Status: {status}");
    assert_eq!(
        summary.lines().find(|line| line.starts_with("Status: ")),
        Some(expected.as_str()),
        "{summary}"
    );
    assert!(!summary.contains("Status: running"), "{summary}");
}

fn two_phase_ultra_plan() -> anvilminimal::planner::ultra_plan::UltraPlan {
    anvilminimal::planner::ultra_plan::UltraPlan {
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
    }
}

fn write_ultra_plan(root: &std::path::Path) -> String {
    let path = root.join("ultra.yaml");
    std::fs::write(
        &path,
        anvilminimal::planner::ultra_plan::render_ultra_plan(&two_phase_ultra_plan()),
    )
    .unwrap();
    "ultra.yaml".to_string()
}

fn implement_step_plan_json() -> String {
    let mut step_plan = anvilminimal::planner::step_plan::StepPlan::single("write app");
    step_plan.steps[0].kind = "implement".to_string();
    step_plan.steps[0]
        .expected_paths
        .push("app.jsx".to_string());
    serde_json::to_string(&step_plan).unwrap()
}

fn interactive_app_source(label: &str) -> String {
    format!(
        r#"import {{ useState }} from "react";
export default function App() {{
  const [items, setItems] = useState([]);
  return <form onSubmit={{(event) => {{ event.preventDefault(); setItems([...items, "{label}"]); }}}}><input onChange={{() => setItems([...items, "{label}"])}} /><button type="submit">Add</button></form>;
}}
"#
    )
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
        .push("app.jsx".to_string());
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
                    json!({"path":"app.jsx","content":interactive_app_source("phase1")}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            },
            AssistantReply {
                content: String::new(),
                tool_calls: vec![ToolCall::new(
                    "Write",
                    json!({"path":"app.jsx","content":interactive_app_source("phase2")}),
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
    assert_exactly_one_tui_stop(&events, "failed");
    assert!(events.contains("\"event\":\"loop_stop\""));
    assert!(events.contains("\"failure_kind\":\"tui_command_failed\""));
    assert!(events.contains("\"lifecycle_stage\":\"tui_command\""));
    assert!(events.contains("\"task_status\":\"failed\""));
    assert!(events.contains("\"session_status\":\"repl_ready\""));
    assert!(events.contains("\"repl_status\":\"ready\""));
    assert!(events.contains("\"recovery_next_action\":\"fix_command_failure\""));
    let summary =
        std::fs::read_to_string(events_path.parent().unwrap().join("summary.md")).unwrap();
    assert_terminal_summary(&summary, "failed");
    assert!(summary.contains("Completion status: incomplete"));
    assert!(summary.contains("Command status: failed"));
    assert!(summary.contains("Task status: failed"));
    assert!(summary.contains("Process: REPL exited cleanly (not task status)"));
    assert!(summary.contains("Session/REPL status: repl_ready"));
    assert!(summary.contains("Recovery next action: fix_command_failure"));
    assert!(summary.contains("TUI command failed"));
    assert!(!summary.contains("\nStatus: complete\n"));
}

#[test]
fn tui_slash_failure_rewrites_existing_partial_summary_with_phase_breakdown() {
    let dir = tempfile::tempdir().unwrap();
    let events_path = dir.path().join(".anvil/runs/test/events.jsonl");
    let mut cfg = config(dir.path().to_path_buf());
    cfg.eval_events_path = Some(events_path.clone());
    anvilminimal::eval_events::write_run_summary(
        cfg.eval_events_path.as_deref(),
        "Status: incomplete\nCompleted phases:\n- scaffold\nFailed phase:\n- final\nPending phases:\n- none\nRecovery next action:\n- /run-ultra-plan .anvil/plans/recovery-ultra-plan-final.yaml",
    );
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
    let summary =
        std::fs::read_to_string(events_path.parent().unwrap().join("summary.md")).unwrap();
    assert_terminal_summary(&summary, "failed");
    assert!(summary.contains("Completed phases:\n- scaffold (completed)"));
    assert!(summary.contains("Failed phases:\n- final (failed)"));
    assert!(summary.contains("Pending phases:\n- none"));
    assert!(summary.contains("Recovery next action:"));
    assert!(summary.contains("TUI command failed"));
}

#[test]
fn tui_slash_success_with_partial_release_gate_is_not_complete_only() {
    let dir = tempfile::tempdir().unwrap();
    let events_path = dir.path().join(".anvil/runs/test/events.jsonl");
    let mut cfg = config(dir.path().to_path_buf());
    cfg.eval_events_path = Some(events_path.clone());
    anvilminimal::eval_events::emit(
        cfg.eval_events_path.as_deref(),
        json!({
            "event": "ultra_final_acceptance",
            "runtime_acceptance_passed": true,
            "runtime_acceptance_status": "pass",
            "final_acceptance_status": "partial",
            "release_gate_status": "partial",
            "release_gate_reasons": ["browser_readiness_or_interaction_evidence_required:browser_readiness_evidence_missing"],
            "browser_readiness_status": "unavailable:browser_readiness_evidence_missing",
            "interaction_evidence_status": "unavailable:interaction_evidence_missing",
            "recovery_prompt_path": ".anvil/repairs/repair-release.yaml.md",
            "recovery_ultra_plan_path": ".anvil/plans/recovery-ultra-plan-release.yaml",
            "suggested_recovery_command": "/ultra-plan-run --profile nextjs \"$(cat .anvil/repairs/repair-release.yaml.md)\"",
            "suggested_recovery_yaml_command": "/run-ultra-plan .anvil/plans/recovery-ultra-plan-release.yaml",
        }),
    );
    let plan_json = r#"{"goal":"test","steps":[{"id":"s1","kind":"report","instruction":"say done","expected_paths":[],"verify":[],"expected_result":"pass"}]}"#;
    let mut planner = FakeClient::new("planner", vec![AssistantReply::text(plan_json)]);
    let mut execution = FakeClient::new("exec", Vec::new());
    let ui = FakeUi::default();
    let output = anvilminimal::tui::slash::handle_command(
        "/plan-steps test",
        &cfg,
        &mut planner,
        &mut execution,
        &ui,
    )
    .unwrap();
    assert!(output.contains("Command completion: completed"));
    assert!(output.contains("Task status: partial"));
    assert!(output.contains("Runtime acceptance: pass"));
    assert!(output.contains("Final acceptance: partial"));
    assert!(output.contains("Release gate: partial"));
    assert!(
        output
            .contains("Next action: collect_missing_release_evidence_or_continue_release_recovery")
    );
    assert!(output.contains("Recovery UltraPlan: .anvil/plans/recovery-ultra-plan-release.yaml"));
    assert!(output.contains(
        "Suggested recovery command: /run-ultra-plan .anvil/plans/recovery-ultra-plan-release.yaml"
    ));
    let events = std::fs::read_to_string(&events_path).unwrap();
    assert!(events.contains("\"event\":\"tui_command_stop\""));
    assert_exactly_one_tui_stop(&events, "completed");
    assert!(events.contains("\"completion_status\":\"complete_with_partial_release_gate\""));
    assert!(events.contains("\"task_status\":\"partial\""));
    assert!(events.contains("\"session_status\":\"repl_ready\""));
    assert!(events.contains("\"release_gate_status\":\"partial\""));
    let summary =
        std::fs::read_to_string(events_path.parent().unwrap().join("summary.md")).unwrap();
    assert_terminal_summary(&summary, "completed");
    assert!(summary.contains("Completion status: complete_with_partial_release_gate"));
    assert!(summary.contains("Session/REPL status: repl_ready"));
    assert!(summary.contains("Command status: completed"));
    assert!(summary.contains("Command completion: completed"));
    assert!(summary.contains("Task status: partial"));
    assert!(summary.contains("Final acceptance: partial"));
    assert!(summary.contains("Release gate: partial"));
    assert!(summary.contains("Recovery handoff:"));
    assert!(summary.contains(
        "Suggested YAML command: /run-ultra-plan .anvil/plans/recovery-ultra-plan-release.yaml"
    ));
    assert!(!summary.contains("\nStatus: running\n"));
}

#[test]
fn tui_slash_completion_guard_records_aborted_on_panic() {
    let dir = tempfile::tempdir().unwrap();
    let events_path = dir.path().join(".anvil/runs/test/events.jsonl");
    let mut cfg = config(dir.path().to_path_buf());
    cfg.eval_events_path = Some(events_path.clone());
    let mut planner = FakeClient::new("planner", Vec::new());
    let mut execution = FakeClient::new("exec", Vec::new());
    let ui = FakeUi::default();

    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = anvilminimal::tui::slash::handle_command(
            "/plan-steps panic",
            &cfg,
            &mut planner,
            &mut execution,
            &ui,
        );
    }));
    assert!(panic.is_err());

    let events = std::fs::read_to_string(&events_path).unwrap();
    assert!(events.contains("\"event\":\"tui_command_start\""));
    assert_exactly_one_tui_stop(&events, "aborted");
    assert!(events.contains("\"failure_kind\":\"tui_command_aborted\""));
    let summary =
        std::fs::read_to_string(events_path.parent().unwrap().join("summary.md")).unwrap();
    assert_terminal_summary(&summary, "aborted");
    assert!(summary.contains("Command status: aborted"));
    assert!(summary.contains("Failure kind: tui_command_aborted"));
    assert!(summary.contains("Completed phases:\n- none"));
    assert!(summary.contains("Failed phases:\n- none"));
    assert!(summary.contains("Pending phases:\n- none"));
}

#[test]
fn tui_slash_completion_guard_records_interrupted_mid_phase() {
    let dir = tempfile::tempdir().unwrap();
    let events_path = dir.path().join(".anvil/runs/test/events.jsonl");
    let plan_path = write_ultra_plan(dir.path());
    let mut cfg = config(dir.path().to_path_buf());
    cfg.eval_events_path = Some(events_path.clone());
    let step_json = implement_step_plan_json();
    let mut planner = FakeClient::new("planner", vec![AssistantReply::text(step_json)]);
    let mut execution = FakeClient::new("exec", Vec::new());
    let ui = InterruptAfterUi::new(2);

    let err = anvilminimal::tui::slash::handle_command(
        &format!("/run-ultra-plan {plan_path}"),
        &cfg,
        &mut planner,
        &mut execution,
        &ui,
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("interrupted by user"), "{err}");

    let events = std::fs::read_to_string(&events_path).unwrap();
    assert!(events.contains("\"event\":\"ultra_phase_start\""));
    assert!(events.contains("\"event\":\"ultra_phase_failed\""));
    assert_exactly_one_tui_stop(&events, "interrupted");
    assert!(events.contains("\"failure_kind\":\"tui_command_interrupted\""));
    let summary =
        std::fs::read_to_string(events_path.parent().unwrap().join("summary.md")).unwrap();
    assert_terminal_summary(&summary, "interrupted");
    assert!(summary.contains("Command status: interrupted"));
    assert!(summary.contains("Failed phases:\n- p1 (interrupted)"));
    assert!(summary.contains("Pending phases:\n- p2 (pending)"));
}

#[test]
fn tui_slash_ultra_plan_completion_records_phase_breakdown_and_acceptance() {
    let dir = tempfile::tempdir().unwrap();
    let events_path = dir.path().join(".anvil/runs/test/events.jsonl");
    let plan_path = write_ultra_plan(dir.path());
    let mut cfg = config(dir.path().to_path_buf());
    cfg.eval_events_path = Some(events_path.clone());
    let step_json = implement_step_plan_json();
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
                    json!({"path":"app.jsx","content":interactive_app_source("phase1")}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            },
            AssistantReply {
                content: String::new(),
                tool_calls: vec![ToolCall::new(
                    "Write",
                    json!({"path":"app.jsx","content":interactive_app_source("phase2")}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            },
        ],
    );
    let ui = FakeUi::default();

    let output = anvilminimal::tui::slash::handle_command(
        &format!("/run-ultra-plan {plan_path}"),
        &cfg,
        &mut planner,
        &mut execution,
        &ui,
    )
    .unwrap();
    assert!(output.contains("ultra-plan-run complete: 2 phases"));

    let events = std::fs::read_to_string(&events_path).unwrap();
    assert!(events.contains("\"event\":\"ultra_final_acceptance\""));
    assert_exactly_one_tui_stop(&events, "completed");
    let summary =
        std::fs::read_to_string(events_path.parent().unwrap().join("summary.md")).unwrap();
    assert_terminal_summary(&summary, "completed");
    assert!(summary.contains("Completed phases:\n- p1 (completed)\n- p2 (completed)"));
    assert!(summary.contains("Failed phases:\n- none"));
    assert!(summary.contains("Pending phases:\n- none"));
    assert!(summary.contains("Final acceptance: full_success"));
}

#[test]
fn plain_renderer_keeps_raw_output() {
    let renderer = anvilminimal::tui::markdown::PlainRenderer;
    renderer
        .render_assistant("<think>secret</think>raw")
        .expect("plain renderer writes");
}
