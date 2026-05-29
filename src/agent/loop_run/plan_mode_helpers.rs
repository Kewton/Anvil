//! Plan-mode helpers extracted from `turn.rs` (parent #680).
//!
//! Hosts the small pure helpers that gate plan-mode behaviour:
//! `should_materialize_plan_after_timeout`,
//! `should_materialize_plan_after_tool_call_format_error`,
//! `should_fallback_plan_model_after_timeout`, `assistant_model_for_mode`,
//! `plan_file_alias`, `prune_plan_mode_messages`,
//! `is_plan_mode_only_system_note`.
//!
//! `pub(super)` limited / no facade re-export (DR3-001).

use std::path::Path;

use crate::modes::plan_act::ExecutionMode;
use crate::session::store::ConversationMessage;

use super::lifecycle;

pub(super) fn should_materialize_plan_after_timeout(
    mode: ExecutionMode,
    plan_model_override: Option<&str>,
    err: &str,
) -> bool {
    let _ = plan_model_override;
    mode == ExecutionMode::Plan && err.to_ascii_lowercase().contains("timed out")
}

pub(super) fn should_materialize_plan_after_tool_call_format_error(
    mode: ExecutionMode,
    err: &str,
) -> bool {
    mode == ExecutionMode::Plan && lifecycle::is_tool_call_format_error(err)
}

pub(super) fn assistant_model_for_mode(
    mode: ExecutionMode,
    main_model: &str,
    plan_model_override: Option<&str>,
) -> String {
    if mode == ExecutionMode::Plan
        && let Some(model) = plan_model_override
    {
        return model.to_string();
    }
    main_model.to_string()
}

pub(super) fn should_fallback_plan_model_after_timeout(
    mode: ExecutionMode,
    plan_model_override: Option<&str>,
    err: &str,
    sidecar_model: &str,
) -> bool {
    if mode != ExecutionMode::Plan {
        return false;
    }
    if plan_model_override.is_some() {
        return false;
    }
    if sidecar_model.trim().is_empty() {
        return false;
    }
    err.to_ascii_lowercase().contains("timed out")
}

pub(super) fn plan_file_alias(path: &Path) -> String {
    path.file_name()
        .map(|name| format!("plans/{}", name.to_string_lossy()))
        .unwrap_or_else(|| "plans/plan.md".to_string())
}

pub(super) fn prune_plan_mode_messages(messages: &mut Vec<ConversationMessage>) {
    messages.retain(|message| {
        if message.role != "system" {
            return true;
        }
        !is_plan_mode_only_system_note(&message.content)
    });
}

fn is_plan_mode_only_system_note(note: &str) -> bool {
    let trimmed = note.trim_start();
    trimmed.starts_with("[Plan Mode /")
        || trimmed.starts_with("[Plan File Alias]")
        || trimmed.contains("plan_no_tool_attempt=")
        || trimmed.contains("plan_progress_attempt=")
        || trimmed.starts_with("Main planning model timed out.")
        || trimmed.starts_with("The plan is still incomplete.")
        || trimmed.starts_with("You are still in Plan mode")
}
