use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

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
        prompt_layout: anvilminimal::config::PromptLayout::Stable,
        planner_model: "pm".to_string(),
        planner_provider: Provider::Gemini,
        ollama_host: "http://localhost:11434".to_string(),
        num_predict: 100,
        max_iterations: 4,
        chat_timeout_secs: 1,
        chat_timeout_source: "override:test".to_string(),
        field_sources: anvilminimal::config::ConfigFieldSources::default(),
        chat_retries: 1,
        resume: None,
        fresh_session: false,
        no_footer: false,
        narration: anvilminimal::config::NarrationMode::Normal,
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
        assert!(
            !self.replies.is_empty(),
            "{} fake replies exhausted",
            self.label
        );
        Ok(self.replies.remove(0))
    }
}

struct PanicClient {
    label: &'static str,
    message: &'static str,
}

impl PanicClient {
    fn new(label: &'static str, message: &'static str) -> Self {
        Self { label, message }
    }
}

impl ChatClient for PanicClient {
    fn label(&self) -> &str {
        self.label
    }

    fn supports_native_tools(&self, _model: &str) -> bool {
        true
    }

    fn chat(
        &mut self,
        _model: &str,
        _messages: &[ConversationMessage],
        _tools: &[ToolSpec],
        _native_tools_enabled: bool,
    ) -> anyhow::Result<AssistantReply> {
        panic!("{}", self.message);
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

#[derive(Clone)]
struct SleepingCloneClient {
    label: &'static str,
    sleep: Duration,
    calls: Arc<AtomicUsize>,
}

impl SleepingCloneClient {
    fn new(label: &'static str, sleep: Duration) -> Self {
        Self {
            label,
            sleep,
            calls: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl ChatClient for SleepingCloneClient {
    fn label(&self) -> &str {
        self.label
    }

    fn supports_native_tools(&self, _model: &str) -> bool {
        true
    }

    fn boxed_clone(&self) -> Option<Box<dyn ChatClient>> {
        Some(Box::new(self.clone()))
    }

    fn chat(
        &mut self,
        _model: &str,
        _messages: &[ConversationMessage],
        _tools: &[ToolSpec],
        _native_tools_enabled: bool,
    ) -> anyhow::Result<AssistantReply> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        std::thread::sleep(self.sleep);
        Ok(AssistantReply::text("late"))
    }
}

struct TimedInterruptUi {
    events: Mutex<Vec<String>>,
    interrupt_at: Instant,
    force_at: Option<Instant>,
}

impl TimedInterruptUi {
    fn new(interrupt_after: Duration) -> Self {
        Self {
            events: Mutex::new(Vec::new()),
            interrupt_at: Instant::now() + interrupt_after,
            force_at: None,
        }
    }
}

impl InteractionUi for TimedInterruptUi {
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
        Instant::now() >= self.interrupt_at
    }

    fn force_interrupted(&self) -> bool {
        self.force_at
            .is_some_and(|force_at| Instant::now() >= force_at)
    }
}

struct ToolTimedInterruptUi {
    events: Mutex<Vec<String>>,
    tool_started_at: Mutex<Option<Instant>>,
    interrupt_after: Duration,
    force_after: Duration,
}

impl ToolTimedInterruptUi {
    fn new(interrupt_after: Duration, force_after: Duration) -> Self {
        Self {
            events: Mutex::new(Vec::new()),
            tool_started_at: Mutex::new(None),
            interrupt_after,
            force_after,
        }
    }

    fn elapsed_since_tool_start(&self) -> Option<Duration> {
        let started = *self.tool_started_at.lock().unwrap();
        started.map(|started| started.elapsed())
    }
}

impl InteractionUi for ToolTimedInterruptUi {
    fn before_model_call(&self, label: &str) -> UiGuard {
        self.events.lock().unwrap().push(format!("model:{label}"));
        UiGuard::noop()
    }

    fn before_tool_call(&self, name: &str) -> UiGuard {
        self.events.lock().unwrap().push(format!("tool:{name}"));
        let mut started = self.tool_started_at.lock().unwrap();
        if started.is_none() {
            *started = Some(Instant::now());
        }
        UiGuard::noop()
    }

    fn publish_status(&self, status: UiStatus) {
        self.events
            .lock()
            .unwrap()
            .push(format!("status:{}:{}", status.provider, status.model));
    }

    fn interrupted(&self) -> bool {
        self.elapsed_since_tool_start()
            .is_some_and(|elapsed| elapsed >= self.interrupt_after)
    }

    fn force_interrupted(&self) -> bool {
        self.elapsed_since_tool_start()
            .is_some_and(|elapsed| elapsed >= self.force_after)
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

fn assert_in_order(text: &str, needles: &[&str]) {
    let mut offset = 0usize;
    for needle in needles {
        let Some(index) = text[offset..].find(needle) else {
            panic!("missing {needle:?} after byte {offset} in:\n{text}");
        };
        offset += index + needle.len();
    }
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

fn assert_recovery_artifacts_exist(root: &std::path::Path, events: &str) {
    let recovery_event = events
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .find(|event| {
            event
                .get("event")
                .and_then(|value| value.as_str())
                .is_some_and(|name| name == "recovery_prompt_saved")
        })
        .unwrap_or_else(|| panic!("missing recovery_prompt_saved event:\n{events}"));
    let prompt_path = recovery_event
        .get("recovery_prompt_path")
        .and_then(|value| value.as_str())
        .filter(|value| !value.is_empty())
        .expect("recovery prompt path");
    let plan_path = recovery_event
        .get("recovery_ultra_plan_path")
        .and_then(|value| value.as_str())
        .filter(|value| !value.is_empty())
        .expect("recovery ultra plan path");
    assert!(root.join(prompt_path).is_file(), "{prompt_path}");
    assert!(root.join(plan_path).is_file(), "{plan_path}");
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

fn nextjs_package_json(port: u16) -> String {
    format!(
        r#"{{"dependencies":{{"next":"^14.2.0","react":"^18.3.0","react-dom":"^18.3.0"}},"devDependencies":{{"typescript":"^5.5.0","@types/node":"^20.14.0","@types/react":"^18.3.0","@types/react-dom":"^18.3.0","tailwindcss":"^3.4.19","postcss":"^8.5.15","autoprefixer":"^10.4.20"}},"scripts":{{"build":"next build","dev":"next dev -p {port}","start":"next start -p {port}"}}}}"#
    )
}

fn nextjs_page_source() -> String {
    r#""use client";
import { useState } from "react";

export default function Page() {
  const [draft, setDraft] = useState("");
  const [items, setItems] = useState<string[]>([]);
  return <main data-anvil-state={items.length}>
    <input aria-label="Memo" value={draft} onChange={(event) => setDraft(event.target.value)} />
    <button data-anvil-action="primary" onClick={() => setItems([...items, draft])}>Add</button>
    <ul>{items.map((item, index) => <li key={index}>{item}</li>)}</ul>
  </main>;
}
"#
    .to_string()
}

#[cfg(unix)]
fn write_fake_nextjs_package_manager(root: &std::path::Path, port: u16) {
    use std::os::unix::fs::PermissionsExt;

    let bin = root.join("node_modules/.bin");
    std::fs::create_dir_all(&bin).unwrap();
    for package in ["next", "tailwindcss", "postcss", "autoprefixer"] {
        std::fs::create_dir_all(root.join("node_modules").join(package)).unwrap();
    }
    let exe = sh_quote(&std::env::current_exe().unwrap().display().to_string());
    let npm = format!(
        "#!/bin/sh\n\
if [ \"$1\" = \"run\" ] && [ \"$2\" = \"build\" ]; then\n\
  echo \"fake build ok\"\n\
  exit 0\n\
fi\n\
if [ \"$1\" = \"run\" ] && [ \"$2\" = \"dev\" ]; then\n\
  ANVIL_TUI_FAKE_DEV_SERVER_CHILD=1 ANVIL_TUI_FAKE_DEV_SERVER_PORT={port} exec {exe} --ignored --exact tui_fake_dev_server_child --nocapture\n\
fi\n\
if [ \"$1\" = \"run\" ] && [ \"$2\" = \"start\" ]; then\n\
  ANVIL_TUI_FAKE_DEV_SERVER_CHILD=1 ANVIL_TUI_FAKE_DEV_SERVER_PORT={port} exec {exe} --ignored --exact tui_fake_dev_server_child --nocapture\n\
fi\n\
echo \"unexpected fake npm args: $*\" >&2\n\
exit 2\n"
    );
    let npm_path = bin.join("npm");
    std::fs::write(&npm_path, npm).unwrap();
    let mut permissions = std::fs::metadata(&npm_path).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&npm_path, permissions).unwrap();

    let next_path = bin.join("next");
    std::fs::write(&next_path, "#!/bin/sh\nexit 0\n").unwrap();
    let mut permissions = std::fs::metadata(&next_path).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&next_path, permissions).unwrap();
}

#[cfg(unix)]
fn sh_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

#[cfg(unix)]
fn free_local_port() -> u16 {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    listener.local_addr().unwrap().port()
}

#[test]
#[ignore]
fn tui_fake_dev_server_child() {
    if std::env::var("ANVIL_TUI_FAKE_DEV_SERVER_CHILD")
        .ok()
        .as_deref()
        != Some("1")
    {
        return;
    }
    let port = std::env::var("ANVIL_TUI_FAKE_DEV_SERVER_PORT")
        .unwrap()
        .parse::<u16>()
        .unwrap();
    let listener = std::net::TcpListener::bind(("127.0.0.1", port)).unwrap();
    for stream in listener.incoming() {
        let mut stream = stream.unwrap();
        let body = r#"<!doctype html><html><head><title>Memo</title></head><body><main data-anvil-state="{&quot;items&quot;:0,&quot;draft&quot;:&quot;&quot;}"><label>Memo <input id="memo" aria-label="Memo" /></label><button id="add" data-anvil-action="primary">Add</button><ul id="items"></ul></main><script>const main=document.querySelector("main");const memo=document.getElementById("memo");const add=document.getElementById("add");const items=document.getElementById("items");let count=0;function sync(){main.setAttribute("data-anvil-state",JSON.stringify({items:count,draft:memo.value}));}memo.addEventListener("input",sync);add.addEventListener("click",()=>{count+=1;const li=document.createElement("li");li.textContent=memo.value||"memo";items.appendChild(li);sync();});</script></body></html>"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        let _ = std::io::Write::write_all(&mut stream, response.as_bytes());
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
fn primary_ultra_plan_run_renders_plan_then_activity_then_summary() {
    let dir = tempfile::tempdir().unwrap();
    let events_path = dir.path().join(".anvil/runs/test/events.jsonl");
    let mut cfg = config(dir.path().to_path_buf());
    cfg.eval_events_path = Some(events_path);
    let plan_text = anvilminimal::planner::ultra_plan::render_ultra_plan(&two_phase_ultra_plan());
    let step_json = implement_step_plan_json();
    let mut planner = FakeClient::new(
        "planner",
        vec![
            AssistantReply::text(plan_text),
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
    let _presentation = anvilminimal::tui::presentation::install(&cfg);
    let capture = anvilminimal::tui::markdown::capture::start();

    let output = anvilminimal::tui::slash::handle_command(
        "/ultra-plan-run build app",
        &cfg,
        &mut planner,
        &mut execution,
        &ui,
    )
    .unwrap();
    let rendered = format!("{}\n{output}", capture.output());

    assert_in_order(
        &rendered,
        &[
            "### Plan",
            "── Phase 1/2: p1 ──",
            "#### Phase: p1",
            "→ Write app.jsx",
            "✓ Write ok",
            "### Terminal summary",
        ],
    );
}

#[test]
fn in_flight_provider_interrupt_finishes_before_sleep_and_writes_terminal_records() {
    let dir = tempfile::tempdir().unwrap();
    let events_path = dir.path().join(".anvil/runs/test/events.jsonl");
    let plan_path = write_ultra_plan(dir.path());
    let mut cfg = config(dir.path().to_path_buf());
    cfg.eval_events_path = Some(events_path.clone());
    cfg.chat_timeout_secs = 30;
    let mut planner = SleepingCloneClient::new("planner", Duration::from_secs(30));
    let mut execution = FakeClient::new("exec", Vec::new());
    let ui = TimedInterruptUi::new(Duration::from_secs(1));

    let started = Instant::now();
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
    assert!(started.elapsed() < Duration::from_secs(5));
    assert_eq!(planner.calls(), 1, "provider abort must not retry");
    let events = std::fs::read_to_string(&events_path).unwrap();
    assert!(events.contains("\"event\":\"provider_turn_aborted_by_user\""));
    assert!(events.contains("\"classification\":\"aborted_by_user\""));
    assert!(!events.contains("\"event\":\"provider_turn_timeout\""));
    assert_exactly_one_tui_stop(&events, "interrupted");
    assert!(events.contains("\"failure_kind\":\"tui_command_interrupted\""));
    assert_recovery_artifacts_exist(dir.path(), &events);
    let summary =
        std::fs::read_to_string(events_path.parent().unwrap().join("summary.md")).unwrap();
    assert_terminal_summary(&summary, "interrupted");
    assert!(summary.contains("Command status: interrupted"));
}

#[test]
fn in_flight_bash_interrupt_force_finalizes_without_waiting_for_grace() {
    let dir = tempfile::tempdir().unwrap();
    let events_path = dir.path().join(".anvil/runs/test/events.jsonl");
    let plan_path = write_ultra_plan(dir.path());
    let mut cfg = config(dir.path().to_path_buf());
    cfg.eval_events_path = Some(events_path.clone());
    let step_json = implement_step_plan_json();
    let mut planner = FakeClient::new("planner", vec![AssistantReply::text(step_json)]);
    let mut execution = FakeClient::new(
        "exec",
        vec![AssistantReply {
            content: String::new(),
            tool_calls: vec![ToolCall::new(
                "Bash",
                json!({"command": "trap '' TERM; while :; do :; done"}),
            )],
            prompt_tokens: None,
            completion_tokens: None,
        }],
    );
    let ui = ToolTimedInterruptUi::new(Duration::from_millis(100), Duration::from_millis(300));

    let started = Instant::now();
    let err = anvilminimal::tui::slash::handle_command(
        &format!("/run-ultra-plan {plan_path}"),
        &cfg,
        &mut planner,
        &mut execution,
        &ui,
    )
    .unwrap_err()
    .to_string();

    assert!(err.contains("command_aborted_by_user"), "{err}");
    assert!(err.contains("interrupted by user"), "{err}");
    assert!(started.elapsed() < Duration::from_secs(2));
    let events = std::fs::read_to_string(&events_path).unwrap();
    assert!(events.contains("\"error_kind\":\"command_aborted_by_user\""));
    assert_exactly_one_tui_stop(&events, "interrupted");
    assert!(events.contains("\"failure_kind\":\"tui_command_interrupted\""));
    let summary =
        std::fs::read_to_string(events_path.parent().unwrap().join("summary.md")).unwrap();
    assert_terminal_summary(&summary, "interrupted");
    assert!(summary.contains("Command status: interrupted"));
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
fn tui_slash_promoted_profile_reflected_in_terminal_summary() {
    let dir = tempfile::tempdir().unwrap();
    #[cfg(unix)]
    let port = free_local_port();
    #[cfg(not(unix))]
    let port = 3011;
    let events_path = dir.path().join(".anvil/runs/test/events.jsonl");
    let plan_path = dir.path().join("ultra.yaml");
    let goal = format!(
        "ちょっとしたメモアプリを作って。ブラウザで使えるようにしてください。{port}ポートで起動可能にしてください。"
    );
    let plan = anvilminimal::planner::ultra_plan::UltraPlan {
        goal,
        profile: "generic".to_string(),
        style: "default".to_string(),
        intent: "create".to_string(),
        phases: vec![
            anvilminimal::planner::ultra_plan::UltraPhase {
                id: "setup-framework".to_string(),
                prompt: "Create the package manifest".to_string(),
            },
            anvilminimal::planner::ultra_plan::UltraPhase {
                id: "implement-ui".to_string(),
                prompt: "Create the promoted Next.js route".to_string(),
            },
        ],
    };
    std::fs::write(
        &plan_path,
        anvilminimal::planner::ultra_plan::render_ultra_plan(&plan),
    )
    .unwrap();
    let mut setup_step = anvilminimal::planner::step_plan::StepPlan::single("create package");
    setup_step.steps[0].kind = "setup".to_string();
    setup_step.steps[0].expected_paths = vec!["package.json".to_string()];
    let mut route_step = anvilminimal::planner::step_plan::StepPlan::single("create route");
    route_step.steps[0].kind = "implement".to_string();
    route_step.steps[0].expected_paths = vec![
        "tsconfig.json".to_string(),
        "postcss.config.js".to_string(),
        "tailwind.config.ts".to_string(),
        "src/app/layout.tsx".to_string(),
        "src/app/page.tsx".to_string(),
        "src/app/globals.css".to_string(),
        "src/app/global.d.ts".to_string(),
    ];
    #[cfg(unix)]
    {
        write_fake_nextjs_package_manager(dir.path(), port);
    }
    let mut cfg = config(dir.path().to_path_buf());
    cfg.eval_events_path = Some(events_path.clone());
    let mut planner = FakeClient::new(
        "planner",
        vec![
            AssistantReply::text(serde_json::to_string(&setup_step).unwrap()),
            AssistantReply::text(serde_json::to_string(&route_step).unwrap()),
        ],
    );
    let mut execution = FakeClient::new(
        "exec",
        vec![
            AssistantReply {
                content: String::new(),
                tool_calls: vec![ToolCall::new(
                    "Write",
                    json!({"path":"package.json","content":nextjs_package_json(port)}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            },
            AssistantReply {
                content: String::new(),
                tool_calls: vec![
                    ToolCall::new(
                        "Write",
                        json!({"path":"src/app/layout.tsx","content":"export default function RootLayout({ children }: { children: React.ReactNode }) { return <html><body>{children}</body></html>; }"}),
                    ),
                    ToolCall::new(
                        "Write",
                        json!({"path":"tsconfig.json","content":r#"{"compilerOptions":{"target":"ES2017","lib":["dom","dom.iterable","esnext"],"allowJs":true,"skipLibCheck":true,"strict":true,"noEmit":true,"esModuleInterop":true,"module":"esnext","moduleResolution":"bundler","resolveJsonModule":true,"isolatedModules":true,"jsx":"preserve","incremental":true,"plugins":[{"name":"next"}]},"include":["next-env.d.ts","**/*.ts","**/*.tsx",".next/types/**/*.ts"],"exclude":["node_modules"]}"#}),
                    ),
                    ToolCall::new(
                        "Write",
                        json!({"path":"postcss.config.js","content":"module.exports = { plugins: { tailwindcss: {}, autoprefixer: {} } };"}),
                    ),
                    ToolCall::new(
                        "Write",
                        json!({"path":"tailwind.config.ts","content":"import type { Config } from 'tailwindcss';\nconst config: Config = { content: ['./src/app/**/*.{js,ts,jsx,tsx,mdx}'], theme: { extend: {} }, plugins: [] };\nexport default config;\n"}),
                    ),
                    ToolCall::new(
                        "Write",
                        json!({"path":"src/app/page.tsx","content":nextjs_page_source()}),
                    ),
                    ToolCall::new(
                        "Write",
                        json!({"path":"src/app/globals.css","content":"body { font-family: sans-serif; }"}),
                    ),
                    ToolCall::new(
                        "Write",
                        json!({"path":"src/app/global.d.ts","content":"declare module '*.css';"}),
                    ),
                    ToolCall::new(
                        "Write",
                        json!({"path":"browser-readiness.json","content":r#"{"ok":true,"http_status":200,"route_rendered":true}"#}),
                    ),
                    ToolCall::new(
                        "Write",
                        json!({"path":"browser-interaction.json","content":r#"{"ok":true,"status":"passed","interaction_success":true,"interaction_performed":true,"surface_visible":true,"start_transition":true,"input_state_change":true,"input_state_evaluated_after_start":true,"input_event_observed":true,"state_changed":true,"canvas_found":true}"#}),
                    ),
                ],
                prompt_tokens: None,
                completion_tokens: None,
            },
        ],
    );
    let ui = FakeUi::default();

    let output = anvilminimal::tui::slash::handle_command(
        "/run-ultra-plan ultra.yaml",
        &cfg,
        &mut planner,
        &mut execution,
        &ui,
    )
    .unwrap();

    assert!(output.contains("ultra-plan-run complete"));
    let events = std::fs::read_to_string(&events_path).unwrap();
    assert!(
        events.contains(r#""event":"profile_reinferred""#),
        "{events}"
    );
    assert!(events.contains(r#""profile":"nextjs""#), "{events}");
    let requested_port = format!("{port} (goal)");
    assert!(
        events.contains(&format!(r#""requested_port":"{requested_port}""#)),
        "{events}"
    );
    let stops = tui_command_stop_events(&events);
    assert_eq!(
        stops[0].get("profile").and_then(|value| value.as_str()),
        Some("nextjs"),
        "{events}"
    );
    assert_eq!(
        stops[0]
            .get("effective_profile")
            .and_then(|value| value.as_str()),
        Some("nextjs"),
        "{events}"
    );
    assert_eq!(
        stops[0]
            .get("contract_origin")
            .and_then(|value| value.as_str()),
        Some("promoted_union"),
        "{events}"
    );
    assert_eq!(
        stops[0]
            .get("assurance_level")
            .and_then(|value| value.as_str()),
        Some("full"),
        "{events}"
    );
    assert_eq!(
        stops[0]
            .get("requested_port")
            .and_then(|value| value.as_str()),
        Some(requested_port.as_str()),
        "{events}"
    );
    let summary =
        std::fs::read_to_string(events_path.parent().unwrap().join("summary.md")).unwrap();
    assert!(
        summary.contains("Profile promoted: generic -> nextjs"),
        "{summary}"
    );
    assert!(summary.contains("Profile: nextjs"), "{summary}");
    assert!(
        summary.contains(&format!("Requested port: {requested_port}")),
        "{summary}"
    );
}

#[test]
fn tui_runs_lists_recent_runs_without_emitting_command_events() {
    let dir = tempfile::tempdir().unwrap();
    let events_path = dir.path().join(".anvil/runs/current/events.jsonl");
    let previous_run = dir.path().join(".anvil/runs/018f2222-bbbb");
    std::fs::create_dir_all(&previous_run).unwrap();
    std::fs::write(
        previous_run.join("events.jsonl"),
        serde_json::json!({
            "event": "tui_command_stop",
            "ok": false,
            "status": "failed",
            "task_status": "failed",
            "assurance_level": "full",
            "runtime_acceptance_status": "pass",
            "final_acceptance_status": "failed",
            "release_gate_status": "failed",
            "stop_reason": "failed because recovery is available",
            "recovery_ultra_plan_path": ".anvil/plans/recovery-ultra-plan-test.yaml"
        })
        .to_string()
            + "\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.path().join(".anvil/plans")).unwrap();
    std::fs::write(
        dir.path()
            .join(".anvil/plans/recovery-ultra-plan-test.yaml"),
        "goal: \"g\"\nphases:\n  - id: \"p\"\n    prompt: \"p\"\n",
    )
    .unwrap();
    let mut cfg = config(dir.path().to_path_buf());
    cfg.eval_events_path = Some(events_path.clone());
    let mut planner = FakeClient::new("planner", Vec::new());
    let mut execution = FakeClient::new("exec", Vec::new());
    let ui = FakeUi::default();

    let output =
        anvilminimal::tui::slash::handle_command("/runs", &cfg, &mut planner, &mut execution, &ui)
            .unwrap();

    assert!(output.contains("018f2222"), "{output}");
    assert!(output.contains("failed/partial"), "{output}");
    assert!(output.contains("yaml"), "{output}");
    assert!(
        !events_path.exists(),
        "/runs should not emit command events"
    );
}

#[test]
fn tui_help_lists_recovery_commands_without_emitting_events() {
    let dir = tempfile::tempdir().unwrap();
    let events_path = dir.path().join(".anvil/runs/current/events.jsonl");
    let mut cfg = config(dir.path().to_path_buf());
    cfg.eval_events_path = Some(events_path.clone());
    let mut planner = FakeClient::new("planner", Vec::new());
    let mut execution = FakeClient::new("exec", Vec::new());
    let ui = FakeUi::default();

    let output =
        anvilminimal::tui::slash::handle_command("/help", &cfg, &mut planner, &mut execution, &ui)
            .unwrap();

    assert!(output.contains("/runs - list recent runs"), "{output}");
    assert!(
        output.contains("/resume [run-id|yaml-path] - resume from a recovery UltraPlan"),
        "{output}"
    );
    assert!(
        output.contains("/plan - show the active plan and current activity"),
        "{output}"
    );
    assert!(output.contains("/exit or /quit"), "{output}");
    assert!(
        !events_path.exists(),
        "/help should not emit command events"
    );
}

#[test]
fn tui_resume_runs_recovery_plan_remaining_phases_and_records_lineage() {
    let dir = tempfile::tempdir().unwrap();
    let events_path = dir.path().join(".anvil/runs/current/events.jsonl");
    std::fs::create_dir_all(dir.path().join(".anvil/plans")).unwrap();
    std::fs::write(dir.path().join("setup.txt"), "done").unwrap();
    std::fs::write(
        dir.path()
            .join(".anvil/plans/recovery-ultra-plan-test.yaml"),
        r#"
recovery_schema_version: "1"
recovery_original_goal: "build app"
recovery_failure_kind: "interrupted"
recovery_profile: "generic"
recovery_expected_completed_artifacts:
  - "setup.txt"
goal: "build app"
profile: "generic"
style: "recovery"
intent: "recover"
phases:
  - id: "repair-phase"
    prompt: "repair the remaining implementation"
  - id: "verify-recovery"
    prompt: "verify the recovered implementation"
"#
        .trim_start(),
    )
    .unwrap();
    let previous_run = dir.path().join(".anvil/runs/018f6666-resumable");
    std::fs::create_dir_all(&previous_run).unwrap();
    std::fs::write(
        previous_run.join("events.jsonl"),
        format!(
            "{}\n{}\n",
            json!({
                "event": "ultra_partial_artifact_summary",
                "completed_phase_ids": ["scaffold"],
                "failed_phase_id": "repair-phase",
                "pending_phase_ids": ["verify"],
                "recovery_ultra_plan_path": ".anvil/plans/recovery-ultra-plan-test.yaml"
            }),
            json!({
                "event": "tui_command_stop",
                "ok": false,
                "status": "interrupted",
                "assurance_level": "partial",
                "failure_kind": "tui_command_interrupted",
                "effective_profile": "generic",
                "requested_port": "",
                "recovery_ultra_plan_path": ".anvil/plans/recovery-ultra-plan-test.yaml"
            })
        ),
    )
    .unwrap();
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
                    json!({"path":"app.jsx","content":interactive_app_source("repaired")}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            },
            AssistantReply {
                content: String::new(),
                tool_calls: vec![ToolCall::new(
                    "Write",
                    json!({"path":"app.jsx","content":interactive_app_source("verified")}),
                )],
                prompt_tokens: None,
                completion_tokens: None,
            },
        ],
    );
    let ui = FakeUi::default();

    let output = anvilminimal::tui::slash::handle_command(
        "/resume 018f6666",
        &cfg,
        &mut planner,
        &mut execution,
        &ui,
    )
    .unwrap();

    assert!(output.contains("### Resume recovery run"), "{output}");
    assert!(
        output.contains("completed phases skipped: scaffold"),
        "{output}"
    );
    assert!(output.contains("- Resumed from: 018f6666"), "{output}");
    assert!(
        output.contains("phases to run: repair-phase, verify-recovery"),
        "{output}"
    );
    assert!(
        output.contains("ultra-plan-run complete: 2 phases"),
        "{output}"
    );
    assert_eq!(planner.requests.len(), 2);
    assert_eq!(execution.requests.len(), 2);
    let events = std::fs::read_to_string(&events_path).unwrap();
    assert!(events.contains("\"event\":\"resume_start\""), "{events}");
    assert!(events.contains("\"resumed_from\":\"018f6666\""), "{events}");
    let phase_starts = events
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter(|event| {
            event.get("event").and_then(|value| value.as_str()) == Some("ultra_phase_start")
        })
        .map(|event| {
            event
                .get("phase_id")
                .and_then(|value| value.as_str())
                .unwrap()
                .to_string()
        })
        .collect::<Vec<_>>();
    assert_eq!(phase_starts, vec!["repair-phase", "verify-recovery"]);
    let stop = tui_command_stop_events(&events).pop().unwrap();
    assert_eq!(
        stop.get("resumed_from").and_then(|value| value.as_str()),
        Some("018f6666")
    );

    let plan_output =
        anvilminimal::tui::slash::handle_command("/plan", &cfg, &mut planner, &mut execution, &ui)
            .unwrap();
    assert!(plan_output.contains("### Plan"), "{plan_output}");
    assert!(plan_output.contains("repair-phase"), "{plan_output}");
    assert!(plan_output.contains("verify-recovery"), "{plan_output}");
    assert!(
        plan_output.contains("Current activity: ✓ Write ok"),
        "{plan_output}"
    );
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
fn tui_slash_ultra_panic_records_diagnostics_and_terminal_summary() {
    let dir = tempfile::tempdir().unwrap();
    let events_path = dir.path().join(".anvil/runs/test/events.jsonl");
    let plan_path = write_ultra_plan(dir.path());
    let mut cfg = config(dir.path().to_path_buf());
    cfg.eval_events_path = Some(events_path.clone());
    let step_json = implement_step_plan_json();
    let mut planner = FakeClient::new("planner", vec![AssistantReply::text(step_json)]);
    let mut execution = PanicClient::new("exec", "simulated ultra panic 日本語");
    let ui = FakeUi::default();

    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = anvilminimal::tui::slash::handle_command(
            &format!("/run-ultra-plan {plan_path}"),
            &cfg,
            &mut planner,
            &mut execution,
            &ui,
        );
    }));
    assert!(panic.is_err());

    let events = std::fs::read_to_string(&events_path).unwrap();
    let panic_event = events
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .find(|event| {
            event
                .get("event")
                .and_then(|value| value.as_str())
                .is_some_and(|name| name == "panic_caught")
        })
        .unwrap_or_else(|| panic!("missing panic_caught in {events}"));
    assert_eq!(
        panic_event.get("message").and_then(|value| value.as_str()),
        Some("simulated ultra panic 日本語"),
        "{panic_event}"
    );
    assert!(
        panic_event
            .get("location")
            .and_then(|value| value.as_str())
            .is_some_and(|location| location.contains("tests/tui_integration.rs:")),
        "{panic_event}"
    );
    assert_exactly_one_tui_stop(&events, "aborted");
    assert!(events.contains("\"failure_kind\":\"tui_command_aborted\""));
    let summary =
        std::fs::read_to_string(events_path.parent().unwrap().join("summary.md")).unwrap();
    assert_terminal_summary(&summary, "aborted");
    assert!(summary.contains("Command status: aborted"));
    assert!(summary.contains("Failure kind: tui_command_aborted"));
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
    assert_recovery_artifacts_exist(dir.path(), &events);
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
