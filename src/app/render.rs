//! Console rendering helpers extracted from the main app module.
//!
//! These are pure functions that produce display strings from application
//! state.  They have no side effects and do not depend on the [`App`] struct.

use crate::config::EffectiveConfig;
use crate::contracts::{AppStateSnapshot, ToolLogView};
use crate::extensions::skills::SkillScope;
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
    format!(
        "[A] anvil > provider: {}\n  url: {}\n  model: {}\n  streaming: {}\n  tool-calling: {}",
        config.runtime.provider,
        config.runtime.provider_url,
        effective_model,
        provider.capabilities.streaming,
        provider.capabilities.tool_calling
    )
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
