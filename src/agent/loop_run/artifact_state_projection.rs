//! TaskContract artifact-state projection extracted from `turn.rs`
//! (parent #680).
//!
//! Hosts the Phase-3 (Issue #659 Task 3.2) artifact-state projection:
//! - `task_contract_artifact_states` (production chokepoint, ledger
//!   authority + legacy shadow + divergence emit)
//! - `task_contract_artifact_states_legacy` (legacy derivation;
//!   side-effect seeder for Existing / Scaffold baseline ledger events)
//! - `task_contract_artifact_states_from_ledger` (ledger-projection
//!   derivation: Scaffold / Existing / RepoEdit-Owned → states + per-turn
//!   evidence rows → `Changed`)
//! - `emit_artifact_state_projection_divergence` (masked observability
//!   emit; raw paths never leak, only bounded path-hash lists)
//! - three `#[cfg(test)]` test seams used by the in-crate Phase 3 tests
//!
//! Originally `impl Agent` methods; converted to free functions taking
//! `&mut Agent` / `&Agent`, matching the
//! `actor_loop_flow` / `reply_retry` / earlier vertical-slice precedent.
//! `pub(super)` limited / no facade re-export (DR3-001).

use super::Agent;
use super::workspace_candidates::existing_workspace_candidate_for_role_in_scope;
use crate::logging::log_llm_event;

/// Issue #659 (Task 3.2) test seam — re-exports
/// `task_contract_artifact_states` to the sibling
/// `artifact_ledger_phase3_tests` module **without** changing the
/// caller-facing signature of the production helper.
#[cfg(test)]
pub(super) fn task_contract_artifact_states_for_test(
    agent: &mut Agent,
    contract: &super::task_contract::TaskContract,
) -> Vec<super::task_contract::ArtifactState> {
    task_contract_artifact_states(agent, contract)
}

/// Issue #659 (Task 3.2) test seam — re-exports the legacy derivation so
/// the equivalence test can read both projections without going through
/// the public helper (which would emit a divergence event if the two
/// derivations disagreed).
#[cfg(test)]
pub(super) fn task_contract_artifact_states_legacy_for_test(
    agent: &mut Agent,
    contract: &super::task_contract::TaskContract,
) -> Vec<super::task_contract::ArtifactState> {
    task_contract_artifact_states_legacy(agent, contract)
}

/// Issue #659 (Task 3.2) test seam — re-exports the ledger projection
/// helper. Mirrors `task_contract_artifact_states_legacy_for_test`.
#[cfg(test)]
pub(super) fn task_contract_artifact_states_from_ledger_for_test(
    agent: &Agent,
    contract: &super::task_contract::TaskContract,
) -> Vec<super::task_contract::ArtifactState> {
    task_contract_artifact_states_from_ledger(agent, contract)
}

pub(super) fn task_contract_artifact_states(
    agent: &mut Agent,
    contract: &super::task_contract::TaskContract,
) -> Vec<super::task_contract::ArtifactState> {
    // v0.4.8: make the ledger projection the production authority for
    // artifact state. The legacy derivation is still computed first because
    // it seeds Existing / Scaffold baseline events into the ledger, and it
    // remains useful as a shadow divergence signal. It must not remain the
    // returned value, otherwise current-task nested test edits can be
    // admitted by the ledger but still dropped by legacy verifier binding.
    let legacy_states = task_contract_artifact_states_legacy(agent, contract);
    let ledger_states = task_contract_artifact_states_from_ledger(agent, contract);
    if legacy_states != ledger_states {
        emit_artifact_state_projection_divergence(agent, &legacy_states, &ledger_states);
    }
    ledger_states
}

