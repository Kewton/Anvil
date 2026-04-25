use super::slash_commands::{self, AnvilEditor, build_editor};
use super::summary::{ExitReason, format_run_summary};
use super::*;
use crate::config::LogLevel;
use crate::logging::log_llm_event;
use crate::modes::plan_act::{PlanStage, TaskProfile};
use crate::ollama::xml_fallback::strip_think_tags;
use crate::session::store::ConversationMessage;
use crossterm::event::{self, Event, KeyCode};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use serde::Deserialize;
use std::time::Duration;

/// First 8 characters of a session id, or the full id if shorter. Used for
/// banner display so users get a stable short handle without exposing the
/// whole UUID.
pub fn short_id(id: &str) -> &str {
    id.get(..8).unwrap_or(id)
}

/// Minimum terminal width (columns) required before the ASCII-art banner is
/// shown. Narrow terminals fall back to the legacy 4-line plain-text banner.
const MIN_BANNER_WIDTH: u16 = 50;

/// 256-color ANSI color codes applied to each of the five ASCII-art lines.
/// Length is pinned to 5 so the compiler guarantees one color per art line
/// (see [`ANVIL_ASCII_ART`]).
const NEON_GRADIENT_256: [u8; 5] = [51, 39, 93, 201, 21];

/// 5-line static ASCII-art logo shown at the top of the neon / mono-art
/// banners. Width is kept at or below 42 columns so that `MIN_BANNER_WIDTH`
/// (50) always leaves horizontal padding.
const ANVIL_ASCII_ART: [&str; 5] = [
    "                                          ",
    "  ╔═╗ ╔╗╔ ╦  ╦ ╦ ╦                        ",
    "  ╠═╣ ║║║ ╚╗╔╝ ║ ║     local-first agent  ",
    "  ╩ ╩ ╝╚╝  ╚╝  ╩ ╩═╝                      ",
    "                                          ",
];

/// Which banner variant to render. The three styles are mutually exclusive and
/// picked by [`decide_banner_style`] at startup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BannerStyle {
    /// ASCII art + 256-color ANSI gradient (interactive main path).
    Neon,
    /// ASCII art without any ANSI escapes (TTY + `NO_COLOR` + wide enough).
    MonoArt,
    /// Original 4-line plain-text banner, byte-identical with the pre-#425
    /// output. Used for non-TTY, unknown size, or narrow terminals.
    Legacy4Line,
}

/// Pure branch-decision: which banner style to render for the given runtime
/// signals. `width = None` collapses both the non-TTY case and the
/// `crossterm::terminal::size()` error case into `Legacy4Line` — if a future
/// requirement needs to distinguish them, split `width` into an explicit
/// `TerminalWidth { NonTty, SizeErr, Known(u16) }` enum.
pub(crate) fn decide_banner_style(tty: bool, no_color: bool, width: Option<u16>) -> BannerStyle {
    if !tty {
        return BannerStyle::Legacy4Line;
    }
    match width {
        None => BannerStyle::Legacy4Line,
        Some(w) if w < MIN_BANNER_WIDTH => BannerStyle::Legacy4Line,
        Some(_) if no_color => BannerStyle::MonoArt,
        Some(_) => BannerStyle::Neon,
    }
}

/// Map the `(fresh, resumed)` flag pair to the `[state]` suffix shared between
/// the stdout banner and the oneshot stderr banner. Kept in a single place so
/// AC-6 (three fixed labels) stays in sync across both paths.
pub(crate) fn state_suffix(fresh: bool, resumed: bool) -> &'static str {
    if fresh {
        "fresh"
    } else if resumed {
        "resumed"
    } else {
        "continued"
    }
}

/// Returns true when the `NO_COLOR` environment variable is set to a non-empty
/// value (https://no-color.org/). Intentionally duplicated from
/// `turn::no_color_requested` so that `commands.rs` does not force a cross-
/// module dependency; a test verifies the two implementations agree under the
/// same environment.
fn banner_no_color_requested() -> bool {
    std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty())
}

#[derive(Debug, Deserialize)]
struct LargeTaskDecision {
    large_task: bool,
    #[serde(default)]
    task_profile: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ClassifiedTask {
    large_task: bool,
    task_profile: TaskProfile,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PlanApprovalChoice {
    Execute,
    Revise,
    Feedback,
}

fn extract_json_object(raw: &str) -> Option<&str> {
    let start = raw.find('{')?;
    let end = raw.rfind('}')?;
    (end > start).then_some(&raw[start..=end])
}

fn parse_task_profile(raw: Option<&str>) -> TaskProfile {
    match raw {
        Some("coding") => TaskProfile::Coding,
        Some("content") => TaskProfile::Content,
        Some("ui") => TaskProfile::Ui,
        Some("research") => TaskProfile::Research,
        _ => TaskProfile::Generic,
    }
}

fn parse_plan_approval_choice(input: &str) -> Option<PlanApprovalChoice> {
    match input.trim().to_ascii_lowercase().as_str() {
        "y" | "yes" | "execute" => Some(PlanApprovalChoice::Execute),
        "n" | "no" | "revise" => Some(PlanApprovalChoice::Revise),
        "o" | "other" | "feedback" => Some(PlanApprovalChoice::Feedback),
        _ => None,
    }
}

fn plan_stage_status_line(stage: PlanStage, next_sections: &[&str]) -> String {
    let focus = if next_sections.is_empty() {
        lifecycle::plan_stage_sections(stage).join(", ")
    } else {
        next_sections.join(", ")
    };
    match stage {
        PlanStage::Stage1 => format!("Current phase: Draft {focus}"),
        PlanStage::Stage2 => format!("Current phase: Define {focus}"),
        PlanStage::Stage3 => "Current phase: Approval review".to_string(),
        PlanStage::Ready => "Current phase: Approval review".to_string(),
    }
}

fn strip_task_status(task: &str) -> &str {
    task.split_once("] ").map(|(_, rest)| rest).unwrap_or(task)
}

fn format_plan_tasks(tasks: &[String], stage_line: &str) -> String {
    let current = tasks
        .iter()
        .find(|task| !task.starts_with("[done]"))
        .map(|task| strip_task_status(task))
        .unwrap_or("Review the completed plan and approve execution");
    let next = tasks
        .iter()
        .filter(|task| !task.starts_with("[done]"))
        .nth(1)
        .map(|task| strip_task_status(task))
        .unwrap_or("Wait for approval feedback");
    format!("Current task: {current}\nUp next: {next}\n{stage_line}")
}

fn infer_task_profile_from_text(raw: &str) -> TaskProfile {
    let lower = raw.to_ascii_lowercase();

    let strong_coding = [
        "code",
        "implement",
        "debug",
        "build",
        "test",
        "fix",
        "refactor",
        "next.js",
        "react",
        "nuxt",
        "vue",
        "vite",
        "typescript",
        "javascript",
        "web app",
        "app router",
        "single-page",
        "game",
    ];
    if strong_coding.iter().any(|keyword| lower.contains(keyword))
        || raw.contains("アプリ")
        || raw.contains("ゲーム")
        || raw.contains("コード")
        || raw.contains("実装")
        || raw.contains("開発")
        || raw.contains("修正")
        || raw.contains("テスト")
        || raw.contains("ビルド")
    {
        TaskProfile::Coding
    } else if lower.contains("readme")
        || lower.contains("markdown")
        || lower.contains("documentation")
        || lower.contains("docs")
        || lower.contains("copy")
        || raw.contains("README")
        || raw.contains("文章")
        || raw.contains("説明")
        || raw.contains("改善")
        || raw.contains("ドキュメント")
    {
        TaskProfile::Content
    } else if lower.contains("ui")
        || lower.contains("ux")
        || lower.contains("design")
        || lower.contains("layout")
        || lower.contains("screen")
        || raw.contains("画面")
        || raw.contains("見た目")
        || raw.contains("デザイン")
    {
        TaskProfile::Ui
    } else if lower.contains("research")
        || lower.contains("investigate")
        || lower.contains("analysis")
        || lower.contains("compare")
        || raw.contains("調査")
        || raw.contains("分析")
        || raw.contains("比較")
    {
        TaskProfile::Research
    } else {
        TaskProfile::Generic
    }
}

fn infer_large_task_from_text(raw: &str) -> bool {
    let lower = raw.to_ascii_lowercase();
    let mut score = 0u8;

    let strong_english = [
        "create",
        "build",
        "develop",
        "implement",
        "scaffold",
        "from scratch",
        "full app",
    ];
    if strong_english.iter().any(|keyword| lower.contains(keyword)) {
        score += 3;
    }

    let strong_japanese = ["作って", "作成", "開発", "実装", "新規", "構築"];
    if strong_japanese.iter().any(|keyword| raw.contains(keyword)) {
        score += 3;
    }

    let stage_english = [
        "first", "then", "finally", "step", "phase", "plan", "verify",
    ];
    let english_stage_hits = stage_english
        .iter()
        .filter(|keyword| lower.contains(**keyword))
        .count()
        .min(2) as u8;
    if english_stage_hits > 0 {
        score += english_stage_hits;
    }

    let stage_japanese = [
        "まず",
        "その後",
        "最後に",
        "段階",
        "ステップ",
        "計画",
        "確認",
    ];
    let japanese_stage_hits = stage_japanese
        .iter()
        .filter(|keyword| raw.contains(**keyword))
        .count()
        .min(3) as u8;
    if japanese_stage_hits > 0 {
        score += japanese_stage_hits;
    }

    let framework_english = ["next.js", "react", "rails", "fastapi", "port"];
    if framework_english
        .iter()
        .any(|keyword| lower.contains(keyword))
    {
        score += 2;
    }

    let framework_japanese = ["アプリ", "ゲーム", "サイト", "ポート", "起動"];
    if framework_japanese
        .iter()
        .any(|keyword| raw.contains(keyword))
    {
        score += 2;
    }

    let quality_english = ["polish", "high quality", "beautiful", "cool"];
    if quality_english
        .iter()
        .any(|keyword| lower.contains(keyword))
    {
        score += 1;
    }

    let quality_japanese = ["高品質", "かっこいい", "作り込", "面白"];
    if quality_japanese.iter().any(|keyword| raw.contains(keyword)) {
        score += 1;
    }

    score >= 3
}

fn heuristic_classified_task(input: &str) -> ClassifiedTask {
    ClassifiedTask {
        large_task: infer_large_task_from_text(input),
        task_profile: infer_task_profile_from_text(input),
    }
}

fn classifier_model<'a>(main_model: &'a str, sidecar_model: Option<&'a str>) -> &'a str {
    sidecar_model.unwrap_or(main_model)
}

fn parse_large_task_decision(raw: &str, request_hint: Option<&str>) -> Option<ClassifiedTask> {
    let normalized = strip_think_tags(raw);
    let body = extract_json_object(normalized.trim())?;
    if let Ok(decision) = serde_json::from_str::<LargeTaskDecision>(body) {
        return Some(ClassifiedTask {
            large_task: decision.large_task,
            task_profile: parse_task_profile(decision.task_profile.as_deref()),
        });
    }

    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    let object = value.as_object()?;
    let action = object.get("action").and_then(serde_json::Value::as_str);
    let has_plan = object.get("plan").is_some();
    let step_count = object
        .get("steps")
        .and_then(serde_json::Value::as_array)
        .map_or(0, |steps| steps.len());
    let profile_hint = object
        .get("task_profile")
        .and_then(serde_json::Value::as_str)
        .map(|value| value.to_string())
        .or_else(|| {
            object
                .get("action_input")
                .and_then(serde_json::Value::as_str)
                .map(|value| value.to_string())
        })
        .or_else(|| {
            object
                .get("plan")
                .and_then(serde_json::Value::as_str)
                .map(|value| value.to_string())
        });
    if matches!(action, Some("create_plan" | "write_plan")) || (has_plan && step_count >= 2) {
        let inferred_profile = parse_task_profile(profile_hint.as_deref());
        let fallback_profile = if inferred_profile == TaskProfile::Generic {
            infer_task_profile_from_text(
                request_hint.unwrap_or_else(|| profile_hint.as_deref().unwrap_or(raw)),
            )
        } else {
            inferred_profile
        };
        return Some(ClassifiedTask {
            large_task: true,
            task_profile: fallback_profile,
        });
    }

    None
}

fn classifier_parse_error() -> String {
    "classifier response was not JSON-only".to_string()
}

fn is_classifier_transport_error(error: &str) -> bool {
    let lower = error.to_ascii_lowercase();
    lower.contains("failed to contact ollama generate api")
        || lower.contains("error sending request for url")
        || lower.contains("connection reset")
        || lower.contains("connection refused")
        || lower.contains("broken pipe")
        || lower.contains("operation timed out")
        || lower.contains("timed out")
}

pub(crate) fn is_plan_execution_request(input: &str) -> bool {
    let trimmed = input
        .trim()
        .trim_end_matches(['。', '.', '!', '！', '?', '？']);
    if trimmed.is_empty() {
        return false;
    }

    let normalized = trimmed.to_ascii_lowercase();
    let english = [
        "yes",
        "y",
        "approve",
        "act",
        "go",
        "run it",
        "execute",
        "start implementation",
        "implement it",
        "proceed",
    ];
    if english.iter().any(|phrase| normalized == *phrase) {
        return true;
    }

    let japanese = [
        "はい",
        "お願いします",
        "実行して",
        "進めて",
        "着手して",
        "実装して",
        "始めて",
        "開始して",
        "やって",
        "この計画で進めて",
    ];
    japanese.contains(&trimmed)
}

pub(crate) fn is_plan_rejection_request(input: &str) -> bool {
    let trimmed = input
        .trim()
        .trim_end_matches(['。', '.', '!', '！', '?', '？']);
    if trimmed.is_empty() {
        return false;
    }

    let normalized = trimmed.to_ascii_lowercase();
    matches!(normalized.as_str(), "no" | "n") || matches!(trimmed, "いいえ" | "だめ" | "却下")
}

/// Replace control characters (C0, DEL, and C1) with spaces, then trim trailing
/// whitespace. Mirrors `turn::sanitize_for_progress` so that model-derived
/// text (model banner, cwd, log path) cannot inject newlines or ANSI escapes
/// into the startup banner output. C1 (`U+0080..U+009F`) is included because
/// some terminals interpret 8-bit CSI (`U+009B`) and OSC (`U+009D`) equivalently
/// to `ESC [` and `ESC ]`.
fn sanitize_banner_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        let cp = ch as u32;
        if cp < 0x20 || cp == 0x7F || (0x80..=0x9F).contains(&cp) {
            out.push(' ');
        } else {
            out.push(ch);
        }
    }
    out.trim_end().to_string()
}

