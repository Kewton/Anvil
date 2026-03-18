//! Console rendering helpers extracted from the main app module.
//!
//! These are pure functions that produce display strings from application
//! state.  They have no side effects and do not depend on the [`App`] struct.

use crate::config::EffectiveConfig;
use crate::contracts::{AppStateSnapshot, ToolLogView};
use crate::extensions::{ExtensionRegistry, SlashCommandSpec, builtin_slash_commands};
use crate::tooling::{ToolExecutionPayload, ToolExecutionResult, ToolExecutionStatus};

use crate::agent::AgentEvent;

pub fn build_tool_logs(logs: &[(String, String, String)]) -> Vec<ToolLogView> {
    logs.iter()
        .map(|(tool_name, action, target)| ToolExecutionResult {
            tool_call_id: format!("{tool_name}:{target}"),
            tool_name: tool_name.clone(),
            status: map_tool_status(action),
            summary: format!("{action} {target}"),
            payload: ToolExecutionPayload::None,
            artifacts: vec![target.clone()],
            elapsed_ms: 0,
        })
        .map(|result| result.to_tool_log_view())
        .collect()
}

pub fn render_help_frame() -> String {
    render_help_frame_for(&builtin_slash_commands())
}

pub fn render_help_frame_for(commands: &[SlashCommandSpec]) -> String {
    let mut lines = vec!["Anvil slash commands".to_string(), String::new()];
    for spec in commands {
        lines.push(format!("{:<10} {}", spec.name, spec.description));
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

pub fn render_model_frame(config: &EffectiveConfig) -> String {
    format!(
        "[A] anvil > current model: {}\n  provider: {}\n  context window: {}",
        config.runtime.model, config.runtime.provider, config.runtime.context_window
    )
}

pub fn render_provider_frame(
    config: &EffectiveConfig,
    provider: &crate::provider::ProviderRuntimeContext,
) -> String {
    format!(
        "[A] anvil > provider: {}\n  url: {}\n  model: {}\n  streaming: {}\n  tool-calling: {}",
        config.runtime.provider,
        config.runtime.provider_url,
        config.runtime.model,
        provider.capabilities.streaming,
        provider.capabilities.tool_calling
    )
}

pub fn render_resume_header(config: &EffectiveConfig) -> String {
    let mut lines = vec![
        "  --------------------------------------------------------------".to_string(),
        "  Resuming existing session".to_string(),
        format!("  Model   : {}", config.runtime.model),
        format!("  Context : {}k", config.runtime.context_window / 1_000),
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
