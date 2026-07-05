use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use anvilminimal::config::{Action, Config, Provider};
use anvilminimal::planner::step_plan::{PlanStep, StepPlan};
use anvilminimal::planner::ultra_plan::{UltraPhase, UltraPlan, render_ultra_plan};
use anvilminimal::providers::{AssistantReply, ChatClient};
use anvilminimal::state::{ConversationMessage, ToolCall};
use anvilminimal::tools::registry::ToolSpec;
use anvilminimal::tui::status::UiStatus;
use anvilminimal::tui::{InteractionUi, UiGuard};
use serde_json::{Value, json};

#[derive(Clone, Copy, Debug)]
enum MatrixScenario {
    Nextjs,
    PythonCli,
    GenericStatic,
    GenericPromoted,
}

impl MatrixScenario {
    fn name(self) -> &'static str {
        match self {
            Self::Nextjs => "nextjs",
            Self::PythonCli => "python_cli",
            Self::GenericStatic => "generic-static",
            Self::GenericPromoted => "generic-promoted",
        }
    }

    fn configured_profile(self) -> &'static str {
        match self {
            Self::Nextjs => "nextjs",
            Self::PythonCli => "python-cli",
            Self::GenericStatic | Self::GenericPromoted => "generic",
        }
    }

    fn profile_explicit(self) -> bool {
        !matches!(self, Self::GenericPromoted)
    }

    fn plan(self) -> UltraPlan {
        match self {
            Self::Nextjs => UltraPlan {
                goal: "Create a route page".to_string(),
                profile: "nextjs".to_string(),
                style: "default".to_string(),
                intent: "create".to_string(),
                phases: vec![
                    UltraPhase {
                        id: "scaffold".to_string(),
                        prompt: "Create the project scaffold and release evidence.".to_string(),
                    },
                    UltraPhase {
                        id: "verify-surface".to_string(),
                        prompt: "Refresh the route page deterministically.".to_string(),
                    },
                ],
            },
            Self::PythonCli => UltraPlan {
                goal: "Build a Python CLI that reads a CSV file path argument and prints sum, average, max, and min for numeric columns.".to_string(),
                profile: "python-cli".to_string(),
                style: "default".to_string(),
                intent: "create".to_string(),
                phases: vec![
                    UltraPhase {
                        id: "python-cli".to_string(),
                        prompt: "Create and verify the Python CLI.".to_string(),
                    },
                    UltraPhase {
                        id: "verify-cli".to_string(),
                        prompt: "Refresh the Python CLI deterministically.".to_string(),
                    },
                ],
            },
            Self::GenericStatic => UltraPlan {
                goal: "ちょっとしたメモアプリを作って".to_string(),
                profile: "generic".to_string(),
                style: "default".to_string(),
                intent: "create".to_string(),
                phases: vec![
                    UltraPhase {
                        id: "scaffold".to_string(),
                        prompt: "Create a generic interactive source artifact.".to_string(),
                    },
                    UltraPhase {
                        id: "finish".to_string(),
                        prompt: "Record a static-tier marker without framework manifest.".to_string(),
                    },
                ],
            },
            Self::GenericPromoted => UltraPlan {
                goal: "Create a route page".to_string(),
                profile: "generic".to_string(),
                style: "default".to_string(),
                intent: "create".to_string(),
                phases: vec![
                    UltraPhase {
                        id: "setup-framework".to_string(),
                        prompt: "Create the package manifest.".to_string(),
                    },
                    UltraPhase {
                        id: "implement-ui".to_string(),
                        prompt: "Create the promoted route page.".to_string(),
                    },
                ],
            },
        }
    }
}

#[derive(Debug)]
struct Trace {
    scenario: MatrixScenario,
    events: Vec<Value>,
    summary: String,
    output: String,
}