/// Inputs for the pure [`render_startup_banner`] function. The wrapper
/// `print_startup_banner` collects runtime state (TTY, width, NO_COLOR, log
/// level) and feeds this struct to the renderer. `llm_log_path` is already
/// gated by the wrapper: `Some` ⇒ emit the `llm log=` line, `None` ⇒ skip it.
pub(crate) struct BannerInputs<'a> {
    pub version: &'a str,
    pub model_banner: &'a str,
    pub mode: &'a crate::modes::plan_act::ExecutionMode,
    pub work_root: &'a std::path::Path,
    pub session_id_short: &'a str,
    pub message_count: usize,
    pub fresh: bool,
    pub resumed: bool,
    pub llm_log_path: Option<&'a std::path::Path>,
    pub style: BannerStyle,
}

/// Pure banner renderer. Returns the complete newline-terminated banner
/// string. ANSI escapes are only ever emitted for the fixed ASCII-art lines
/// when `style == Neon`; all dynamic text is run through `sanitize_banner_text`
/// first to prevent terminal injection.
pub(crate) fn render_startup_banner(inputs: &BannerInputs<'_>) -> String {
    let mut out = String::new();

    match inputs.style {
        BannerStyle::Neon => {
            for (i, line) in ANVIL_ASCII_ART.iter().enumerate() {
                let code = NEON_GRADIENT_256[i];
                out.push_str(&format!("\x1b[38;5;{code}m{line}\x1b[0m\n"));
            }
        }
        BannerStyle::MonoArt => {
            for line in ANVIL_ASCII_ART.iter() {
                out.push_str(line);
                out.push('\n');
            }
        }
        BannerStyle::Legacy4Line => {}
    }

    let safe_model = sanitize_banner_text(inputs.model_banner);
    let safe_cwd = sanitize_banner_text(&inputs.work_root.display().to_string());
    let state = state_suffix(inputs.fresh, inputs.resumed);

    out.push_str(&format!("anvil {}\n", inputs.version));
    out.push_str(&format!("{safe_model}\n"));
    out.push_str(&format!("mode={:?} cwd={}\n", inputs.mode, safe_cwd));
    out.push_str(&format!(
        "session={} messages={} [{}]\n",
        inputs.session_id_short, inputs.message_count, state
    ));
    if let Some(path) = inputs.llm_log_path {
        let safe_path = sanitize_banner_text(&path.display().to_string());
        out.push_str(&format!("llm log={safe_path}\n"));
    }

    out
}

/// Print the REPL / resume startup banner to stdout. The `fresh` and
/// `resumed` flags drive the `[state]` suffix so users can tell at a glance
/// which session they are in. This is a thin I/O wrapper around the pure
/// [`render_startup_banner`] renderer: it collects TTY / NO_COLOR / width
/// signals, gates the optional `llm log=` line on `log_level >= Verbose`, and
/// writes the result in a single `print!` call.
#[allow(clippy::too_many_arguments)]
pub fn print_startup_banner(
    version: &str,
    model_banner: &str,
    mode: &crate::modes::plan_act::ExecutionMode,
    work_root: &std::path::Path,
    session_id_short: &str,
    message_count: usize,
    fresh: bool,
    resumed: bool,
    log_level: LogLevel,
) {
    use std::io::IsTerminal;
    let tty = std::io::stdout().is_terminal();
    let no_color = banner_no_color_requested();
    // Skip the ioctl when we know we aren't on a TTY; the result would collapse
    // to `Legacy4Line` either way.
    let width = if tty {
        crossterm::terminal::size().ok().map(|(w, _)| w)
    } else {
        None
    };
    let style = decide_banner_style(tty, no_color, width);
    let llm_log_path: Option<&std::path::Path> = if log_level >= LogLevel::Verbose {
        crate::logging::llm_io_log_path()
    } else {
        None
    };
    let inputs = BannerInputs {
        version,
        model_banner,
        mode,
        work_root,
        session_id_short,
        message_count,
        fresh,
        resumed,
        llm_log_path,
        style,
    };
    print!("{}", render_startup_banner(&inputs));
}

/// Oneshot startup banner: single stderr line so script output on stdout stays
/// clean.
pub fn print_startup_banner_stderr_oneshot(
    session_id_short: &str,
    message_count: usize,
    fresh: bool,
    resumed: bool,
) {
    let state = state_suffix(fresh, resumed);
    eprintln!("anvil session={session_id_short} messages={message_count} [{state}]");
}