/// Issue #659 (Task 3.2): the pre-Phase-3 implementation of
/// `task_contract_artifact_states`. Kept intact (other than being renamed)
/// so the caller-facing decision uses the legacy authority during the
/// adapter period. Seed side-effects into the ledger remain here — the
/// ledger reads the seeded events back in
/// `task_contract_artifact_states_from_ledger`.
fn task_contract_artifact_states_legacy(
    agent: &mut Agent,
    contract: &super::task_contract::TaskContract,
) -> Vec<super::task_contract::ArtifactState> {
    let scope = agent.current_workspace_scope();
    let mut states = Vec::new();
    for role in &contract.required_artifacts {
        if let Some(path) =
            super::scaffold_pipeline::scaffold_candidate_for_missing_role(agent, *role)
        {
            // Issue #659 (Task 2.4): seed the Scaffold-origin event
            // alongside the existing `ArtifactState::scaffold` push so
            // the ledger sees the same scaffold baseline. `post_scaffold_delta`
            // is the production no-op gate, kept consistent with
            // `observe_evidence_from_repo_edit`.
            let post_scaffold_delta =
                super::scaffold_pipeline::repo_edit_has_post_scaffold_delta(agent, &path);
            super::artifact_ledger_state::seed_artifact_ledger_scaffold(
                agent,
                &path,
                *role,
                post_scaffold_delta,
                &scope,
            );
            states.push(super::task_contract::ArtifactState::scaffold(*role, path));
        }
        // Issue #646: an existing workspace artifact only enters as
        // `ExistsButUnverified` when ownership classification returns
        // `Owned`. Pre-existing nested-subtree artifacts the active
        // task did not produce stay out of the artifact-state vector
        // and therefore cannot satisfy `artifact_ready_for_verification`.
        if let Some(path) =
            existing_workspace_candidate_for_role_in_scope(&agent.work_root, *role, &scope)
        {
            let scaffold_changed =
                super::scaffold_pipeline::repo_edit_has_post_scaffold_delta(agent, &path);
            let edited_this_session = agent.turn_edited_relative_paths.contains(&path);
            let ownership = super::artifact_ownership::classify_ownership(
                super::artifact_ownership::OwnershipInputs {
                    work_root: &agent.work_root,
                    relative_path: &path,
                    scope: &scope,
                    edited_this_session,
                    scaffold_changed,
                    verifier_passed_in_scope: false,
                    // Issue #661 (Task 3.1): Existing-origin classification
                    // path — preserves legacy semantics. The verifier-path
                    // SSOT switch lands in iteration-3.
                    nested_test_admission: super::artifact_ownership::NestedTestAdmission::default(
                    ),
                },
            );
            // Issue #659 (Task 2.3): seed the Existing-origin event
            // regardless of the `Owned` gate above. The ledger's own
            // admission re-runs `classify_ownership`, so role-mismatch
            // / OutOfScope downgrades happen at admission time;
            // baseline seed is idempotent across repeated evaluations.
            super::artifact_ledger_state::seed_artifact_ledger_existing(
                agent, &path, *role, &scope,
            );
            if matches!(
                ownership,
                super::artifact_ownership::ArtifactOwnership::Owned
            ) {
                states.push(super::task_contract::ArtifactState::exists(*role, path));
            }
        }
    }
    for evidence in agent.task_contract_evidence_set_this_turn.iter() {
        if let super::completion_evidence::CompletionEvidence::RepoEdit { category, .. } = evidence
            && let Some(role) = super::task_contract::role_from_repo_edit(*category)
        {
            states.push(super::task_contract::ArtifactState::changed(role));
        }
    }
    states
}