#[test]
fn conformance_matrix_runs_ultra_lifecycle_paths() {
    for scenario in [
        MatrixScenario::Nextjs,
        MatrixScenario::PythonCli,
        MatrixScenario::GenericStatic,
        MatrixScenario::GenericPromoted,
    ] {
        let trace = run_matrix_scenario(scenario);
        assert!(
            trace.output.contains("ultra-plan-run complete"),
            "{} output:\n{}",
            scenario.name(),
            trace.output
        );
        assert_has_event(&trace, "tui_command_start");
        assert_has_event(&trace, "ultra_phase_start");
        assert_has_event(&trace, "ultra_final_acceptance");
        assert_has_event(&trace, "ultra_plan_complete");
        assert_has_event(&trace, "tui_command_stop");
        assert_eq!(
            events_named(&trace.events, "tui_command_stop").len(),
            1,
            "{} events:\n{}",
            scenario.name(),
            render_events(&trace.events)
        );
        assert!(
            trace
                .summary
                .starts_with(&format!("{}\n", anvilminimal::build_info::summary_line())),
            "{} summary:\n{}",
            scenario.name(),
            trace.summary
        );
        if matches!(scenario, MatrixScenario::GenericPromoted) {
            assert_has_event(&trace, "profile_reinferred");
        }
    }
}

fn run_matrix_scenario(scenario: MatrixScenario) -> Trace {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let events_path = root.join(".anvil/runs/conformance/events.jsonl");
    let plan = scenario.plan();
    std::fs::write(root.join("ultra.yaml"), render_ultra_plan(&plan)).unwrap();
    scenario_prepare_workspace(scenario, root, events_path.parent().unwrap());

    let mut cfg = config(root.to_path_buf());
    cfg.eval_events_path = Some(events_path.clone());
    cfg.profile = scenario.configured_profile().to_string();
    cfg.profile_explicit = scenario.profile_explicit();

    let mut planner = FakeClient::new("planner", scenario_planner_replies(scenario));
    let mut execution = FakeClient::new("execution", scenario_execution_replies(scenario));
    let ui = FakeUi::default();
    let output = match anvilminimal::tui::slash::handle_command(
        "/run-ultra-plan ultra.yaml",
        &cfg,
        &mut planner,
        &mut execution,
        &ui,
    ) {
        Ok(output) => output,
        Err(err) => {
            let events = read_events(&events_path);
            panic!(
                "{} failed: {err}\nevents:\n{}",
                scenario.name(),
                render_events(&events)
            );
        }
    };

    let events = read_events(&events_path);
    let summary = std::fs::read_to_string(events_path.parent().unwrap().join("summary.md"))
        .unwrap_or_default();
    Trace {
        scenario,
        events,
        summary,
        output,
    }
}

fn scenario_prepare_workspace(scenario: MatrixScenario, root: &Path, run_dir: &Path) {
    match scenario {
        MatrixScenario::Nextjs => {
            write_fake_npm_dependency_installer(root);
            write_fake_nextjs_dependencies_ready(root);
            write_browser_release_evidence(run_dir);
        }
        MatrixScenario::GenericPromoted => {
            write_fake_npm_dependency_installer(root);
            write_browser_release_evidence(run_dir);
        }
        MatrixScenario::PythonCli | MatrixScenario::GenericStatic => {}
    }
}

fn scenario_planner_replies(scenario: MatrixScenario) -> Vec<AssistantReply> {
    match scenario {
        MatrixScenario::Nextjs => vec![
            AssistantReply::text(step_plan_json(
                "Create Next.js workspace",
                "implement",
                nextjs_expected_paths(),
                vec!["npm run build".to_string()],
            )),
            AssistantReply::text(step_plan_json(
                "Refresh Next.js workspace",
                "implement",
                nextjs_expected_paths(),
                vec!["npm run build".to_string()],
            )),
        ],
        MatrixScenario::PythonCli => vec![
            AssistantReply::text(step_plan_json(
                "Create Python CSV CLI",
                "implement",
                vec![
                    "pyproject.toml".to_string(),
                    "src/csv_stats/main.py".to_string(),
                ],
                vec!["python -m compileall -q src".to_string()],
            )),
            AssistantReply::text(step_plan_json(
                "Refresh Python CSV CLI",
                "implement",
                vec![
                    "pyproject.toml".to_string(),
                    "src/csv_stats/main.py".to_string(),
                ],
                vec!["python -m compileall -q src".to_string()],
            )),
        ],
        MatrixScenario::GenericStatic => vec![
            AssistantReply::text(step_plan_json(
                "Create generic memo app source",
                "implement",
                vec!["memo.jsx".to_string()],
                Vec::new(),
            )),
            AssistantReply::text(step_plan_json(
                "Record generic static marker",
                "implement",
                vec!["generic-static.txt".to_string()],
                Vec::new(),
            )),
        ],
        MatrixScenario::GenericPromoted => vec![
            AssistantReply::text(step_plan_json(
                "Create package manifest",
                "setup",
                vec!["package.json".to_string()],
                Vec::new(),
            )),
            AssistantReply::text(step_plan_json(
                "Complete promoted Next.js app",
                "implement",
                nextjs_expected_paths()
                    .into_iter()
                    .filter(|path| path != "package.json")
                    .collect(),
                vec!["npm run build".to_string()],
            )),
        ],
    }
}

