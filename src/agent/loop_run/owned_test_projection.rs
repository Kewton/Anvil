//! `owned_test_artifacts_for_verifier` projection extracted from
//! `turn.rs` (parent #680).
//!
//! Hosts the Phase-3 (Issue #659 Task 3.3) Owned-test-artifacts
//! projection that the verifier binding consumes:
//!
//! - `owned_test_artifacts_for_verifier` — production chokepoint.
//!   Side-effect-seeds the ledger by calling
//!   `artifact_state_projection::task_contract_artifact_states`,
//!   computes both the legacy + ledger derivations, emits a masked
//!   `agent.artifact_ledger.divergence_detected` event when they
//!   disagree, and returns the **ledger** projection as the v0.4.8
//!   production authority.
//! - `emit_owned_test_artifacts_projection_divergence` (private) —
//!   masked observability emit (authority=ledger, projection name,
//!   bounded path-hash lists; no raw paths).
//!
//! Originally `impl Agent` methods; converted to free functions taking
//! `&mut Agent` / `&Agent`, matching the `actor_loop_flow` /
//! `reply_retry` / earlier vertical-slice precedent. `pub(super)`
//! limited / no facade re-export (DR3-001).

use super::Agent;
use crate::logging::log_llm_event;

/// Issue #659 (Task 3.3) — verifier-binding Owned-test-artifact slice.
/// Reads through the production-authoritative ledger projection. When
/// legacy and ledger disagree, emit a masked
/// `agent.artifact_ledger.divergence_detected` event and return the
/// ledger slice. The caller signature (`&mut Agent`, `&TaskContract`,
/// `Vec<String>`) is preserved so every existing consumer
/// (`success.rs`, `verifier_orchestration.rs`,
/// `task_contract_recovery.rs`) remains source-compatible.
pub(super) fn owned_test_artifacts_for_verifier(
    agent: &mut Agent,
    contract: &super::task_contract::TaskContract,
) -> Vec<String> {
    let scope = agent.current_workspace_scope();
    // `task_contract_artifact_states` has the side effect of seeding the
    // ledger with Existing / Scaffold baseline events. We MUST call it
    // before reading the ledger projection so the Phase 3 path sees the
    // same baseline the legacy derivation sees.
    let states = super::artifact_state_projection::task_contract_artifact_states(agent, contract);
    let legacy = super::artifact_ownership::owned_test_artifacts(
        &states,
        &agent.work_root,
        &scope,
        &|path| agent.turn_edited_relative_paths.contains(path),
        &|path| super::scaffold_pipeline::repo_edit_has_post_scaffold_delta(agent, path),
    );
    let ledger = agent
        .artifact_ledger
        .owned_test_artifacts(super::task_contract::ArtifactRole::Test);
    if legacy != ledger {
        emit_owned_test_artifacts_projection_divergence(agent, &legacy, &ledger);
    }
    ledger
}

/// Issue #659 (Task 3.3): masked observability emit when the legacy
/// and ledger-projection derivations of `owned_test_artifacts_for_verifier`
/// disagree. v0.4.8 makes the ledger projection the production
/// authority, so `authority="ledger"` is emitted for these
/// projection-level rows. No raw paths are emitted; only role / count
/// metadata.
///
/// Issue #659 PR-001: also emit bounded masked path-hash lists (max
/// 16 entries each, deterministic order via BTreeSet) so dataset
/// consumers can join divergence rows back to per-event rows.
fn emit_owned_test_artifacts_projection_divergence(
    agent: &Agent,
    legacy: &[String],
    ledger: &[String],
) {
    let legacy_set: std::collections::BTreeSet<&str> = legacy.iter().map(String::as_str).collect();
    let ledger_set: std::collections::BTreeSet<&str> = ledger.iter().map(String::as_str).collect();
    let legacy_path_hashes =
        super::artifact_ledger::bounded_masked_path_hashes(legacy_set.iter().copied());
    let ledger_path_hashes =
        super::artifact_ledger::bounded_masked_path_hashes(ledger_set.iter().copied());
    log_llm_event(
        "agent.artifact_ledger.divergence_detected",
        serde_json::json!({
            "session_id": agent.session_store.session_id(),
            "turn_index": agent.current_turn_index,
            "authority": "ledger",
            "projection": "owned_test_artifacts_for_verifier",
            "legacy_count": legacy.len() as u32,
            "ledger_count": ledger.len() as u32,
            "legacy_path_hashes": legacy_path_hashes,
            "ledger_path_hashes": ledger_path_hashes,
        }),
    );
}
