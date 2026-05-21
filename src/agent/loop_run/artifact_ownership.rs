//! Issue #646: ownership classification for existing workspace artifacts.
//!
//! `ArtifactOwnership` separates "the current task is allowed to claim this
//! file as completion evidence" (`Owned`) from "this file exists but the
//! task did not produce it" (`CandidateOnly`) and "this file is outside the
//! active scope" (`OutOfScope`).
//!
//! Visibility: every export here is `pub(super)` and **must not** be
//! re-exported from `src/agent/loop_run.rs` (CLAUDE.md DR3-001).
//!
//! Security: canonical-path / symlink escape is checked *here* — even if a
//! caller hands us a path that already passed `resolve_user_path`, we
//! re-confirm scope containment on the canonicalized form before allowing
//! `Owned`.

use std::path::Path;

use super::task_contract::{ArtifactRole, ArtifactState, ArtifactStateKind};
use super::task_workspace_scope::{ScopeMode, TaskWorkspaceScope};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ArtifactOwnership {
    /// The current task owns this file — counts as completion evidence.
    Owned,
    /// The file exists in scope but the task did not produce / modify it.
    /// Allowed as a *reference* read but never as completion evidence.
    CandidateOnly,
    /// The file is outside the active scope, escapes the workspace via
    /// symlink/canonicalization, or sits inside an ignored top-level dir.
    /// Never read for completion evidence or repair target.
    OutOfScope,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct OwnershipInputs<'a> {
    pub(super) work_root: &'a Path,
    pub(super) relative_path: &'a str,
    pub(super) scope: &'a TaskWorkspaceScope,
    /// `true` when the file was written/edited in the current session.
    pub(super) edited_this_session: bool,
    /// `true` when the file was originally scaffolded by Anvil **and** the
    /// current content differs from the scaffold snapshot (post-scaffold
    /// delta). Scaffolded-but-unchanged should be `false` because the
    /// scaffold body itself is not yet request-shaped.
    pub(super) scaffold_changed: bool,
    /// `true` when a verifier targeting this file passed in the current
    /// active scope. Currently only consumed for forward extensibility;
    /// the planner already short-circuits on verifier success so passing
    /// `false` here is safe today.
    #[allow(dead_code)]
    pub(super) verifier_passed_in_scope: bool,
}

/// Classify a single workspace-relative artifact path.
///
/// Returns `OutOfScope` when *any* of:
/// - the path is absolute, contains `..`, or has a control character
/// - canonicalization escapes `work_root`
/// - the path resolves to a top-level ignored directory
/// - the scope does not contain the path
///
/// Returns `Owned` when the path is in scope **and** at least one ownership
/// signal is set (edited this session / scaffold delta / verifier success).
///
/// Returns `CandidateOnly` otherwise.
pub(super) fn classify_ownership(inputs: OwnershipInputs<'_>) -> ArtifactOwnership {
    if !path_is_syntactically_safe(inputs.relative_path) {
        return ArtifactOwnership::OutOfScope;
    }
    if path_in_ignored_top_dir(inputs.relative_path) {
        return ArtifactOwnership::OutOfScope;
    }
    if canonical_escape(inputs.work_root, inputs.relative_path) {
        return ArtifactOwnership::OutOfScope;
    }
    if !inputs.scope.contains(inputs.relative_path) {
        return ArtifactOwnership::OutOfScope;
    }
    // Issue #646: `Owned` への昇格条件:
    // (a) ユーザーが明示した subtree 内のファイル (ScopeMode::Explicit)
    // (b) active scope 内で今回 turn に Write/Edit された
    // (c) Anvil scaffold + post-scaffold delta
    // (d) active scope 内 verifier 成功
    let promoted_by_explicit_scope = matches!(inputs.scope.mode, ScopeMode::Explicit { .. });
    if promoted_by_explicit_scope
        || inputs.edited_this_session
        || inputs.scaffold_changed
        || inputs.verifier_passed_in_scope
    {
        ArtifactOwnership::Owned
    } else {
        ArtifactOwnership::CandidateOnly
    }
}

