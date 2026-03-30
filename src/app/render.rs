//! Console rendering helpers extracted from the main app module.
//!
//! These are pure functions that produce display strings from application
//! state.  They have no side effects and do not depend on the [`App`] struct.

use crate::config::EffectiveConfig;
use crate::contracts::{AppStateSnapshot, ToolLogView};
use crate::extensions::skills::SkillScope;
use crate::extensions::{ExtensionRegistry, SlashCommandSpec, builtin_slash_commands};
use crate::tooling::ToolExecutionStatus;

use crate::agent::AgentEvent;
use crate::spinner::format_elapsed_ms;

// --- Fold display constants ---
const FOLD_THRESHOLD: usize = 10;
const FOLD_PREVIEW_LINES: usize = 3;
const MAX_OUTPUT_BYTES: usize = 102_400;
const MAX_LINE_CHARS: usize = 200;
const MAX_TOOL_NAME_CHARS: usize = 30;
const MAX_SUMMARY_CHARS: usize = 100;

pub fn build_tool_logs(logs: &[(String, String, String)]) -> Vec<ToolLogView> {
    logs.iter()
        .map(|(tool_name, action, target)| ToolLogView {
            tool_name: tool_name.clone(),
            action: action.clone(),
            target: target.clone(),
            elapsed_ms: None,
        })
        .collect()
}

pub fn render_help_frame() -> String {
    render_help_frame_for(&builtin_slash_commands())
}

pub fn render_help_frame_for(commands: &[SlashCommandSpec]) -> String {
    let mut lines = vec!["Anvil slash commands".to_string(), String::new()];
    let max_name_len = commands.iter().map(|s| s.name.len()).max().unwrap_or(10);
    let width = max_name_len.max(10);
    for spec in commands {
        let scope_tag = match &spec.scope {
            Some(SkillScope::User) => " [user]",
            Some(SkillScope::Project) => " [project]",
            None => "",
        };
        lines.push(format!(
            "{:<width$} {}{}",
            spec.name, spec.description, scope_tag
        ));
    }
    lines.join("\n")
}

pub fn render_plan_frame(snapshot: &AppStateSnapshot) -> String {
    let mut lines = vec!["[A] anvil > plan".to_string()];
    if let Some(plan) = &snapshot.plan {
        for (index, item) in plan.items.iter().enumerate() {
            let marker = if plan.active_index == Some(index) {
                "*"
            } else {
                "-"
            };
            lines.push(format!("  {marker} {}. {}", index + 1, item));
        }
    } else {
        lines.push("  no active plan".to_string());
    }
    lines.join("\n")
}

pub fn render_model_frame(effective_model: &str, provider: &str, context_window: u32) -> String {
    format!(
        "[A] anvil > current model: {}\n  provider: {}\n  context window: {}",
        effective_model, provider, context_window
    )
}

pub fn render_provider_frame(
    effective_model: &str,
    config: &EffectiveConfig,
    provider: &crate::provider::ProviderRuntimeContext,
) -> String {
    let mut output = format!(
        "[A] anvil > provider: {}\n  url: {}\n  model: {}\n  streaming: {}\n  tool-calling: {}",
        config.runtime.provider,
        config.runtime.provider_url,
        effective_model,
        provider.capabilities.streaming,
        provider.capabilities.tool_calling
    );
    // Show sidecar configuration when sidecar_model is set
    if let Some(ref sidecar_model) = config.runtime.sidecar_model {
        let sidecar_url = config
            .runtime
            .sidecar_provider_url
            .as_deref()
            .unwrap_or(crate::config::DEFAULT_OLLAMA_URL);
        output.push_str(&format!(
            "\n  sidecar model: {}\n  sidecar url: {}",
            sidecar_model, sidecar_url
        ));
    }
    output
}

/// Render the model list from Ollama.
pub fn render_model_list_frame(
    models: &[crate::provider::OllamaModelEntry],
    current_model: &str,
) -> String {
    let mut lines = vec![format!("[A] anvil > {} model(s) available", models.len())];
    for entry in models {
        let marker = if entry.name == current_model {
            " *"
        } else {
            ""
        };
        let size_mb = entry.size / 1_048_576;
        lines.push(format!("  {}{} ({}MB)", entry.name, marker, size_mb));
    }
    lines.join("\n")
}

