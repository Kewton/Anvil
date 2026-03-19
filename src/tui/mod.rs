//! Terminal user interface rendering.
//!
//! [`Tui`] is a stateless renderer that converts [`ConsoleRenderContext`]
//! snapshots into plain-text frames for the terminal.

use crate::config::EffectiveConfig;
use crate::contracts::{
    AppStateSnapshot, ConsoleMessageRole, ConsoleMessageView, ConsoleRenderContext,
    ContextWarningLevel, RuntimeState,
};

/// Stateless console renderer.
pub struct Tui;

impl Default for Tui {
    fn default() -> Self {
        Self::new()
    }
}

impl Tui {
    pub fn new() -> Self {
        Self
    }

    pub fn render_startup(&self, config: &EffectiveConfig, snapshot: &AppStateSnapshot) -> String {
        let mode = if !config.mode.approval_required {
            "local / auto"
        } else if config.mode.trust_all {
            "local / trust"
        } else {
            "local / confirm"
        };

        let mut lines = vec![
            "    ___              _ __".to_string(),
            "   /   |  ____ _   _(_) /_".to_string(),
            "  / /| | / __ \\ | / / / __/".to_string(),
            " / ___ |/ / / / |/ / / /_".to_string(),
            "/_/  |_/_/ /_/|___/_/\\__/".to_string(),
            String::new(),
            "  local coding agent for serious terminal work".to_string(),
            String::new(),
            format!("  Model   : {}", config.runtime.model),
            format!("  Context : {}k", config.runtime.context_window / 1_000),
            format!("  Mode    : {mode}"),
            format!("  Project : {}", config.paths.cwd.display()),
        ];

        if config.project_instructions().is_some() {
            lines.push("  ANVIL.md: loaded".to_string());
        }

        lines.extend([
            String::new(),
            "  --------------------------------------------------------------".to_string(),
            format!("  {}", snapshot.status.line),
            "  Ask for a task, or use /help, /model, /plan, /status".to_string(),
            "  --------------------------------------------------------------".to_string(),
            String::new(),
            "  [U] you >".to_string(),
        ]);

        lines.join("\n")
    }

    pub fn render_console(&self, view: &ConsoleRenderContext) -> String {
        let mut lines = Vec::new();
        let snapshot = &view.snapshot;

        if let Some(history_summary) = &view.history_summary {
            lines.push(format!("[A] anvil > {history_summary}"));
        }

        for message in &view.messages {
            lines.push(render_message(message));
        }

        lines.push(format!("[A] anvil > {}", snapshot.status.line));

        if let Some(plan) = &snapshot.plan {
            lines.push("[A] anvil > plan".to_string());
            for (index, item) in plan.items.iter().enumerate() {
                let marker = if plan.active_index == Some(index) {
                    "*"
                } else {
                    "-"
                };
                lines.push(format!("  {marker} {}. {}", index + 1, item));
            }

            if let Some(index) = plan.active_index
                && let Some(item) = plan.items.get(index)
            {
                lines.push(format!(
                    "[A] anvil > working on {}/{}: {}",
                    index + 1,
                    plan.items.len(),
                    item
                ));
            }
        }

        if !snapshot.reasoning_summary.is_empty() {
            lines.push("[A] anvil > thinking".to_string());
            for item in &snapshot.reasoning_summary {
                lines.push(format!("  - {item}"));
            }
        }

        if let Some(approval) = &snapshot.approval {
            lines.push("[A] anvil > approval".to_string());
            lines.push(format!("  tool : {}", approval.tool_name));
            lines.push(format!("  action : {}", approval.summary));
            lines.push(format!("  risk : {}", approval.risk));
            lines.push(format!("  call : {}", approval.tool_call_id));
            if let Some(diff) = &approval.diff_preview {
                lines.push("  diff :".to_string());
                for diff_line in colorize_diff(diff).lines() {
                    lines.push(format!("    {diff_line}"));
                }
            }
        }

        if let Some(interrupt) = &snapshot.interrupt {
            lines.push("[A] anvil > interrupted".to_string());
            lines.push(format!("  what : {}", interrupt.interrupted_what));
            lines.push(format!("  saved : {}", interrupt.saved_status));
            if !interrupt.next_actions.is_empty() {
                lines.push("  next :".to_string());
                for item in &interrupt.next_actions {
                    lines.push(format!("    - {item}"));
                }
            }
        }

        if !snapshot.tool_logs.is_empty() {
            let (completed, failed, interrupted) = summarize_tool_logs(&snapshot.tool_logs);
            lines.push("[T] tool  > progress".to_string());
            lines.push(format!(
                "  completed:{completed} failed:{failed} interrupted:{interrupted}"
            ));
            for log in &snapshot.tool_logs {
                lines.push(format!(
                    "[T] tool  > {:<6} {} {}",
                    log.tool_name, log.action, log.target
                ));
            }
        }

        if let Some(summary) = &snapshot.completion_summary {
            lines.push("[A] anvil > result".to_string());
            lines.push(format!("  {summary}"));
        }

        if let Some(error_summary) = &snapshot.error_summary {
            lines.push("[A] anvil > error".to_string());
            lines.push(format!("  {error_summary}"));
            for action in &snapshot.recommended_actions {
                lines.push(format!("  next : {action}"));
            }
        }

        if let Some(warning_level) = &snapshot.context_warning {
            let percent = snapshot
                .context_usage
                .as_ref()
                .map(|u| u.usage_percent())
                .unwrap_or(0);
            match warning_level {
                ContextWarningLevel::Warning => {
                    lines.push(format!(
                        "[!] Warning: Context usage at {percent}%. Consider running /compact to free space."
                    ));
                }
                ContextWarningLevel::Critical => {
                    lines.push(format!(
                        "[!] CRITICAL: Context usage at {percent}%! Run /compact immediately to avoid degraded responses."
                    ));
                }
            }
        }

        lines.push(status_divider());
        lines.push(self.render_footer(snapshot, &view.model_name));
        lines.push(render_hint_line(snapshot));
        lines.push(status_divider());

        if matches!(snapshot.state, RuntimeState::Done | RuntimeState::Ready) {
            lines.push("[U] you >".to_string());
        } else if matches!(
            snapshot.state,
            RuntimeState::Thinking | RuntimeState::Working | RuntimeState::AwaitingApproval
        ) {
            lines.push("[U] you > /status /help /plan".to_string());
        }

        lines.join("\n")
    }