fn path_is_syntactically_safe(relative_path: &str) -> bool {
    if relative_path.is_empty() {
        return false;
    }
    if relative_path.chars().any(|c| c.is_control()) {
        return false;
    }
    let path = Path::new(relative_path);
    if path.is_absolute() {
        return false;
    }
    !path
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
}

fn path_in_ignored_top_dir(relative_path: &str) -> bool {
    Path::new(relative_path).components().any(|c| match c {
        std::path::Component::Normal(s) => {
            super::task_workspace_scope::is_workspace_ignored_dir(&s.to_string_lossy())
        }
        _ => false,
    })
}

/// Returns `true` when canonicalizing `work_root.join(relative_path)`
/// escapes `work_root` (symlink swap, etc.).
///
/// On missing files we return `false`: the file simply has not been
/// created yet, which is not a canonicalization escape — the file is still
/// inside the workspace by construction (`work_root.join(...)`).
fn canonical_escape(work_root: &Path, relative_path: &str) -> bool {
    let target = work_root.join(relative_path);
    let Ok(target_canon) = std::fs::canonicalize(&target) else {
        return false;
    };
    let root_canon = std::fs::canonicalize(work_root).unwrap_or_else(|_| work_root.to_path_buf());
    target_canon.strip_prefix(&root_canon).is_err()
}

