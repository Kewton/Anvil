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
//! Phase 2 scope: deterministic first-pass populate + read accessor. Issue #926
//! (P0.5b) wired the low-confidence TaskKind confirm second-pass — it rebuilds
//! the contract via `from_request_with_kind` before `OnceCell::set`, which is
//! why the eager populate entry point (`&mut Agent`) is the only
//! mutation-capable path; the lazy read accessor (`&Agent`) is a
//! deterministic-first-pass edge backstop and never dispatches the confirm.

use std::rc::Rc;

use super::Agent;
use super::task_contract::TaskContract;
use crate::logging::log_llm_event;

/// Eager populate at the single known point: called once in `run_turn`
/// immediately after `push_user_message`. Idempotent via `OnceCell`; a no-op
/// when the request is absent (degenerate turn) or already populated.
///
/// Placing the populate here means every downstream reader (including
/// `agent_misc::refresh_artifact_completion_satisfied`, which runs *after*
/// `run_turn` returns) observes a memo that was built from the original
/// user request — `active_request_text` strips any auto-plan wrapper (D9).
pub(super) fn populate_task_contract_authority(agent: &mut Agent) {
    // Issue #926 (DR3-001): a pre-set cell at the eager-populate entry means a
    // production reader called `task_contract_authority()` for this turn's
    // request BEFORE `run_turn`'s eager populate — an ordering bug, because the
    // lazy net would have sealed an un-confirmed first-pass contract that the
    // confirm second-pass can no longer override (`OnceCell` is single-set).
    // We no-op in release (idempotent), but assert in debug to surface it. This
    // is NOT a "make early lazy read safe" path; the invariant is that no such
    // early read happens (the lazy `get_or_init` is an edge backstop only).
    debug_assert!(
        agent.task_contract_this_turn.get().is_none()
            || super::workspace_access::active_request_text(agent).is_none(),
        "task_contract_this_turn was sealed before run_turn's eager populate \
         (lazy-net ordering bug — the confirm override can no longer apply)"
    );
    if agent.task_contract_this_turn.get().is_some() {
        return;
    }
    let Some(request) = super::workspace_access::active_request_text(agent) else {
        return;
    };
    // Deterministic first pass.
    let first_pass = TaskContract::from_request(&request);
    // Issue #926 (D1/D4): only a no-keyword-match default (`needs_confirm`, i.e.
    // `matched == false`) is eligible for the confirm second pass; a
    // high-confidence (`matched == true`) classification is immutable and never
    // dispatches. The confirm runs BEFORE `OnceCell::set` so every downstream
    // reader and the divergence assert observe the (possibly overridden)
    // contract. The dispatcher returns `Some(kind)` only on an actual override
    // (different kind); agree / skip / fallback return `None`.
    let forced_kind = if first_pass.classification().needs_confirm() {
        let first_pass_class = first_pass.classification();
        let turn_index = agent.current_turn_index;
        super::classify_confirm_flow::maybe_invoke_task_kind_confirm(
            agent,
            &first_pass_class,
            &request,
            turn_index,
        )
    } else {
        None
    };
    let turn_index = agent.current_turn_index;
    let project_profile = super::classify_confirm_flow::maybe_invoke_project_profile_confirm(
        agent,
        &first_pass,
        &request,
        turn_index,
    );
    // Single `set`: the overridden/profile-confirmed contract is rebuilt
    // coherently via the construction SSOT before the OnceCell is sealed; every
    // downstream reader observes the same objective contract.
    let contract = match (forced_kind, project_profile.as_ref()) {
        (None, None) => Rc::new(first_pass),
        (kind, profile) => Rc::new(TaskContract::from_request_with_kind_and_project_profile(
            &request, kind, profile,
        )),
    };
    emit_semantic_candidate_shadow(agent, &contract);
    let _ = agent.task_contract_this_turn.set(contract);
}

/// Issue #926 test seam (DR1-005): deterministically apply a confirmed kind
/// override to the per-turn authority, exercising the REAL populate-side
/// override path (`from_request_with_kind` + single `OnceCell::set` + per-turn
/// cap consume) without a live sidecar (which is `None` in all tests, so the
/// confirm dispatcher itself never overrides). The cell must be unset (call
/// before any read / `populate_*`). `forced_kind` must differ from the
/// deterministic first-pass kind (production only overrides on disagreement),
/// otherwise the divergence assert's same-kind branch would compare a
/// confidence-1.0 override against the confidence-0.0 recompute.
#[cfg(test)]
pub(crate) fn task_kind_confirm_apply_for_test(
    agent: &mut Agent,
    forced_kind: super::task_contract::TaskKind,
) {
    let request = super::workspace_access::active_request_text(agent)
        .expect("active request required for the confirm override seam");
    // Simulate the dispatch having occurred (cap consumed) and seal the
    // coherently-rebuilt overridden contract with a single `set`.
    agent.task_kind_confirm_called_this_turn = true;
    let contract = Rc::new(TaskContract::from_request_with_kind(
        &request,
        Some(forced_kind),
    ));
    let _ = agent.task_contract_this_turn.set(contract);
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
        if agent.project_profile_confirm_called_this_turn {
            // The ProjectProfile second pass may keep the coarse task kind while
            // refining objective-level details such as deliverable/evidence roles.
            // That is a legal divergence from the deterministic first pass.
        } else if contract.task_kind == first_pass.task_kind {
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

fn emit_semantic_candidate_shadow(agent: &Agent, contract: &TaskContract) {
    let candidate =
        super::task_contract_semantic_candidate::SemanticCandidate::deterministic_shadow_from_contract(
            contract,
        );
    let decision = super::task_contract_admission::admit_semantic_candidate(
        super::task_contract_admission::SemanticCandidateAdmissionInput {
            candidate: &candidate,
            contract,
            allow_equivalent_current_behavior: false,
        },
    );
    log_llm_event(
        "agent.semantic_candidate.shadow",
        serde_json::json!({
            "session_id": agent.session_store.session_id(),
            "turn_index": agent.current_turn_index,
            "origin": "deterministic_shadow",
            "status": decision.status.label(),
            "reasons": decision.reason_labels(),
            "objective_kind": candidate.objective_kind.label(),
            "deliverable_kind": candidate.deliverable_kind.label(),
            "evidence_kind": candidate.evidence_kind.label(),
            "deliverable_candidate_count": candidate.deliverable_candidates.len(),
            "artifact_identity_count": candidate.artifact_identities.len(),
            "schema_expectation_count": candidate.schema_expectations.len(),
            "authoring_style": candidate.authoring_style.style.label(),
            "style_authority": candidate.authoring_style.authority.label(),
            "compatibility_risks": candidate.compatibility_risks,
            "disagreement_count": decision.disagreements.len(),
        }),
    );
}
