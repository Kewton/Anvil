//! Session message push helpers extracted from `turn.rs` (parent
//! #680).
//!
//! Hosts the two `pub(super)` mutators that append messages to
//! `Agent.session.messages`:
//!
//! - `push_system_note` — pushes a `ConversationMessage::system`
//!   note after the `prompting::should_skip_system_note` dedup
//!   filter (DR4-002: redactors and other note-stacking filters
//!   live one layer up at the prompt builder level).
//! - `push_user_message` — pushes a `ConversationMessage::user`
//!   message and also sets it as the active task in the working
//!   memory (so downstream classifiers / scope detectors see the
//!   fresh task even before the next turn starts).
//!
//! Originally `impl Agent` methods; converted to free functions taking
//! `&mut Agent`, matching the `actor_loop_flow` / `reply_retry` /
//! earlier vertical-slice precedent. `pub(super)` limited / no facade
//! re-export (DR3-001).

use super::Agent;
use crate::agent::prompting;
use crate::session::store::ConversationMessage;

pub(super) fn push_system_note(agent: &mut Agent, note: String) {
    if prompting::should_skip_system_note(&agent.session.messages, &note) {
        return;
    }
    agent
        .session
        .messages
        .push(ConversationMessage::system(note));
}

pub(super) fn push_user_message(agent: &mut Agent, content: String) {
    agent
        .session
        .working_memory
        .set_active_task(Some(content.clone()));
    agent
        .session
        .messages
        .push(ConversationMessage::user(content));
}
