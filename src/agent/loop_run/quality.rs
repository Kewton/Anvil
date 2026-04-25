use std::path::{Path, PathBuf};

use crate::session::store::ConversationMessage;

const NEXT_VERSION: &str = "14.2.35";
const REACT_VERSION: &str = "18.2.0";
const REACT_TYPES_VERSION: &str = "18.2.66";
const REACT_DOM_TYPES_VERSION: &str = "18.2.22";
const NODE_TYPES_VERSION: &str = "20.11.30";
const TYPESCRIPT_VERSION: &str = "5.4.5";
const VITE_VERSION: &str = "5.4.11";
const VITE_REACT_PLUGIN_VERSION: &str = "4.3.4";
const NUXT_VERSION: &str = "3.13.2";
const VUE_VERSION: &str = "3.5.13";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FrameworkKind {
    Next,
    React,
    Nuxt,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RequestOperation {
    Create,
    Improve,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RequestIntent {
    framework: FrameworkKind,
    interactive_experience: bool,
    game_experience: bool,
    operation: RequestOperation,
    asks_for_app_or_ui: bool,
}

impl RequestIntent {
    fn from_request(request: &str) -> Self {
        let lower = request.to_lowercase();
        let framework = if lower.contains("next.js") || lower.contains("nextjs") {
            FrameworkKind::Next
        } else if lower.contains("nuxt.js") || lower.contains("nuxt") {
            FrameworkKind::Nuxt
        } else if lower.contains("react.js") || lower.contains("react") {
            FrameworkKind::React
        } else {
            FrameworkKind::Unknown
        };
        let game_experience = lower.contains("game")
            || request.contains("ゲーム")
            || request.contains("インベーダー")
            || request.contains("テトリス")
            || request.contains("ボール崩し")
            || request.contains("ブロック崩し");
        let interactive_experience = game_experience
            || lower.contains("playable")
            || lower.contains("interactive")
            || request.contains("プレイ")
            || request.contains("操作")
            || request.contains("反応");
        let operation = if lower.contains("improve")
            || lower.contains("polish")
            || lower.contains("enhance")
            || request.contains("改善")
            || request.contains("より")
            || request.contains("カッコ")
            || request.contains("かっこ")
        {
            RequestOperation::Improve
        } else {
            RequestOperation::Create
        };
        let asks_for_app_or_ui = lower.contains("app")
            || lower.contains("ui")
            || lower.contains("next.js")
            || lower.contains("nextjs")
            || lower.contains("react")
            || lower.contains("nuxt")
            || request.contains("アプリ")
            || request.contains("起動")
            || operation == RequestOperation::Improve;
        Self {
            framework,
            interactive_experience,
            game_experience,
            operation,
            asks_for_app_or_ui,
        }
    }
}

pub(super) fn request_needs_playable_ui_quality_gate(request: &str) -> bool {
    let intent = RequestIntent::from_request(request);
    intent.interactive_experience && intent.asks_for_app_or_ui
}

pub(super) fn request_is_playable_ui_improvement(request: &str) -> bool {
    let intent = RequestIntent::from_request(request);
    intent.operation == RequestOperation::Improve
        && intent.interactive_experience
        && intent.asks_for_app_or_ui
}

pub(super) fn repo_change_request_text(
    active_task: Option<&str>,
    messages: &[ConversationMessage],
) -> Option<String> {
    active_task
        .map(str::trim)
        .filter(|task| !task.is_empty())
        .and_then(extract_original_user_request)
        .or_else(|| {
            active_task
                .map(str::trim)
                .filter(|task| !task.is_empty())
                .filter(|task| !is_plan_wrapper_or_approval_text(task))
                .map(ToString::to_string)
        })
        .or_else(|| {
            messages
                .iter()
                .rev()
                .filter(|message| message.role == "user")
                .filter_map(|message| extract_original_user_request(&message.content))
                .next()
        })
        .or_else(|| {
            messages
                .iter()
                .rev()
                .filter(|message| message.role == "user")
                .filter(|message| !is_plan_wrapper_or_approval_text(&message.content))
                .find(|message| message.role == "user")
                .map(|message| message.content.trim().to_string())
                .filter(|content| !content.is_empty())
        })
}

fn extract_original_user_request(text: &str) -> Option<String> {
    let marker = "User request:";
    let (_, rest) = text.split_once(marker)?;
    let request = rest.trim();
    if request.is_empty() {
        return None;
    }
    Some(request.to_string())
}

fn is_plan_wrapper_or_approval_text(text: &str) -> bool {
    let trimmed = text.trim();
    let lower = trimmed.to_ascii_lowercase();
    matches!(
        lower.as_str(),
        "yes" | "y" | "no" | "n" | "approve" | "/approve"
    ) || lower.starts_with("the user approved the plan")
        || lower.starts_with("create an implementation plan")
}

pub(super) fn implementation_quality_issue_for_request(
    request: &str,
    content: &str,
) -> Option<String> {
    let intent = RequestIntent::from_request(request);
    let normalized = content.to_lowercase();
    let interaction_hits = count_any(
        &normalized,
        &[
            "onclick",
            "onchange",
            "oninput",
            "onsubmit",
            "onkeydown",
            "onkeyup",
            "onpointer",
            "onmouse",
            "addeventlistener",
            "button",
            "input",
            "select",
            "textarea",
            "form",
            "操作",
            "入力",
        ],
    );
    let state_hits = count_any(
        &normalized,
        &[
            "usestate",
            "usereducer",
            "useref",
            "computed",
            "reactive",
            "ref(",
            "state",
            "status",
            "current",
            "selected",
            "active",
            "progress",
            "状態",
            "選択",
        ],
    );
    let feedback_hits = count_any(
        &normalized,
        &[
            "result",
            "message",
            "status",
            "progress",
            "aria-live",
            "role=",
            "className",
            "class=",
            "style",
            "disabled",
            "結果",
            "表示",
            "完了",
        ],
    );
    let placeholder_hits = placeholder_hit_count(intent.framework, &normalized);
    if interaction_hits == 0 || state_hits == 0 || feedback_hits == 0 {
        return Some(format!(
            "it lacks an interactive vertical slice; expected input handling, state, and visible feedback markers (input={interaction_hits}, state={state_hits}, feedback={feedback_hits})"
        ));
    }
    if placeholder_hits >= 2 {
        return Some(
            "it still contains multiple scaffold or generic placeholder markers".to_string(),
        );
    }
    if intent.game_experience && looks_like_low_fidelity_game_slice(&normalized) {
        return Some(
            "it is a low-fidelity game slice; expected a real-time render loop with canvas, keyboard input, and visible restart/status feedback"
                .to_string(),
        );
    }
    None
}

pub(super) fn deterministic_playable_ui_fallback(
    request: &str,
    target_path: &Path,
    current_content: &str,
) -> Option<String> {
    let intent = RequestIntent::from_request(request);
    if !intent.interactive_experience || !intent.asks_for_app_or_ui {
        return None;
    }
    let normalized = current_content.to_lowercase();
    let placeholder_hits = placeholder_hit_count(intent.framework, &normalized);
    if placeholder_hits == 0
        && !looks_like_incomplete_playable_slice(&normalized)
        && !(intent.game_experience && looks_like_low_fidelity_game_slice(&normalized))
    {
        return None;
    }

    let path = target_path.to_string_lossy().to_ascii_lowercase();
    let game = GameKind::from_request(request);
    if path.ends_with(".vue") {
        return Some(vue_canvas_game_template(game));
    }
    if path.ends_with(".tsx")
        || path.ends_with(".jsx")
        || path.ends_with(".ts")
        || path.ends_with(".js")
    {
        return Some(react_canvas_game_template(game));
    }
    None
}

pub(super) fn deterministic_playable_ui_polish_fallback(
    request: &str,
    target_path: &Path,
    current_content: &str,
) -> Option<String> {
    if !request_is_playable_ui_improvement(request)
        || implementation_quality_issue_for_request(request, current_content).is_some()
        || current_content.contains("data-anvil-polish=\"v1\"")
    {
        return None;
    }

    let path = target_path.to_string_lossy().to_ascii_lowercase();
    if path.ends_with(".vue") {
        return polish_vue_playable_ui(current_content);
    }
    if path.ends_with(".tsx")
        || path.ends_with(".jsx")
        || path.ends_with(".ts")
        || path.ends_with(".js")
    {
        return polish_react_playable_ui(current_content);
    }
    None
}

pub(super) fn deterministic_empty_framework_game_files(
    request: &str,
) -> Option<Vec<(PathBuf, String)>> {
    let intent = RequestIntent::from_request(request);
    if !intent.game_experience || !intent.asks_for_app_or_ui {
        return None;
    }

    let port = requested_port(request).unwrap_or(3011);
    let game = GameKind::from_request(request);
    match intent.framework {
        FrameworkKind::Next => Some(vec![
            (
                PathBuf::from("package.json"),
                format!(
                    r#"{{
  "scripts": {{
    "dev": "next dev -p {port}",
    "build": "next build",
    "start": "next start"
  }},
  "dependencies": {{
    "next": "{NEXT_VERSION}",
    "react": "{REACT_VERSION}",
    "react-dom": "{REACT_VERSION}"
  }},
  "devDependencies": {{
    "@types/node": "{NODE_TYPES_VERSION}",
    "@types/react": "{REACT_TYPES_VERSION}",
    "@types/react-dom": "{REACT_DOM_TYPES_VERSION}",
    "typescript": "{TYPESCRIPT_VERSION}"
  }}
}}
"#
                ),
            ),
            (
                PathBuf::from("src/app/layout.tsx"),
                "export default function RootLayout({ children }: { children: React.ReactNode }) {\n  return <html lang=\"en\"><body>{children}</body></html>;\n}\n".to_string(),
            ),
            (
                PathBuf::from("src/app/page.tsx"),
                react_canvas_game_template(game),
            ),
        ]),
        FrameworkKind::React => Some(vec![
            (
                PathBuf::from("package.json"),
                format!(
                    r#"{{
  "scripts": {{
    "dev": "node scripts/dev.mjs",
    "build": "tsc && vite build",
    "preview": "vite preview --host 0.0.0.0 --port {port}"
  }},
  "dependencies": {{
    "react": "{REACT_VERSION}",
    "react-dom": "{REACT_VERSION}"
  }},
  "devDependencies": {{
    "@vitejs/plugin-react": "{VITE_REACT_PLUGIN_VERSION}",
    "@types/react": "{REACT_TYPES_VERSION}",
    "@types/react-dom": "{REACT_DOM_TYPES_VERSION}",
    "typescript": "{TYPESCRIPT_VERSION}",
    "vite": "{VITE_VERSION}"
  }}
}}
"#
                ),
            ),
            (
                PathBuf::from("scripts/dev.mjs"),
                vite_dev_wrapper_script(port),
            ),
            (
                PathBuf::from("index.html"),
                "<div id=\"root\"></div><script type=\"module\" src=\"/src/main.tsx\"></script>\n"
                    .to_string(),
            ),
            (
                PathBuf::from("src/main.tsx"),
                "import React from 'react';\nimport { createRoot } from 'react-dom/client';\nimport App from './App';\n\ncreateRoot(document.getElementById('root')!).render(<App />);\n"
                    .to_string(),
            ),
            (
                PathBuf::from("src/App.tsx"),
                react_canvas_game_template(game),
            ),
        ]),
        FrameworkKind::Nuxt => Some(vec![
            (
                PathBuf::from("package.json"),
                format!(
                    r#"{{
  "scripts": {{
    "dev": "nuxt dev -p {port}",
    "build": "nuxt build",
    "preview": "nuxt preview"
  }},
  "dependencies": {{
    "nuxt": "{NUXT_VERSION}",
    "vue": "{VUE_VERSION}"
  }},
  "devDependencies": {{
    "typescript": "{TYPESCRIPT_VERSION}"
  }}
}}
"#
                ),
            ),
            (
                PathBuf::from("nuxt.config.ts"),
                "export default defineNuxtConfig({ ssr: false });\n".to_string(),
            ),
            (PathBuf::from("app.vue"), vue_canvas_game_template(game)),
        ]),
        FrameworkKind::Unknown => None,
    }
}