/// Issue #659 (Task 3.2): build `Vec<ArtifactState>` from the
/// `ArtifactLedger` projection. The legacy helper above seeds Scaffold /
/// Existing events as a side-effect of its own iteration; this helper
/// reads those seeded events back, projecting:
/// - `Scaffold`-origin events → `ArtifactState::scaffold(role, path)`
/// - `Existing` or `RepoEdit` events with `Owned` ownership
///   → `ArtifactState::exists(role, path)`
/// - `task_contract_evidence_set_this_turn` rows (RepoEdit category)
///   → `ArtifactState::changed(role)` (path-less, ledger does NOT carry
///   these rows because their `path: None` shape is rejected at
///   admission per DR2-002 of the design policy)
///
/// Order follows `contract.required_artifacts` iteration so the legacy
/// shape is matched byte-for-byte; `(role, path)` dedupe is applied to
/// guard against the `Existing + RepoEdit` overlap that
/// `seed_artifact_ledger_repo_edit` produces when an existing file is
/// edited this turn.
fn task_contract_artifact_states_from_ledger(
    agent: &Agent,
    contract: &super::task_contract::TaskContract,
) -> Vec<super::task_contract::ArtifactState> {
    use super::artifact_ledger::ArtifactOrigin;
    use super::artifact_ownership::ArtifactOwnership;
    use std::collections::BTreeSet;

    let mut states = Vec::new();
    for role in &contract.required_artifacts {
        // Scaffold rows for this role.
        let mut scaffold_seen: BTreeSet<&str> = BTreeSet::new();
        for ev in agent.artifact_ledger.events_for_role(*role) {
            if matches!(ev.origin, ArtifactOrigin::Scaffold)
                && scaffold_seen.insert(ev.path.as_str())
            {
                states.push(super::task_contract::ArtifactState::scaffold(
                    *role,
                    ev.path.clone(),
                ));
            }
        }
        // Exists rows for this role (Existing or RepoEdit with Owned
        // ownership). Dedupe by path so the same workspace-relative
        // entry doesn't appear twice when an Existing baseline + a
        // RepoEdit observation collide.
        let mut exists_seen: BTreeSet<&str> = BTreeSet::new();
        for ev in agent.artifact_ledger.events_for_role(*role) {
            if matches!(ev.origin, ArtifactOrigin::Scaffold) {
                continue;
            }
            if !matches!(ev.ownership, ArtifactOwnership::Owned) {
                continue;
            }
            if exists_seen.insert(ev.path.as_str()) {
                states.push(super::task_contract::ArtifactState::exists(
                    *role,
                    ev.path.clone(),
                ));
            }
        }
    }
    for evidence in agent.task_contract_evidence_set_this_turn.iter() {
        if let super::completion_evidence::CompletionEvidence::RepoEdit { category, .. } = evidence
            && let Some(role) = super::task_contract::role_from_repo_edit(*category)
        {
            states.push(super::task_contract::ArtifactState::changed(role));
        }
    }
    states
}

/// Issue #659 (Task 3.2): masked observability emit when the legacy and
/// ledger-projection derivations of `task_contract_artifact_states`
/// disagree. v0.4.8 makes the ledger projection the production authority,
/// so `authority="ledger"` is emitted for these projection-level rows.
/// No raw paths are emitted; only role / kind counts.
///
/// Issue #659 PR-001: also emit bounded masked path-hash lists (max
/// 16 entries each, deterministic order via BTreeSet) so dataset
/// consumers can join divergence rows back to per-event rows. Rows
/// without a path (`ArtifactState::changed(role)`) are skipped per
/// DR2-002 — only path-bearing states contribute.
fn emit_artifact_state_projection_divergence(
    agent: &Agent,
    legacy: &[super::task_contract::ArtifactState],
    ledger: &[super::task_contract::ArtifactState],
) {
    let legacy_paths: std::collections::BTreeSet<&str> =
        legacy.iter().filter_map(|s| s.path.as_deref()).collect();
    let ledger_paths: std::collections::BTreeSet<&str> =
        ledger.iter().filter_map(|s| s.path.as_deref()).collect();
    let legacy_path_hashes =
        super::artifact_ledger::bounded_masked_path_hashes(legacy_paths.iter().copied());
    let ledger_path_hashes =
        super::artifact_ledger::bounded_masked_path_hashes(ledger_paths.iter().copied());
    log_llm_event(
        "agent.artifact_ledger.divergence_detected",
        serde_json::json!({
            "session_id": agent.session_store.session_id(),
            "turn_index": agent.current_turn_index,
            "authority": "ledger",
            "projection": "task_contract_artifact_states",
            "legacy_count": legacy.len() as u32,
            "ledger_count": ledger.len() as u32,
            "legacy_path_hashes": legacy_path_hashes,
            "ledger_path_hashes": ledger_path_hashes,
        }),
    );
}