    fn render_footer(&self, snapshot: &AppStateSnapshot, model_name: &str) -> String {
        let state = match snapshot.state {
            RuntimeState::Ready => "Ready",
            RuntimeState::Thinking => "Thinking",
            RuntimeState::Working => "Working",
            RuntimeState::AwaitingApproval => "AwaitingApproval",
            RuntimeState::Interrupted => "Interrupted",
            RuntimeState::Done => "Done",
            RuntimeState::Error => "Error",
        };

        let elapsed = snapshot
            .elapsed_ms
            .map(|value| format!("{}s", value / 1_000))
            .unwrap_or_else(|| "-".to_string());

        let ctx = snapshot
            .context_usage
            .as_ref()
            .map(|usage| format!("ctx:{}%", usage.usage_percent()))
            .unwrap_or_else(|| "ctx:-".to_string());

        let active = snapshot
            .plan
            .as_ref()
            .and_then(|plan| {
                plan.active_index
                    .map(|index| format!("active:{}/{}", index + 1, plan.items.len()))
            })
            .unwrap_or_else(|| "active:-".to_string());

        let perf = snapshot
            .inference_performance
            .as_ref()
            .and_then(|p| p.formatted_tokens_per_sec())
            .map(|s| format!("perf:{s}"))
            .unwrap_or_else(|| "perf:-".to_string());

        let event = snapshot
            .last_event
            .map(|event| format!("event:{event:?}"))
            .unwrap_or_else(|| "event:-".to_string());

        format!("{state}. {elapsed}   model:{model_name}   {ctx}   {perf}   {active}   {event}")
    }
}

fn render_message(message: &ConsoleMessageView) -> String {
    match message.role {
        ConsoleMessageRole::User => format!("[U] you > {}", message.content),
        ConsoleMessageRole::Assistant => format!("[A] anvil > {}", message.content),
        ConsoleMessageRole::Tool => format!("[T] tool  > {}", message.content),
        ConsoleMessageRole::System => format!("[A] anvil > {}", message.content),
    }
}

fn render_hint_line(snapshot: &crate::contracts::AppStateSnapshot) -> String {
    match snapshot.state {
        RuntimeState::Ready => "Enter to send  /help  /status  /plan".to_string(),
        RuntimeState::Thinking => "ESC stop  /help  /status  /plan  typeahead enabled".to_string(),
        RuntimeState::Working => "ESC stop  /help  /status  /plan  tools active".to_string(),
        RuntimeState::AwaitingApproval => "approve:y  deny:n  /help  /status".to_string(),
        RuntimeState::Interrupted => "/status  /resume  /reset".to_string(),
        RuntimeState::Done => "/diff  /save  /continue  /compact".to_string(),
        RuntimeState::Error => "/status  /retry  /reset".to_string(),
    }
}

fn status_divider() -> String {
    "--------------------------------------------------------------".to_string()
}

/// Apply ANSI colour codes to a plain-text diff string.
///
/// - Lines starting with `+` (but not `+++`) are coloured green.
/// - Lines starting with `-` (but not `---`) are coloured red.
/// - All other lines are left unmodified.
pub fn colorize_diff(diff_text: &str) -> String {
    const GREEN: &str = "\x1b[32m";
    const RED: &str = "\x1b[31m";
    const RESET: &str = "\x1b[0m";

    diff_text
        .lines()
        .map(|line| {
            if line.starts_with('+') && !line.starts_with("+++") {
                format!("{GREEN}{line}{RESET}")
            } else if line.starts_with('-') && !line.starts_with("---") {
                format!("{RED}{line}{RESET}")
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn summarize_tool_logs(logs: &[crate::contracts::ToolLogView]) -> (usize, usize, usize) {
    let mut completed = 0;
    let mut failed = 0;
    let mut interrupted = 0;

    for log in logs {
        match log.action.as_str() {
            "failed" => failed += 1,
            "interrupted" => interrupted += 1,
            _ => completed += 1,
        }
    }

    (completed, failed, interrupted)
}
