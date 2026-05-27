//! Tool-call history projections shared by focused edit and verifier repair.
//!
//! This module intentionally does not decide recovery flow. It only answers
//! narrow evidence questions such as "has this target been successfully read
//! since the latest write/edit?" so dispatch ownership can stay in the job
//! state machines.

use std::path::{Path, PathBuf};

use crate::ollama::xml_fallback::ToolCall;
use crate::safety::path_guard::resolve_user_path;
use crate::session::store::ConversationMessage;
use crate::util::file_classify::{is_implementation_file, is_setup_file, is_test_file};

#[derive(Debug, Clone)]
struct ToolExchange {
    result_index: Option<usize>,
    tool_call: ToolCall,
    result: Option<ConversationMessage>,
}

fn tool_exchanges(messages: &[ConversationMessage]) -> Vec<ToolExchange> {
    let mut exchanges = Vec::new();
    for (assistant_index, message) in messages.iter().enumerate() {
        if message.role != "assistant" || message.tool_calls.is_empty() {
            continue;
        }

        let mut result_index = assistant_index + 1;
        for tool_call in &message.tool_calls {
            let result = messages
                .get(result_index)
                .filter(|candidate| candidate.role == "tool")
                .cloned();
            let exchange_result_index = result.as_ref().map(|_| result_index);
            if result.is_some() {
                result_index += 1;
            }
            exchanges.push(ToolExchange {
                result_index: exchange_result_index,
                tool_call: tool_call.clone(),
                result,
            });
        }
    }
    exchanges
}

fn tool_call_path_matches_target(tool_call: &ToolCall, target: &Path, work_root: &Path) -> bool {
    let Some(path) = tool_call
        .arguments
        .get("path")
        .and_then(serde_json::Value::as_str)
    else {
        return false;
    };
    tool_path_matches_target(path, target, work_root)
}

fn exchange_is_successful_read_for_target(
    exchange: &ToolExchange,
    target: &Path,
    work_root: &Path,
) -> bool {
    exchange_is_successful_named_tool_for_target(exchange, &["Read"], target, work_root)
}

fn exchange_is_successful_write_or_edit_for_target(
    exchange: &ToolExchange,
    target: &Path,
    work_root: &Path,
) -> bool {
    exchange_is_successful_named_tool_for_target(exchange, &["Write", "Edit"], target, work_root)
}

fn exchange_is_successful_named_tool_for_target(
    exchange: &ToolExchange,
    tool_names: &[&str],
    target: &Path,
    work_root: &Path,
) -> bool {
    tool_names.contains(&exchange.tool_call.name.as_str())
        && tool_call_path_matches_target(&exchange.tool_call, target, work_root)
        && exchange.result.as_ref().is_some_and(|result| {
            result.role == "tool"
                && result.name.as_deref() == Some(exchange.tool_call.name.as_str())
                && !result.content.trim_start().starts_with("Error:")
        })
}

pub(super) fn focused_edit_target_already_read(
    messages: &[ConversationMessage],
    target: &Path,
    work_root: &Path,
) -> bool {
    latest_read_exchange_for_target(messages, target, work_root).is_some()
}

pub(super) fn latest_successful_read_existing_path(
    messages: &[ConversationMessage],
    work_root: &Path,
    after_message_index: Option<usize>,
) -> Option<PathBuf> {
    let mut latest_existing = None;
    for exchange in tool_exchanges(messages).into_iter().rev() {
        if let Some(after_message_index) = after_message_index
            && exchange
                .result_index
                .is_none_or(|result_index| result_index <= after_message_index)
        {
            continue;
        }
        if exchange.tool_call.name != "Read" {
            continue;
        }
        if !exchange.result.as_ref().is_some_and(|result| {
            result.role == "tool"
                && result.name.as_deref() == Some("Read")
                && !result.content.trim_start().starts_with("Error:")
        }) {
            continue;
        }
        let Some(path) = exchange
            .tool_call
            .arguments
            .get("path")
            .and_then(serde_json::Value::as_str)
        else {
            continue;
        };
        let Ok(candidate) = resolve_user_path(work_root, path) else {
            continue;
        };
        if !candidate.is_file() {
            continue;
        }
        latest_existing.get_or_insert_with(|| candidate.clone());
        if is_preferred_read_edit_target(&candidate) {
            return Some(candidate);
        }
    }
    latest_existing
}

pub(super) fn latest_verifier_repair_note_index(messages: &[ConversationMessage]) -> Option<usize> {
    messages.iter().rposition(|message| {
        message.role == "system"
            && (message.content.contains("task_contract_verify_attempt=")
                || message
                    .content
                    .contains("task_contract_verify_edit_attempt=")
                || message
                    .content
                    .contains("task_contract_verify_discovery_attempt=")
                || message
                    .content
                    .contains("task_contract_verify_read_attempt="))
    })
}

pub(super) fn latest_read_exchange_for_target(
    messages: &[ConversationMessage],
    target: &Path,
    work_root: &Path,
) -> Option<(ConversationMessage, ConversationMessage)> {
    let exchanges = tool_exchanges(messages);
    for exchange in exchanges.iter().rev() {
        if !exchange_is_successful_read_for_target(exchange, target, work_root) {
            continue;
        }
        let result_index = exchange.result_index?;
        if exchanges.iter().any(|later| {
            later
                .result_index
                .is_some_and(|later_index| later_index > result_index)
                && exchange_is_successful_write_or_edit_for_target(later, target, work_root)
        }) {
            continue;
        }
        let assistant =
            ConversationMessage::assistant(String::new(), vec![exchange.tool_call.clone()]);
        let tool_message = exchange.result.clone()?;
        return Some((assistant, tool_message));
    }
    None
}

pub(super) fn tool_path_matches_target(raw_path: &str, target: &Path, work_root: &Path) -> bool {
    let Ok(resolved) = resolve_user_path(work_root, raw_path) else {
        return false;
    };
    let canonical_target = std::fs::canonicalize(target).unwrap_or_else(|_| {
        target
            .strip_prefix(work_root)
            .ok()
            .and_then(|relative| resolve_user_path(work_root, &relative.to_string_lossy()).ok())
            .unwrap_or_else(|| target.to_path_buf())
    });
    let canonical_resolved = std::fs::canonicalize(&resolved).unwrap_or(resolved);
    canonical_resolved == canonical_target
}

pub(super) fn is_preferred_read_edit_target(path: &Path) -> bool {
    is_implementation_file(path) && !is_test_file(path) && !is_setup_file(path)
}