fn scenario_execution_replies(scenario: MatrixScenario) -> Vec<AssistantReply> {
    match scenario {
        MatrixScenario::Nextjs => vec![
            AssistantReply {
                content: String::new(),
                tool_calls: nextjs_tool_calls()
                    .into_iter()
                    .chain(browser_release_evidence_tool_calls())
                    .collect(),
                prompt_tokens: None,
                completion_tokens: None,
            },
            AssistantReply {
                content: String::new(),
                tool_calls: nextjs_tool_calls()
                    .into_iter()
                    .chain(browser_release_evidence_tool_calls())
                    .collect(),
                prompt_tokens: None,
                completion_tokens: None,
            },
        ],
        MatrixScenario::PythonCli => vec![
            AssistantReply {
                content: String::new(),
                tool_calls: vec![
                    ToolCall::new(
                        "Write",
                        json!({"path":"pyproject.toml","content":python_cli_pyproject()}),
                    ),
                    ToolCall::new(
                        "Write",
                        json!({"path":"src/csv_stats/main.py","content":python_cli_main()}),
                    ),
                ],
                prompt_tokens: None,
                completion_tokens: None,
            },
            AssistantReply {
                content: String::new(),
                tool_calls: vec![
                    ToolCall::new(
                        "Write",
                        json!({"path":"pyproject.toml","content":python_cli_pyproject()}),
                    ),
                    ToolCall::new(
                        "Write",
                        json!({"path":"src/csv_stats/main.py","content":python_cli_main()}),
                    ),
                ],
                prompt_tokens: None,
                completion_tokens: None,
            },
        ],
        MatrixScenario::GenericStatic => vec![
            write_reply("memo.jsx", generic_interactive_source()),
            write_reply("generic-static.txt", "static fallback observed\n"),
        ],
        MatrixScenario::GenericPromoted => vec![
            write_reply("package.json", nextjs_complete_package_json()),
            AssistantReply {
                content: String::new(),
                tool_calls: nextjs_tool_calls()
                    .into_iter()
                    .filter(|call| {
                        call.arguments.get("path").and_then(Value::as_str) != Some("package.json")
                    })
                    .chain(browser_release_evidence_tool_calls())
                    .collect(),
                prompt_tokens: None,
                completion_tokens: None,
            },
        ],
    }
}

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
        max_iterations: 6,
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
        if self.replies.is_empty() {
            anyhow::bail!("{} fake replies exhausted", self.label);
        }
        Ok(self.replies.remove(0))
    }
}

