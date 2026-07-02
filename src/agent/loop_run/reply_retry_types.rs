//! Assistant-reply retry state types extracted from `turn.rs` (parent
//! #680).
//!
//! Hosts the small shared types that drive the assistant-reply retry
//! loop in `reply_retry.rs`:
//!
//! - `AssistantReplyRetryState` — per-attempt accumulator
//!   (`downgraded_native_tools`, `retries_remaining`,
//!   `tool_call_format_retries_remaining`, `extra_transport_retries`,
//!   `transport_retry_count`, `focused_edit_timeout_retry_count`,
//!   `tool_call_format_retry_count`) + `new(chat_retries, message_count)`
//!   constructor that doubles the transport budget on long
//!   conversations.
//! - `AssistantReplyRetryDecision` — `Retry` / `ReturnReply(reply)` /
//!   `Fail(err)` triplet returned by the per-attempt handler so the
//!   driver can drive the loop accordingly.
//!
//! `pub(super)` limited / no facade re-export (DR3-001).

use crate::ollama::client::AssistantReply;

#[derive(Debug, Clone, Copy)]
pub(super) struct AssistantReplyRetryState {
    pub(super) downgraded_native_tools: bool,
    pub(super) retries_remaining: usize,
    pub(super) tool_call_format_retries_remaining: usize,
    pub(super) extra_transport_retries: usize,
    pub(super) transport_retry_count: usize,
    pub(super) focused_edit_timeout_retry_count: usize,
    pub(super) tool_call_format_retry_count: usize,
}

pub(super) enum AssistantReplyRetryDecision {
    Retry,
    ReturnReply(AssistantReply),
    Fail(String),
}

impl AssistantReplyRetryState {
    pub(super) fn new(chat_retries: usize, message_count: usize) -> Self {
        Self {
            downgraded_native_tools: false,
            retries_remaining: chat_retries,
            tool_call_format_retries_remaining: 2,
            extra_transport_retries: if message_count >= 12 { 4 } else { 2 },
            transport_retry_count: 0,
            focused_edit_timeout_retry_count: 0,
            tool_call_format_retry_count: 0,
        }
    }
}
