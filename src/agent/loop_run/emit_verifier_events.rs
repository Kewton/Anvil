//! Verifier-event emit helpers extracted from `turn.rs` (parent #680).
//!
//! Hosts the two `agent.verifier.*` event emitters previously sitting on
//! `impl Agent`. Both apply `mask_payload_inplace` as the final defence
//! line and respect per-turn dedup / single-shot caps via
//! `Agent.last_verifier_invoked_payload_digest` and
//! `Agent.external_import_rejected_emitted_this_turn`.
//!
//! Entry points (pub(super)):
//! - `emit_agent_verifier_invoked_if_new` — pre-spawn
//!   `agent.verifier.invoked` event with per-turn payload-digest dedup.
//! - `emit_agent_verifier_external_import_rejected_if_first` —
//!   `agent.verifier.external_import_rejected` event with single-shot
//!   per-turn cap.
//!
//! Originally `impl Agent` methods; converted to free functions taking
//! `&mut Agent`. `pub(super)` limited / no facade re-export (DR3-001).

use super::Agent;
use super::auto_test::{
    VerifierInvokedSnapshot, build_agent_verifier_external_import_rejected_payload,
    build_agent_verifier_invoked_payload,
};
use crate::logging::{compute_payload_digest, log_llm_event, mask_payload_inplace};

/// Issue #661 iteration-4 Task 5.2 / DR1-005 emit ownership: pre-spawn
/// `agent.verifier.invoked` event emit + per-turn dedup. The payload
/// schema matches design Section 8-1 exactly. See
/// [`build_agent_verifier_invoked_payload`] for the pure-fn builder
/// (testable without an Agent instance) and the field-by-field schema.
///
/// Emit ownership rules (DR1-005 / DR1-004):
/// - Caller MUST have built the snapshot at pre-spawn (before
///   `run_structured` spawns `Command::new`)
/// - `mask_payload_inplace` is the final defence line — applied here
///   BEFORE the digest computation so the dedup key matches the
///   post-mask representation log consumers see
/// - Per-turn dedup: same digest as `last_verifier_invoked_payload_digest`
///   suppresses re-emit; a different digest emits and replaces the
///   field. Reset at `handle_user_message` head clears the digest.
///
/// Returns `true` if the event was emitted, `false` if suppressed by
/// dedup (used by unit tests; production callers ignore the return
/// value).
pub(super) fn emit_agent_verifier_invoked_if_new(
    agent: &mut Agent,
    snapshot: &VerifierInvokedSnapshot,
) -> bool {
    let mut payload = build_agent_verifier_invoked_payload(
        agent.session_store.session_id(),
        agent.current_turn_index,
        agent.session.iter_count_this_turn,
        snapshot,
    );
    // DR1-004 step 1: apply mask_payload_inplace BEFORE digest so the
    // dedup key matches the post-mask representation log consumers see.
    mask_payload_inplace(&mut payload);
    let digest = compute_payload_digest(&payload);
    if agent.last_verifier_invoked_payload_digest == Some(digest) {
        return false;
    }
    agent.last_verifier_invoked_payload_digest = Some(digest);
    // log_llm_event masks again — idempotent for already-masked
    // payloads (final defence line invariant).
    log_llm_event("agent.verifier.invoked", payload);
    true
}

/// Issue #661 iteration-5 Task 7.3: emit
/// `agent.verifier.external_import_rejected` event subject to per-turn
/// cap (`external_import_rejected_emitted_this_turn`). Caller passes the
/// already-hashed module hashes + their static source labels so raw paths
/// never reach the payload (DR4-005).
///
/// Returns `true` if emitted, `false` if suppressed by the per-turn cap.
/// Caller (`run_task_contract_verifier_once`) wires both pre-execution
/// (PYTHONPATH) and post-execution (stdout/stderr) detection through
/// this single SSOT.
pub(super) fn emit_agent_verifier_external_import_rejected_if_first(
    agent: &mut Agent,
    runner: &str,
    reason: &'static str,
    detected_hashes: &[(&str, &'static str)],
    detected_count: usize,
    detected_truncated: bool,
) -> bool {
    if agent.external_import_rejected_emitted_this_turn {
        return false;
    }
    agent.external_import_rejected_emitted_this_turn = true;
    let mut payload = build_agent_verifier_external_import_rejected_payload(
        agent.session_store.session_id(),
        agent.current_turn_index,
        runner,
        reason,
        detected_hashes,
        detected_count,
        detected_truncated,
    );
    mask_payload_inplace(&mut payload);
    log_llm_event("agent.verifier.external_import_rejected", payload);
    true
}
