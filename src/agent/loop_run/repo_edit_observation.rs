//! Post-Edit/Write repo-edit evidence observation extracted from
//! `turn.rs` (parent #680).
//!
//! Hosts the Issue #606 (T-1.7) post-hoc `RepoEdit` evidence
//! observation that the production tool-call pipeline invokes after a
//! successful Edit/Write. Order of gates and side-effects:
//!
//! 1. `workspace_relative_path_for_tool_arg` workspace-relative
//!    normalization.
//! 2. `is_ignored_workspace_display_path` ignored-top-dir gate (emits
//!    `repo_edit_ignored_controller_state` and returns).
//! 3. `classify_repo_edit_path` + `repo_edit_has_post_scaffold_delta`
//!    scaffold-delta gate (emits `repo_edit_scaffold_unchanged` and
//!    returns when the path is a scaffold body that didn't change).
//! 4. Issue #646 (C2 / A4) content-no-op gate: pre-tool hash vs.
//!    current on-disk hash. Identical → emit `repo_edit_no_op` and
//!    return. The pre-tool entry is removed in either branch to keep
//!    the cache turn-local and bounded.
//! 5. Record the path into `turn_edited_relative_paths` (the legacy
//!    adapter-period authority).
//! 6. Issue #646 (A1/B2): record an in-scope edit against the active
//!    `MissingVerifierJob` (if any).
//! 7. Append `CompletionEvidence::RepoEdit` to the per-turn evidence
//!    set; when it satisfies the current artifact-recovery target,
//!    mirror it into `task_contract_evidence_set_this_turn` + capture
//!    a bounded `bounded_post_edit_excerpt` for the role.
//! 8. Emit `agent.completion_evidence.observed` with category + path.
//! 9. Issue #659 Task 2.5 write-through seed into the
//!    `ArtifactLedger` SSOT.
//!
//! Originally an `impl Agent` method; converted to a free function
//! taking `&mut Agent`, matching the `actor_loop_flow` / `reply_retry`
//! / earlier vertical-slice precedent. `pub(super)` limited / no
//! facade re-export (DR3-001).

use super::Agent;
use super::completion_evidence::is_repo_edit_no_op;
use super::file_excerpt::current_file_hash_for_relative_path;
use super::tool_policy::workspace_relative_path_for_tool_arg;
use crate::logging::stable_path_hash;
use crate::util::workspace_paths::is_ignored_workspace_display_path;

/// Issue #606 (T-1.7): post-hoc observation of an Edit/Write success
/// as `RepoEdit` completion evidence. The path is run through
/// `classify_repo_edit_path` which uses the SSOT in
/// `util::file_classify` and applies the DR1-001 ordering rule
/// (`.mdx → Docs` even though `is_implementation_file` would otherwise
/// claim it).
pub(super) fn observe_evidence_from_repo_edit(agent: &mut Agent, path: &str) {
    let Some(relative_path) = workspace_relative_path_for_tool_arg(&agent.work_root, path) else {
        return;
    };
    if is_ignored_workspace_display_path(&relative_path) {
        crate::logging::log_completion_evidence_observed(
            agent.current_turn_index,
            0,
            "repo_edit_ignored_controller_state",
            serde_json::json!({
                "path_hash": stable_path_hash(&relative_path),
            }),
        );
        return;
    }
    let category =
        super::completion_evidence::classify_repo_edit_path(std::path::Path::new(&relative_path));
    if !super::scaffold_pipeline::repo_edit_has_post_scaffold_delta(agent, &relative_path) {
        crate::logging::log_completion_evidence_observed(
            agent.current_turn_index,
            0,
            "repo_edit_scaffold_unchanged",
            serde_json::json!({
                "category": format!("{:?}", category),
                "path": relative_path,
            }),
        );
        return;
    }
    // Issue #646 (C2 / A4): even after the scaffold-delta gate, a
    // Write/Edit can be a content no-op for a NON-scaffold file (e.g.
    // model writes the same body back, or `Edit` whose `old_string`
    // equals `new_string`). Compare the pre-tool hash captured in
    // `execute_tool_call` against the current on-disk hash. Identical
    // hashes mean the file did not actually change — bail out so the
    // path does NOT enter `turn_edited_relative_paths` and does NOT
    // contribute completion evidence. The pre-tool entry is removed in
    // either branch to keep the cache turn-local and bounded.
    let pre_tool_hash = agent.turn_pre_tool_file_hashes.remove(&relative_path);
    let current_hash = current_file_hash_for_relative_path(&agent.work_root, &relative_path);
    if is_repo_edit_no_op(
        pre_tool_hash.as_ref().and_then(Option::as_deref),
        current_hash.as_deref(),
    ) {
        crate::logging::log_completion_evidence_observed(
            agent.current_turn_index,
            0,
            "repo_edit_no_op",
            serde_json::json!({
                "category": format!("{:?}", category),
                "path": relative_path,
            }),
        );
        return;
    }
    // Issue #646 (C2): record the edited path AFTER both the
    // scaffold-delta gate AND the no-op hash check so a
    // content-unchanged Write/Edit (scaffold body re-written, or
    // `Edit` with `old_string == new_string`) never promotes the file
    // to `Owned`.
    agent
        .turn_edited_relative_paths
        .insert(relative_path.clone());
    // Issue #646 (A1/B2): once an in-scope edit has landed, the
    // MissingVerifierJob can begin retrying verifier creation.
    if agent.missing_verifier_job.is_some() {
        let in_scope = agent.current_workspace_scope().contains(&relative_path);
        if in_scope && let Some(job) = agent.missing_verifier_job.as_mut() {
            job.record_in_scope_edit();
        }
    }
    agent
        .evidence_set_this_turn
        .push(super::completion_evidence::CompletionEvidence::RepoEdit { category, count: 1 });
    if super::task_contract::repo_edit_satisfies_artifact_recovery_target(
        category,
        &relative_path,
        agent.current_artifact_recovery_target.as_ref(),
    ) {
        agent
            .task_contract_evidence_set_this_turn
            .push(super::completion_evidence::CompletionEvidence::RepoEdit { category, count: 1 });
        // Issue #636: capture bounded post-edit excerpt for the
        // current role so `plan_artifact_recovery` can assert that the
        // edit actually carries the requested behavior. Silent skip on
        // role-miss / read failure (back-compat with the existing
        // `repo_edit_has_post_scaffold_delta` no-data path).
        if let Some(role) = super::task_contract::role_from_repo_edit(category)
            && let Some(excerpt) =
                super::post_edit_excerpt::bounded_post_edit_excerpt(agent, &relative_path)
        {
            agent.task_contract_excerpts.insert(role, excerpt);
        }
    }
    crate::logging::log_completion_evidence_observed(
        agent.current_turn_index,
        0,
        "repo_edit",
        serde_json::json!({
            "category": format!("{:?}", category),
            "path": relative_path,
        }),
    );
    // Issue #659 Task 2.5: write-through seed into the ArtifactLedger
    // SSOT. `relative_path` has already passed the workspace-relative
    // / scaffold-delta / no-op guards; the legacy
    // `turn_edited_relative_paths` insert above stays as the
    // adapter-period authority. The seed is gated by category-to-role
    // mapping so the `Other` category (which legacy callers do not
    // classify into a role) does not inject an ambiguous event.
    if let Some(role) = super::task_contract::role_from_repo_edit(category) {
        let scope = agent.current_workspace_scope();
        super::artifact_ledger_state::seed_artifact_ledger_repo_edit(
            agent,
            &relative_path,
            role,
            &scope,
        );
    }
}