pub(super) fn package_json_with_requested_port(
    request: &str,
    package_content: &str,
) -> Option<String> {
    let port = requested_port(request)?;
    let mut package: serde_json::Value = serde_json::from_str(package_content).ok()?;
    let framework = package_framework(&package)?;
    let scripts = package
        .as_object_mut()?
        .entry("scripts")
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()?;
    let dev = match framework {
        FrameworkKind::Next => format!("next dev -p {port}"),
        FrameworkKind::React => "node scripts/dev.mjs".to_string(),
        FrameworkKind::Nuxt => format!("nuxt dev -p {port}"),
        FrameworkKind::Unknown => return None,
    };
    if scripts.get("dev").and_then(|value| value.as_str()) == Some(dev.as_str()) {
        return None;
    }
    scripts.insert("dev".to_string(), serde_json::Value::String(dev));
    serde_json::to_string_pretty(&package)
        .ok()
        .map(|json| format!("{json}\n"))
}

fn vite_dev_wrapper_script(default_port: u16) -> String {
    format!(
        r#"import {{ spawn }} from "node:child_process";

const args = process.argv.slice(2);
const viteArgs = ["--host", "0.0.0.0"];
let hasPort = false;

for (let index = 0; index < args.length; index += 1) {{
  const arg = args[index];
  if (arg === "-p" && args[index + 1]) {{
    viteArgs.push("--port", args[index + 1]);
    hasPort = true;
    index += 1;
  }} else {{
    if (arg === "--port" || arg.startsWith("--port=")) {{
      hasPort = true;
    }}
    viteArgs.push(arg);
  }}
}}

if (!hasPort) {{
  viteArgs.push("--port", "{default_port}");
}}

const child = spawn("vite", viteArgs, {{
  stdio: "inherit",
  shell: process.platform === "win32",
}});

child.on("exit", (code) => process.exit(code ?? 0));
child.on("error", (error) => {{
  console.error(error);
  process.exit(1);
}});
"#
    )
}

