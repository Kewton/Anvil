//! Playable-UI quality gate decision helpers extracted from `turn.rs`
//! (parent #680).
//!
//! Hosts the per-turn quality-gate decisions that the actor loop and
//! the scaffold pipeline consult before running the
//! `implementation_quality_issue_with_confirm` sidecar:
//!
//! - `current_request_needs_playable_ui_quality_gate` — Act mode +
//!   `quality_gate_enabled` policy + not in an unsupported UI
//!   framework context + active request mentions playable UI.
//! - `accepted_repo_change_quality_issue` — runs the quality issue
//!   classifier (with second-pass sidecar confirmation per Issue
//!   #580) against the first existing impl target and returns
//!   `(request, relative_path, issue_text)` when a quality issue is
//!   accepted.
//! - `accepted_repo_change_polish_target` — runs the deterministic
//!   `playable_ui_polish` pre-checker against the first existing impl
//!   target and returns `(request, relative_path)` when a polish
//!   action should run. A `Some(...)` quality verdict from the
//!   second-pass adapter suppresses polishing (treats it as a quality
//!   issue instead).
//! - `unsupported_ui_framework_context` (private) — request mentions
//!   unsupported framework OR workspace already contains one.
//!
//! Originally `impl Agent` methods; converted to free functions taking
//! `&mut Agent` / `&Agent`, matching the `actor_loop_flow` /
//! `reply_retry` / earlier vertical-slice precedent. `pub(super)`
//! limited / no facade re-export (DR3-001).

use super::Agent;
use super::deterministic;
use super::quality::{
    first_existing_impl_target, request_allows_fast_polish_fallback,
    request_mentions_unsupported_ui_framework, request_needs_playable_ui_quality_gate,
    workspace_has_unsupported_ui_framework,
};
use crate::modes::plan_act::ExecutionMode;

pub(super) fn current_request_needs_playable_ui_quality_gate(agent: &Agent) -> bool {
    agent.session.mode_state.mode == ExecutionMode::Act
        && agent.session.mode_state.policy().quality_gate_enabled
        && !unsupported_ui_framework_context(agent)
        && super::workspace_access::active_request_text(agent)
            .as_deref()
            .is_some_and(request_needs_playable_ui_quality_gate)
}

pub(super) fn accepted_repo_change_quality_issue(
    agent: &mut Agent,
) -> Option<(String, String, String)> {
    if !agent.session.mode_state.policy().quality_gate_enabled {
        return None;
    }
    if unsupported_ui_framework_context(agent) {
        return None;
    }
    let request = super::workspace_access::active_request_text(agent)?;
    let request = request.trim().to_string();
    if !request_needs_playable_ui_quality_gate(&request) {
        return None;
    }
    let target = first_existing_impl_target(&agent.work_root)?;
    let content = std::fs::read_to_string(&target).ok()?;
    // Issue #580: route through the second-pass adapter so borderline
    // UI verdicts can be confirmed/overridden by the sidecar LLM.
    let issue = super::classify_confirm_flow::implementation_quality_issue_with_confirm(
        agent, &request, &content,
    )?;
    let relative = target
        .strip_prefix(&agent.work_root)
        .unwrap_or(&target)
        .to_string_lossy()
        .replace('\\', "/");
    Some((request, relative, issue))
}

pub(super) fn accepted_repo_change_polish_target(agent: &mut Agent) -> Option<(String, String)> {
    if !agent.session.mode_state.policy().allow_polish_fallback {
        return None;
    }
    if unsupported_ui_framework_context(agent) {
        return None;
    }
    let request = super::workspace_access::active_request_text(agent)?;
    let request = request.trim().to_string();
    if !request_allows_fast_polish_fallback(&request) {
        return None;
    }
    let target = first_existing_impl_target(&agent.work_root)?;
    let content = std::fs::read_to_string(&target).ok()?;
    // Issue #580: a second-pass `interactive=false` verdict surfaces
    // here as `Some(...)` which correctly suppresses the polish action
    // (treating the file as a quality issue rather than polishing
    // static code).
    if super::classify_confirm_flow::implementation_quality_issue_with_confirm(
        agent, &request, &content,
    )
    .is_some()
    {
        return None;
    }
    deterministic::playable_ui_polish(&request, &target, &content)?;
    let relative = target
        .strip_prefix(&agent.work_root)
        .unwrap_or(&target)
        .to_string_lossy()
        .replace('\\', "/");
    Some((request, relative))
}

fn unsupported_ui_framework_context(agent: &Agent) -> bool {
    super::workspace_access::active_request_text(agent)
        .as_deref()
        .is_some_and(request_mentions_unsupported_ui_framework)
        || workspace_has_unsupported_ui_framework(&agent.work_root)
}
