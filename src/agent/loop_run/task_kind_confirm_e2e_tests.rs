//! Issue #926 (P0.5b) — TaskKind confirm sidecar E2E suite (in-crate
//! `#[cfg(test)] mod`).
//!
//! Pattern lifted from `scaffold_coding_guard_e2e_tests.rs` /
//! `data_capability_e2e_tests.rs` (CB-001 / DR3-001): an in-crate
//! `#[cfg(test)] mod` that drives the PRODUCTION populate / confirm path
//! without Ollama (all test agents have `models.sidecar == None`, so the
//! confirm dispatcher is a guaranteed no-op — D3). The override-application
//! path is exercised through the `task_kind_confirm_apply_for_test` seam, which
//! drives the real `from_request_with_kind` + single `OnceCell::set` + cap
//! consume that `populate_task_contract_authority` performs on a confirmed
//! override. The production binary excludes this module.

use super::task_classification::{populate_task_contract_authority, task_contract_authority};
use super::task_contract::{TaskContract, TaskKind};
use super::verifier::capability_for;
use crate::agent::loop_run::commands::test_agent_with_config;
use crate::config::Config;
use crate::session::store::ConversationMessage;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build a test agent (sidecar `None`) whose active request is `request`.
fn agent_with_request(request: &str) -> (super::Agent, tempfile::TempDir) {
    let (mut agent, temp) = test_agent_with_config(Config::default());
    assert!(
        agent.models.sidecar.is_none(),
        "test agent must have sidecar None (D3 determinism invariant)"
    );
    agent
        .session
        .messages
        .push(ConversationMessage::user(request.to_string()));
    (agent, temp)
}

/// An ambiguous, no-keyword-match request: `infer_task_kind` defaults it to
/// `Coding` with `matched == false` (confidence 0.0, `needs_confirm()` true).
const AMBIGUOUS: &str = "hey";

/// A clear coding request: a keyword branch fires (`matched == true`,
/// confidence 1.0, `needs_confirm()` false).
const CODING: &str = "write rust code to implement a fibonacci function";

fn authority(agent: &super::Agent) -> std::rc::Rc<TaskContract> {
    task_contract_authority(agent).expect("active request must classify")
}

// ---------------------------------------------------------------------------
// Pre-condition pins (fixtures behave as the suite assumes)
// ---------------------------------------------------------------------------

#[test]
fn fixtures_have_expected_confidence() {
    let amb = TaskContract::from_request(AMBIGUOUS).classification();
    assert_eq!(amb.task_kind, TaskKind::Coding);
    assert!(amb.needs_confirm(), "AMBIGUOUS must be matched==false");

    let code = TaskContract::from_request(CODING).classification();
    assert_eq!(code.task_kind, TaskKind::Coding);
    assert!(!code.needs_confirm(), "CODING must be matched==true");
}

// ---------------------------------------------------------------------------
// AC5 — a confirmed override propagates to the capability gates in-turn, via a
// single OnceCell::set, with a coherently-rebuilt contract.
// ---------------------------------------------------------------------------

#[test]
fn override_to_docs_propagates_to_capability_and_scaffold_gates() {
    let (mut agent, _t) = agent_with_request(AMBIGUOUS);
    // Simulate the confirm overriding the no-keyword Coding default to Docs.
    super::task_classification::task_kind_confirm_apply_for_test(&mut agent, TaskKind::Docs);

    let contract = authority(&agent);
    assert_eq!(
        contract.task_kind,
        TaskKind::Docs,
        "override must be observed"
    );
    // Capability spine reads the overridden kind: non-coding ⇒ no process spawn.
    assert!(
        !capability_for(contract.task_kind).allows_process_exec(),
        "Docs override must disable process-exec capability"
    );
    // Scaffold gate (reads the authority): non-coding ⇒ scaffold demoted.
    assert!(
        !super::scaffold_pipeline::scaffold_allowed_for_active_task(&agent),
        "Docs override must demote scaffold materialization"
    );
    // Coherence (D2): the rebuilt contract is a Docs contract, not a bare field
    // swap on a Coding contract — Docs is verifier-free.
    assert!(
        !contract.verification_required,
        "Docs contract must not require executable verification"
    );
}

#[test]
fn override_confidence_is_high_and_idempotent() {
    let (mut agent, _t) = agent_with_request(AMBIGUOUS);
    super::task_classification::task_kind_confirm_apply_for_test(&mut agent, TaskKind::Research);
    let class = authority(&agent).classification();
    assert_eq!(class.task_kind, TaskKind::Research);
    // DR1-002: an override is authoritative (confidence 1.0) so a re-read cannot
    // re-trigger the confirm.
    assert!((class.confidence - 1.0).abs() < 1e-6);
    assert!(
        !class.needs_confirm(),
        "overridden kind must not need re-confirm"
    );
}

// ---------------------------------------------------------------------------
// AC6 — determinism: populate with sidecar None is byte-identical to
// from_request (the confirm is a guaranteed no-op).
// ---------------------------------------------------------------------------

#[test]
fn populate_with_sidecar_none_is_byte_identical_to_from_request() {
    let (mut agent, _t) = agent_with_request(AMBIGUOUS);
    populate_task_contract_authority(&mut agent);
    let contract = authority(&agent);
    assert_eq!(
        *contract,
        TaskContract::from_request(AMBIGUOUS),
        "populate(sidecar None) must equal the deterministic from_request"
    );
    // No dispatch occurred (sidecar None path does not consume the cap).
    assert!(!agent.task_kind_confirm_called_this_turn);
}

// ---------------------------------------------------------------------------
// AC1c — a matched==true (high-confidence) request is immutable: populate keeps
// the first-pass kind and never dispatches.
// ---------------------------------------------------------------------------

#[test]
fn matched_request_keeps_first_pass_and_does_not_dispatch() {
    let (mut agent, _t) = agent_with_request(CODING);
    populate_task_contract_authority(&mut agent);
    let class = authority(&agent).classification();
    assert_eq!(class.task_kind, TaskKind::Coding);
    assert!((class.confidence - 1.0).abs() < 1e-6);
    assert!(!class.needs_confirm());
    // Coding ⇒ process-exec allowed (positive capability control).
    assert!(capability_for(class.task_kind).allows_process_exec());
    // Confirm cap untouched — a matched==true request never reaches dispatch.
    assert!(!agent.task_kind_confirm_called_this_turn);
}

// ---------------------------------------------------------------------------
// DR3-001 — the lazy `task_contract_authority` net never dispatches/overrides:
// it returns the deterministic first pass even for an ambiguous request.
// ---------------------------------------------------------------------------

#[test]
fn lazy_authority_returns_deterministic_first_pass_without_override() {
    let (agent, _t) = agent_with_request(AMBIGUOUS);
    // Read WITHOUT calling the eager populate: the lazy get_or_init backstop
    // must seal the deterministic first pass (no confirm, &Agent cannot
    // dispatch).
    let contract = authority(&agent);
    assert_eq!(*contract, TaskContract::from_request(AMBIGUOUS));
    assert!(contract.classification().needs_confirm());
}
