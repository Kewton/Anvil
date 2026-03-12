use crate::config::EffectiveConfig;
use crate::contracts::{
    AppStateSnapshot, ConsoleMessageRole, ConsoleMessageView, ConsoleRenderContext, RuntimeState,
};

pub struct Tui;

impl Tui {
    pub fn new() -> Self {
        Self
    }

    pub fn render_startup(&self, config: &EffectiveConfig, snapshot: &AppStateSnapshot) -> String {
        let mode = if config.mode.approval_required {
            "local / confirm"
        } else {
            "local / auto"
        };

        [
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
            String::new(),
            "  --------------------------------------------------------------".to_string(),
            format!("  {}", snapshot.status.line),
            "  Ask for a task, or use /help, /model, /plan, /status".to_string(),
            "  --------------------------------------------------------------".to_string(),
            String::new(),
            "  [U] you >".to_string(),
        ]
        .join("\n")
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

            if let Some(index) = plan.active_index {
                if let Some(item) = plan.items.get(index) {
                    lines.push(format!(
                        "[A] anvil > working on {}/{}: {}",
                        index + 1,
                        plan.items.len(),
                        item
                    ));
                }
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

        for log in &snapshot.tool_logs {
            lines.push(format!(
                "[T] tool  > {:<6} {} {}",
                log.tool_name, log.action, log.target
            ));
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
            .map(|usage| {
                let percent = if usage.max_tokens == 0 {
                    0
                } else {
                    ((usage.estimated_tokens as f64 / usage.max_tokens as f64) * 100.0).round()
                        as u32
                };
                format!("ctx:{percent}%")
            })
            .unwrap_or_else(|| "ctx:-".to_string());

        let active = snapshot
            .plan
            .as_ref()
            .and_then(|plan| {
                plan.active_index
                    .map(|index| format!("active:{}/{}", index + 1, plan.items.len()))
            })
            .unwrap_or_else(|| "active:-".to_string());

        format!("{state}. {elapsed}   model:{model_name}   {ctx}   {active}")
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
        RuntimeState::Done => "/diff  /save  /continue".to_string(),
        RuntimeState::Error => "/status  /retry  /reset".to_string(),
    }
}

fn status_divider() -> String {
    "--------------------------------------------------------------".to_string()
}
