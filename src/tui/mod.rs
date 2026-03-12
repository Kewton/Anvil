use crate::contracts::AppStateSnapshot;

pub struct Tui;

impl Tui {
    pub fn new() -> Self {
        Self
    }

    pub fn render(&self, snapshot: &AppStateSnapshot) -> String {
        let mut lines = vec![format!("[A] anvil > {}", snapshot.status.line)];

        if let Some(plan) = &snapshot.plan {
            lines.push("[A] anvil > plan".to_string());
            for (index, item) in plan.items.iter().enumerate() {
                let marker = if plan.active_index == Some(index) { "*" } else { "-" };
                lines.push(format!("  {marker} {}. {}", index + 1, item));
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

        lines.join("\n")
    }
}