impl Agent {
    fn execute_approved_plan(
        &mut self,
        trigger_text: &str,
        stream_output: bool,
    ) -> Result<AgentEvent, String> {
        let status = self.approve_plan_mode()?;
        log_llm_event(
            "agent.milestone.plan_approved",
            serde_json::json!({
                "session_id": self.session_store.session_id(),
                "task_profile": self.session.mode_state.task_profile.as_str(),
                "trigger": trigger_text,
            }),
        );
        let plan_contents = self.current_plan_contents()?.unwrap_or_default();
        let plan_summary = lifecycle::plan_act_summary(&plan_contents);
        let profile_guidance = match self.session.mode_state.task_profile {
            TaskProfile::Generic => {
                "Work through the plan in phases, verify key outcomes, evaluate the result against the acceptance criteria and quality bar, and run one improvement pass if the result is only minimally complete."
            }
            TaskProfile::Coding => {
                "Work through the plan in phases: design, implement, review, test, evaluate. Preserve the intended quality bar, polish the user-facing result, and avoid unnecessary re-scaffolding or workspace resets."
            }
            TaskProfile::Content => {
                "Work through the plan in phases: edit, verify, evaluate, improve. After the first edit, review the output for clarity, specificity, usefulness, and signal density. If it still reads like filler, improve it before stopping."
            }
            TaskProfile::Ui => {
                "Work through the plan in phases: implement, verify, evaluate, improve. Check hierarchy, polish, completeness, and user-facing quality before stopping."
            }
            TaskProfile::Research => {
                "Work through the plan in phases: gather, verify, evaluate, refine. Prefer concise evidence-backed output over generic summaries."
            }
        };
        let exec_prompt = format!(
            "The user approved the plan and said: {trigger_text}\nExecute the approved plan now. Follow this accepted plan summary:\n\n{plan_summary}\n\n{profile_guidance} Start with one small, self-contained repository change, then continue until the requested work is complete."
        );
        println!("{status}");
        match self.handle_user_message(&exec_prompt, stream_output) {
            Ok((prose, stats)) => {
                if !stream_output {
                    if crate::tui::markdown::markdown_fully_disabled() {
                        println!("{prose}");
                    } else {
                        let color = crate::tui::markdown::color_enabled_for_markdown();
                        let utf8 = crate::tui::markdown::markdown_unicode_enabled();
                        let mut r = crate::tui::markdown::MarkdownRenderer::new(color, utf8);
                        let mut body = r.push_chunk(&prose);
                        body.push_str(&r.flush());
                        if !body.ends_with('\n') {
                            body.push('\n');
                        }
                        let _ = std::io::stdout().write_all(body.as_bytes());
                        let _ = std::io::stdout().flush();
                    }
                }
                println!();
                println!("{}", format_run_summary(ExitReason::Done, &stats));
                self.persist_session()?;
                Ok(AgentEvent::Continue(None))
            }
            Err((reason, error_text, stats)) => {
                println!();
                println!("{}", format_run_summary(reason, &stats));
                if reason.keeps_repl_alive() && !error_text.trim().is_empty() {
                    println!("{error_text}");
                }
                self.persist_session()?;
                if reason.keeps_repl_alive() {
                    Ok(AgentEvent::Continue(None))
                } else {
                    Err(error_text)
                }
            }
        }
    }

    fn prompt_for_plan_approval_choice(&self) -> Result<Option<PlanApprovalChoice>, String> {
        if !self.config.auto_plan || !io::stdin().is_terminal() || !io::stdout().is_terminal() {
            return Ok(None);
        }

        let _footer_freeze = self.footer.freeze_for_prompt();
        if !super::footer::footer_terminal_is_compatible() {
            return self.prompt_for_plan_approval_choice_plain();
        }
        let options = [("yes", "execute"), ("no", "revise"), ("other", "feedback")];
        let mut selected = 0usize;
        let redraw = |selected: usize| {
            print!("\x1b[4F\x1b[J");
            println!("plan approval:");
            for (index, (label, description)) in options.iter().enumerate() {
                let prefix = if index == selected { "▶" } else { " " };
                println!("  {prefix} {label:<5} {description}");
            }
            let _ = io::stdout().flush();
        };
        println!("plan approval:");
        println!("  ▶ yes   execute");
        println!("    no    revise");
        println!("    other feedback");
        let _ = io::stdout().flush();
        enable_raw_mode().map_err(|err| format!("failed to enable raw mode: {err}"))?;
        let choice = loop {
            match event::read() {
                Ok(Event::Key(key)) => match key.code {
                    KeyCode::Up => {
                        selected = selected.saturating_sub(1);
                        redraw(selected);
                    }
                    KeyCode::Down => {
                        selected = (selected + 1).min(options.len() - 1);
                        redraw(selected);
                    }
                    KeyCode::Char('y') | KeyCode::Char('Y') => {
                        println!("selected: execute");
                        break Some(PlanApprovalChoice::Execute);
                    }
                    KeyCode::Char('n') | KeyCode::Char('N') => {
                        println!("selected: revise");
                        break Some(PlanApprovalChoice::Revise);
                    }
                    KeyCode::Char('o') | KeyCode::Char('O') => {
                        println!("selected: feedback");
                        break Some(PlanApprovalChoice::Feedback);
                    }
                    KeyCode::Enter => {
                        let selected_choice = match selected {
                            0 => PlanApprovalChoice::Execute,
                            1 => PlanApprovalChoice::Revise,
                            _ => PlanApprovalChoice::Feedback,
                        };
                        println!("selected: {}", options[selected].1);
                        break Some(selected_choice);
                    }
                    KeyCode::Esc => {
                        println!("selected: revise");
                        break Some(PlanApprovalChoice::Revise);
                    }
                    _ => {}
                },
                Ok(_) => {}
                Err(err) => {
                    disable_raw_mode().ok();
                    return Err(format!("failed to read plan approval choice: {err}"));
                }
            }
        };
        disable_raw_mode().map_err(|err| format!("failed to disable raw mode: {err}"))?;
        Ok(choice)
    }

    fn prompt_for_plan_approval_choice_plain(&self) -> Result<Option<PlanApprovalChoice>, String> {
        println!("plan approval:");
        println!("  yes   execute");
        println!("  no    revise");
        println!("  other feedback");
        print!("select [yes/no/other]: ");
        let _ = io::stdout().flush();

        let mut input = String::new();
        io::stdin()
            .read_line(&mut input)
            .map_err(|err| format!("failed to read plan approval choice: {err}"))?;
        Ok(parse_plan_approval_choice(&input))
    }