#[derive(Default)]
struct FakeUi {
    events: Mutex<Vec<String>>,
    interrupted: AtomicBool,
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

fn step_plan_json(
    goal: &str,
    kind: &str,
    expected_paths: Vec<String>,
    verify: Vec<String>,
) -> String {
    serde_json::to_string(&StepPlan {
        goal: goal.to_string(),
        steps: vec![PlanStep {
            id: "step-1".to_string(),
            kind: kind.to_string(),
            expected_result: "pass".to_string(),
            instruction: goal.to_string(),
            expected_paths,
            verify,
        }],
    })
    .unwrap()
}

fn write_reply(path: &str, content: &str) -> AssistantReply {
    AssistantReply {
        content: String::new(),
        tool_calls: vec![ToolCall::new(
            "Write",
            json!({"path": path, "content": content}),
        )],
        prompt_tokens: None,
        completion_tokens: None,
    }
}

fn nextjs_tool_calls() -> Vec<ToolCall> {
    vec![
        ToolCall::new(
            "Write",
            json!({"path":"package.json","content":nextjs_complete_package_json()}),
        ),
        ToolCall::new(
            "Write",
            json!({"path":"tsconfig.json","content":nextjs_tsconfig_json()}),
        ),
        ToolCall::new(
            "Write",
            json!({"path":"postcss.config.js","content":nextjs_postcss_config()}),
        ),
        ToolCall::new(
            "Write",
            json!({"path":"tailwind.config.ts","content":nextjs_tailwind_config_ts()}),
        ),
        ToolCall::new(
            "Write",
            json!({"path":"src/app/page.tsx","content":nextjs_page_source()}),
        ),
        ToolCall::new(
            "Write",
            json!({"path":"src/app/layout.tsx","content":nextjs_layout_source()}),
        ),
        ToolCall::new(
            "Write",
            json!({"path":"src/app/globals.css","content":nextjs_globals_css()}),
        ),
        ToolCall::new(
            "Write",
            json!({"path":"src/app/global.d.ts","content":"declare module \"*.css\";\n"}),
        ),
    ]
}

fn browser_release_evidence_tool_calls() -> Vec<ToolCall> {
    vec![
        ToolCall::new(
            "Write",
            json!({"path":"browser-readiness.json","content":r#"{"ok":true,"http_status":200,"route_rendered":true}"#}),
        ),
        ToolCall::new(
            "Write",
            json!({"path":"browser-interaction.json","content":r#"{"ok":true,"status":"passed","interaction_success":true,"interaction_performed":true,"surface_visible":true,"start_transition":true,"input_state_change":true,"input_state_evaluated_after_start":true,"input_event_observed":true,"state_changed":true,"canvas_found":true}"#}),
        ),
    ]
}

fn write_browser_release_evidence(run_dir: &Path) {
    std::fs::create_dir_all(run_dir).unwrap();
    std::fs::write(
        run_dir.join("browser-readiness.json"),
        r#"{"ok":true,"http_status":200,"route_rendered":true}"#,
    )
    .unwrap();
    std::fs::write(
        run_dir.join("browser-interaction.json"),
        r#"{"ok":true,"status":"passed","interaction_success":true,"interaction_performed":true,"surface_visible":true,"start_transition":true,"input_state_change":true,"input_state_evaluated_after_start":true,"input_event_observed":true,"state_changed":true,"canvas_found":true}"#,
    )
    .unwrap();
}

fn nextjs_expected_paths() -> Vec<String> {
    vec![
        "package.json".to_string(),
        "tsconfig.json".to_string(),
        "postcss.config.js".to_string(),
        "tailwind.config.ts".to_string(),
        "src/app/layout.tsx".to_string(),
        "src/app/page.tsx".to_string(),
        "src/app/globals.css".to_string(),
        "src/app/global.d.ts".to_string(),
    ]
}

fn nextjs_complete_package_json() -> &'static str {
    r#"{"dependencies":{"next":"^14.2.0","react":"^18.3.0","react-dom":"^18.3.0"},"devDependencies":{"typescript":"^5.5.0","@types/node":"^20.14.0","@types/react":"^18.3.0","@types/react-dom":"^18.3.0","tailwindcss":"^3.4.19","postcss":"^8.5.15","autoprefixer":"^10.4.20"},"scripts":{"build":"next build","dev":"next dev -p 3011","start":"next start -p 3011"}}"#
}

fn nextjs_tsconfig_json() -> &'static str {
    r#"{"compilerOptions":{"target":"ES2017","lib":["dom","dom.iterable","esnext"],"allowJs":true,"skipLibCheck":true,"strict":true,"noEmit":true,"esModuleInterop":true,"module":"esnext","moduleResolution":"bundler","resolveJsonModule":true,"isolatedModules":true,"jsx":"preserve","incremental":true,"plugins":[{"name":"next"}],"baseUrl":".","paths":{"@/*":["./src/*"]}},"include":["next-env.d.ts","**/*.ts","**/*.tsx",".next/types/**/*.ts"],"exclude":["node_modules"]}"#
}

fn nextjs_postcss_config() -> &'static str {
    "module.exports = { plugins: { tailwindcss: {}, autoprefixer: {} } };\n"
}

fn nextjs_tailwind_config_ts() -> &'static str {
    "import type { Config } from 'tailwindcss';\nconst config: Config = { content: ['./src/pages/**/*.{js,ts,jsx,tsx,mdx}', './src/components/**/*.{js,ts,jsx,tsx,mdx}', './src/app/**/*.{js,ts,jsx,tsx,mdx}'], theme: { extend: {} }, plugins: [] };\nexport default config;\n"
}

fn nextjs_layout_source() -> &'static str {
    "import './globals.css';\nexport default function Layout({children}:{children:React.ReactNode}){return <html><body>{children}</body></html>;}"
}

