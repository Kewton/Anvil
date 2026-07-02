//! Per-turn artifact-ledger state management extracted from `turn.rs`
//! (parent #680).
//!
//! Hosts the per-turn lifecycle of `Agent::artifact_ledger` (Issue #659):
//! - per-turn reset + observability stamp
//! - end-of-turn `agent.artifact_ledger.turn_summary` emit
//! - four seed helpers (`existing` / `scaffold` / `repo_edit` /
//!   `verifier_observation`) that admit events through
//!   `classify_ownership` and (for `repo_edit`) keep the legacy
//!   `turn_edited_relative_paths` set in lockstep
//! - dual-source divergence assertion + release-mode emitter +
//!   private collection / event helpers
//!
//! Originally `impl Agent` methods; converted to free functions taking
//! `&mut Agent` / `&Agent`, matching the
//! `actor_loop_flow` / `reply_retry` / earlier vertical-slice precedent.
//! `pub(super)` limited / no facade re-export (DR3-001).

use super::Agent;
use crate::logging::{log_llm_event, stable_path_hash};
use crate::session::feedback::mask_secrets;
use crate::util::workspace_paths::{
    WorkspacePathAdmission, artifact_admission_decision_display_path,
};

/// Issue #659 PR-002 — per-turn reset + observability stamp. Callers
/// pass `upcoming_turn_index` explicitly so stamp timing is decoupled
/// from `current_turn_index` mutation in `handle_user_message`.
pub(super) fn clear_per_turn_ledger_state_for_turn(agent: &mut Agent, upcoming_turn_index: u32) {
    agent.artifact_ledger.clear();
    let session_id = agent.session_store.session_id().to_string();
    agent
        .artifact_ledger
        .set_log_context(super::artifact_ledger::ArtifactLedgerLogContext::new(
            session_id,
            upcoming_turn_index,
        ));
}

/// Issue #659 PR-002 — back-compat shim for unit tests that already
/// position `agent.current_turn_index` to the value they want stamped
/// before calling the per-turn reset. Production code MUST use
/// `clear_per_turn_ledger_state_for_turn(upcoming_turn_index)` so the
/// upcoming index is explicit at the call site.
#[cfg(test)]
pub(super) fn clear_per_turn_ledger_state(agent: &mut Agent) {
    let upcoming = u32::try_from(agent.current_turn_index).unwrap_or(u32::MAX);
    clear_per_turn_ledger_state_for_turn(agent, upcoming);
}

/// Issue #659 (Task 2.2) — emit the end-of-turn
/// `agent.artifact_ledger.turn_summary` event exactly once.
pub(super) fn record_turn_end_artifact_ledger_summary(agent: &Agent) {
    agent.artifact_ledger.emit_turn_summary();
}

/// Issue #659 (Task 2.3) — admit an Existing-origin event for a
/// workspace-relative path that came from the task-contract candidate
/// iteration / workspace scan SSOT.
pub(super) fn seed_artifact_ledger_existing(
    agent: &mut Agent,
    relative_path: &str,
    role: super::task_contract::ArtifactRole,
    scope: &super::task_workspace_scope::TaskWorkspaceScope,
) {
    if log_policy_rejection(agent, "artifact_ledger_existing", relative_path) {
        return;
    }
    let ctx = super::artifact_ledger::LedgerAdmissionContext::new(&agent.work_root, scope);
    let _ = agent
        .artifact_ledger
        .record_existing_event(&ctx, relative_path.to_string(), role);
}

/// Issue #659 (Task 2.4) — admit a Scaffold-origin event. The caller
/// MUST pass `post_scaffold_delta` from
/// `repo_edit_has_post_scaffold_delta(...)` so the seed stays
/// consistent with the production no-op gate.
pub(super) fn seed_artifact_ledger_scaffold(
    agent: &mut Agent,
    relative_path: &str,
    role: super::task_contract::ArtifactRole,
    post_scaffold_delta: bool,
    scope: &super::task_workspace_scope::TaskWorkspaceScope,
) {
    if log_policy_rejection(agent, "artifact_ledger_scaffold", relative_path) {
        return;
    }
    let ctx = super::artifact_ledger::LedgerAdmissionContext::new(&agent.work_root, scope);
    let _ = agent.artifact_ledger.record_scaffold_event(
        &ctx,
        relative_path.to_string(),
        role,
        post_scaffold_delta,
    );
}