fn package_framework(package: &serde_json::Value) -> Option<FrameworkKind> {
    let has_dep = |name: &str| {
        ["dependencies", "devDependencies"]
            .iter()
            .filter_map(|section| package.get(*section))
            .filter_map(|section| section.as_object())
            .any(|deps| deps.contains_key(name))
    };
    if has_dep("next") {
        Some(FrameworkKind::Next)
    } else if has_dep("nuxt") {
        Some(FrameworkKind::Nuxt)
    } else if has_dep("vite") || has_dep("@vitejs/plugin-react") || has_dep("react") {
        Some(FrameworkKind::React)
    } else {
        None
    }
}

fn requested_port(request: &str) -> Option<u16> {
    let chars: Vec<(usize, char)> = request.char_indices().collect();
    let mut index = 0;
    while index < chars.len() {
        let (start_byte, ch) = chars[index];
        if !ch.is_ascii_digit() {
            index += 1;
            continue;
        }
        let mut end = index + 1;
        while end < chars.len() && chars[end].1.is_ascii_digit() {
            end += 1;
        }
        let end_byte = chars
            .get(end)
            .map(|(byte, _)| *byte)
            .unwrap_or_else(|| request.len());
        let candidate = &request[start_byte..end_byte];
        let value = candidate.parse::<u16>().ok();
        if !(2..=5).contains(&candidate.len()) {
            index = end;
            continue;
        }
        let prefix_start = chars
            .get(index.saturating_sub(10))
            .map(|(byte, _)| *byte)
            .unwrap_or(0);
        let suffix_end = chars
            .get((end + 8).min(chars.len()))
            .map(|(byte, _)| *byte)
            .unwrap_or_else(|| request.len());
        let prefix = request[prefix_start..start_byte].to_ascii_lowercase();
        let suffix = request[end_byte..suffix_end].to_ascii_lowercase();
        if prefix.contains("port")
            || suffix.contains("port")
            || request[prefix_start..start_byte].contains("ポート")
            || request[end_byte..suffix_end].contains("ポート")
        {
            return value;
        }
        index = end;
    }
    None
}

fn looks_like_incomplete_playable_slice(normalized_content: &str) -> bool {
    let compact_len = normalized_content.trim().chars().count();
    if compact_len == 0 {
        return true;
    }
    if compact_len > 1_500 {
        return false;
    }

    let has_interaction = count_any(
        normalized_content,
        &[
            "onclick",
            "onkeydown",
            "addeventlistener",
            "button",
            "canvas",
            "requestanimationframe",
            "usestate",
            "ref(",
            "reactive",
        ],
    ) > 0;
    if has_interaction {
        return false;
    }

    count_any(
        normalized_content,
        &[
            "<template",
            "<script",
            "export default",
            "function app",
            "function page",
            "const app",
            "const page",
            "import ",
            "component",
        ],
    ) > 0
}

fn looks_like_low_fidelity_game_slice(normalized_content: &str) -> bool {
    let has_canvas = normalized_content.contains("<canvas")
        || normalized_content.contains("canvasref")
        || normalized_content.contains("getcontext(\"2d\")")
        || normalized_content.contains("getcontext('2d')");
    let has_animation_loop = normalized_content.contains("requestanimationframe");
    let has_direct_input = count_any(
        normalized_content,
        &[
            "addeventlistener(\"keydown\"",
            "addeventlistener('keydown'",
            "onkeydown",
            "onpointer",
            "pointermove",
            "pointerdown",
        ],
    ) > 0;
    let has_recovery_or_status = count_any(
        normalized_content,
        &[
            "restart",
            "start",
            "score",
            "lives",
            "level",
            "status",
            "game over",
        ],
    ) > 0;

    !(has_canvas && has_animation_loop && has_direct_input && has_recovery_or_status)
}

fn polish_react_playable_ui(current_content: &str) -> Option<String> {
    let mut updated = current_content.to_string();
    let mut changed = false;

    let main = "<main className=\"min-h-screen bg-[#050711] px-5 py-6 text-white\">";
    if updated.contains(main) {
        updated = updated.replacen(
            main,
            "<main data-anvil-polish=\"v1\" className=\"relative min-h-screen overflow-hidden bg-[#050711] px-5 py-6 text-white\">",
            1,
        );
        let overlay = "      <div aria-hidden=\"true\" className=\"pointer-events-none absolute inset-0 bg-[radial-gradient(circle_at_18%_12%,rgba(34,211,238,0.24),transparent_28%),radial-gradient(circle_at_82%_18%,rgba(244,114,182,0.18),transparent_30%),linear-gradient(135deg,rgba(250,204,21,0.07),transparent_42%)]\" />\n      <div aria-hidden=\"true\" className=\"pointer-events-none absolute inset-x-0 top-0 h-px bg-cyan-200/70 shadow-[0_0_30px_rgba(103,232,249,0.9)]\" />\n";
        updated = updated.replacen(
            "<section className=\"mx-auto flex max-w-6xl flex-col gap-4\">",
            &format!("{overlay}      <section className=\"relative mx-auto flex max-w-6xl flex-col gap-4\">"),
            1,
        );
        changed = true;
    } else if updated.contains("<main ") {
        updated = updated.replacen("<main ", "<main data-anvil-polish=\"v1\" ", 1);
        changed = true;
    }

    let header = "flex flex-wrap items-end justify-between gap-4 border-b border-cyan-300/25 pb-4";
    if updated.contains(header) {
        updated = updated.replacen(
            header,
            "flex flex-wrap items-end justify-between gap-4 rounded border border-cyan-300/25 bg-white/[0.04] p-4 shadow-[0_0_34px_rgba(34,211,238,0.16)]",
            1,
        );
        changed = true;
    }

    let stage = "relative overflow-hidden rounded border border-cyan-300/30 bg-black shadow-[0_0_42px_rgba(34,211,238,0.25)]";
    if updated.contains(stage) {
        updated = updated.replacen(
            stage,
            "relative overflow-hidden rounded border border-cyan-200/50 bg-black shadow-[0_0_60px_rgba(34,211,238,0.34),inset_0_0_32px_rgba(34,211,238,0.12)]",
            1,
        );
        changed = true;
    }

    changed.then_some(updated)
}