/// Render detailed model information from Ollama.
pub fn render_model_info_frame(
    model: &str,
    info: &crate::provider::OllamaModelInfo,
    context_window: u32,
) -> String {
    let mut lines = vec![format!("[A] anvil > model info: {model}")];
    if let Some(ref param_size) = info.parameter_size {
        lines.push(format!("  parameters: {param_size}"));
    }
    if let Some(ref quant) = info.quantization_level {
        lines.push(format!("  quantization: {quant}"));
    }
    if let Some(ctx) = info.context_length {
        lines.push(format!("  context length: {ctx}"));
    }
    lines.push(format!("  effective context window: {context_window}"));
    lines.join("\n")
}

/// Render a successful model switch message.
pub fn render_model_switch_success(model_name: &str, context_window: u32) -> String {
    format!(
        "[A] anvil > switched to model: {} (context window: {})\n  note: this change is for the current session only",
        model_name, context_window
    )
}

pub fn render_resume_header(
    effective_model: &str,
    effective_context_window: u32,
    config: &EffectiveConfig,
    session_name: &str,
) -> String {
    let mut lines = vec![
        "  --------------------------------------------------------------".to_string(),
        "  Resuming existing session".to_string(),
        format!("  Session : {session_name}"),
        format!("  Model   : {}", effective_model),
        format!("  Context : {}k", effective_context_window / 1_000),
        format!("  Project : {}", config.paths.cwd.display()),
    ];

    if config.project_instructions().is_some() {
        lines.push("  ANVIL.md: loaded".to_string());
    }

    lines.push("  --------------------------------------------------------------".to_string());
    lines.join("\n")
}

pub fn cli_prompt() -> &'static str {
    "[U] you > "
}

pub fn slash_commands() -> Vec<SlashCommandSpec> {
    ExtensionRegistry::new().slash_commands().to_vec()
}

pub fn render_status_detail(snapshot: &AppStateSnapshot) -> String {
    if let Some(usage) = &snapshot.context_usage {
        format!(
            "  tokens: {}/{} ({}%)",
            usage.estimated_tokens,
            usage.max_tokens,
            usage.usage_percent()
        )
    } else {
        "  tokens: -/-".to_string()
    }
}

pub fn render_pending_approval_frame(snapshot: &AppStateSnapshot) -> String {
    if let Some(approval) = &snapshot.approval {
        let mut text = format!(
            "[A] anvil > resolve the pending approval before starting a new turn\n  pending: {} {}\n  call: {}\n  use /approve or /deny",
            approval.tool_name, approval.summary, approval.tool_call_id
        );
        if let Some(diff) = &approval.diff_preview {
            text.push_str(&format!("\n{}", crate::tui::colorize_diff(diff)));
        }
        text
    } else {
        "[A] anvil > resolve the pending approval before starting a new turn\n  use /approve or /deny"
            .to_string()
    }
}

pub fn map_tool_status(action: &str) -> ToolExecutionStatus {
    match action {
        "failed" => ToolExecutionStatus::Failed,
        "interrupted" => ToolExecutionStatus::Interrupted,
        _ => ToolExecutionStatus::Completed,
    }
}

pub fn should_render_stream_progress(
    token_buffer: &str,
    delta: &str,
    last_rendered_len: usize,
) -> bool {
    last_rendered_len == 0
        || token_buffer.len().saturating_sub(last_rendered_len) >= 512
        || delta.contains('\n')
        || delta.contains("```ANVIL_")
}

pub fn recent_stream_excerpt(content: &str, max_chars: usize) -> String {
    let chars: Vec<char> = content.chars().collect();
    if chars.len() <= max_chars {
        return content.to_string();
    }

    let tail: String = chars[chars.len() - max_chars..].iter().collect();
    format!("...{tail}")
}

pub fn approval_tool_call_id(event: &AgentEvent) -> String {
    match event {
        AgentEvent::ApprovalRequested { tool_call_id, .. } => tool_call_id.clone(),
        _ => "pending_approval".to_string(),
    }
}