/// Issue #659 (Task 2.5) — write-through adapter for a successful,
/// non-no-op Write/Edit observation. The legacy
/// `turn_edited_relative_paths` set is updated in the same instruction
/// so adapter-period divergence (Task 2.7) is detectable at turn end.
pub(super) fn seed_artifact_ledger_repo_edit(
    agent: &mut Agent,
    relative_path: &str,
    role: super::task_contract::ArtifactRole,
    scope: &super::task_workspace_scope::TaskWorkspaceScope,
) {
    if log_policy_rejection(agent, "artifact_ledger_repo_edit", relative_path) {
        return;
    }
    // Write-through adapter (divergence anchor): keep the legacy set
    // in lockstep with the ledger seed. Caller-facing decisions stay
    // on the legacy authority during the adapter period (Phase 6.1).
    agent
        .turn_edited_relative_paths
        .insert(relative_path.to_string());
    let ctx = super::artifact_ledger::LedgerAdmissionContext::new(&agent.work_root, scope);
    let _ =
        agent
            .artifact_ledger
            .record_repo_edit_event(&ctx, relative_path.to_string(), role, true);
}

/// Issue #659 (Task 2.6) — record verifier observations for each bound
/// test path produced by a structured `VerifierCommand`. Unbound
/// verifier paths (empty `bound_paths`) intentionally produce no
/// observation so the projection-side absence-as-`NotRun` rule stays
/// consistent.
pub(super) fn seed_artifact_ledger_verifier_observation(
    agent: &mut Agent,
    bound_paths: &[String],
    last_outcome: super::artifact_ledger::VerifierOutcome,
    scope: &super::task_workspace_scope::TaskWorkspaceScope,
) {
    if bound_paths.is_empty() {
        return;
    }
    let ctx = super::artifact_ledger::LedgerAdmissionContext::new(&agent.work_root, scope);
    for path in bound_paths {
        if log_policy_rejection(agent, "artifact_ledger_verifier_observation", path) {
            continue;
        }
        let _ = agent.artifact_ledger.record_verifier_observation(
            &ctx,
            path,
            super::artifact_ledger::VerifierObservation {
                argv_path_matched: true,
                last_outcome,
            },
        );
    }
}

fn log_policy_rejection(agent: &Agent, component: &str, relative_path: &str) -> bool {
    match artifact_admission_decision_display_path(relative_path) {
        WorkspacePathAdmission::Accepted => false,
        WorkspacePathAdmission::Rejected { class, reason } => {
            let masked = mask_secrets(relative_path);
            log_llm_event(
                "agent.workspace_policy.rejected_path",
                serde_json::json!({
                    "session_id": agent.session_store.session_id(),
                    "turn_index": agent.current_turn_index,
                    "component": component,
                    "path_hash": stable_path_hash(&masked),
                    "path_len": relative_path.len() as u32,
                    "class": format!("{:?}", class),
                    "reason": reason.as_str(),
                }),
            );
            true
        }
    }
}