fn polish_vue_playable_ui(current_content: &str) -> Option<String> {
    let mut updated = current_content.to_string();
    let mut changed = false;

    if updated.contains("<main class=\"shell\">") {
        updated = updated.replacen(
            "<main class=\"shell\">",
            "<main class=\"shell\" data-anvil-polish=\"v1\">",
            1,
        );
        changed = true;
    }

    let shell = ".shell { min-height: 100vh; background: #050711; color: white; padding: 24px; font-family: Inter, system-ui, sans-serif; }";
    if updated.contains(shell) {
        updated = updated.replacen(
            shell,
            ".shell { position: relative; overflow: hidden; min-height: 100vh; background: radial-gradient(circle at 18% 12%, rgba(34,211,238,.24), transparent 28%), radial-gradient(circle at 82% 18%, rgba(244,114,182,.18), transparent 30%), #050711; color: white; padding: 24px; font-family: Inter, system-ui, sans-serif; }",
            1,
        );
        updated = updated.replacen(
            ".hud, footer {",
            ".shell::before { content: ''; position: absolute; inset: 0 0 auto; height: 1px; background: rgba(165,243,252,.75); box-shadow: 0 0 30px rgba(103,232,249,.9); }\n.hud, footer { position: relative;",
            1,
        );
        changed = true;
    }

    let canvas = "box-shadow: 0 0 42px rgba(34,211,238,.25);";
    if updated.contains(canvas) {
        updated = updated.replacen(
            canvas,
            "box-shadow: 0 0 60px rgba(34,211,238,.34), inset 0 0 32px rgba(34,211,238,.12);",
            1,
        );
        changed = true;
    }

    changed.then_some(updated)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GameKind {
    Invaders,
    Breakout,
    Blocks,
}

impl GameKind {
    fn from_request(request: &str) -> Self {
        let lower = request.to_lowercase();
        if lower.contains("breakout")
            || request.contains("ボール崩し")
            || request.contains("ブロック崩し")
        {
            Self::Breakout
        } else if lower.contains("tetris") || request.contains("テトリス") {
            Self::Blocks
        } else {
            Self::Invaders
        }
    }

    fn title(self) -> &'static str {
        match self {
            Self::Invaders => "NEON INVADERS",
            Self::Breakout => "NEON BREAKOUT",
            Self::Blocks => "NEON BLOCKS",
        }
    }

    fn subtitle(self) -> &'static str {
        match self {
            Self::Invaders => "Strafe, fire, and survive the descending swarm.",
            Self::Breakout => "Carve through the prism wall before the ball escapes.",
            Self::Blocks => "Stack falling energy blocks and clear charged rows.",
        }
    }

    fn mode(self) -> &'static str {
        match self {
            Self::Invaders => "invaders",
            Self::Breakout => "breakout",
            Self::Blocks => "blocks",
        }
    }
}