/// Strip ANSI escape sequences from a string.
///
/// Handles CSI sequences (`ESC[...X`), OSC sequences (`ESC]...ST`), and
/// simple two-byte sequences (`ESC X`).  Implemented without regex.
pub(crate) fn strip_ansi_escapes(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            match chars.peek() {
                Some('[') => {
                    // CSI sequence: consume until a letter in '@'..='~'
                    chars.next(); // consume '['
                    while let Some(&ch) = chars.peek() {
                        chars.next();
                        if ch.is_ascii_alphabetic() || ch == '~' || ch == '@' {
                            break;
                        }
                    }
                }
                Some(']') => {
                    // OSC sequence: consume until ST (\x1b\\) or BEL (\x07)
                    chars.next(); // consume ']'
                    while let Some(&ch) = chars.peek() {
                        if ch == '\x07' {
                            chars.next();
                            break;
                        }
                        if ch == '\x1b' {
                            chars.next();
                            if chars.peek() == Some(&'\\') {
                                chars.next();
                            }
                            break;
                        }
                        chars.next();
                    }
                }
                Some(_) => {
                    // Simple two-byte escape
                    chars.next();
                }
                None => {}
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Sanitize a display string: strip ANSI escapes, replace control characters,
/// and truncate to `max_chars` with an ellipsis marker.
#[allow(dead_code)]
pub(crate) fn sanitize_display_string(s: &str, max_chars: usize) -> String {
    let stripped = strip_ansi_escapes(s);
    let clean: String = stripped
        .chars()
        .map(|c| if c.is_control() && c != '\n' { ' ' } else { c })
        .collect();
    truncate_with_ellipsis(&clean, max_chars)
}

/// Sanitize a single output line: strip ANSI escapes, collapse to one line,
/// replace control characters, and truncate.
#[allow(dead_code)]
pub(crate) fn sanitize_output_line(s: &str, max_chars: usize) -> String {
    let stripped = strip_ansi_escapes(s);
    let one_line: String = stripped
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let trimmed = one_line.trim().to_string();
    truncate_with_ellipsis(&trimmed, max_chars)
}

fn truncate_with_ellipsis(s: &str, max_chars: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max_chars {
        return s.to_string();
    }
    if max_chars <= 3 {
        return chars[..max_chars].iter().collect();
    }
    let mut result: String = chars[..max_chars - 3].iter().collect();
    result.push_str("...");
    result
}

/// Format a tool execution result for folded display on stderr.
///
/// Short outputs (fewer than `FOLD_THRESHOLD` lines) are shown in full.
/// Longer outputs are folded: only `FOLD_PREVIEW_LINES` lines are shown,
/// followed by a "... N lines hidden" marker.
///
/// All output is sanitized: ANSI escapes are stripped, individual lines are
/// truncated to `MAX_LINE_CHARS`, and the total input is capped at
/// `MAX_OUTPUT_BYTES`.
pub(crate) fn format_tool_result_for_display(
    tool_name: &str,
    summary: &str,
    output: &str,
    elapsed_ms: u64,
) -> String {
    let safe_tool_name = sanitize_output_line(tool_name, MAX_TOOL_NAME_CHARS);
    let safe_summary = sanitize_output_line(summary, MAX_SUMMARY_CHARS);
    let elapsed = format_elapsed_ms(elapsed_ms);

    // Cap input size
    let capped = if output.len() > MAX_OUTPUT_BYTES {
        &output[..MAX_OUTPUT_BYTES]
    } else {
        output
    };

    // Strip ANSI from entire output
    let clean = strip_ansi_escapes(capped);

    let mut lines = Vec::new();
    lines.push(format!("  [{safe_tool_name}] {safe_summary} ({elapsed})"));

    if clean.is_empty() {
        return lines.join("\n");
    }

    let output_lines: Vec<&str> = clean.lines().collect();

    if output_lines.len() < FOLD_THRESHOLD {
        // Show all lines, each truncated
        for line in &output_lines {
            lines.push(format!("  {}", sanitize_output_line(line, MAX_LINE_CHARS)));
        }
    } else {
        // Show preview lines + hidden marker
        for line in output_lines.iter().take(FOLD_PREVIEW_LINES) {
            lines.push(format!("  {}", sanitize_output_line(line, MAX_LINE_CHARS)));
        }
        let hidden = output_lines.len() - FOLD_PREVIEW_LINES;
        lines.push(format!("  ... {hidden} lines hidden"));
    }

    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_ansi_escapes_plain_text() {
        assert_eq!(strip_ansi_escapes("hello world"), "hello world");
    }

    #[test]
    fn strip_ansi_escapes_color_codes() {
        assert_eq!(strip_ansi_escapes("\x1b[31mred\x1b[0m"), "red");
    }

    #[test]
    fn strip_ansi_escapes_complex_sequence() {
        assert_eq!(
            strip_ansi_escapes("\x1b[1;32mbold green\x1b[0m normal"),
            "bold green normal"
        );
    }

    #[test]
    fn sanitize_display_string_plain() {
        assert_eq!(sanitize_display_string("hello", 10), "hello");
    }

    #[test]
    fn sanitize_display_string_truncates() {
        assert_eq!(sanitize_display_string("hello world!", 8), "hello...");
    }

    #[test]
    fn sanitize_output_line_strips_and_truncates() {
        let input = "\x1b[31mred text is long\x1b[0m";
        let result = sanitize_output_line(input, 10);
        assert_eq!(result, "red tex...");
    }

    #[test]
    fn sanitize_output_line_collapses_newlines() {
        let input = "line1\nline2\nline3";
        let result = sanitize_output_line(input, 100);
        assert_eq!(result, "line1 line2 line3");
    }

    #[test]
    fn fold_display_short_output() {
        // 9 lines (below FOLD_THRESHOLD=10) should display all lines
        let output = (1..=9)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let result = format_tool_result_for_display("file.read", "read ok", &output, 500);
        for i in 1..=9 {
            assert!(
                result.contains(&format!("line {i}")),
                "should contain line {i}"
            );
        }
        assert!(
            !result.contains("hidden"),
            "should not contain hidden marker"
        );
    }

    #[test]
    fn fold_display_10_lines_folds() {
        // 10 lines (at FOLD_THRESHOLD) should fold: 3 preview + hidden marker
        let output = (1..=10)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let result = format_tool_result_for_display("file.read", "read ok", &output, 1200);
        assert!(result.contains("line 1"), "should show first preview line");
        assert!(result.contains("line 2"), "should show second preview line");
        assert!(result.contains("line 3"), "should show third preview line");
        assert!(
            result.contains("7 lines hidden"),
            "should show hidden count"
        );
        assert!(!result.contains("line 4"), "line 4 should be hidden");
    }

    #[test]
    fn fold_display_empty_output() {
        let result = format_tool_result_for_display("file.read", "read ok", "", 100);
        assert!(result.contains("file.read"), "should contain tool name");
        assert!(result.contains("read ok"), "should contain summary");
    }

    #[test]
    fn fold_display_elapsed_format() {
        let result = format_tool_result_for_display("file.read", "ok", "hello", 2300);
        assert!(result.contains("(2.3s)"), "should format elapsed as (2.3s)");
    }

    #[test]
    fn fold_display_sanitizes_ansi() {
        let output = "\x1b[31mred text\x1b[0m";
        let result = format_tool_result_for_display("file.read", "ok", output, 100);
        assert!(!result.contains("\x1b["), "should strip ANSI escapes");
        assert!(result.contains("red text"), "should keep text content");
    }

    #[test]
    fn fold_display_truncates_long_lines() {
        let long_line = "x".repeat(250);
        let result = format_tool_result_for_display("file.read", "ok", &long_line, 100);
        // Each line should be truncated to MAX_LINE_CHARS (200)
        for line in result.lines() {
            assert!(
                line.chars().count() <= 210,
                "line should be truncated to ~200 chars: len={}",
                line.chars().count()
            );
        }
    }

    #[test]
    fn fold_display_limits_input_size() {
        // 100KB+ input should be truncated to first 100KB
        let large = "x".repeat(150_000);
        let result = format_tool_result_for_display("file.read", "ok", &large, 100);
        // Should not panic and should produce output
        assert!(result.contains("file.read"), "should contain tool name");
    }

    #[test]
    fn fold_display_sanitizes_tool_name() {
        let bad_name = "a\x1b[31m_evil\nname_that_is_way_too_long_for_display_purposes_really";
        let result = format_tool_result_for_display(bad_name, "ok", "hello", 100);
        assert!(
            !result.contains("\x1b["),
            "should strip ANSI from tool name"
        );
        assert!(
            !result.contains('\n') || result.lines().count() <= 5,
            "tool name should not inject newlines into header"
        );
    }
}