    fn enter_plan_mode(&mut self, task_profile: TaskProfile) -> Result<String, String> {
        if self.session.mode_state.mode == ExecutionMode::Plan {
            let current = self
                .session
                .mode_state
                .active_plan_path
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "-".to_string());
            return Ok(format!("already in plan mode: {current}"));
        }
        let plan_path = self
            .session
            .mode_state
            .enter_plan(self.session_store.plan_dir(), task_profile)?;
        self.ensure_plan_file(&plan_path)?;
        self.push_system_note(format!(
            "[Plan Mode / {}] Explore with Read, Glob, and Grep. Write the plan to {}. Wait for /approve before making code changes.",
            self.session.mode_state.task_profile.as_str(),
            plan_path.display()
        ));
        self.footer.publish_flags(
            self.session.mode_state.mode,
            self.config.log_level,
            self.config.yes_mode,
        );
        let stage = self.session.mode_state.plan_stage;
        let next_sections = lifecycle::plan_stage_sections(stage);
        let plan_contents = self.current_plan_contents()?.unwrap_or_default();
        let tasks = lifecycle::plan_task_list(&plan_contents, task_profile);
        let stage_line = plan_stage_status_line(stage, next_sections);
        Ok(format!(
            "plan file: {}\nmode: plan ({})\n{}",
            plan_path.display(),
            task_profile.as_str(),
            format_plan_tasks(&tasks, &stage_line),
        ))
    }

    fn approve_plan_mode(&mut self) -> Result<String, String> {
        if self.session.mode_state.mode != ExecutionMode::Plan {
            return Err("approve is only available from plan mode".to_string());
        }
        let plan_contents = self
            .current_plan_contents()?
            .ok_or_else(|| "plan file is missing".to_string())?;
        if !self.plan_is_approval_ready_with_fallback(&plan_contents) {
            return Err("plan file is not ready for approval yet".to_string());
        }
        self.session.mode_state.approve();
        super::turn::prune_plan_mode_messages(&mut self.session.messages);
        self.push_system_note(format!(
            "[Act Mode / {}] Execute the accepted plan in phases and keep the work aligned with its acceptance criteria and quality bar.\n\n{}",
            self.session.mode_state.task_profile.as_str(),
            lifecycle::plan_act_summary(&plan_contents)
        ));
        self.footer.publish_flags(
            self.session.mode_state.mode,
            self.config.log_level,
            self.config.yes_mode,
        );
        Ok("act mode".to_string())
    }

    fn classify_large_task_with_main_model(&self, input: &str) -> Result<ClassifiedTask, String> {
        let classifier_model = classifier_model(&self.models.main, self.models.sidecar.as_deref());
        let messages = vec![
            ConversationMessage::system(
                "You classify whether a user request for a local-first repository agent should go through planning before execution, and which act profile fits best. Reply with JSON only in this shape: {\"large_task\":true|false,\"task_profile\":\"generic\"|\"coding\"|\"content\"|\"ui\"|\"research\"}. Use task_profile=\"coding\" for code changes, software implementation, debugging, tests, build changes, or repository edits. Use task_profile=\"ui\" for user-facing interface, visual design, interaction design, motion, or layout-heavy work. Use task_profile=\"content\" for writing, rewriting, documentation quality, copy, structured text, or reader-facing improvements where output quality matters. Use task_profile=\"research\" for investigation, comparison, or analysis-heavy work. Use task_profile=\"generic\" only for broader mixed work that does not clearly fit the others. Use large_task=true for broad multi-step work that benefits from a plan before execution."
                    .to_string(),
            ),
            ConversationMessage::user(format!(
                "Current mode: Act\nProject root: {}\nUser request:\n{}",
                self.work_root.display(),
                input
            )),
        ];
        let max_attempts = 2usize;
        let mut last_error = None;
        for attempt in 1..=max_attempts {
            match self
                .client
                .classify_task_request(classifier_model, &messages)
            {
                Ok(reply) => {
                    if let Some(classified) = parse_large_task_decision(&reply.content, Some(input))
                    {
                        return Ok(classified);
                    }
                    let error = classifier_parse_error();
                    log_llm_event(
                        "agent.classifier.error",
                        serde_json::json!({
                            "session_id": self.session_store.session_id(),
                            "model": classifier_model,
                            "input": input,
                            "attempt": attempt,
                            "kind": "parse",
                            "retryable": false,
                            "error": error,
                        }),
                    );
                    return Err(error);
                }
                Err(err) => {
                    let retryable = is_classifier_transport_error(&err);
                    log_llm_event(
                        "agent.classifier.error",
                        serde_json::json!({
                            "session_id": self.session_store.session_id(),
                            "model": classifier_model,
                            "input": input,
                            "attempt": attempt,
                            "kind": if retryable { "transport" } else { "other" },
                            "retryable": retryable,
                            "error": err,
                        }),
                    );
                    if retryable && attempt < max_attempts {
                        std::thread::sleep(Duration::from_millis(350));
                        last_error = Some(err);
                        continue;
                    }
                    return Err(err);
                }
            }
        }
        Err(last_error.unwrap_or_else(|| "classifier failed".to_string()))
    }

    fn maybe_auto_plan_prompt(&mut self, input: &str) -> Result<Option<String>, String> {
        if !self.config.auto_plan
            || self.session.mode_state.mode != ExecutionMode::Act
            || self.config.oneshot
        {
            return Ok(None);
        }

        match self.classify_large_task_with_main_model(input) {
            Ok(classified) if classified.large_task => {
                log_llm_event(
                    "agent.classifier.result",
                    serde_json::json!({
                        "session_id": self.session_store.session_id(),
                        "input": input,
                        "large_task": true,
                        "task_profile": classified.task_profile.as_str(),
                    }),
                );
                let status = self.enter_plan_mode(classified.task_profile)?;
                Ok(Some(format!(
                    "auto-plan: large {} task detected; entering plan mode\n{status}",
                    classified.task_profile.as_str(),
                )))
            }
            Ok(classified) => {
                log_llm_event(
                    "agent.classifier.result",
                    serde_json::json!({
                        "session_id": self.session_store.session_id(),
                        "input": input,
                        "large_task": false,
                        "task_profile": classified.task_profile.as_str(),
                    }),
                );
                self.session.mode_state.task_profile = classified.task_profile;
                Ok(None)
            }
            Err(err) => {
                if is_classifier_transport_error(&err) {
                    tracing::warn!("auto-plan classifier failed: {err}");
                } else {
                    tracing::debug!("auto-plan classifier fallback: {err}");
                }
                let fallback = heuristic_classified_task(input);
                log_llm_event(
                    "agent.classifier.fallback_used",
                    serde_json::json!({
                        "session_id": self.session_store.session_id(),
                        "input": input,
                        "reason": "classifier_failure",
                        "error": err,
                        "large_task": fallback.large_task,
                        "task_profile": fallback.task_profile.as_str(),
                        "fallback": "heuristic",
                    }),
                );
                if fallback.large_task {
                    let status = self.enter_plan_mode(fallback.task_profile)?;
                    Ok(Some(format!(
                        "auto-plan: classifier unavailable; using heuristic {} fallback\n{status}",
                        fallback.task_profile.as_str(),
                    )))
                } else {
                    self.session.mode_state.task_profile = fallback.task_profile;
                    Ok(None)
                }
            }
        }
    }

    pub fn initial_prompt_from_cli_or_stdin(&self) -> Result<Option<String>, String> {
        if let Some(prompt) = &self.config.prompt {
            return Ok(Some(prompt.clone()));
        }
        if self.config.oneshot {
            return stdin_prompt();
        }
        // Non-oneshot startup must not drain stdin. Wrappers such as
        // `anvildev` keep stdin open and exchange line-oriented commands over
        // a pipe; a blocking `read_to_string()` here would stall before the
        // fallback REPL has a chance to process `/help` or other commands.
        Ok(None)
    }

    pub fn run_oneshot(&mut self, prompt: &str) -> Result<String, String> {
        match self.process_line(prompt, false)? {
            AgentEvent::Continue(Some(message)) => Ok(message),
            AgentEvent::Continue(None) => Ok(String::new()),
            AgentEvent::Exit => Ok(String::new()),
        }
    }

    /// Replay the last user turn and then continue in the REPL loop.
    /// Used by `--resume` / `--resume <ID>` after the session has been loaded
    /// and reconciled. The caller is responsible for printing the startup
    /// banner; this method does not re-print it.
    pub fn run_resume(&mut self, replay_prompt: &str) -> Result<(), String> {
        match self.process_line(replay_prompt, self.config.stream)? {
            AgentEvent::Continue(Some(message)) => {
                if !self.config.stream {
                    println!("{message}");
                }
            }
            AgentEvent::Continue(None) => {}
            AgentEvent::Exit => return Ok(()),
        }
        self.run_repl_loop()
    }

    /// REPL body without the startup banner. Separated from `run_repl` so that
    /// `run_cli` can print the banner once (in a single location) and have
    /// both the fresh-REPL and resumed-REPL paths share the same loop.
    ///
    /// Delegates to either the rustyline-powered path (when stdin/stdout are
    /// both TTYs) or the plain `read_line` fallback (for pipes / CI / redirect).
    ///
    /// # Preconditions
    ///
    /// This function assumes it is called via `run_cli`, which has already
    /// invoked `crate::ensure_state_dirs` to materialize `state_root` with the
    /// correct 0o700 permissions. `prepare_editor()` still re-creates the
    /// directory defensively (`fs::create_dir_all(state_root)`) so direct
    /// callers that forgot to run `ensure_state_dirs` don't silently lose
    /// history persistence, but they also won't get the SSOT permission
    /// guarantees. Prefer `run_cli` for all new entry points.
    pub fn run_repl_loop(&mut self) -> Result<(), String> {
        let is_tty = io::stdin().is_terminal() && io::stdout().is_terminal();
        if !is_tty {
            return self.run_repl_loop_fallback();
        }
        self.run_repl_loop_rustyline()
    }

    /// Plain `read_line` REPL kept for non-TTY contexts (pipe input, CI,
    /// redirected stdout). When stdout is a TTY we still print the legacy
    /// `anvil> ` prompt for shell convenience (`echo foo | anvil`), but when
    /// stdout is also non-TTY we suppress the prompt so line-based wrappers can
    /// exchange commands and replies without extra framing.
    fn run_repl_loop_fallback(&mut self) -> Result<(), String> {
        let mut line = String::new();
        let show_prompt = should_render_fallback_prompt(io::stdout().is_terminal());
        loop {
            if show_prompt {
                print!("anvil> ");
                io::stdout()
                    .flush()
                    .map_err(|err| format!("failed to flush stdout: {err}"))?;
            }
            line.clear();
            let bytes = io::stdin()
                .read_line(&mut line)
                .map_err(|err| format!("failed to read line: {err}"))?;
            if bytes == 0 {
                break;
            }
            let trimmed = line.trim();
            let is_command = trimmed.starts_with('/');
            if is_command {
                let _footer_freeze = self.footer.freeze_for_prompt();
                match self.process_line(trimmed, self.config.stream)? {
                    AgentEvent::Continue(Some(message)) => println!("{message}"),
                    AgentEvent::Continue(None) => {}
                    AgentEvent::Exit => break,
                }
            } else {
                match self.process_line(trimmed, self.config.stream)? {
                    AgentEvent::Continue(Some(message)) => println!("{message}"),
                    AgentEvent::Continue(None) => {}
                    AgentEvent::Exit => break,
                }
            }
        }
        Ok(())
    }

    /// Rustyline-powered REPL: persistent history + tab-completed slash
    /// commands + Ctrl+A/E/K/U etc. Thin: builds an editor, runs the loop,
    /// then best-effort appends history and tightens file perms on unix.
    fn run_repl_loop_rustyline(&mut self) -> Result<(), String> {
        let (mut editor, history_path) = self.prepare_editor()?;
        let outcome = self.repl_loop_body(&mut editor);
        if let Err(err) = editor.append_history(&history_path) {
            tracing::warn!(
                "readline: failed to append history ({}): {err}",
                history_path.display()
            );
        }
        #[cfg(unix)]
        {
            tighten_history_perms(&history_path);
        }
        outcome
    }

    /// Resolve state_root, build the editor, and load any existing history
    /// file. The parent `state_root` is normally created by `ensure_state_dirs`
    /// before we get here (SSOT, via `run_cli`). We additionally call
    /// `create_dir_all(state_root)` here as a defensive fallback for direct
    /// callers of `run_repl` / `run_repl_loop` (see `run_repl_loop` preconditions):
    /// without it, `append_history` would silently fail on REPL teardown and we
    /// would lose history persistence — a latent regression the wrapper can
    /// prevent cheaply (a single `mkdir -p`).
    fn prepare_editor(&self) -> Result<(AnvilEditor, std::path::PathBuf), String> {
        let state_root = crate::resolve_state_root(&self.config)?;
        if let Err(err) = std::fs::create_dir_all(&state_root) {
            tracing::warn!(
                "readline: failed to ensure state_root exists ({}): {err}",
                state_root.display()
            );
        }
        let history_path = state_root.join("history");

        let mut editor = build_editor().map_err(|err| {
            format!("readline: failed to init editor (Editor::with_config): {err}")
        })?;

        if let Err(err) = editor.load_history(&history_path)
            && !is_not_found(&err)
        {
            tracing::warn!(
                "readline: failed to load history ({}): {err}",
                history_path.display()
            );
        }

        Ok((editor, history_path))
    }

    /// The readline loop body itself — no history I/O, no permission work.
    /// Ctrl+C discards the current line and keeps going, Ctrl+D (on empty
    /// line) exits cleanly, other errors are logged and cause a graceful exit.
    fn repl_loop_body(&mut self, editor: &mut AnvilEditor) -> Result<(), String> {
        use rustyline::error::ReadlineError;

        loop {
            // Issue #430 Phase D: pause footer redraw for the rustyline
            // prompt. The footer line stays painted (DR1-006 #6: maintain
            // DECSTBM), only the daemon worker stops re-emitting ANSI so
            // rustyline owns stdout / cursor positioning. Guard drops once
            // readline returns and the worker resumes within the next tick.
            let _footer_freeze = self.footer.freeze_for_prompt();
            match editor.readline("anvil> ") {
                Ok(line) => {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    let is_command = trimmed.starts_with('/');
                    if is_command {
                        let _footer_freeze = self.footer.freeze_for_prompt();
                        match self.process_line(trimmed, self.config.stream) {
                            Ok(AgentEvent::Continue(Some(msg))) => println!("{msg}"),
                            Ok(AgentEvent::Continue(None)) => {}
                            Ok(AgentEvent::Exit) => break Ok(()),
                            Err(err) => break Err(err),
                        }
                    } else {
                        match self.process_line(trimmed, self.config.stream) {
                            Ok(AgentEvent::Continue(Some(msg))) => println!("{msg}"),
                            Ok(AgentEvent::Continue(None)) => {}
                            Ok(AgentEvent::Exit) => break Ok(()),
                            Err(err) => break Err(err),
                        }
                    }
                }
                Err(ReadlineError::Interrupted) => continue,
                Err(ReadlineError::Eof) => break Ok(()),
                Err(other) => {
                    tracing::error!("readline: failed to read input: {other}");
                    break Ok(());
                }
            }
        }
    }

    /// Backwards-compatible wrapper that prints the startup banner and then
    /// runs the REPL loop. Kept for consumers that still call `run_repl`
    /// directly; `run_cli` no longer uses it because the banner is printed
    /// one level up for consistency across REPL / oneshot / resume.
    ///
    /// # Preconditions
    ///
    /// Same as `run_repl_loop`: the caller is expected to have invoked
    /// `crate::ensure_state_dirs` (via `run_cli`) so `state_root` exists with
    /// 0o700 permissions. `prepare_editor` will still `create_dir_all` the
    /// state root as a fallback, but direct callers do not get the
    /// permission-hardening SSOT — prefer `run_cli` for new entry points.
    pub fn run_repl(&mut self) -> Result<(), String> {
        print_startup_banner(
            env!("CARGO_PKG_VERSION"),
            &format_model_banner(&self.models),
            &self.session.mode_state.mode,
            &self.work_root,
            short_id(&self.session.id),
            self.session.messages.len(),
            self.config.fresh_session,
            false,
            self.config.log_level,
        );
        self.run_repl_loop()
    }

    pub fn process_line(&mut self, input: &str, stream_output: bool) -> Result<AgentEvent, String> {
        if input.trim().is_empty() {
            return Ok(AgentEvent::Continue(None));
        }

        let trimmed = input.trim();

        if self.session.mode_state.mode == ExecutionMode::Plan && is_plan_execution_request(trimmed)
        {
            return self.execute_approved_plan(trimmed, stream_output);
        }

        if self.session.mode_state.mode == ExecutionMode::Plan && is_plan_rejection_request(trimmed)
        {
            return Ok(AgentEvent::Continue(Some(
                "plan not approved; stay in plan mode and provide feedback or revisions"
                    .to_string(),
            )));
        }

        let auto_plan_entered = if !trimmed.starts_with('/') {
            self.maybe_auto_plan_prompt(trimmed)?
        } else {
            None
        };
        if let Some(message) = auto_plan_entered.as_deref() {
            println!("{message}");
        }

        let outcome = if trimmed.starts_with('/') {
            self.handle_command(trimmed)
        } else {
            let user_input = if auto_plan_entered.is_some() {
                format!(
                    "Create an implementation plan for the user's request. Do not make code changes yet.\n\
Focus only on the current plan stage that the runtime indicates. In Stage 1, bootstrap the plan from the user request first: make one small Write or Edit to the active plan file before doing any exploration. Only inspect a directly relevant file if one specific detail is still missing after that first plan update. Use one small Write or Edit at a time, and do not try to write the full completed plan in one large tool call.\n\
The plan must still define: (1) the first shippable vertical slice, (2) concrete acceptance criteria for that slice, (3) the quality bar that defines what makes the result genuinely good, (4) the implementation phases after that, (5) the specific files/modules likely to change, and (6) the verification steps. End your user-facing response only after the plan is complete, and then tell the user to reply yes to execute, no to revise, or provide feedback.\n\nUser request:\n{trimmed}"
                )
            } else {
                trimmed.to_string()
            };
            match self.handle_user_message(&user_input, stream_output) {
                Ok((prose, stats)) => {
                    if !stream_output {
                        // Issue #431: non-streaming assistant prose also goes
                        // through the markdown renderer. `prose` here is the
                        // raw LLM text (session has already stored this raw
                        // content in `run_actor_loop`). Feed the raw text
                        // directly into a one-shot renderer — its own
                        // `<think>`-stripping matches the streaming path and
                        // does not trim leading/trailing whitespace (unlike
                        // `xml_fallback::strip_think_tags`). Skip entirely
                        // when `ANVIL_NO_MARKDOWN` disables markdown.
                        if crate::tui::markdown::markdown_fully_disabled() {
                            println!("{prose}");
                        } else {
                            let color = crate::tui::markdown::color_enabled_for_markdown();
                            let utf8 = crate::tui::markdown::markdown_unicode_enabled();
                            let mut r = crate::tui::markdown::MarkdownRenderer::new(color, utf8);
                            let mut body = r.push_chunk(&prose);
                            body.push_str(&r.flush());
                            // Preserve the trailing newline that `println!`
                            // used to add.
                            if !body.ends_with('\n') {
                                body.push('\n');
                            }
                            let _ = std::io::stdout().write_all(body.as_bytes());
                            let _ = std::io::stdout().flush();
                        }
                    }
                    let summary = format_run_summary(ExitReason::Done, &stats);
                    println!();
                    if self.session.mode_state.mode == ExecutionMode::Plan {
                        let plan_contents = self.current_plan_contents()?.unwrap_or_default();
                        if self.plan_is_approval_ready_with_fallback(&plan_contents) {
                            match self.prompt_for_plan_approval_choice()? {
                                Some(PlanApprovalChoice::Execute) => {
                                    return self.execute_approved_plan("yes", stream_output);
                                }
                                Some(PlanApprovalChoice::Revise) => {
                                    println!(
                                        "plan ready: not approved; provide revisions or feedback"
                                    );
                                }
                                Some(PlanApprovalChoice::Feedback) => {
                                    println!("plan ready: provide feedback to revise the plan");
                                }
                                None => {
                                    println!(
                                        "plan ready: reply yes to execute, no to revise, or provide feedback"
                                    );
                                }
                            }
                        } else {
                            let next_sections = lifecycle::plan_next_stage_sections(&plan_contents);
                            if next_sections.is_empty() {
                                println!("plan in progress: continue refining the plan");
                            } else {
                                let stage = lifecycle::current_plan_stage(&plan_contents);
                                println!(
                                    "plan in progress: {}",
                                    plan_stage_status_line(stage, &next_sections)
                                );
                            }
                        }
                    }
                    println!("{summary}");
                    Ok(AgentEvent::Continue(None))
                }
                Err((reason, error_text, stats)) => {
                    let summary = format_run_summary(reason, &stats);
                    println!();
                    println!("{summary}");
                    // Soft failures keep the REPL alive so the user can give a
                    // follow-up instruction without losing the session.
                    if reason.keeps_repl_alive() {
                        if !error_text.trim().is_empty() {
                            println!("{error_text}");
                        }
                        Ok(AgentEvent::Continue(None))
                    } else {
                        Err(error_text)
                    }
                }
            }
        };

        let persist_error = self.persist_session().err();
        match (outcome, persist_error) {
            (Ok(event), None) => Ok(event),
            (Ok(_), Some(err)) => Err(err),
            (Err(err), None) => Err(err),
            (Err(err), Some(persist_err)) => Err(format!(
                "{err} (also failed to persist session: {persist_err})"
            )),
        }
    }

    fn handle_command(&mut self, input: &str) -> Result<AgentEvent, String> {
        let (command, _) = input.split_once(' ').unwrap_or((input, ""));
        match command {
            "/help" => Ok(AgentEvent::Continue(Some(slash_commands::help_line()))),
            "/status" => Ok(AgentEvent::Continue(Some(format!(
                "mode={:?} task_profile={} auto_approve={} auto_plan={} native_tools={} cwd={} session={} plan={} approx_tokens={} log_level={} core_only=true",
                self.session.mode_state.mode,
                self.session.mode_state.task_profile.as_str(),
                self.config.yes_mode,
                self.config.auto_plan,
                self.native_tools_enabled,
                self.work_root.display(),
                self.session_store.path().display(),
                self.session
                    .mode_state
                    .active_plan_path
                    .as_ref()
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|| "-".to_string()),
                approximate_token_count(&self.session.messages),
                self.config.log_level,
            )))),
            "/model" => Ok(AgentEvent::Continue(Some(format_model_banner(
                &self.models,
            )))),
            "/yes" => {
                self.config.yes_mode = true;
                // Issue #430: republish flags so the footer reflects the new
                // yes-mode bit on the next render tick.
                self.footer.publish_flags(
                    self.session.mode_state.mode,
                    self.config.log_level,
                    self.config.yes_mode,
                );
                Ok(AgentEvent::Continue(Some(
                    "auto-approve enabled".to_string(),
                )))
            }
            "/no" => {
                self.config.yes_mode = false;
                self.footer.publish_flags(
                    self.session.mode_state.mode,
                    self.config.log_level,
                    self.config.yes_mode,
                );
                Ok(AgentEvent::Continue(Some(
                    "auto-approve disabled".to_string(),
                )))
            }
            "/plan" => Ok(AgentEvent::Continue(Some(
                self.enter_plan_mode(self.session.mode_state.task_profile)?,
            ))),
            "/approve" | "/act" => Ok(AgentEvent::Continue(Some(self.approve_plan_mode()?))),
            "/compact" => {
                let changed = self.maybe_compact_session(20);
                Ok(AgentEvent::Continue(Some(if changed {
                    "session compacted".to_string()
                } else {
                    "session already compact".to_string()
                })))
            }
            "/logs" => {
                let args: Vec<&str> = input
                    .trim_start_matches("/logs")
                    .split_whitespace()
                    .collect();
                match args.as_slice() {
                    ["path"] => {
                        let path = self.session_store.log_dir().display().to_string();
                        Ok(AgentEvent::Continue(Some(format!("log dir: {path}"))))
                    }
                    ["path", session_id] => match uuid::Uuid::parse_str(session_id) {
                        Err(_) => Ok(AgentEvent::Continue(Some(format!(
                            "invalid session id: {session_id}"
                        )))),
                        Ok(_) => match self.session_store.log_dir_for(session_id) {
                            Some(path) => Ok(AgentEvent::Continue(Some(format!(
                                "log dir: {}",
                                path.display()
                            )))),
                            None => Ok(AgentEvent::Continue(Some(format!(
                                "session not found: {session_id}"
                            )))),
                        },
                    },
                    _ => Ok(AgentEvent::Continue(Some(
                        "/logs path [<session_id>] — show log dir".to_string(),
                    ))),
                }
            }
            "/checkpoint" | "/rollback" | "/watch" | "/autotest" | "/skills" | "/skill"
            | "/mcp" | "/parallel" => Ok(AgentEvent::Continue(Some(format!(
                "{command} is unavailable in the v0.1.0 core rebuild"
            )))),
            "/exit" | "/quit" => Ok(AgentEvent::Exit),
            _ => Ok(AgentEvent::Continue(Some(format!(
                "unknown command: {command}"
            )))),
        }
    }
}