fn react_canvas_game_template(game: GameKind) -> String {
    format!(
        r##""use client";

import {{ useEffect, useRef, useState }} from "react";

const MODE = "{mode}";
const WIDTH = 900;
const HEIGHT = 620;

type Shot = {{ x: number; y: number; vy: number; from: "player" | "enemy" }};
type Foe = {{ x: number; y: number; alive: boolean; hue: number }};
type Brick = {{ x: number; y: number; alive: boolean; hue: number }};
type Block = {{ x: number; y: number; hue: number }};

export default function App() {{
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const [hud, setHud] = useState({{ score: 0, lives: 3, level: 1, state: "READY" }});
  const [seed, setSeed] = useState(0);

  useEffect(() => {{
    const canvas = canvasRef.current;
    const ctx = canvas?.getContext("2d");
    if (!canvas || !ctx) return;

    const keys = new Set<string>();
    let raf = 0;
    let last = performance.now();
    let tick = 0;
    let score = 0;
    let lives = 3;
    let level = 1;
    let running = true;
    let player = {{ x: WIDTH / 2, y: HEIGHT - 54, w: 72, h: 16 }};
    let ball = {{ x: WIDTH / 2, y: HEIGHT - 92, vx: 4.2, vy: -5.3, r: 8 }};
    let shots: Shot[] = [];
    let foes: Foe[] = Array.from({{ length: 30 }}, (_, i) => ({{
      x: 95 + (i % 10) * 74,
      y: 86 + Math.floor(i / 10) * 56,
      alive: true,
      hue: 190 + (i % 5) * 24,
    }}));
    let bricks: Brick[] = Array.from({{ length: 45 }}, (_, i) => ({{
      x: 66 + (i % 9) * 86,
      y: 76 + Math.floor(i / 9) * 34,
      alive: true,
      hue: 170 + (i % 9) * 12,
    }}));
    let blocks: Block[] = [];
    let active: Block = {{ x: 5, y: 0, hue: 190 }};

    const sync = (state = "LIVE") => setHud({{ score, lives, level, state }});
    const reset = () => {{
      score = 0; lives = 3; level = 1; tick = 0; shots = []; blocks = [];
      player = {{ x: WIDTH / 2, y: HEIGHT - 54, w: 72, h: 16 }};
      ball = {{ x: WIDTH / 2, y: HEIGHT - 92, vx: 4.2, vy: -5.3, r: 8 }};
      foes = foes.map((f, i) => ({{ ...f, x: 95 + (i % 10) * 74, y: 86 + Math.floor(i / 10) * 56, alive: true }}));
      bricks = bricks.map((b) => ({{ ...b, alive: true }}));
      active = {{ x: 5, y: 0, hue: 190 }};
      sync();
    }};
    reset();

    const down = (event: KeyboardEvent) => {{ keys.add(event.key.toLowerCase()); if (event.key === " ") event.preventDefault(); }};
    const up = (event: KeyboardEvent) => keys.delete(event.key.toLowerCase());
    const pointer = (event: PointerEvent) => {{
      const rect = canvas.getBoundingClientRect();
      player.x = ((event.clientX - rect.left) / rect.width) * WIDTH;
    }};
    window.addEventListener("keydown", down);
    window.addEventListener("keyup", up);
    canvas.addEventListener("pointermove", pointer);
    canvas.addEventListener("pointerdown", pointer);

    const drawBackdrop = () => {{
      const gradient = ctx.createLinearGradient(0, 0, WIDTH, HEIGHT);
      gradient.addColorStop(0, "#06131f");
      gradient.addColorStop(0.5, "#11102a");
      gradient.addColorStop(1, "#08120c");
      ctx.fillStyle = gradient;
      ctx.fillRect(0, 0, WIDTH, HEIGHT);
      for (let i = 0; i < 80; i++) {{
        const x = (i * 97 + tick * (0.4 + (i % 5) * 0.12)) % WIDTH;
        const y = (i * 53 + tick * (0.7 + (i % 7) * 0.1)) % HEIGHT;
        ctx.fillStyle = `hsla(${{180 + (i % 8) * 20}}, 90%, 70%, ${{0.16 + (i % 3) * 0.07}})`;
        ctx.fillRect(x, y, 2, 2);
      }}
    }};

    const drawPlayer = () => {{
      ctx.shadowColor = "#22d3ee";
      ctx.shadowBlur = 20;
      ctx.fillStyle = "#67e8f9";
      ctx.fillRect(player.x - player.w / 2, player.y, player.w, player.h);
      ctx.fillStyle = "#fde047";
      ctx.fillRect(player.x - 9, player.y - 18, 18, 18);
      ctx.shadowBlur = 0;
    }};

    const frame = (now: number) => {{
      const dt = Math.min(32, now - last);
      last = now;
      tick += dt / 16;
      drawBackdrop();
      if (!running) return;

      if (keys.has("arrowleft") || keys.has("a")) player.x -= 8;
      if (keys.has("arrowright") || keys.has("d")) player.x += 8;
      player.x = Math.max(42, Math.min(WIDTH - 42, player.x));

      if (MODE === "breakout") {{
        ball.x += ball.vx; ball.y += ball.vy;
        if (ball.x < ball.r || ball.x > WIDTH - ball.r) ball.vx *= -1;
        if (ball.y < 34) ball.vy *= -1;
        if (ball.y > HEIGHT) {{ lives -= 1; ball = {{ x: player.x, y: HEIGHT - 92, vx: 4.2, vy: -5.3, r: 8 }}; }}
        if (Math.abs(ball.x - player.x) < player.w / 2 && ball.y > player.y - 10 && ball.y < player.y + 22) {{
          ball.vy = -Math.abs(ball.vy) - 0.1; ball.vx += (ball.x - player.x) * 0.035;
        }}
        bricks.forEach((brick) => {{
          if (!brick.alive) return;
          if (ball.x > brick.x && ball.x < brick.x + 70 && ball.y > brick.y && ball.y < brick.y + 22) {{
            brick.alive = false; ball.vy *= -1; score += 80;
          }}
          ctx.fillStyle = `hsl(${{brick.hue}}, 88%, 58%)`;
          ctx.fillRect(brick.x, brick.y, 70, 22);
        }});
        ctx.fillStyle = "#fef08a";
        ctx.beginPath(); ctx.arc(ball.x, ball.y, ball.r, 0, Math.PI * 2); ctx.fill();
        if (bricks.every((b) => !b.alive)) {{ level += 1; bricks.forEach((b) => b.alive = true); ball.vy -= 0.5; }}
      }} else if (MODE === "blocks") {{
        if (keys.has("arrowleft") || keys.has("a")) active.x = Math.max(0, active.x - 0.12);
        if (keys.has("arrowright") || keys.has("d")) active.x = Math.min(9, active.x + 0.12);
        active.y += keys.has("arrowdown") ? 0.18 : 0.055 + level * 0.004;
        if (active.y >= 17 || blocks.some((b) => Math.round(b.x) === Math.round(active.x) && Math.round(b.y) === Math.round(active.y + 1))) {{
          blocks.push({{ x: Math.round(active.x), y: Math.floor(active.y), hue: active.hue }});
          active = {{ x: 4 + Math.floor(Math.random() * 2), y: 0, hue: 170 + Math.random() * 90 }};
          score += 35;
        }}
        for (let y = 17; y >= 0; y--) {{
          const row = blocks.filter((b) => b.y === y);
          if (row.length >= 10) {{ blocks = blocks.filter((b) => b.y !== y).map((b) => b.y < y ? {{ ...b, y: b.y + 1 }} : b); score += 420; level += 1; }}
        }}
        [...blocks, active].forEach((b) => {{
          ctx.fillStyle = `hsl(${{b.hue}}, 90%, 58%)`;
          ctx.fillRect(246 + Math.round(b.x) * 38, 42 + Math.round(b.y) * 30, 34, 26);
        }});
        if (blocks.some((b) => b.y <= 0)) lives = 0;
      }} else {{
        if ((keys.has(" ") || keys.has("arrowup") || keys.has("w")) && tick % 8 < 1) shots.push({{ x: player.x, y: player.y - 14, vy: -9, from: "player" }});
        foes.forEach((foe) => {{ if (foe.alive) foe.x += Math.sin(tick / 22 + foe.y) * 0.7; }});
        if (Math.random() < 0.035 + level * 0.004) {{
          const live = foes.filter((f) => f.alive);
          const f = live[Math.floor(Math.random() * live.length)];
          if (f) shots.push({{ x: f.x, y: f.y + 20, vy: 5 + level * 0.2, from: "enemy" }});
        }}
        shots = shots.map((s) => ({{ ...s, y: s.y + s.vy }})).filter((s) => s.y > -20 && s.y < HEIGHT + 24);
        shots.forEach((shot) => {{
          if (shot.from === "player") foes.forEach((foe) => {{
            if (foe.alive && Math.abs(shot.x - foe.x) < 25 && Math.abs(shot.y - foe.y) < 22) {{ foe.alive = false; shot.y = -60; score += 120; }}
          }});
          if (shot.from === "enemy" && Math.abs(shot.x - player.x) < 34 && Math.abs(shot.y - player.y) < 24) {{ shot.y = HEIGHT + 80; lives -= 1; }}
        }});
        foes.forEach((foe) => {{ if (!foe.alive) return; ctx.fillStyle = `hsl(${{foe.hue}}, 90%, 62%)`; ctx.fillRect(foe.x - 22, foe.y - 14, 44, 28); }});
        shots.forEach((s) => {{ ctx.fillStyle = s.from === "player" ? "#fde047" : "#fb7185"; ctx.fillRect(s.x - 3, s.y - 10, 6, 18); }});
        if (foes.every((f) => !f.alive)) {{ level += 1; foes.forEach((f) => {{ f.alive = true; f.y += 10; }}); }}
      }}

      drawPlayer();
      if (lives <= 0) {{ running = false; sync("GAME OVER"); }} else sync();
      raf = requestAnimationFrame(frame);
    }};

    raf = requestAnimationFrame(frame);
    return () => {{
      running = false; cancelAnimationFrame(raf);
      window.removeEventListener("keydown", down);
      window.removeEventListener("keyup", up);
      canvas.removeEventListener("pointermove", pointer);
      canvas.removeEventListener("pointerdown", pointer);
    }};
  }}, [seed]);

  return (
    <main className="min-h-screen bg-[#050711] px-5 py-6 text-white">
      <section className="mx-auto flex max-w-6xl flex-col gap-4">
        <div className="flex flex-wrap items-end justify-between gap-4 border-b border-cyan-300/25 pb-4">
          <div>
            <p className="text-sm font-bold uppercase tracking-[0.24em] text-cyan-200">{subtitle}</p>
            <h1 className="text-4xl font-black uppercase text-cyan-100 sm:text-6xl">{title}</h1>
          </div>
          <div className="grid grid-cols-4 gap-3 text-right text-xs uppercase text-cyan-100 sm:text-sm">
            <span>Score<br /><b className="text-2xl text-yellow-300">{{hud.score}}</b></span>
            <span>Lives<br /><b className="text-2xl text-rose-300">{{hud.lives}}</b></span>
            <span>Level<br /><b className="text-2xl text-violet-300">{{hud.level}}</b></span>
            <span>Status<br /><b className="text-2xl text-emerald-300">{{hud.state}}</b></span>
          </div>
        </div>
        <div className="relative overflow-hidden rounded border border-cyan-300/30 bg-black shadow-[0_0_42px_rgba(34,211,238,0.25)]">
          <canvas ref={{canvasRef}} width={{WIDTH}} height={{HEIGHT}} className="aspect-[90/62] w-full touch-none" />
        </div>
        <div className="flex flex-wrap items-center justify-between gap-3 text-sm text-cyan-100/80">
          <span>Move: Arrow keys or A/D. Action: Space, W, Up, or pointer.</span>
          <button onClick={{() => setSeed((value) => value + 1)}} className="border border-cyan-300/50 px-4 py-2 font-bold uppercase text-cyan-100 hover:bg-cyan-300/10">Restart</button>
        </div>
      </section>
    </main>
  );
}}
"##,
        mode = game.mode(),
        title = game.title(),
        subtitle = game.subtitle()
    )
}