/// Issue #659 (Task 2.7) — dual-source divergence assertion. See the
/// extracted method's original doc comment in `turn.rs` history for the
/// adapter-period contract and CB-004 panic-message redaction.
pub(super) fn assert_dual_source_alignment_at_turn_end(agent: &Agent) {
    let (legacy, ledger) = collect_dual_source_repo_edit_sets(agent);
    let ledger_only: std::collections::BTreeSet<&String> = ledger.difference(&legacy).collect();
    #[cfg(debug_assertions)]
    {
        // CB-004: the panic message must not leak raw workspace-relative
        // paths (they may contain LLM-injected secrets). Mirror the
        // release-shape observability schema: counts + masked path
        // hashes only.
        assert!(
            ledger_only.is_empty(),
            "DR3 dual-source divergence: ledger admitted paths absent from legacy set \
             (write-through adapter bug). legacy_count={legacy_count}, \
             ledger_count={ledger_count}, ledger_only_count={only_count}, \
             ledger_only_hashes={hashes:?}",
            legacy_count = legacy.len(),
            ledger_count = ledger.len(),
            only_count = ledger_only.len(),
            hashes = super::small_helpers::masked_path_hash_bounded_list(
                ledger_only.iter().map(|s| s.as_str())
            ),
        );
        if legacy != ledger {
            // Stricter-ledger case: legitimate gating difference. Still
            // emit the observability event so dataset consumers can
            // count how often the ledger rejects what legacy accepts.
            emit_artifact_ledger_divergence_event(agent, &legacy, &ledger);
        }
    }
    #[cfg(not(debug_assertions))]
    {
        let _ = ledger_only;
        if legacy != ledger {
            emit_artifact_ledger_divergence_event(agent, &legacy, &ledger);
        }
    }
}

/// Issue #659 (Task 2.7) — release-shape divergence helper. Always
/// emits the event (no panic) when sources disagree. Exposed so the
/// in-crate test suite can drive the release shape regardless of the
/// `debug_assertions` cfg under which `cargo test` actually runs.
#[allow(dead_code)]
pub(super) fn emit_artifact_ledger_divergence_if_any(agent: &Agent) {
    let (legacy, ledger) = collect_dual_source_repo_edit_sets(agent);
    if legacy != ledger {
        emit_artifact_ledger_divergence_event(agent, &legacy, &ledger);
    }
}

fn collect_dual_source_repo_edit_sets(
    agent: &Agent,
) -> (
    std::collections::BTreeSet<String>,
    std::collections::BTreeSet<String>,
) {
    let legacy: std::collections::BTreeSet<String> =
        agent.turn_edited_relative_paths.iter().cloned().collect();
    // Issue #659 Task 3.1: route through the ledger's SSOT projection
    // method so the divergence helper and the Phase 3 internal-switch
    // helpers share a single source of truth (`repo_edit_projection_set`).
    let ledger = agent.artifact_ledger.repo_edit_projection_set();
    (legacy, ledger)
}

#[allow(dead_code)] // invoked from release-build `assert_dual_source_alignment_at_turn_end` and the in-crate test seam.
fn emit_artifact_ledger_divergence_event(
    agent: &Agent,
    legacy: &std::collections::BTreeSet<String>,
    ledger: &std::collections::BTreeSet<String>,
) {
    let only_legacy: Vec<String> = legacy.difference(ledger).cloned().collect();
    let only_ledger: Vec<String> = ledger.difference(legacy).cloned().collect();
    // Issue #659 PR-001: emit bounded masked path-hash lists per
    // Section 7.1 of the design policy. The hash space matches
    // `event_recorded.path_hash` exactly (mask_secrets → DefaultHasher,
    // 16-char hex) so dataset consumers can join divergence rows back
    // to per-event rows. Hard cap at 16 entries each (raw path is
    // never emitted).
    let legacy_path_hashes =
        super::artifact_ledger::bounded_masked_path_hashes(legacy.iter().map(String::as_str));
    let ledger_path_hashes =
        super::artifact_ledger::bounded_masked_path_hashes(ledger.iter().map(String::as_str));
    log_llm_event(
        "agent.artifact_ledger.divergence_detected",
        serde_json::json!({
            "session_id": agent.session_store.session_id(),
            "turn_index": agent.current_turn_index,
            "authority": "legacy",
            "legacy_count": legacy.len() as u32,
            "ledger_count": ledger.len() as u32,
            "only_legacy_count": only_legacy.len() as u32,
            "only_ledger_count": only_ledger.len() as u32,
            "legacy_path_hashes": legacy_path_hashes,
            "ledger_path_hashes": ledger_path_hashes,
        }),
    );
}
