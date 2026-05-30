//! Forced-small-edit recovery target / note builders extracted from
//! `turn.rs` (parent #680).
//!
//! Hosts the focused-edit policy's "the previous tool call truncated;
//! force a small follow-up edit on the file we just read" branch:
//!
//! - `forced_small_edit_recovery_target` — Act mode + last tool call
//!   truncated + no successful non-plan repo edit since → pick the
//!   `latest_turn_preferred_read_edit_target`, falling back to the
//!   last `Read` tool path.
//! - `forced_small_edit_recovery_message` — renders the
//!   `forced_small_edit_recovery_note` prompt for the chosen target.
//!
//! Originally `impl Agent` methods; converted to free functions taking
//! `&Agent`, matching the `actor_loop_flow` / `reply_retry` / earlier
//! vertical-slice precedent. `pub(super)` limited / no facade
//! re-export (DR3-001).

use std::path::PathBuf;

use super::Agent;
use super::read_target_helpers::{last_read_tool_path, latest_turn_preferred_read_edit_target};
use super::tool_display::progress_path_display;
use super::tool_history::{
    has_successful_non_plan_repo_edit_after_latest_truncated_tool_call,
    recent_truncated_tool_call_attempt,
};
use crate::agent::recovery;
use crate::modes::plan_act::ExecutionMode;
use crate::safety::path_guard::resolve_user_path;

pub(super) fn forced_small_edit_recovery_message(agent: &Agent) -> Option<String> {
    let path = forced_small_edit_recovery_target(agent)?;
    let attempt = recent_truncated_tool_call_attempt(&agent.session.messages).max(1);
    Some(recovery::forced_small_edit_recovery_note(
        &progress_path_display(
            &path.display().to_string(),
            &agent.work_root,
            agent.session.mode_state.active_plan_path.as_deref(),
            120,
        ),
        attempt,
    ))
}

pub(super) fn forced_small_edit_recovery_target(agent: &Agent) -> Option<PathBuf> {
    if agent.session.mode_state.mode != ExecutionMode::Act {
        return None;
    }
    if recent_truncated_tool_call_attempt(&agent.session.messages) == 0 {
        return None;
    }
    if has_successful_non_plan_repo_edit_after_latest_truncated_tool_call(
        &agent.session.messages,
        &agent.work_root,
        agent.session.mode_state.active_plan_path.as_deref(),
    ) {
        return None;
    }
    latest_turn_preferred_read_edit_target(&agent.session.messages, &agent.work_root).or_else(
        || {
            let path = last_read_tool_path(&agent.session.messages)?;
            let candidate = resolve_user_path(&agent.work_root, &path).ok()?;
            candidate.is_file().then_some(candidate)
        },
    )
}