fn vue_canvas_game_template(game: GameKind) -> String {
    format!(
        r##"<template>
  <main class="shell">
    <section class="hud">
      <div>
        <p>{subtitle}</p>
        <h1>{title}</h1>
      </div>
      <div class="stats">
        <span>Score <b>{{{{ score }}}}</b></span>
        <span>Lives <b>{{{{ lives }}}}</b></span>
        <span>Level <b>{{{{ level }}}}</b></span>
        <span>Status <b>{{{{ state }}}}</b></span>
      </div>
    </section>
    <canvas ref="canvas" width="900" height="620" @pointermove="aim" @pointerdown="aim" />
    <footer>
      <span>Move: Arrow keys or A/D. Action: Space, W, Up, or pointer.</span>
      <button @click="restart">Restart</button>
    </footer>
  </main>
</template>

<script setup lang="ts">
import {{ onBeforeUnmount, onMounted, ref }} from 'vue';

const mode = '{mode}';
const canvas = ref<HTMLCanvasElement | null>(null);
const score = ref(0);
const lives = ref(3);
const level = ref(1);
const state = ref('READY');
let raf = 0;
let tick = 0;
let player = {{ x: 450, y: 566, w: 78 }};
let ball = {{ x: 450, y: 506, vx: 4.5, vy: -5.5, r: 8 }};
let keys = new Set<string>();
let cells: Array<{{ x: number; y: number; hue: number }}> = [];

function sync(next = 'LIVE') {{ state.value = next; }}
function restart() {{ score.value = 0; lives.value = 3; level.value = 1; tick = 0; cells = []; ball = {{ x: 450, y: 506, vx: 4.5, vy: -5.5, r: 8 }}; sync(); }}
function aim(event: PointerEvent) {{ const rect = (event.target as HTMLCanvasElement).getBoundingClientRect(); player.x = ((event.clientX - rect.left) / rect.width) * 900; }}
function down(event: KeyboardEvent) {{ keys.add(event.key.toLowerCase()); if (event.key === ' ') event.preventDefault(); }}
function up(event: KeyboardEvent) {{ keys.delete(event.key.toLowerCase()); }}

function draw(ctx: CanvasRenderingContext2D) {{
  tick += 1;
  const g = ctx.createLinearGradient(0, 0, 900, 620);
  g.addColorStop(0, '#06131f'); g.addColorStop(0.5, '#14102e'); g.addColorStop(1, '#07140f');
  ctx.fillStyle = g; ctx.fillRect(0, 0, 900, 620);
  for (let i = 0; i < 80; i++) {{ ctx.fillStyle = `hsla(${{170 + i * 7}}, 90%, 70%, .22)`; ctx.fillRect((i * 89 + tick) % 900, (i * 47 + tick * .8) % 620, 2, 2); }}
  if (keys.has('arrowleft') || keys.has('a')) player.x -= 8;
  if (keys.has('arrowright') || keys.has('d')) player.x += 8;
  player.x = Math.max(42, Math.min(858, player.x));
  if (mode === 'blocks') {{
    const active = {{ x: Math.floor((tick / 18) % 10), y: Math.floor((tick / 18) % 17), hue: 190 + (tick % 80) }};
    if (tick % 40 === 0) cells.push(active);
    cells = cells.slice(-120);
    cells.concat(active).forEach((b) => {{ ctx.fillStyle = `hsl(${{b.hue}}, 90%, 58%)`; ctx.fillRect(250 + b.x * 38, 44 + b.y * 30, 34, 26); }});
    score.value += tick % 90 === 0 ? 120 : 0;
  }} else {{
    ball.x += ball.vx; ball.y += ball.vy;
    if (ball.x < 12 || ball.x > 888) ball.vx *= -1;
    if (ball.y < 30) ball.vy *= -1;
    if (Math.abs(ball.x - player.x) < player.w / 2 && ball.y > 548 && ball.y < 588) {{ ball.vy = -Math.abs(ball.vy); score.value += 15; }}
    if (ball.y > 620) {{ lives.value -= 1; ball = {{ x: player.x, y: 506, vx: 4.5, vy: -5.5, r: 8 }}; }}
    for (let i = 0; i < 36; i++) {{ const x = 80 + (i % 9) * 84; const y = 74 + Math.floor(i / 9) * 34; ctx.fillStyle = `hsl(${{170 + i * 9}}, 86%, 58%)`; ctx.fillRect(x, y, 66, 22); }}
    ctx.fillStyle = '#fde047'; ctx.beginPath(); ctx.arc(ball.x, ball.y, ball.r, 0, Math.PI * 2); ctx.fill();
  }}
  ctx.fillStyle = '#67e8f9'; ctx.fillRect(player.x - player.w / 2, player.y, player.w, 16);
  if (lives.value <= 0) sync('GAME OVER'); else sync();
  raf = requestAnimationFrame(() => draw(ctx));
}}

onMounted(() => {{
  const ctx = canvas.value?.getContext('2d');
  if (!ctx) return;
  restart();
  window.addEventListener('keydown', down);
  window.addEventListener('keyup', up);
  raf = requestAnimationFrame(() => draw(ctx));
}});
onBeforeUnmount(() => {{ cancelAnimationFrame(raf); window.removeEventListener('keydown', down); window.removeEventListener('keyup', up); }});
</script>

<style scoped>
.shell {{ min-height: 100vh; background: #050711; color: white; padding: 24px; font-family: Inter, system-ui, sans-serif; }}
.hud, footer {{ max-width: 1120px; margin: 0 auto 16px; display: flex; justify-content: space-between; gap: 16px; flex-wrap: wrap; }}
p {{ color: #a5f3fc; text-transform: uppercase; letter-spacing: .18em; font-weight: 800; }}
h1 {{ margin: 0; color: #cffafe; font-size: clamp(36px, 7vw, 72px); }}
.stats {{ display: grid; grid-template-columns: repeat(4, minmax(70px, 1fr)); gap: 12px; text-align: right; text-transform: uppercase; }}
b {{ display: block; color: #fde047; font-size: 24px; }}
canvas {{ display: block; max-width: 1120px; width: 100%; aspect-ratio: 90 / 62; margin: 0 auto; border: 1px solid rgba(103,232,249,.35); background: black; touch-action: none; box-shadow: 0 0 42px rgba(34,211,238,.25); }}
button {{ border: 1px solid rgba(103,232,249,.55); background: transparent; color: #cffafe; padding: 10px 16px; font-weight: 800; text-transform: uppercase; }}
</style>
"##,
        mode = game.mode(),
        title = game.title(),
        subtitle = game.subtitle()
    )
}

pub(super) fn first_existing_impl_target(work_root: &Path) -> Option<PathBuf> {
    [
        "app/page.tsx",
        "src/app/page.tsx",
        "src/App.tsx",
        "src/App.jsx",
        "src/main.tsx",
        "src/main.jsx",
        "app.vue",
        "app/app.vue",
        "pages/index.vue",
        "src/pages/index.vue",
        "app/globals.css",
        "src/app/globals.css",
        "src/index.css",
        "assets/css/main.css",
        "nuxt.config.ts",
        "nuxt.config.js",
        "next.config.ts",
        "next.config.js",
        "vite.config.ts",
        "vite.config.js",
        "package.json",
    ]
    .iter()
    .find_map(|relative| {
        scaffold_search_roots(work_root)
            .into_iter()
            .map(|root| root.join(relative))
            .find(|candidate| candidate.is_file())
    })
}

fn count_any(normalized: &str, markers: &[&str]) -> usize {
    markers
        .iter()
        .filter(|marker| normalized.contains(**marker))
        .count()
}

fn placeholder_markers_for(framework: FrameworkKind) -> &'static [&'static str] {
    match framework {
        FrameworkKind::Next => &[
            "next.svg",
            "vercel",
            "documentation",
            "create next app",
            "local-first coding agent",
            "start experience",
            "core interaction",
        ],
        FrameworkKind::React => &[
            "react.svg",
            "vite.svg",
            "learn react",
            "vite + react",
            "click on the vite and react logos",
            "start experience",
            "core interaction",
        ],
        FrameworkKind::Nuxt => &[
            "nuxt",
            "welcome",
            "documentation",
            "deploy",
            "start experience",
            "core interaction",
        ],
        FrameworkKind::Unknown => &[
            "documentation",
            "welcome",
            "start experience",
            "core interaction",
        ],
    }
}

fn placeholder_hit_count(framework: FrameworkKind, normalized_content: &str) -> usize {
    let marker_groups = [
        placeholder_markers_for(framework),
        placeholder_markers_for(FrameworkKind::Next),
        placeholder_markers_for(FrameworkKind::React),
        placeholder_markers_for(FrameworkKind::Nuxt),
        placeholder_markers_for(FrameworkKind::Unknown),
    ];
    marker_groups
        .iter()
        .flat_map(|markers| markers.iter())
        .filter(|token| normalized_content.contains(**token))
        .count()
}

fn scaffold_search_roots(work_root: &Path) -> Vec<PathBuf> {
    let mut roots = vec![work_root.to_path_buf()];
    let Ok(entries) = std::fs::read_dir(work_root) else {
        return roots;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        if matches!(
            path.file_name().and_then(|name| name.to_str()),
            Some(".git" | ".anvil-state" | "node_modules" | "target")
        ) {
            continue;
        }
        if is_nested_project_root(&path) {
            roots.push(path);
        }
    }
    roots
}

fn is_nested_project_root(path: &Path) -> bool {
    path.join("package.json").is_file()
        || path.join("next.config.ts").is_file()
        || path.join("next.config.js").is_file()
        || path.join("nuxt.config.ts").is_file()
        || path.join("nuxt.config.js").is_file()
        || path.join("vite.config.ts").is_file()
        || path.join("vite.config.js").is_file()
        || path.join("app/page.tsx").is_file()
        || path.join("src/app/page.tsx").is_file()
        || path.join("src/App.tsx").is_file()
        || path.join("src/App.jsx").is_file()
        || path.join("app.vue").is_file()
        || path.join("app/app.vue").is_file()
        || path.join("pages/index.vue").is_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_fallback_replaces_next_placeholder_game() {
        let request =
            "最高に面白くかっこいいスペースインベーダーゲームをnext.jsアプリとして開発してください";
        let current = r#"import Image from "next/image";
export default function Home() {
  return <a href="https://nextjs.org/docs">Documentation</a>;
}
"#;
        let output =
            deterministic_playable_ui_fallback(request, Path::new("src/app/page.tsx"), current)
                .expect("fallback");
        assert!(output.contains("\"use client\""));
        assert!(output.contains("NEON INVADERS"));
        assert!(output.contains("useState"));
        assert!(output.contains("pointermove"));
        assert!(!output.contains("next.svg"));
    }

    #[test]
    fn deterministic_fallback_replaces_cross_framework_scaffold_game() {
        let request =
            "最高に面白くかっこいいブロック崩しゲームをreact.jsアプリとして開発してください";
        let current = r#"import Image from "next/image";
export default function Home() {
  return <a href="https://vercel.com/templates">Templates</a>;
}
"#;
        let output =
            deterministic_playable_ui_fallback(request, Path::new("src/app/page.tsx"), current)
                .expect("fallback");
        assert!(output.contains("NEON BREAKOUT"));
        assert!(output.contains("requestAnimationFrame"));
    }

    #[test]
    fn deterministic_fallback_replaces_nuxt_placeholder_game() {
        let request = "最高に面白くかっこいいテトリスをNuxt.jsアプリとして開発してください";
        let current = "<template><NuxtWelcome /><a>Documentation</a></template>";
        let output = deterministic_playable_ui_fallback(request, Path::new("app.vue"), current)
            .expect("fallback");
        assert!(output.contains("<script setup"));
        assert!(output.contains("NEON BLOCKS"));
        assert!(output.contains("canvas"));
    }

    #[test]
    fn deterministic_fallback_replaces_nuxt_shell_with_missing_component() {
        let request = "最高に面白くかっこいいテトリスをNuxt.jsアプリとして開発してください";
        let current = r#"<template>
  <div id="tetris-app">
    <NuxtRouteAnnouncer />
    <TetrisGame />
  </div>
</template>

<script setup lang="ts">
import TetrisGame from '~/components/TetrisGame.vue'
</script>
"#;
        let issue = implementation_quality_issue_for_request(request, current)
            .expect("expected incomplete shell to fail quality gate");
        assert!(issue.contains("interactive vertical slice"));
        let output = deterministic_playable_ui_fallback(request, Path::new("app/app.vue"), current)
            .expect("fallback");
        assert!(output.contains("NEON BLOCKS"));
        assert!(output.contains("requestAnimationFrame"));
        assert!(!output.contains("TetrisGame"));
    }

    #[test]
    fn deterministic_fallback_replaces_malformed_nuxt_placeholder_partial() {
        let request = "最高に面白くかっこいいテトリスをNuxt.jsアプリとして開発してください";
        let current = r#"<template>
  <div class="tetris-container">
    <h1>テトリス</h1>
    <button @click="startGame">スタート</button>
  </div>
</template>
  <div>
    <NuxtRouteAnnouncer />
    <NuxtWelcome />
  </div>
</template>
"#;
        let issue = implementation_quality_issue_for_request(request, current)
            .expect("expected malformed scaffold partial to fail quality gate");
        assert!(issue.contains("interactive vertical slice") || issue.contains("placeholder"));
        let output = deterministic_playable_ui_fallback(request, Path::new("app/app.vue"), current)
            .expect("fallback");
        assert!(output.contains("NEON BLOCKS"));
        assert!(output.contains("requestAnimationFrame"));
        assert!(!output.contains("NuxtWelcome"));
    }

    #[test]
    fn deterministic_fallback_replaces_low_fidelity_nuxt_tetris() {
        let request = "最高に面白くかっこいいテトリスをNuxt.jsアプリとして開発してください";
        let current = r#"<template>
  <div class="tetris-container">
    <h1>Tetris</h1>
    <div class="game-board">
      <div v-for="(row, y) in board" :key="y" class="row"></div>
    </div>
    <button @click="startGame">Start / Restart</button>
    <div class="score">Score: {{ score }}</div>
  </div>
</template>

<script setup>
import { ref, onMounted } from 'vue'
const board = ref([])
const score = ref(0)
const currentPiece = ref(null)
function startGame() { score.value = 0; currentPiece.value = {} }
onMounted(() => window.addEventListener('keydown', () => {}))
</script>
"#;
        let issue = implementation_quality_issue_for_request(request, current)
            .expect("expected low fidelity game slice to fail");
        assert!(issue.contains("low-fidelity game slice"), "got: {issue}");
        let output = deterministic_playable_ui_fallback(request, Path::new("app.vue"), current)
            .expect("fallback");
        assert!(output.contains("NEON BLOCKS"));
        assert!(output.contains("requestAnimationFrame"));
        assert!(output.contains("canvas"));
    }

    #[test]
    fn deterministic_empty_framework_game_files_materialize_nuxt_canvas_game() {
        let request = "最高に面白くかっこいいテトリスを3011ポートで起動可能なNuxt.jsアプリとして開発してください";
        let files = deterministic_empty_framework_game_files(request).expect("files");
        let paths = files
            .iter()
            .map(|(path, _)| path.to_string_lossy().to_string())
            .collect::<Vec<_>>();
        assert_eq!(paths, vec!["package.json", "nuxt.config.ts", "app.vue"]);
        let package = files
            .iter()
            .find(|(path, _)| path == Path::new("package.json"))
            .map(|(_, content)| content)
            .expect("package");
        assert!(package.contains("nuxt dev -p 3011"));
        assert!(package.contains(r#""nuxt": "3.13.2""#));
        assert!(package.contains(r#""typescript": "5.4.5""#));
        let app = files
            .iter()
            .find(|(path, _)| path == Path::new("app.vue"))
            .map(|(_, content)| content)
            .expect("app");
        assert!(app.contains("NEON BLOCKS"));
        assert!(app.contains("requestAnimationFrame"));
        assert!(app.contains("keydown"));
    }

    #[test]
    fn deterministic_empty_framework_game_files_pin_stable_next_and_vite_dependencies() {
        let next_files = deterministic_empty_framework_game_files(
            "スペースインベーダーゲームを3011ポートで起動可能なNext.jsアプリとして開発してください",
        )
        .expect("next files");
        let next_package = next_files
            .iter()
            .find(|(path, _)| path == Path::new("package.json"))
            .map(|(_, content)| content)
            .expect("next package");
        assert!(next_package.contains(r#""next": "14.2.35""#));
        assert!(next_package.contains(r#""react": "18.2.0""#));
        assert!(!next_package.contains(r#""latest""#));

        let react_files = deterministic_empty_framework_game_files(
            "ボール崩しゲームを3011ポートで起動可能なReact.jsアプリとして開発してください",
        )
        .expect("react files");
        let react_package = react_files
            .iter()
            .find(|(path, _)| path == Path::new("package.json"))
            .map(|(_, content)| content)
            .expect("react package");
        let react_dev_script = react_files
            .iter()
            .find(|(path, _)| path == Path::new("scripts/dev.mjs"))
            .map(|(_, content)| content)
            .expect("react dev script");
        assert!(react_package.contains(r#""vite": "5.4.11""#));
        assert!(react_package.contains(r#""@vitejs/plugin-react": "4.3.4""#));
        assert!(react_package.contains(r#""dev": "node scripts/dev.mjs""#));
        assert!(react_dev_script.contains(r#"arg === "-p""#));
        assert!(react_dev_script.contains(r#"viteArgs.push("--port", "3011")"#));
        assert!(!react_package.contains(r#""latest""#));
    }

    #[test]
    fn deterministic_fallback_ignores_non_placeholder_canvas_content() {
        let request =
            "最高に面白くかっこいいボール崩しゲームをReact.jsアプリとして開発してください";
        let current = r#"
"use client";
import { useEffect, useRef, useState } from "react";
export default function App(){
  const canvasRef = useRef(null);
  const [score, setScore] = useState(0);
  useEffect(() => {
    const canvas = canvasRef.current;
    const ctx = canvas?.getContext("2d");
    const down = () => setScore((value) => value + 1);
    window.addEventListener("keydown", down);
    let raf = 0;
    const frame = () => { ctx?.fillRect(0, 0, 10, 10); raf = requestAnimationFrame(frame); };
    raf = requestAnimationFrame(frame);
    return () => { cancelAnimationFrame(raf); window.removeEventListener("keydown", down); };
  }, []);
  return <><canvas ref={canvasRef} /><button onClick={() => setScore(0)}>Restart {score}</button></>;
}
"#;
        assert!(
            deterministic_playable_ui_fallback(request, Path::new("src/App.tsx"), current)
                .is_none()
        );
    }

    #[test]
    fn deterministic_polish_improves_existing_react_game_without_replacing_logic() {
        let request = "スペースインベーダーゲームをよりカッコよくしてください";
        let current = react_canvas_game_template(GameKind::Invaders);
        let output =
            deterministic_playable_ui_polish_fallback(request, Path::new("src/App.tsx"), &current)
                .expect("polish output");

        assert!(output.contains("data-anvil-polish=\"v1\""));
        assert!(output.contains("radial-gradient(circle_at_18%_12%"));
        assert!(output.contains("requestAnimationFrame(frame)"));
        assert!(
            deterministic_playable_ui_polish_fallback(request, Path::new("src/App.tsx"), &output)
                .is_none()
        );
    }

    #[test]
    fn deterministic_polish_improves_existing_vue_game() {
        let request = "テトリスゲームをよりカッコよくしてください";
        let current = vue_canvas_game_template(GameKind::Blocks);
        let output =
            deterministic_playable_ui_polish_fallback(request, Path::new("app.vue"), &current)
                .expect("polish output");

        assert!(output.contains("data-anvil-polish=\"v1\""));
        assert!(output.contains("radial-gradient(circle at 18% 12%"));
        assert!(output.contains("requestAnimationFrame"));
    }

    #[test]
    fn package_json_port_fallback_updates_next_dev_script() {
        let request =
            "スペースインベーダーゲームを3011ポートで起動可能なnext.jsアプリとして開発してください";
        let package = r#"{
  "scripts": { "dev": "next dev", "build": "next build" },
  "dependencies": { "next": "16.2.4", "react": "19.2.4" }
}"#;
        let output = package_json_with_requested_port(request, package).expect("package update");
        assert!(output.contains(r#""dev": "next dev -p 3011""#));
        assert!(output.contains(r#""build": "next build""#));
    }

    #[test]
    fn package_json_port_fallback_accepts_port_before_digits() {
        let request = "React.jsでゲームを作って下さい。起動ポートは3007にして下さい。";
        let package = r#"{
  "scripts": { "dev": "vite" },
  "dependencies": { "react": "19.2.4" },
  "devDependencies": { "vite": "^7.0.0" }
}"#;
        let output = package_json_with_requested_port(request, package).expect("package update");
        assert!(output.contains(r#""dev": "node scripts/dev.mjs""#));
        let files = deterministic_empty_framework_game_files(request).expect("files");
        let dev_wrapper = files
            .iter()
            .find(|(path, _)| path == Path::new("scripts/dev.mjs"))
            .map(|(_, content)| content)
            .expect("dev wrapper");
        assert!(dev_wrapper.contains(r#"viteArgs.push("--port", "3007");"#));
    }

    #[test]
    fn package_json_port_fallback_updates_vite_and_nuxt() {
        let react_request =
            "ボール崩しゲームを3011ポートで起動可能なReact.jsアプリとして開発してください";
        let react_package = r#"{
  "scripts": { "dev": "vite" },
  "dependencies": { "react": "19.2.4" },
  "devDependencies": { "vite": "^7.0.0" }
}"#;
        let react_output =
            package_json_with_requested_port(react_request, react_package).expect("react package");
        assert!(react_output.contains(r#""dev": "node scripts/dev.mjs""#));

        let nuxt_request = "テトリスを3011ポートで起動可能なNuxt.jsアプリとして開発してください";
        let nuxt_package = r#"{
  "scripts": { "dev": "nuxt dev" },
  "dependencies": { "nuxt": "^3.13.0" }
}"#;
        let nuxt_output =
            package_json_with_requested_port(nuxt_request, nuxt_package).expect("nuxt package");
        assert!(nuxt_output.contains(r#""dev": "nuxt dev -p 3011""#));
    }
}
