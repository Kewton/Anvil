//! Issue #917 (P0.5): the per-turn single task-classification authority.
//!
//! `TaskContract::from_request` used to be recomputed at ~18 production sites
//! with no memoization, and `infer_task_kind` silently defaulted to `Coding`
//! on a no-keyword match. This module is the single chokepoint that classifies
//! the current-turn request exactly once and memoizes the full `TaskContract`
//! (the 18 reader sites need the whole contract, not just the `TaskKind` — see
//! design policy §3 / S5-001).
//!
//! Free functions (not `impl Agent`) match the dominant `loop_run` convention
//! (DR2-003). Private module, not re-exported (DR3-001).
//!
//! Phase 2 scope: deterministic first-pass populate + read accessor. The
//! low-confidence TaskKind confirm second-pass (which can rebuild the contract
//! before `OnceCell::set`) is layered on in Phase 4 and is the reason the
//! populate entry point is the only mutation-capable path.

use std::rc::Rc;

use super::Agent;
use super::task_contract::TaskContract;

/// Eager populate at the single known point: called once in `run_turn`
/// immediately after `push_user_message`. Idempotent via `OnceCell`; a no-op
/// when the request is absent (degenerate turn) or already populated.
///
/// Placing the populate here means every downstream reader (including
/// `agent_misc::refresh_artifact_completion_satisfied`, which runs *after*
/// `run_turn` returns) observes a memo that was built from the original
/// user request — `active_request_text` strips any auto-plan wrapper (D9).
pub(super) fn populate_task_contract_authority(agent: &Agent) {
    if agent.task_contract_this_turn.get().is_some() {
        return;
    }
    let Some(request) = super::workspace_access::active_request_text(agent) else {
        return;
    };
    // `set` returns `Err` only if another path won the race within this turn;
    // both produce the same deterministic first-pass contract, so ignore it.
    let _ = agent
        .task_contract_this_turn
        .set(Rc::new(TaskContract::from_request(&request)));
}

/// Per-turn classification authority read by the former `from_request` sites.
/// Returns the memoized contract, lazily populating as a defensive net if the
/// eager `populate_*` has not run yet. Returns `None` only when there is no
/// current-turn request (the caller then keeps its legacy local behavior).
pub(super) fn task_contract_authority(agent: &Agent) -> Option<Rc<TaskContract>> {
    let request = super::workspace_access::active_request_text(agent)?;
    let contract = agent
        .task_contract_this_turn
        .get_or_init(|| Rc::new(TaskContract::from_request(&request)));
    #[cfg(debug_assertions)]
    {
        // Divergence assert (S1-008 / S3-001): the memo must equal a fresh
        // recompute of the same `active_request_text`. The branch tolerates a
        // *legal* confirm override (Phase 4): when the memoized kind differs
        // from the deterministic first pass, the first pass must have been
        // low-confidence (i.e. eligible for confirm).
        let first_pass = TaskContract::from_request(&request);
        if contract.task_kind == first_pass.task_kind {
            debug_assert_eq!(
                **contract, first_pass,
                "task classification divergence: memo != recompute(active_request_text)"
            );
        } else {
            debug_assert!(
                first_pass.classification().needs_confirm(),
                "task classification override without a low-confidence first pass"
            );
        }
    }
    Some(contract.clone())
}