fn should_render_fallback_prompt(stdout_is_tty: bool) -> bool {
    stdout_is_tty
}

/// NotFound (or PermissionDenied on some CI/sandbox environments) on
/// `load_history` means "no history yet" — swallow silently so REPL starts
/// cleanly on a fresh environment.
fn is_not_found(err: &rustyline::error::ReadlineError) -> bool {
    matches!(
        err,
        rustyline::error::ReadlineError::Io(io_err)
            if io_err.kind() == std::io::ErrorKind::NotFound
                || io_err.kind() == std::io::ErrorKind::PermissionDenied
    )
}

/// Defense-in-depth: rustyline 14 already chmods 0o600 on save, but in case
/// an external tool widened the file, we re-apply 0o600 after append. Best
/// effort; the path-not-found case is silent (common on first run before
/// `append_history` has created the file).
///
/// Hardening against CB-002: we use `symlink_metadata()` + `FileType::is_file()`
/// so a symlink-swap or non-regular-file substitution at `state_root/history`
/// does NOT cause us to chmod an unintended target. A symlink here already
/// implies an adversarial / misconfigured state (state_root is owner-only
/// 0o700 on Unix) so we log a warning and skip instead of following the link.
#[cfg(unix)]
fn tighten_history_perms(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let meta = match std::fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return,
        Err(err) => {
            tracing::warn!(
                "readline: failed to stat history for chmod ({}): {err}",
                path.display()
            );
            return;
        }
    };
    if !meta.file_type().is_file() {
        tracing::warn!(
            "readline: refusing to chmod non-regular history path ({}): file_type={:?}",
            path.display(),
            meta.file_type()
        );
        return;
    }
    if let Err(err) = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)) {
        tracing::warn!(
            "readline: failed to chmod history ({}): {err}",
            path.display()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modes::plan_act::ExecutionMode;
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;

    /// Serialize env-mutating tests within this module so `cargo test`'s
    /// default parallel runner cannot race on `NO_COLOR`.
    static ENV_MUTEX: Mutex<()> = Mutex::new(());

    /// RAII guard that snapshots `NO_COLOR` on construction and restores it on
    /// drop. All env mutation is confined to `#[cfg(test)]` per CLAUDE.md.
    struct NoColorGuard {
        prior: Option<std::ffi::OsString>,
    }

    impl NoColorGuard {
        fn capture() -> Self {
            Self {
                prior: std::env::var_os("NO_COLOR"),
            }
        }

        fn set(&self, value: &str) {
            // SAFETY: test-only env mutation, serialized by `ENV_MUTEX`.
            unsafe {
                std::env::set_var("NO_COLOR", value);
            }
        }

        fn unset(&self) {
            // SAFETY: test-only env mutation, serialized by `ENV_MUTEX`.
            unsafe {
                std::env::remove_var("NO_COLOR");
            }
        }
    }

    impl Drop for NoColorGuard {
        fn drop(&mut self) {
            match &self.prior {
                Some(value) => {
                    // SAFETY: test-only env mutation, serialized by `ENV_MUTEX`.
                    unsafe {
                        std::env::set_var("NO_COLOR", value);
                    }
                }
                None => {
                    // SAFETY: test-only env mutation, serialized by `ENV_MUTEX`.
                    unsafe {
                        std::env::remove_var("NO_COLOR");
                    }
                }
            }
        }
    }

    fn sample_mode() -> ExecutionMode {
        ExecutionMode::Act
    }

    fn build_inputs<'a>(
        model_banner: &'a str,
        mode: &'a ExecutionMode,
        work_root: &'a Path,
        llm_log_path: Option<&'a Path>,
        fresh: bool,
        resumed: bool,
        style: BannerStyle,
    ) -> BannerInputs<'a> {
        BannerInputs {
            version: "9.9.9",
            model_banner,
            mode,
            work_root,
            session_id_short: "abcd1234",
            message_count: 3,
            fresh,
            resumed,
            llm_log_path,
            style,
        }
    }

    // --- decide_banner_style truth table -----------------------------------

    #[test]
    fn decide_banner_style_non_tty_returns_legacy() {
        assert_eq!(
            decide_banner_style(false, false, Some(200)),
            BannerStyle::Legacy4Line
        );
        assert_eq!(
            decide_banner_style(false, true, Some(200)),
            BannerStyle::Legacy4Line
        );
        assert_eq!(
            decide_banner_style(false, false, None),
            BannerStyle::Legacy4Line
        );
    }

    #[test]
    fn decide_banner_style_tty_size_err_returns_legacy() {
        assert_eq!(
            decide_banner_style(true, false, None),
            BannerStyle::Legacy4Line
        );
    }

    #[test]
    fn decide_banner_style_tty_narrow_returns_legacy() {
        assert_eq!(
            decide_banner_style(true, false, Some(MIN_BANNER_WIDTH - 1)),
            BannerStyle::Legacy4Line
        );
        assert_eq!(
            decide_banner_style(true, true, Some(10)),
            BannerStyle::Legacy4Line
        );
    }

    #[test]
    fn fallback_prompt_only_renders_on_tty_stdout() {
        assert!(should_render_fallback_prompt(true));
        assert!(!should_render_fallback_prompt(false));
    }

    #[test]
    fn parses_large_task_classifier_json() {
        assert_eq!(
            parse_large_task_decision(r#"{"large_task":true}"#, None),
            Some(ClassifiedTask {
                large_task: true,
                task_profile: TaskProfile::Generic,
            })
        );
        assert_eq!(
            parse_large_task_decision(
                "```json\n{\"large_task\":false,\"task_profile\":\"coding\"}\n```",
                None,
            ),
            Some(ClassifiedTask {
                large_task: false,
                task_profile: TaskProfile::Coding,
            })
        );
        assert_eq!(parse_large_task_decision("not json", None), None);
    }

    #[test]
    fn parses_large_task_classifier_json_after_think_block() {
        assert_eq!(
            parse_large_task_decision(
                "<think>classify the request first</think>\n{\"large_task\":true,\"task_profile\":\"coding\"}",
                None,
            ),
            Some(ClassifiedTask {
                large_task: true,
                task_profile: TaskProfile::Coding,
            })
        );
    }

    #[test]
    fn classifier_parse_error_does_not_echo_model_text() {
        let error = classifier_parse_error();
        assert!(!error.contains("<think>"));
        assert!(!error.contains("analysis"));
        assert_eq!(error, "classifier response was not JSON-only");
    }

    #[test]
    fn parses_large_task_classifier_fallback_shape() {
        assert_eq!(
            parse_large_task_decision(
                r#"{
                    "action":"create_plan",
                    "plan":"README improvement process",
                    "steps":["draft","edit","verify"]
                }"#,
                None,
            ),
            Some(ClassifiedTask {
                large_task: true,
                task_profile: TaskProfile::Content,
            })
        );
    }

    #[test]
    fn parses_large_task_classifier_write_plan_fallback_shape() {
        assert_eq!(
            parse_large_task_decision(
                r#"{
                    "action":"write_plan",
                    "action_input":"README.md を改善する3段階の作業です"
                }"#,
                None,
            ),
            Some(ClassifiedTask {
                large_task: true,
                task_profile: TaskProfile::Content,
            })
        );
    }

    #[test]
    fn infers_coding_profile_for_nextjs_app_requests() {
        assert_eq!(
            infer_task_profile_from_text("Next.js アプリとしてブラウザゲームを開発してください。"),
            TaskProfile::Coding
        );
        assert_eq!(
            infer_task_profile_from_text("Nuxt.js アプリとしてテトリスゲームを開発してください。"),
            TaskProfile::Coding
        );
    }

    #[test]
    fn detects_classifier_transport_errors() {
        assert!(is_classifier_transport_error(
            "failed to contact Ollama generate API: error sending request for url"
        ));
        assert!(is_classifier_transport_error("connection refused"));
        assert!(!is_classifier_transport_error(
            "classifier response was not JSON-only"
        ));
    }

    #[test]
    fn classifier_model_prefers_sidecar_when_available() {
        assert_eq!(
            classifier_model("main-model", Some("sidecar-model")),
            "sidecar-model"
        );
        assert_eq!(classifier_model("main-model", None), "main-model");
    }

    #[test]
    fn infers_large_task_for_staged_japanese_request() {
        assert!(infer_large_task_from_text(
            "README.md を改善する3段階の作業です。まず変更計画を書き、その後見出しを1つ追加し、最後に内容を確認してください。"
        ));
        assert!(!infer_large_task_from_text(
            "README の typo を1箇所だけ修正して"
        ));
    }

    #[test]
    fn heuristic_classification_prefers_content_for_readme_work() {
        let classified = heuristic_classified_task(
            "README.md を改善する3段階の作業です。まず変更計画を書き、その後見出しを1つ追加し、最後に内容を確認してください。",
        );
        assert!(classified.large_task);
        assert_eq!(classified.task_profile, TaskProfile::Content);
    }

    #[test]
    fn detects_plan_execution_requests_in_english_and_japanese() {
        assert!(is_plan_execution_request("実行して"));
        assert!(is_plan_execution_request("進めて。"));
        assert!(is_plan_execution_request("approve"));
        assert!(is_plan_execution_request("run it"));
        assert!(!is_plan_execution_request("計画をもう少し詳しくして"));
    }

    #[test]
    fn detects_plan_rejection_requests_in_english_and_japanese() {
        assert!(is_plan_rejection_request("no"));
        assert!(is_plan_rejection_request("いいえ。"));
        assert!(!is_plan_rejection_request("計画をもう少し詳しくして"));
    }

    #[test]
    fn parses_plan_approval_choice_aliases() {
        assert_eq!(
            parse_plan_approval_choice("yes"),
            Some(PlanApprovalChoice::Execute)
        );
        assert_eq!(
            parse_plan_approval_choice("n"),
            Some(PlanApprovalChoice::Revise)
        );
        assert_eq!(
            parse_plan_approval_choice("feedback"),
            Some(PlanApprovalChoice::Feedback)
        );
        assert_eq!(parse_plan_approval_choice("maybe"), None);
    }

    #[test]
    fn plan_stage_status_line_describes_focus() {
        assert_eq!(
            plan_stage_status_line(PlanStage::Stage1, &["Goal", "Constraints"]),
            "Current phase: Draft Goal, Constraints"
        );
        assert_eq!(
            plan_stage_status_line(PlanStage::Stage2, &["First Action", "Verification"]),
            "Current phase: Define First Action, Verification"
        );
    }

    #[test]
    fn decide_banner_style_tty_wide_no_color_returns_mono_art() {
        assert_eq!(
            decide_banner_style(true, true, Some(MIN_BANNER_WIDTH)),
            BannerStyle::MonoArt
        );
        assert_eq!(
            decide_banner_style(true, true, Some(200)),
            BannerStyle::MonoArt
        );
    }

    #[test]
    fn decide_banner_style_tty_wide_color_returns_neon() {
        assert_eq!(
            decide_banner_style(true, false, Some(MIN_BANNER_WIDTH)),
            BannerStyle::Neon
        );
        assert_eq!(
            decide_banner_style(true, false, Some(200)),
            BannerStyle::Neon
        );
    }

    // --- state_suffix ------------------------------------------------------

    #[test]
    fn state_suffix_fresh() {
        assert_eq!(state_suffix(true, false), "fresh");
    }

    #[test]
    fn state_suffix_resumed() {
        assert_eq!(state_suffix(false, true), "resumed");
    }

    #[test]
    fn state_suffix_continued() {
        assert_eq!(state_suffix(false, false), "continued");
    }

    #[test]
    fn state_suffix_fresh_beats_resumed() {
        // Defensive: `fresh` dominates so a caller that mistakenly sets both
        // flags still gets the intended `fresh` label.
        assert_eq!(state_suffix(true, true), "fresh");
    }

    // --- banner_no_color_requested vs turn::no_color_requested -------------

    #[test]
    fn banner_no_color_requested_unset_is_false() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let guard = NoColorGuard::capture();
        guard.unset();
        assert!(!banner_no_color_requested());
        assert!(!super::super::turn::no_color_requested());
    }

    #[test]
    fn banner_no_color_requested_empty_is_false() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let guard = NoColorGuard::capture();
        guard.set("");
        assert!(!banner_no_color_requested());
        assert!(!super::super::turn::no_color_requested());
    }

    #[test]
    fn banner_no_color_requested_nonempty_is_true() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let guard = NoColorGuard::capture();
        guard.set("1");
        assert!(banner_no_color_requested());
        assert!(super::super::turn::no_color_requested());
    }

    #[test]
    fn banner_no_color_requested_matches_turn_across_values() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let guard = NoColorGuard::capture();
        for value in ["1", "yes", "0", "false", ""] {
            guard.set(value);
            assert_eq!(
                banner_no_color_requested(),
                super::super::turn::no_color_requested(),
                "drift detected for NO_COLOR={value:?}"
            );
        }
        guard.unset();
        assert_eq!(
            banner_no_color_requested(),
            super::super::turn::no_color_requested()
        );
    }

    // --- sanitize_banner_text ---------------------------------------------

    #[test]
    fn sanitize_banner_text_replaces_esc() {
        assert_eq!(sanitize_banner_text("\x1b[31mred"), " [31mred");
    }

    #[test]
    fn sanitize_banner_text_replaces_all_c0_and_del() {
        let hostile = "a\nb\rc\x07d\x1bE\x7f";
        let cleaned = sanitize_banner_text(hostile);
        assert!(!cleaned.contains('\x1b'));
        assert!(!cleaned.contains('\n'));
        assert!(!cleaned.contains('\r'));
        assert!(!cleaned.contains('\x07'));
        assert!(!cleaned.contains('\x7f'));
    }

    #[test]
    fn sanitize_banner_text_replaces_c1_controls_including_8bit_csi_osc() {
        // Some terminals interpret U+009B (8-bit CSI) and U+009D (8-bit OSC)
        // equivalently to `ESC [` and `ESC ]`, so they must be sanitized too.
        let hostile = "a\u{009B}31mred\u{009D}8;;bad.example\u{009C}tail";
        let cleaned = sanitize_banner_text(hostile);
        assert!(!cleaned.contains('\u{009B}'));
        assert!(!cleaned.contains('\u{009D}'));
        assert!(!cleaned.contains('\u{009C}'));
        // A valid 2-byte UTF-8 (e.g. \u{00C0}..=\u{00FF}) outside C1 must survive.
        assert_eq!(sanitize_banner_text("caf\u{00E9}"), "caf\u{00E9}");
    }

    #[test]
    fn sanitize_banner_text_trims_trailing_whitespace() {
        assert_eq!(sanitize_banner_text("ok   "), "ok");
        assert_eq!(sanitize_banner_text("ok\n\n"), "ok");
    }

    #[test]
    fn sanitize_banner_text_matches_turn_sanitize_spec() {
        // Ensure our sanitizer agrees with the progress-line sanitizer spec:
        // C0, DEL, and C1 become spaces, then trailing whitespace is trimmed.
        for input in [
            "plain",
            "hello\x1b[0m",
            "line1\nline2",
            "bell\x07tail   ",
            "  mid  space  ",
            "csi8\u{009B}31mred",
            "osc8\u{009D}8;;evil.example",
        ] {
            // Re-implement the expected algorithm inline so drift in either
            // implementation is caught by the assertion.
            let mut expected = String::with_capacity(input.len());
            for ch in input.chars() {
                let cp = ch as u32;
                if cp < 0x20 || cp == 0x7F || (0x80..=0x9F).contains(&cp) {
                    expected.push(' ');
                } else {
                    expected.push(ch);
                }
            }
            let expected = expected.trim_end().to_string();
            assert_eq!(sanitize_banner_text(input), expected);
        }
    }

    // --- render_startup_banner: Legacy4Line byte-identical -----------------

    #[test]
    fn render_legacy4line_bytes_fresh_no_log() {
        let mode = sample_mode();
        let cwd = PathBuf::from("/tmp/anvil-test");
        let inputs = build_inputs(
            "llama3.2 model-banner",
            &mode,
            &cwd,
            None,
            true,
            false,
            BannerStyle::Legacy4Line,
        );
        let out = render_startup_banner(&inputs);
        assert_eq!(
            out,
            "anvil 9.9.9\n\
             llama3.2 model-banner\n\
             mode=Act cwd=/tmp/anvil-test\n\
             session=abcd1234 messages=3 [fresh]\n"
        );
    }

    #[test]
    fn render_legacy4line_bytes_resumed_no_log() {
        let mode = sample_mode();
        let cwd = PathBuf::from("/tmp/anvil-test");
        let inputs = build_inputs(
            "llama3.2 model-banner",
            &mode,
            &cwd,
            None,
            false,
            true,
            BannerStyle::Legacy4Line,
        );
        let out = render_startup_banner(&inputs);
        assert!(out.ends_with("[resumed]\n"));
        assert!(!out.contains("\x1b["));
    }

    #[test]
    fn render_legacy4line_bytes_continued_no_log() {
        let mode = sample_mode();
        let cwd = PathBuf::from("/tmp/anvil-test");
        let inputs = build_inputs(
            "m",
            &mode,
            &cwd,
            None,
            false,
            false,
            BannerStyle::Legacy4Line,
        );
        let out = render_startup_banner(&inputs);
        assert!(out.ends_with("[continued]\n"));
    }

    #[test]
    fn render_legacy4line_bytes_with_llm_log() {
        let mode = sample_mode();
        let cwd = PathBuf::from("/tmp/anvil-test");
        let log = PathBuf::from("/tmp/anvil-test/llm-io.jsonl");
        let inputs = build_inputs(
            "llama3.2 model-banner",
            &mode,
            &cwd,
            Some(log.as_path()),
            true,
            false,
            BannerStyle::Legacy4Line,
        );
        let out = render_startup_banner(&inputs);
        assert_eq!(
            out,
            "anvil 9.9.9\n\
             llama3.2 model-banner\n\
             mode=Act cwd=/tmp/anvil-test\n\
             session=abcd1234 messages=3 [fresh]\n\
             llm log=/tmp/anvil-test/llm-io.jsonl\n"
        );
    }

    // --- render_startup_banner: MonoArt -----------------------------------

    #[test]
    fn render_mono_art_has_no_ansi() {
        let mode = sample_mode();
        let cwd = PathBuf::from("/tmp/anvil-test");
        let inputs = build_inputs(
            "llama3.2",
            &mode,
            &cwd,
            None,
            true,
            false,
            BannerStyle::MonoArt,
        );
        let out = render_startup_banner(&inputs);
        assert!(
            !out.contains("\x1b["),
            "mono art must not contain ANSI escape: {out:?}"
        );
        // Sanity check: output contains the dynamic lines.
        assert!(out.contains("anvil 9.9.9\n"));
        assert!(out.contains("session=abcd1234 messages=3 [fresh]\n"));
    }

    #[test]
    fn render_mono_art_includes_ascii_art_lines() {
        let mode = sample_mode();
        let cwd = PathBuf::from("/tmp/a");
        let inputs = build_inputs("m", &mode, &cwd, None, false, true, BannerStyle::MonoArt);
        let out = render_startup_banner(&inputs);
        for line in ANVIL_ASCII_ART.iter() {
            assert!(out.contains(line), "missing art line: {line:?}");
        }
        assert!(out.contains("[resumed]\n"));
    }

    #[test]
    fn render_mono_art_with_log_path() {
        let mode = sample_mode();
        let cwd = PathBuf::from("/tmp/a");
        let log = PathBuf::from("/tmp/llm.jsonl");
        let inputs = build_inputs(
            "m",
            &mode,
            &cwd,
            Some(log.as_path()),
            false,
            false,
            BannerStyle::MonoArt,
        );
        let out = render_startup_banner(&inputs);
        assert!(out.contains("llm log=/tmp/llm.jsonl\n"));
        assert!(out.contains("[continued]\n"));
        assert!(!out.contains("\x1b["));
    }

    // --- render_startup_banner: Neon --------------------------------------

    #[test]
    fn render_neon_contains_expected_ansi_codes() {
        let mode = sample_mode();
        let cwd = PathBuf::from("/tmp/anvil-test");
        let inputs = build_inputs(
            "llama3.2",
            &mode,
            &cwd,
            None,
            true,
            false,
            BannerStyle::Neon,
        );
        let out = render_startup_banner(&inputs);
        // First gradient color is 51.
        assert!(
            out.contains("\x1b[38;5;51m"),
            "expected first gradient code: {out:?}"
        );
        // Every ASCII-art line terminates with a reset.
        assert!(out.contains("\x1b[0m"));
        // All five gradient codes should appear.
        for code in NEON_GRADIENT_256 {
            let needle = format!("\x1b[38;5;{code}m");
            assert!(
                out.contains(&needle),
                "missing gradient escape {needle:?} in {out:?}"
            );
        }
    }

    #[test]
    fn render_neon_resumed_and_log_combinations() {
        let mode = sample_mode();
        let cwd = PathBuf::from("/tmp/anvil-test");
        let log = PathBuf::from("/tmp/anvil-test/llm.jsonl");
        let inputs = build_inputs(
            "m",
            &mode,
            &cwd,
            Some(log.as_path()),
            false,
            true,
            BannerStyle::Neon,
        );
        let out = render_startup_banner(&inputs);
        assert!(out.contains("[resumed]\n"));
        assert!(out.contains("llm log=/tmp/anvil-test/llm.jsonl\n"));
        assert!(out.contains("\x1b[38;5;51m"));
    }

    #[test]
    fn render_neon_continued_no_log() {
        let mode = sample_mode();
        let cwd = PathBuf::from("/tmp/a");
        let inputs = build_inputs("m", &mode, &cwd, None, false, false, BannerStyle::Neon);
        let out = render_startup_banner(&inputs);
        assert!(out.contains("[continued]\n"));
        assert!(!out.contains("llm log="));
    }

    // --- Hostile-input regression -----------------------------------------

    #[test]
    fn render_sanitizes_hostile_model_banner() {
        let mode = sample_mode();
        let cwd = PathBuf::from("/tmp/anvil-test");
        let inputs = build_inputs(
            "bad\x1b[31m!\nnewline",
            &mode,
            &cwd,
            None,
            true,
            false,
            BannerStyle::Legacy4Line,
        );
        let out = render_startup_banner(&inputs);
        // Dynamic lines must never carry raw ESC / bare newlines from user input.
        assert!(
            !out.contains("\x1b["),
            "raw ESC leaked into Legacy4Line: {out:?}"
        );
        // Sanitized placeholder `!` should still be present.
        assert!(out.contains(" [31m!"));
    }

    #[test]
    fn render_sanitizes_hostile_cwd_and_log_path() {
        let mode = sample_mode();
        let cwd = PathBuf::from("/tmp/with\x1b[31mansi");
        let log = PathBuf::from("/tmp/l\nog.jsonl");
        let inputs = build_inputs(
            "m",
            &mode,
            &cwd,
            Some(log.as_path()),
            true,
            false,
            BannerStyle::Legacy4Line,
        );
        let out = render_startup_banner(&inputs);
        assert!(!out.contains('\x07'));
        assert!(!out.contains("\x1b["));
        // The log path's embedded newline must be neutralized, so the output
        // should still have exactly as many lines as the renderer emitted.
        let line_count = out.split_terminator('\n').count();
        assert_eq!(line_count, 5, "unexpected extra lines: {out:?}");
    }

    #[test]
    fn render_neon_hostile_inputs_contain_no_extra_escapes() {
        let mode = sample_mode();
        let cwd = PathBuf::from("/tmp/with\x1b[31mansi");
        let inputs = build_inputs(
            "m\x1b]8;;https://x\x07",
            &mode,
            &cwd,
            None,
            true,
            false,
            BannerStyle::Neon,
        );
        let out = render_startup_banner(&inputs);
        // Every ESC must belong to a banner-owned SGR escape (38;5;N or 0).
        for (i, byte) in out.as_bytes().iter().enumerate() {
            if *byte == 0x1b {
                // Next bytes must be `[38;5;` or `[0m`.
                let tail = &out.as_bytes()[i..];
                assert!(
                    tail.starts_with(b"\x1b[38;5;") || tail.starts_with(b"\x1b[0m"),
                    "unexpected ESC sequence at {i}: {:?}",
                    &out[i..i + 8.min(out.len() - i)]
                );
            }
        }
    }
}