/// Issue #651: SSOT extraction of "test artifact paths the current task is
/// allowed to bind to the structured verifier command".
///
/// Filters `artifacts` to entries that are
/// - `role == Test`
/// - `kind ∈ {ChangedThisTurn, ExistsButUnverified}` (i.e. the planner is
///   actively interested in this turn's edit)
/// - have a known `path` (entries with `path: None` are skipped — the
///   verifier cannot bind a path it does not know)
/// - classify as [`ArtifactOwnership::Owned`] under
///   [`classify_ownership`] (so scope / symlink / `..` / absolute checks
///   all run inside the SSOT, never duplicated by callers).
///
/// The two predicate closures (`edited_this_session_for`,
/// `scaffold_changed_for`) keep the helper decoupled from `turn.rs`
/// internals (`turn_edited_relative_paths`, scaffold delta logic) per
/// design judgement #4 / DR3-005.
///
/// Returned paths preserve their input order (artifact-list order) and
/// are deduplicated.
///
/// `#[allow(dead_code)]` is intentional in Phase 1.3 — Phase 3/5 (caller
/// migration) will wire `verifier_skill.rs` / `success.rs` /
/// `turn.rs::run_task_contract_verifier_once` to this helper.
#[allow(dead_code)]
pub(super) fn owned_test_artifacts(
    artifacts: &[ArtifactState],
    work_root: &Path,
    scope: &TaskWorkspaceScope,
    edited_this_session_for: &dyn Fn(&str) -> bool,
    scaffold_changed_for: &dyn Fn(&str) -> bool,
) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for state in artifacts {
        if state.role != ArtifactRole::Test {
            continue;
        }
        if !matches!(
            state.kind,
            ArtifactStateKind::ChangedThisTurn | ArtifactStateKind::ExistsButUnverified
        ) {
            continue;
        }
        let Some(path) = state.path.as_deref() else {
            // `path: None` means the turn observed a Test edit but did
            // not retain the path (e.g. an aggregate evidence row).
            // Such entries cannot be bound to a structured verifier;
            // verifier_missing semantics are the caller's responsibility.
            continue;
        };
        let ownership = classify_ownership(OwnershipInputs {
            work_root,
            relative_path: path,
            scope,
            edited_this_session: edited_this_session_for(path),
            scaffold_changed: scaffold_changed_for(path),
            verifier_passed_in_scope: false,
        });
        if !matches!(ownership, ArtifactOwnership::Owned) {
            continue;
        }
        let path_owned = path.to_string();
        if !out.contains(&path_owned) {
            out.push(path_owned);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::super::task_workspace_scope::{ScopeMode, TaskWorkspaceScope};
    use super::*;
    use std::path::PathBuf;
    use tempfile::tempdir;

    fn ambiguous_scope(nested: Vec<PathBuf>) -> TaskWorkspaceScope {
        TaskWorkspaceScope {
            mode: ScopeMode::AmbiguousParent {
                nested_subtrees: nested,
            },
        }
    }

    fn single_root_scope() -> TaskWorkspaceScope {
        TaskWorkspaceScope {
            mode: ScopeMode::SingleProjectRoot,
        }
    }

    #[test]
    fn existing_subtree_file_is_out_of_scope_in_ambiguous_parent() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("0517_003/app")).unwrap();
        std::fs::write(dir.path().join("0517_003/app/main.py"), "").unwrap();
        let scope = ambiguous_scope(vec![PathBuf::from("0517_003")]);
        let out = classify_ownership(OwnershipInputs {
            work_root: dir.path(),
            relative_path: "0517_003/app/main.py",
            scope: &scope,
            edited_this_session: false,
            scaffold_changed: false,
            verifier_passed_in_scope: false,
        });
        assert_eq!(out, ArtifactOwnership::OutOfScope);
    }

    #[test]
    fn in_scope_pre_existing_file_without_signal_is_candidate_only() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("README.md"), "# Hello\n").unwrap();
        let scope = single_root_scope();
        let out = classify_ownership(OwnershipInputs {
            work_root: dir.path(),
            relative_path: "README.md",
            scope: &scope,
            edited_this_session: false,
            scaffold_changed: false,
            verifier_passed_in_scope: false,
        });
        assert_eq!(out, ArtifactOwnership::CandidateOnly);
    }

    #[test]
    fn edited_this_session_promotes_to_owned() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("README.md"), "").unwrap();
        let scope = single_root_scope();
        let out = classify_ownership(OwnershipInputs {
            work_root: dir.path(),
            relative_path: "README.md",
            scope: &scope,
            edited_this_session: true,
            scaffold_changed: false,
            verifier_passed_in_scope: false,
        });
        assert_eq!(out, ArtifactOwnership::Owned);
    }

    #[test]
    fn scaffold_changed_promotes_to_owned() {
        let dir = tempdir().unwrap();
        let scope = single_root_scope();
        let out = classify_ownership(OwnershipInputs {
            work_root: dir.path(),
            relative_path: "app/main.py",
            scope: &scope,
            edited_this_session: false,
            scaffold_changed: true,
            verifier_passed_in_scope: false,
        });
        assert_eq!(out, ArtifactOwnership::Owned);
    }

    #[test]
    fn absolute_path_is_out_of_scope() {
        let dir = tempdir().unwrap();
        let scope = single_root_scope();
        let out = classify_ownership(OwnershipInputs {
            work_root: dir.path(),
            relative_path: "/etc/passwd",
            scope: &scope,
            edited_this_session: true,
            scaffold_changed: false,
            verifier_passed_in_scope: false,
        });
        assert_eq!(out, ArtifactOwnership::OutOfScope);
    }

    #[test]
    fn parent_dir_traversal_is_out_of_scope() {
        let dir = tempdir().unwrap();
        let scope = single_root_scope();
        let out = classify_ownership(OwnershipInputs {
            work_root: dir.path(),
            relative_path: "../sibling/file.py",
            scope: &scope,
            edited_this_session: true,
            scaffold_changed: false,
            verifier_passed_in_scope: false,
        });
        assert_eq!(out, ArtifactOwnership::OutOfScope);
    }

    #[test]
    fn ignored_top_dir_is_out_of_scope_even_when_marked_edited() {
        let dir = tempdir().unwrap();
        let scope = single_root_scope();
        let out = classify_ownership(OwnershipInputs {
            work_root: dir.path(),
            relative_path: "node_modules/foo/index.js",
            scope: &scope,
            edited_this_session: true,
            scaffold_changed: false,
            verifier_passed_in_scope: false,
        });
        assert_eq!(out, ArtifactOwnership::OutOfScope);
    }

    #[cfg(unix)]
    #[test]
    fn symlink_escape_is_out_of_scope() {
        use std::os::unix::fs::symlink;
        let outside = tempdir().unwrap();
        std::fs::write(outside.path().join("secret.txt"), "secret\n").unwrap();
        let work = tempdir().unwrap();
        symlink(
            outside.path().join("secret.txt"),
            work.path().join("alias.txt"),
        )
        .unwrap();
        let scope = single_root_scope();
        let out = classify_ownership(OwnershipInputs {
            work_root: work.path(),
            relative_path: "alias.txt",
            scope: &scope,
            edited_this_session: true,
            scaffold_changed: false,
            verifier_passed_in_scope: false,
        });
        assert_eq!(out, ArtifactOwnership::OutOfScope);
    }

    #[test]
    fn explicit_scope_promotes_existing_file_to_owned() {
        // user explicitly named the subtree: the file is in scope AND the
        // scope mode itself counts as an ownership signal (Issue #646
        // §修正方針 2 `Owned` 条件 1).
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("0517_003/app")).unwrap();
        std::fs::write(dir.path().join("0517_003/app/main.py"), "").unwrap();
        let scope = TaskWorkspaceScope {
            mode: ScopeMode::Explicit {
                paths: vec![PathBuf::from("0517_003")],
            },
        };
        let out = classify_ownership(OwnershipInputs {
            work_root: dir.path(),
            relative_path: "0517_003/app/main.py",
            scope: &scope,
            edited_this_session: false,
            scaffold_changed: false,
            verifier_passed_in_scope: false,
        });
        assert_eq!(out, ArtifactOwnership::Owned);
    }

    #[test]
    fn turn_boundary_clear_demotes_owned_back_to_candidate_only() {
        // Issue #646 (A4): the `turn_edited_relative_paths` set is reset at
        // each `handle_user_message` head (CLAUDE.md per-turn cap pattern).
        // Once that set is cleared, the same file the previous turn promoted
        // to Owned must classify as CandidateOnly again — preventing the
        // new task from auto-inheriting prior-turn ownership.
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("app.py"), "x = 1\n").unwrap();
        let scope = single_root_scope();

        // Inside the original turn — the set contained the path → Owned.
        let during_turn = classify_ownership(OwnershipInputs {
            work_root: dir.path(),
            relative_path: "app.py",
            scope: &scope,
            edited_this_session: true,
            scaffold_changed: false,
            verifier_passed_in_scope: false,
        });
        assert_eq!(during_turn, ArtifactOwnership::Owned);

        // After `handle_user_message` cleared the set — same file, no
        // signals → CandidateOnly.
        let after_turn_boundary = classify_ownership(OwnershipInputs {
            work_root: dir.path(),
            relative_path: "app.py",
            scope: &scope,
            edited_this_session: false,
            scaffold_changed: false,
            verifier_passed_in_scope: false,
        });
        assert_eq!(after_turn_boundary, ArtifactOwnership::CandidateOnly);
    }

    #[test]
    fn no_op_write_does_not_promote_to_owned() {
        // Issue #646 (A4 / C2): an in-scope file exists but the model
        // wrote the exact same body back (no scaffold delta, no recorded
        // edit). Ownership must stay CandidateOnly even though the path
        // matches an artifact role.
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("README.md"), "# Scaffold body\n").unwrap();
        let scope = single_root_scope();
        let out = classify_ownership(OwnershipInputs {
            work_root: dir.path(),
            relative_path: "README.md",
            scope: &scope,
            edited_this_session: false,
            scaffold_changed: false,
            verifier_passed_in_scope: false,
        });
        assert_eq!(out, ArtifactOwnership::CandidateOnly);
    }

    #[test]
    fn ambiguous_parent_root_root_file_is_owned_when_edited_this_turn() {
        // Issue #646 (A4): a fresh in-scope artifact created at the parent
        // root (not inside any nested project subtree) promotes to Owned
        // as soon as it is recorded in `turn_edited_relative_paths`.
        let dir = tempdir().unwrap();
        let scope = ambiguous_scope(vec![PathBuf::from("0517_003")]);
        let out = classify_ownership(OwnershipInputs {
            work_root: dir.path(),
            relative_path: "app/main.py",
            scope: &scope,
            edited_this_session: true,
            scaffold_changed: false,
            verifier_passed_in_scope: false,
        });
        assert_eq!(out, ArtifactOwnership::Owned);
    }

    #[test]
    fn missing_file_is_classified_without_canonicalization_escape() {
        // A path that does not yet exist is allowed; the upstream caller
        // is in the middle of creating it. `canonical_escape` only triggers
        // when canonicalization succeeds and lands outside the workspace.
        let dir = tempdir().unwrap();
        let scope = single_root_scope();
        let out = classify_ownership(OwnershipInputs {
            work_root: dir.path(),
            relative_path: "future/file.py",
            scope: &scope,
            edited_this_session: true,
            scaffold_changed: false,
            verifier_passed_in_scope: false,
        });
        assert_eq!(out, ArtifactOwnership::Owned);
    }

    // -----------------------------------------------------------------
    // Issue #651: owned_test_artifacts helper (SSOT for verifier binding).
    // -----------------------------------------------------------------

    fn test_state_exists(path: &str) -> ArtifactState {
        ArtifactState {
            role: ArtifactRole::Test,
            path: Some(path.to_string()),
            kind: ArtifactStateKind::ExistsButUnverified,
        }
    }

    fn test_state_changed_with_path(path: &str) -> ArtifactState {
        ArtifactState {
            role: ArtifactRole::Test,
            path: Some(path.to_string()),
            kind: ArtifactStateKind::ChangedThisTurn,
        }
    }

    fn test_state_changed_no_path() -> ArtifactState {
        ArtifactState {
            role: ArtifactRole::Test,
            path: None,
            kind: ArtifactStateKind::ChangedThisTurn,
        }
    }

    fn impl_state_changed(path: &str) -> ArtifactState {
        ArtifactState {
            role: ArtifactRole::Implementation,
            path: Some(path.to_string()),
            kind: ArtifactStateKind::ChangedThisTurn,
        }
    }

    #[test]
    fn owned_test_artifacts_collects_changed_and_exists_test_paths() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("app/tests")).unwrap();
        std::fs::write(dir.path().join("app/tests/test_a.py"), "").unwrap();
        std::fs::write(dir.path().join("app/tests/test_b.py"), "").unwrap();
        let scope = single_root_scope();
        let artifacts = vec![
            test_state_changed_with_path("app/tests/test_a.py"),
            test_state_exists("app/tests/test_b.py"),
            // ChangedThisTurn for Implementation must be ignored.
            impl_state_changed("app/main.py"),
        ];
        let edited_for: Box<dyn Fn(&str) -> bool> = Box::new(|p| {
            p == "app/tests/test_a.py" || p == "app/tests/test_b.py" || p == "app/main.py"
        });
        let scaffold_for: Box<dyn Fn(&str) -> bool> = Box::new(|_| false);
        let owned = owned_test_artifacts(
            &artifacts,
            dir.path(),
            &scope,
            edited_for.as_ref(),
            scaffold_for.as_ref(),
        );
        assert_eq!(
            owned,
            vec![
                "app/tests/test_a.py".to_string(),
                "app/tests/test_b.py".to_string(),
            ],
            "expected both Test artifact rows in input order"
        );
    }

    #[test]
    fn owned_test_artifacts_skips_entries_with_no_path() {
        let dir = tempdir().unwrap();
        let scope = single_root_scope();
        let artifacts = vec![
            test_state_changed_no_path(),
            test_state_changed_with_path("tests/test_real.py"),
        ];
        let edited_for: Box<dyn Fn(&str) -> bool> = Box::new(|_| true);
        let scaffold_for: Box<dyn Fn(&str) -> bool> = Box::new(|_| false);
        let owned = owned_test_artifacts(
            &artifacts,
            dir.path(),
            &scope,
            edited_for.as_ref(),
            scaffold_for.as_ref(),
        );
        assert_eq!(owned, vec!["tests/test_real.py".to_string()]);
    }

    #[test]
    fn owned_test_artifacts_rejects_absolute_path() {
        let dir = tempdir().unwrap();
        let scope = single_root_scope();
        let artifacts = vec![test_state_changed_with_path("/etc/passwd")];
        let edited_for: Box<dyn Fn(&str) -> bool> = Box::new(|_| true);
        let scaffold_for: Box<dyn Fn(&str) -> bool> = Box::new(|_| false);
        let owned = owned_test_artifacts(
            &artifacts,
            dir.path(),
            &scope,
            edited_for.as_ref(),
            scaffold_for.as_ref(),
        );
        assert!(
            owned.is_empty(),
            "absolute path must be filtered by classify_ownership, got {owned:?}"
        );
    }

    #[test]
    fn owned_test_artifacts_rejects_parent_dir_traversal() {
        let dir = tempdir().unwrap();
        let scope = single_root_scope();
        let artifacts = vec![test_state_changed_with_path("../sibling/test_x.py")];
        let edited_for: Box<dyn Fn(&str) -> bool> = Box::new(|_| true);
        let scaffold_for: Box<dyn Fn(&str) -> bool> = Box::new(|_| false);
        let owned = owned_test_artifacts(
            &artifacts,
            dir.path(),
            &scope,
            edited_for.as_ref(),
            scaffold_for.as_ref(),
        );
        assert!(
            owned.is_empty(),
            "parent-dir traversal must be filtered, got {owned:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn owned_test_artifacts_rejects_symlink_escape() {
        use std::os::unix::fs::symlink;
        let outside = tempdir().unwrap();
        std::fs::write(outside.path().join("secret_test.py"), "").unwrap();
        let work = tempdir().unwrap();
        symlink(
            outside.path().join("secret_test.py"),
            work.path().join("alias_test.py"),
        )
        .unwrap();
        let scope = single_root_scope();
        let artifacts = vec![test_state_changed_with_path("alias_test.py")];
        let edited_for: Box<dyn Fn(&str) -> bool> = Box::new(|_| true);
        let scaffold_for: Box<dyn Fn(&str) -> bool> = Box::new(|_| false);
        let owned = owned_test_artifacts(
            &artifacts,
            work.path(),
            &scope,
            edited_for.as_ref(),
            scaffold_for.as_ref(),
        );
        assert!(
            owned.is_empty(),
            "symlink escape must be filtered, got {owned:?}"
        );
    }

    #[test]
    fn owned_test_artifacts_keeps_only_owned_classifications() {
        // No `edited_this_session`, no `scaffold_changed`, scope is the
        // generic single-project root → classify_ownership returns
        // CandidateOnly, so the helper must drop the entry.
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("test_pre_existing.py"), "").unwrap();
        let scope = single_root_scope();
        let artifacts = vec![test_state_exists("test_pre_existing.py")];
        let edited_for: Box<dyn Fn(&str) -> bool> = Box::new(|_| false);
        let scaffold_for: Box<dyn Fn(&str) -> bool> = Box::new(|_| false);
        let owned = owned_test_artifacts(
            &artifacts,
            dir.path(),
            &scope,
            edited_for.as_ref(),
            scaffold_for.as_ref(),
        );
        assert!(
            owned.is_empty(),
            "CandidateOnly classification must be dropped, got {owned:?}"
        );
    }
}