fn nextjs_globals_css() -> &'static str {
    "@tailwind base;\n@tailwind components;\n@tailwind utilities;\n"
}

fn nextjs_page_source() -> &'static str {
    r#""use client";
import { useEffect, useState } from "react";
export default function Page(){
  const [score, setScore] = useState(0);
  const [gameOver, setGameOver] = useState(false);
  const [bullets, setBullets] = useState<{ x: number; y: number }[]>([]);
  const [enemies, setEnemies] = useState([{ x: 10, y: 20 }]);
  const fireBullet = () => setBullets((items) => [...items, { x: 10, y: 90 }]);
  const restart = () => {
    setGameOver(false);
    setScore(0);
    setBullets([]);
    setEnemies([{ x: 10, y: 20 }]);
  };
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "ArrowLeft") fireBullet();
    };
    const frame = requestAnimationFrame(() => {
      bullets.forEach((bullet) => {
        enemies.forEach((enemy) => {
          if (Math.abs(bullet.x - enemy.x) < 12 && Math.abs(bullet.y - enemy.y) < 12) {
            setGameOver(true);
            setScore((value) => value + 10);
          }
        });
      });
      setEnemies((items) => items.map((enemy) => ({ ...enemy, x: enemy.x + 1 })));
    });
    window.addEventListener("keydown", onKeyDown);
    return () => {
      cancelAnimationFrame(frame);
      window.removeEventListener("keydown", onKeyDown);
    };
  }, [bullets, enemies]);
  return <main data-anvil-state={score}><button data-anvil-action="primary" onClick={fireBullet}>Start</button><button onClick={restart}>Restart</button><canvas /><p>score {score} enemy collision {gameOver ? "game over" : "playing"}</p></main>;
}
"#
}

fn generic_interactive_source() -> &'static str {
    r#"import { useState } from "react";
export default function Memo(){
  const [items, setItems] = useState([]);
  return <form onSubmit={(event) => { event.preventDefault(); setItems([...items, "note"]); }}>
    <input onChange={() => setItems([...items, "draft"])} />
    <button type="submit">Add</button>
    <ul>{items.map((item, index) => <li key={index}>{item}</li>)}</ul>
  </form>;
}
"#
}

fn python_cli_pyproject() -> &'static str {
    r#"[project]
name = "csv-stats"
version = "0.1.0"

[project.scripts]
csv-stats = "csv_stats.main:main"
"#
}

fn python_cli_main() -> &'static str {
    r#"#!/usr/bin/env python3
import csv
import sys
from pathlib import Path


def fmt(value: float) -> str:
    if value.is_integer():
        return str(int(value))
    return f"{value:.3f}".rstrip("0").rstrip(".")


