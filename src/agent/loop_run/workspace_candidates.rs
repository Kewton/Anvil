//! Workspace candidate lookup helpers extracted from `turn.rs` (parent
//! #680).
//!
//! Hosts the scoped + legacy variants of `existing_workspace_candidate_for_role*`
//! used by `task_contract_artifact_states` /
//! `task_contract_recovery_target` and the test-only `target_path_in_scope`
//! probe. The walker delegates to
//! `workspace_walk::meaningful_workspace_files` for traversal, sorts by
//! `scaffold_pipeline::scaffold_candidate_priority`, and filters through
//! `task_workspace_scope::TaskWorkspaceScope::contains` (Issue #646) so
//! out-of-scope nested subtrees cannot leak into completion evidence.
//!
//! `pub(super)` limited / no facade re-export (DR3-001).

use std::path::Path;

use super::completion_evidence::classify_repo_edit_path;
use super::scaffold_pipeline::scaffold_candidate_priority;
use super::task_contract::{ArtifactRole, role_from_repo_edit};
use super::task_workspace_scope::TaskWorkspaceScope;
use super::workspace_walk::meaningful_workspace_files;

/// Legacy un-scoped lookup kept for unit-test fixtures that exercise the
/// `scaffold_candidate_priority` ordering independently of scope detection.
/// Production code MUST use [`existing_workspace_candidate_for_role_in_scope`]
/// so out-of-scope nested-subtree files cannot leak into completion evidence
/// (Issue #646).
#[cfg(test)]
pub(super) fn existing_workspace_candidate_for_role(
    work_root: &Path,
    role: ArtifactRole,
) -> Option<String> {
    let mut candidates = meaningful_workspace_files(work_root, 64)?
        .into_iter()
        .filter(|path| {
            role_from_repo_edit(classify_repo_edit_path(path))
                .is_some_and(|candidate| candidate == role)
        })
        .collect::<Vec<_>>();
    candidates.sort_by_key(|path| {
        scaffold_candidate_priority(work_root, role, &path.to_string_lossy().replace('\\', "/"))
    });
    candidates
        .first()
        .map(|path| path.to_string_lossy().replace('\\', "/"))
}

/// Issue #646: confirm that an absolute repair target lies inside the active
/// workspace scope. The path is normalized against `work_root` and the
/// resulting relative form is handed to [`TaskWorkspaceScope::contains`].
/// Targets that fail to strip the prefix (escape via canonical/symlink) are
/// treated as out-of-scope.
#[cfg(test)]
pub(super) fn target_path_in_scope(
    target: &Path,
    work_root: &Path,
    scope: &TaskWorkspaceScope,
) -> bool {
    let root_canon = std::fs::canonicalize(work_root).unwrap_or_else(|_| work_root.to_path_buf());
    let target_canon = std::fs::canonicalize(target).unwrap_or_else(|_| target.to_path_buf());
    let Ok(relative) = target_canon.strip_prefix(&root_canon) else {
        return false;
    };
    scope.contains(&relative.to_string_lossy().replace('\\', "/"))
}

/// Issue #646: scope-aware variant of [`existing_workspace_candidate_for_role`].
///
/// Filters workspace files through `scope.contains(...)` before priority
/// sorting so candidates inside out-of-scope nested subtrees (the
/// fresh-session-parent-directory bug case) are never surfaced. Used by
/// `task_contract_artifact_states` and `task_contract_recovery_target`;
/// the legacy un-scoped function is preserved for unit-test fixtures that
/// exercise the priority logic independently.
pub(super) fn existing_workspace_candidate_for_role_in_scope(
    work_root: &Path,
    role: ArtifactRole,
    scope: &TaskWorkspaceScope,
) -> Option<String> {
    let mut candidates = meaningful_workspace_files(work_root, 64)?
        .into_iter()
        .filter(|path| {
            role_from_repo_edit(classify_repo_edit_path(path))
                .is_some_and(|candidate| candidate == role)
        })
        .filter(|path| scope.contains(&path.to_string_lossy().replace('\\', "/")))
        .collect::<Vec<_>>();
    candidates.sort_by_key(|path| {
        scaffold_candidate_priority(work_root, role, &path.to_string_lossy().replace('\\', "/"))
    });
    candidates
        .first()
        .map(|path| path.to_string_lossy().replace('\\', "/"))
}
