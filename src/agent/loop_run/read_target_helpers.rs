//! Read-tool path lookup helpers extracted from `turn.rs` (parent
//! #680).
//!
//! Hosts two small free functions that mine the conversation message
//! log for previous `Read` tool calls so the recovery / focused-edit
//! pipelines can pick a continuation target:
//!
//! - `last_read_tool_path` — the most recent (across all turns)
//!   `Read` tool-call `path` argument as a raw string.
//! - `latest_turn_preferred_read_edit_target` — the latest **user
//!   turn**'s `Read` calls, resolved against `work_root` and filtered
//!   to existing files; prefers `is_preferred_read_edit_target`
//!   matches, otherwise falls back to the most-recent existing read.
//!
//! Originally `pub(super) fn` helpers in `turn.rs` (NOT `impl Agent`);
//! these are pure projections over message slices and don't need
//! `Agent`. `pub(super)` limited / no facade re-export (DR3-001).

use std::path::{Path, PathBuf};

use super::tool_history::{is_preferred_read_edit_target, latest_user_turn_slice};
use crate::safety::path_guard::resolve_user_path;
use crate::session::store::ConversationMessage;

pub(super) fn last_read_tool_path(messages: &[ConversationMessage]) -> Option<String> {
    messages.iter().rev().find_map(|message| {
        if message.role != "assistant" {
            return None;
        }
        message.tool_calls.iter().rev().find_map(|tool_call| {
            if tool_call.name != "Read" {
                return None;
            }
            tool_call
                .arguments
                .get("path")
                .and_then(serde_json::Value::as_str)
                .map(ToString::to_string)
        })
    })
}

pub(super) fn latest_turn_preferred_read_edit_target(
    messages: &[ConversationMessage],
    work_root: &Path,
) -> Option<PathBuf> {
    let mut latest_existing = None;
    for message in latest_user_turn_slice(messages).iter().rev() {
        if message.role != "assistant" {
            continue;
        }
        for tool_call in message.tool_calls.iter().rev() {
            if tool_call.name != "Read" {
                continue;
            }
            let Some(path) = tool_call
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
    }
    latest_existing
}
