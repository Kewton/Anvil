//! Small standalone helpers extracted from `turn.rs` (parent #680).
//!
//! - `rfc3339_now_utc`: wall-clock timestamp for verifier invocation records
//! - `masked_path_hash_bounded_list`: debug-only panic / log path hash projection
//! - `latest_tool_result_since_last_user`: tool-result scan within current turn
//! - `raw_mode_safe_text`: LF -> CRLF rewriter for raw-mode TTY output
//! - `user_interrupt_result`: canonical interrupt tool-result text
//! - `anti_pattern_failed_action_summary`: feedback frame -> short action summary
//!
//! `pub(super)` limited / no facade re-export (DR3-001).

use crate::session::feedback::FeedbackFrame;
use crate::session::store::ConversationMessage;

/// Issue #659 / #664 sidecar: produce an RFC 3339 UTC timestamp from system
/// wall-clock. Delegates to `case_photon_bridge::format_rfc3339_utc` (which
/// already implements the no-chrono civil-from-days algorithm). Pure helper
/// for the `last_verifier_invocation.recorded_at` field.
pub(super) fn rfc3339_now_utc() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    crate::session::case_photon_bridge::format_rfc3339_utc(secs)
}

/// CB-004: bounded, masked-hash projection of an iterator of paths for use
/// in panic / log messages. Each path is masked via
/// `session::feedback::mask_secrets` and then hashed with the same stable
/// `DefaultHasher` shape `artifact_ledger::stable_path_hash` uses, so the
/// hash space is identical between debug-build panics and release-build
/// observability events. Output is hard-capped at 16 entries so a flood of
/// divergent paths cannot blow up the panic message size.
#[cfg(debug_assertions)]
pub(super) fn masked_path_hash_bounded_list<'a, I>(paths: I) -> Vec<String>
where
    I: IntoIterator<Item = &'a str>,
{
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    const MAX_PANIC_HASH_ENTRIES: usize = 16;
    let mut out: Vec<String> = Vec::new();
    for p in paths.into_iter().take(MAX_PANIC_HASH_ENTRIES) {
        let masked = crate::session::feedback::mask_secrets(p);
        let mut hasher = DefaultHasher::new();
        masked.hash(&mut hasher);
        out.push(format!("{:016x}", hasher.finish()));
    }
    out
}

pub(super) fn latest_tool_result_since_last_user<'a>(
    messages: &'a [ConversationMessage],
    tool_name: &str,
) -> Option<&'a str> {
    for message in messages.iter().rev() {
        if message.role == "user" {
            break;
        }
        if message.role == "tool" && message.name.as_deref() == Some(tool_name) {
            return Some(message.content.as_str());
        }
    }
    None
}

pub(super) fn raw_mode_safe_text(text: &str) -> String {
    text.replace('\n', "\r\n")
}

pub(super) fn user_interrupt_result() -> String {
    "exit_code=-1\ninterrupted=true\ninterrupt requested by user".to_string()
}

pub(super) fn anti_pattern_failed_action_summary(frame: &FeedbackFrame) -> String {
    frame
        .primary_error
        .clone()
        .or_else(|| frame.command().map(|s| s.to_string()))
        .unwrap_or_else(|| format!("{:?}", frame.kind))
}
