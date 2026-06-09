//! Active-job + behavior-contract event emit helpers extracted from
//! `turn.rs` (parent #680).
//!
//! Hosts three production entry points that bridge per-turn state to
//! observability:
//!
//! - `emit_active_job_selected_if_changed` — Issue #660 (Phase C / DD-4
//!   / DR1-007) per-turn diff-based emit of
//!   `agent.active_job.selected`.
//! - `emit_behavior_contract_projected_if_changed` — Issue #665 (Phase
//!   6 / S5-006 / S7-002) per-turn diff-based emit of
//!   `agent.behavior_contract.projected`.
//! - `current_active_job_selection` — Issue #660 (Phase C) pure
//!   `ActiveJobSelection` computation over `&Agent`, used by the emit
//!   site above and by tests.
//!
//! Originally `impl Agent` methods; converted to free functions taking
//! `&mut Agent` / `&Agent`, matching `actor_loop_flow` / `reply_retry`
//! / earlier vertical-slice patterns. `pub(super)` limited / no facade
//! re-export (DR3-001).

use super::Agent;
use super::active_job_arbiter::{
    ActiveJobSelection, build_active_job_selected_payload, select_active_job,
};
use super::required_behavior::{
    BehaviorContractProjection, BehaviorProjectionEventKey, behavior_contract_projected_payload,
};
use crate::logging::log_llm_event;
use crate::modes::plan_act::ExecutionMode;

pub(super) fn emit_active_job_selected_if_changed(agent: &mut Agent, iteration_seq: u32) -> bool {
    let selection = current_active_job_selection(agent);
    if agent.turn_state.last_active_job_selection.as_ref() == Some(&selection) {
        return false;
    }
    let payload = build_active_job_selected_payload(
        &selection,
        iteration_seq,
        agent.repair_job_artifact_attempts as u32,
        agent
            .artifact_completion_job
            .as_ref()
            .map(|job| job.attempts().len() as u32)
            .unwrap_or(0),
    );
    log_llm_event("agent.active_job.selected", payload);
    agent.turn_state.last_active_job_selection = Some(selection);
    true
}

/// Issue #665 (Phase 6 / S5-006 / S7-002): emit the
/// `agent.behavior_contract.projected` event with per-turn diff-based
/// dedup. Only emits when:
/// - `projection` is `Some(...)` (i.e. consumed by a prompt site), AND
/// - the payload-shaped key differs from
///   `agent.turn_state.last_behavior_contract_projection_event`.
///
/// **Per-turn rule** (DR1-007): `last_behavior_contract_projection_event`
/// is reset to `None` at the head of every `handle_user_message`, so the
/// first consumed projection in a new turn always emits.
///
/// **Security**: payload contains only `schema_version`, `session_id`,
/// `turn_index`, `consumer`, `confidence`, `fields_used`. Raw `label` /
/// `excerpt` are NEVER included (S5-006). `log_llm_event` →
/// `mask_payload_inplace` is the final defense.
///
/// Returns `true` when the event was emitted, `false` when dedup skipped
/// it or no projection was supplied.
pub(super) fn emit_behavior_contract_projected_if_changed(
    agent: &mut Agent,
    projection: Option<&BehaviorContractProjection>,
    consumer: &'static str,
) -> bool {
    let Some(proj) = projection else {
        return false;
    };
    let key = BehaviorProjectionEventKey::from_projection(proj, consumer);
    if agent
        .turn_state
        .last_behavior_contract_projection_event
        .as_ref()
        == Some(&key)
    {
        return false;
    }
    let session_id = agent.session_store.session_id().to_string();
    let turn_index = agent.current_turn_index as u64;
    let payload =
        behavior_contract_projected_payload(&key, proj.confidence, &session_id, turn_index);
    log_llm_event("agent.behavior_contract.projected", payload);
    agent.turn_state.last_behavior_contract_projection_event = Some(key);
    true
}

/// Issue #660 (Phase C): compute the current `ActiveJobSelection`
/// using the same `build_arbiter_candidates` + `select_active_job`
/// pipeline as `effective_tool_policy()`. Pure on `&Agent` — no log
/// emit, no state mutation.
///
/// Issue #660 (Codex CB-001 / §4): mirrors the `effective_tool_policy`
/// pre-arbitration gate for `ExecutionMode::Plan`. The PAM gate at
/// `src/tools/registry.rs::resolve_plan_mode_write_target` /
/// `enforce_plan_stage_scope` is the authority for plan-file Write/Edit
/// arbitration; the arbiter does not see any candidate while Plan mode
/// is active, so observers see a `None` selection that accurately
/// reflects the design.
pub(super) fn current_active_job_selection(agent: &Agent) -> ActiveJobSelection {
    if agent.session.mode_state.mode == ExecutionMode::Plan {
        return ActiveJobSelection {
            selected: None,
            rejected: Vec::new(),
        };
    }
    let candidates = super::effective_tool_policy_flow::build_arbiter_candidates(agent);
    select_active_job(&candidates)
}