def main() -> None:
    if len(sys.argv) != 2:
        print("usage: csv-stats <file>", file=sys.stderr)
        raise SystemExit(2)
    path = Path(sys.argv[1])
    if not path.is_file():
        print(f"missing file: {path}", file=sys.stderr)
        raise SystemExit(1)
    with path.open(newline="") as handle:
        rows = list(csv.DictReader(handle))
    numeric = {}
    for column in (rows[0].keys() if rows else []):
        values = []
        for row in rows:
            try:
                values.append(float(row[column]))
            except ValueError:
                pass
        if values:
            numeric[column] = (sum(values), sum(values) / len(values), max(values), min(values))
    if not numeric:
        print("no numeric columns", file=sys.stderr)
        raise SystemExit(1)
    print("column | sum | average | max | min")
    for column in sorted(numeric):
        total, average, maximum, minimum = numeric[column]
        print(f"{column} | {fmt(total)} | {fmt(average)} | {fmt(maximum)} | {fmt(minimum)}")


if __name__ == "__main__":
    main()
"#
}

#[cfg(unix)]
fn write_fake_npm_dependency_installer(root: &Path) {
    use std::os::unix::fs::PermissionsExt;

    let bin = root.join("node_modules/.bin");
    std::fs::create_dir_all(&bin).unwrap();
    let exe = shell_quote(&std::env::current_exe().unwrap().display().to_string());
    let script = r#"#!/bin/sh
set -eu
install_pkg() {
  name="$1"
  if grep -q "\"$name\"" package.json 2>/dev/null; then
    mkdir -p "node_modules/$name"
    printf '{"name":"%s"}\n' "$name" > "node_modules/$name/package.json"
  fi
}
if [ "$1" = "install" ]; then
  mkdir -p node_modules/.bin
  install_pkg next
  install_pkg react
  install_pkg react-dom
  install_pkg typescript
  install_pkg @types/node
  install_pkg @types/react
  install_pkg @types/react-dom
  install_pkg tailwindcss
  install_pkg postcss
  install_pkg autoprefixer
  if [ -d node_modules/next ]; then
    printf '#!/bin/sh\nexit 0\n' > node_modules/.bin/next
    chmod +x node_modules/.bin/next
  fi
  printf '{"lockfileVersion":3}\n' > package-lock.json
  exit 0
fi
if [ "$1" = "run" ] && [ "$2" = "build" ]; then
  test -x node_modules/.bin/next || { echo "next missing" >&2; exit 1; }
  if grep -q "\"tailwindcss\"" package.json 2>/dev/null; then
    test -d node_modules/tailwindcss || { echo "tailwindcss missing" >&2; exit 1; }
    test -d node_modules/postcss || { echo "postcss missing" >&2; exit 1; }
    test -d node_modules/autoprefixer || { echo "autoprefixer missing" >&2; exit 1; }
  fi
  echo "fake build ok"
  exit 0
fi
if [ "$1" = "run" ] && [ "$2" = "dev" ]; then
  ANVIL_CONFORMANCE_FAKE_DEV_SERVER_CHILD=1 ANVIL_CONFORMANCE_FAKE_DEV_SERVER_PORT=3011 exec __CONFORMANCE_TEST_EXE__ --ignored --exact suite::conformance_fake_dev_server_child --nocapture
fi
if [ "$1" = "run" ] && [ "$2" = "start" ]; then
  ANVIL_CONFORMANCE_FAKE_DEV_SERVER_CHILD=1 ANVIL_CONFORMANCE_FAKE_DEV_SERVER_PORT=3011 exec __CONFORMANCE_TEST_EXE__ --ignored --exact suite::conformance_fake_dev_server_child --nocapture
fi
echo "unexpected fake npm args: $*" >&2
exit 2
"#
    .replace("__CONFORMANCE_TEST_EXE__", &exe);
    let path = bin.join("npm");
    std::fs::write(&path, script).unwrap();
    let mut permissions = std::fs::metadata(&path).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(path, permissions).unwrap();
}

#[cfg(unix)]
fn write_fake_nextjs_dependencies_ready(root: &Path) {
    use std::os::unix::fs::PermissionsExt;

    let bin = root.join("node_modules/.bin");
    std::fs::create_dir_all(&bin).unwrap();
    for package in [
        "next",
        "react",
        "react-dom",
        "typescript",
        "@types/node",
        "@types/react",
        "@types/react-dom",
        "tailwindcss",
        "postcss",
        "autoprefixer",
    ] {
        let package_dir = root.join("node_modules").join(package);
        std::fs::create_dir_all(&package_dir).unwrap();
        std::fs::write(package_dir.join("package.json"), "{}\n").unwrap();
    }
    let next_path = bin.join("next");
    std::fs::write(&next_path, "#!/bin/sh\nexit 0\n").unwrap();
    let mut permissions = std::fs::metadata(&next_path).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(next_path, permissions).unwrap();
}

