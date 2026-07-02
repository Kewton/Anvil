//! Per-turn driver extracted from `turn.rs` (parent #680).
//!
//! Hosts `run_turn` (pub(super)) — the turn-level driver invoked from
//! `handle_user_message::handle_user_message` after the per-turn state
//! resets. Pushes the user message, runs the work-mode classification
//! second-pass + history compaction (Act mode), refreshes the plan
//! stage, classifies the action expectation, fires the pre-turn photon
//! context-pack hook, and finally enters `actor_loop_flow::run_actor_loop`.
//!
//! Originally an `impl Agent` method; converted to a free function
//! taking `&mut Agent`, matching `actor_loop_flow` / `reply_retry` /
//! `handle_user_message` pattern. `pub(super)` limited / no facade
//! re-export (DR3-001).

use super::Agent;
use super::DEFAULT_KEEP_TAIL;
use super::actor_loop_flow::run_actor_loop;
use super::interrupt::InterruptMonitor;
use super::summary::LoopResult;
use crate::agent::recovery;
use crate::logging::log_llm_event;
use crate::modes::plan_act::{ExecutionMode, WorkMode};

#[allow(clippy::result_large_err)]
pub(super) fn run_turn(
    agent: &mut Agent,
    input: &str,
    stream_output: bool,
    monitor: &mut InterruptMonitor,
) -> LoopResult {
    super::message_push::push_user_message(agent, input.to_string());
    let request_view = super::task_contract::RequestInferenceView::from_raw(input);
    let controller_owned_turn = request_view.is_controller_owned_turn();
    // Issue #917 (P0.5): eager-populate the per-turn classification authority at
    // this single known point — right after the request is set, before any
    // reader and regardless of mode. `active_request_text` strips the auto-plan
    // wrapper so the memo classifies the original user request (D7/D9).
    super::task_classification::populate_task_contract_authority(agent);
    if agent.session.mode_state.mode != ExecutionMode::Plan {
        // Issue #576: replace direct `classify_work_mode_json` + event
        // emit with the shared `classify_with_confirmation` wrapper. The
        // wrapper emits the existing `agent.work_mode.classified` event
        // (now with `turn_index`) and drives the LLM second-pass via
        // `maybe_invoke_work_mode_confirm`. Final (LLM-corrected when
        // applicable) work_mode lives in `agent.session.mode_state.work_mode`.
        if controller_owned_turn {
            // Controller-owned packets are execution state, not user intent.
            // Keep them out of the natural-language WorkMode classifier so
            // schema keys such as `required_artifacts` cannot inject Python/UI
            // mode policy into a worker turn.
            agent.session.mode_state.work_mode = WorkMode::Auto;
        } else {
            let classifier_input = request_view.visible_text();
            let _ = super::classify_confirm_flow::classify_with_confirmation(
                agent,
                classifier_input,
                "turn_start",
            );
        }
        agent.maybe_compact_session(DEFAULT_KEEP_TAIL);
    }
    let _ = agent.refresh_plan_stage();

    let mut action_expectation =
        recovery::classify_action_expectation(input, agent.session.mode_state.mode);
    if !super::workspace_access::repo_edit_required_by_mode_or_objective(agent) {
        action_expectation = recovery::ActionExpectation::None;
    }
    let requires_action = action_expectation != recovery::ActionExpectation::None;

    // [Issue #556] pre-turn photon context_pack hook
    if agent.session.mode_state.mode != ExecutionMode::Plan {
        super::photon_feedback_derive::invoke_photon_context_pack(agent);
    } else if agent.photon.is_some() {
        // Issue #594: surface plan-mode skip via /photon-why.
        agent.last_photon_context_pack_status =
            crate::agent::loop_run::PhotonContextPackStatus::PlanMode;
        agent.record_pam_unused_reason("plan_mode");
        // CB-003 (Issue #592): Plan-mode skip path must also clear stale
        // inject tracking so a previous Act-turn's seed ids do not survive
        // into a Plan turn and become "visible" to `/photon-thumbs-*`.
        agent.last_injected_summary_ids.clear();
        agent.last_injected_summary_turn_index = None;
        log_llm_event(
            "agent.photon_context_pack.skipped",
            serde_json::json!({
                "session_id": agent.session_store.session_id(),
                "turn_index": agent.current_turn_index,
                "reason": "plan_mode",
            }),
        );
    }

    run_actor_loop(
        agent,
        action_expectation,
        requires_action,
        stream_output,
        false,
        monitor,
    )
}