#[cfg(unix)]
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

#[cfg(unix)]
#[test]
#[ignore]
fn conformance_fake_dev_server_child() {
    if std::env::var("ANVIL_CONFORMANCE_FAKE_DEV_SERVER_CHILD")
        .ok()
        .as_deref()
        != Some("1")
    {
        return;
    }
    let port = std::env::var("ANVIL_CONFORMANCE_FAKE_DEV_SERVER_PORT")
        .unwrap()
        .parse::<u16>()
        .unwrap();
    let listener = std::net::TcpListener::bind(("127.0.0.1", port)).unwrap();
    for stream in listener.incoming() {
        let mut stream = stream.unwrap();
        let body = r#"<!doctype html><html><head><title>Conformance</title></head><body><main data-anvil-state="{&quot;score&quot;:0}"><button data-anvil-action="primary">Start</button><button>Restart</button><canvas></canvas><p>score 0 enemy collision playing</p></main></body></html>"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        let _ = std::io::Write::write_all(&mut stream, response.as_bytes());
    }
}

#[cfg(not(unix))]
fn write_fake_npm_dependency_installer(root: &Path) {
    let bin = root.join("node_modules/.bin");
    std::fs::create_dir_all(&bin).unwrap();
    let script = r#"@echo off
setlocal
if "%1"=="install" (
  if exist package.json (
    findstr /c:"\"next\"" package.json >nul && mkdir node_modules\next 2>nul
    findstr /c:"\"tailwindcss\"" package.json >nul && mkdir node_modules\tailwindcss 2>nul
    findstr /c:"\"postcss\"" package.json >nul && mkdir node_modules\postcss 2>nul
    findstr /c:"\"autoprefixer\"" package.json >nul && mkdir node_modules\autoprefixer 2>nul
    if exist node_modules\next (
      echo @echo off> node_modules\.bin\next.cmd
      echo exit /b 0>> node_modules\.bin\next.cmd
      echo {"name":"next"}> node_modules\next\package.json
    )
  )
  echo {"lockfileVersion":3}> package-lock.json
  exit /b 0
)
if "%1"=="run" if "%2"=="build" (
  if not exist node_modules\.bin\next.cmd exit /b 1
  echo fake build ok
  exit /b 0
)
echo unexpected fake npm args: %*
exit /b 2
"#;
    std::fs::write(bin.join("npm.cmd"), script).unwrap();
}

#[cfg(not(unix))]
fn write_fake_nextjs_dependencies_ready(root: &Path) {
    let bin = root.join("node_modules/.bin");
    std::fs::create_dir_all(&bin).unwrap();
    for package in ["next", "tailwindcss", "postcss", "autoprefixer"] {
        let package_dir = root.join("node_modules").join(package);
        std::fs::create_dir_all(&package_dir).unwrap();
        std::fs::write(package_dir.join("package.json"), "{}\n").unwrap();
    }
    std::fs::write(bin.join("next.cmd"), "@echo off\r\nexit /b 0\r\n").unwrap();
}

fn read_events(path: &Path) -> Vec<Value> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .collect()
}

fn events_named<'a>(events: &'a [Value], name: &str) -> Vec<&'a Value> {
    events
        .iter()
        .filter(|event| event.get("event").and_then(Value::as_str) == Some(name))
        .collect()
}

fn assert_has_event(trace: &Trace, name: &str) {
    assert!(
        trace
            .events
            .iter()
            .any(|event| event.get("event").and_then(Value::as_str) == Some(name)),
        "{} missing event {name}; events:\n{}",
        trace.scenario.name(),
        render_events(&trace.events)
    );
}

fn render_events(events: &[Value]) -> String {
    events
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n")
}
