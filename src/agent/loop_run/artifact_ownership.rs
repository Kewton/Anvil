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

/// Issue #661 (Task 2.1 / DR1-003): admission flag for `classify_ownership`
/// that lets the **verifier-local** 4 propagation paths
/// (`record_repo_edit_event` / `projection::owned_test_artifacts` /
/// `validate_bound_test_artifacts_for_execution` /
/// `seed_artifact_ledger_verifier_observation`) treat nested test subdirs
/// such as `app/tests/foo.py` as `Owned`, while every other caller continues
/// to receive the legacy reject behaviour through `Default::default()` /
/// `disabled()`.
///
/// The single private boolean field is **not** public; callers must use the
/// named constructors `disabled()` / `enabled()` so the flag cannot be
/// confused with arbitrary booleans flowing through plumbing code.
///
/// Iteration-2 introduces the type only; the `OwnershipInputs` field that
/// consumes it lands in Task 3.1 below, and the 4 verifier-path callers that
/// pass `enabled()` are wired up in iteration-3 (Task 3.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct NestedTestAdmission(bool);

impl NestedTestAdmission {
    /// Reject nested test subdirs (legacy / default behaviour). All callers
    /// outside the 4 verifier-path SSOT sites pass this.
    pub(super) const fn disabled() -> Self {
        Self(false)
    }

    /// Admit nested test subdirs as `Owned`. Only the 4 verifier-path
    /// callers in iteration-3 use this constructor.
    #[allow(dead_code)] // Iteration-3 wires the verifier-path callers; until then the constructor has no production caller other than dedicated unit tests.
    pub(super) const fn enabled() -> Self {
        Self(true)
    }

    /// Read accessor. Marked `self` (by-value) because `NestedTestAdmission`
    /// is `Copy`; callers don't need to think about lifetimes.
    pub(super) fn allow_nested_test_subdirs(self) -> bool {
        self.0
    }
}

impl Default for NestedTestAdmission {
    /// `disabled()` — preserves existing caller semantics when
    /// `OwnershipInputs` is constructed without explicitly setting the
    /// admission field.
    fn default() -> Self {
        Self::disabled()
    }
}

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
    /// Issue #661 (Task 3.1 / DR2-003): admission flag that lets the 4
    /// verifier-path SSOT sites treat nested test subdirs such as
    /// `app/tests/foo.py` as `Owned` when an edit/scaffold signal is
    /// present. Every non-verifier caller passes `Default::default()`
    /// (= `disabled()`), preserving the legacy reject behaviour. The flag
    /// never bypasses workspace-relative / symlink containment /
    /// ignored_top_dir / role re-confirm; those checks remain
    /// authoritative.
    pub(super) nested_test_admission: NestedTestAdmission,
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
    let has_promotion_signal = promoted_by_explicit_scope
        || inputs.edited_this_session
        || inputs.scaffold_changed
        || inputs.verifier_passed_in_scope;
    // Issue #661 (Task 3.3 / 判断 #1 / DR2-003): the verifier-local 4
    // propagation paths pass `nested_test_admission.enabled()` to opt the
    // verifier binding SSOT into treating nested test subdirs (e.g.
    // `app/tests/foo.py`) as `Owned`. Every other caller keeps
    // `Default::default()` (= `disabled()`), preserving the legacy reject
    // behaviour exactly. The admission only triggers when the path is
    // recognised as a test file by `util::file_classify::is_test_file`
    // (which already enumerates `/tests/` substrings, `__tests__`,
    // `_test.` / `.test.` / `.spec.`, `test_` prefix). Workspace-relative
    // / symlink / ignored_top_dir / scope.contains checks above remain
    // authoritative — admission cannot bypass any of them.
    if has_promotion_signal {
        ArtifactOwnership::Owned
    } else if inputs.nested_test_admission.allow_nested_test_subdirs()
        && crate::util::file_classify::is_test_file(Path::new(inputs.relative_path))
    {
        // Verifier-binding SSOT path: the planner needs to bind nested
        // `app/tests/...` style paths to the structured verifier even
        // when no explicit edit signal has been recorded on the path
        // (e.g. an existing test file the model now wants to run). Other
        // callers (which always pass `disabled()`) preserve the legacy
        // CandidateOnly outcome.
        //
        // CB-003 mitigation: `canonical_escape` above returns `false` for
        // missing leaves (the file may legitimately not exist yet on the
        // edit signal path). For the no-signal `nested_test_admission`
        // branch, however, we have no separate signal that guarantees the
        // path lives under work_root. Walk the nearest existing ancestor
        // through `nearest_existing_ancestor_within_work_root` so a
        // dangling-symlink parent (e.g. `app/tests` → /outside) cannot
        // smuggle a not-yet-created test leaf into `Owned` classification.
        let target_full = inputs.work_root.join(inputs.relative_path);
        if !nearest_existing_ancestor_within_work_root(inputs.work_root, &target_full) {
            ArtifactOwnership::OutOfScope
        } else {
            ArtifactOwnership::Owned
        }
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
            // Issue #661 (Task 3.1 / 判断 #1 path #2): projection trusts the
            // admission decision already encoded by `record_repo_edit_event`
            // / `record_verifier_observation`; do NOT re-evaluate the flag
            // here. Iteration-3 leaves this `disabled()` and the 4 verifier
            // entry points switch to `enabled()` instead (Task 3.4).
            nested_test_admission: NestedTestAdmission::default(),
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

/// Issue #652 PR-002 SSOT: missing-tail parent / dangling-symlink ancestor
/// validation for a candidate path.
///
/// First inspects the **candidate (leaf) itself** via `symlink_metadata`.
/// PRR-001 (re-review v2): the previous version skipped this check and
/// advanced straight to `candidate.parent()`, which let a dangling-symlink
/// leaf (e.g. `tests/test_artifact.py -> /outside/missing.py`) slip through
/// whenever the parent dir was inside `work_root`. A subsequent
/// `std::fs::write` on the leaf dereferences the link and writes outside
/// the workspace.
///
/// Behaviour:
/// - If the candidate exists (per `symlink_metadata` — does NOT follow
///   links): require `canonicalize(candidate)` to succeed and resolve to a
///   path inside `canonicalize(work_root)`. A dangling symlink leaf
///   (metadata succeeds, canonicalize fails) is rejected.
/// - If the candidate does not exist (per `symlink_metadata`): walk up to
///   the nearest existing ancestor and apply the same `canonicalize` +
///   prefix check. A dangling-symlink ancestor is rejected.
///
/// Returns `false` when:
/// - `canonicalize(work_root)` fails (defensive — work_root must exist);
/// - the walk falls off the filesystem root before any ancestor is found;
/// - any leaf-or-ancestor is a dangling symlink (`symlink_metadata().is_ok()`
///   but `canonicalize` fails — CB2-002 / PRR-001);
/// - the canonicalized leaf-or-ancestor escapes `work_root` via a symlink.
///
/// Callers: `artifact_completion_job::ArtifactCompletionJob::new`
/// (missing-leaf creation target) and `turn.rs::
/// tool_path_matches_target_via_workspace_ssot` (tool-argument match).
/// `candidate` is the fully joined `work_root.join(relative_path)` for
/// relative paths, or the raw absolute path for absolute inputs.
pub(super) fn nearest_existing_ancestor_within_work_root(
    work_root: &Path,
    candidate: &Path,
) -> bool {
    let Ok(root_canon) = std::fs::canonicalize(work_root) else {
        return false;
    };
    // PRR-001: inspect the leaf itself first. `symlink_metadata` does not
    // follow links, so a dangling symlink leaf surfaces here as
    // `Ok(meta)` while `canonicalize` will subsequently fail. A genuine
    // file/dir at the leaf canonicalizes successfully — we still verify
    // the canonical form is contained inside the workspace to block
    // in-root symlinks pointing outside.
    if candidate.symlink_metadata().is_ok() {
        let Ok(canon) = std::fs::canonicalize(candidate) else {
            // Dangling symlink leaf (metadata succeeds, canonicalize
            // fails). Reject — a follow-up Write/Edit would dereference
            // the link and escape the workspace.
            return false;
        };
        return canon.starts_with(&root_canon);
    }
    // Candidate does not exist on disk — walk up to the nearest existing
    // ancestor and apply the same containment check.
    let mut cursor: &Path = candidate;
    loop {
        match cursor.parent() {
            Some(parent) => cursor = parent,
            None => return false,
        }
        if cursor.symlink_metadata().is_ok() {
            // First existing ancestor (per the link itself). `canonicalize`
            // resolves symlinks AND fails on dangling links — both cases
            // collapse cleanly here.
            let Ok(canon) = std::fs::canonicalize(cursor) else {
                return false;
            };
            return canon.starts_with(&root_canon);
        }
        // Ancestor is genuinely missing — keep walking up.
    }
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
            nested_test_admission: NestedTestAdmission::default(),
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
            nested_test_admission: NestedTestAdmission::default(),
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
            nested_test_admission: NestedTestAdmission::default(),
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
            nested_test_admission: NestedTestAdmission::default(),
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
            nested_test_admission: NestedTestAdmission::default(),
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
            nested_test_admission: NestedTestAdmission::default(),
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
            nested_test_admission: NestedTestAdmission::default(),
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
            nested_test_admission: NestedTestAdmission::default(),
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
            nested_test_admission: NestedTestAdmission::default(),
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
            nested_test_admission: NestedTestAdmission::default(),
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
            nested_test_admission: NestedTestAdmission::default(),
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
            nested_test_admission: NestedTestAdmission::default(),
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
            nested_test_admission: NestedTestAdmission::default(),
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
            nested_test_admission: NestedTestAdmission::default(),
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

    // ----------------------------------------------------------------
    // Issue #652 PR-002 SSOT: nearest_existing_ancestor_within_work_root.
    // ----------------------------------------------------------------

    #[test]
    fn nearest_existing_ancestor_accepts_missing_leaf_under_existing_parent() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("tests")).unwrap();
        let candidate = dir.path().join("tests/test_new.py");
        assert!(
            nearest_existing_ancestor_within_work_root(dir.path(), &candidate),
            "missing leaf with existing parent inside work_root must be accepted"
        );
    }

    #[test]
    fn nearest_existing_ancestor_rejects_when_work_root_does_not_exist() {
        let dir = tempdir().unwrap();
        let phantom = dir.path().join("phantom_root");
        let candidate = phantom.join("tests/test_new.py");
        assert!(
            !nearest_existing_ancestor_within_work_root(&phantom, &candidate),
            "non-existent work_root must produce a defensive reject"
        );
    }

    #[cfg(unix)]
    #[test]
    fn nearest_existing_ancestor_rejects_dangling_symlink_ancestor() {
        use std::os::unix::fs::symlink;
        let work = tempdir().unwrap();
        let outside = tempdir().unwrap();
        // Build a dangling symlink: outside/nonexistent does not exist.
        symlink(
            outside.path().join("nonexistent"),
            work.path().join("dangle"),
        )
        .unwrap();
        let candidate = work.path().join("dangle").join("file.py");
        assert!(
            !nearest_existing_ancestor_within_work_root(work.path(), &candidate),
            "dangling-symlink ancestor must be rejected (CB2-002)"
        );
    }

    #[cfg(unix)]
    #[test]
    fn nearest_existing_ancestor_rejects_symlinked_parent_escaping_root() {
        use std::os::unix::fs::symlink;
        let outside = tempdir().unwrap();
        std::fs::create_dir_all(outside.path().join("tests")).unwrap();
        let work = tempdir().unwrap();
        symlink(outside.path().join("tests"), work.path().join("tests")).unwrap();
        let candidate = work.path().join("tests/test_new.py");
        assert!(
            !nearest_existing_ancestor_within_work_root(work.path(), &candidate),
            "symlinked parent that canonicalizes outside work_root must be rejected"
        );
    }

    // ----------------------------------------------------------------
    // PRR-001 (re-review v2): the **leaf** itself must be inspected.
    // `nearest_existing_ancestor_within_work_root` previously advanced to
    // `candidate.parent()` before any `symlink_metadata` check, which let a
    // dangling-symlink leaf slip through (parent dir is inside work_root,
    // so the function returned `true`). The follow-up Write/Edit would
    // dereference the symlink and write outside the workspace.
    // ----------------------------------------------------------------

    #[cfg(unix)]
    #[test]
    fn nearest_existing_ancestor_rejects_dangling_symlink_leaf() {
        use std::os::unix::fs::symlink;
        let work = tempdir().unwrap();
        std::fs::create_dir_all(work.path().join("tests")).unwrap();
        let outside = tempdir().unwrap();
        // Build a dangling symlink leaf: tests/test_artifact.py points at
        // a path outside the workspace that does not exist.
        symlink(
            outside.path().join("missing.py"),
            work.path().join("tests/test_artifact.py"),
        )
        .unwrap();
        let candidate = work.path().join("tests/test_artifact.py");
        assert!(
            !nearest_existing_ancestor_within_work_root(work.path(), &candidate),
            "PRR-001: dangling symlink leaf must be rejected even when the parent is inside work_root"
        );
    }

    #[cfg(unix)]
    #[test]
    fn nearest_existing_ancestor_rejects_symlink_leaf_escaping_root() {
        use std::os::unix::fs::symlink;
        let outside = tempdir().unwrap();
        std::fs::write(outside.path().join("secret.py"), "secret").unwrap();
        let work = tempdir().unwrap();
        std::fs::create_dir_all(work.path().join("tests")).unwrap();
        // A symlink leaf that points to an existing file outside work_root.
        // Even though canonicalize() succeeds, the canonical form escapes
        // the workspace and must be rejected.
        symlink(
            outside.path().join("secret.py"),
            work.path().join("tests/escape.py"),
        )
        .unwrap();
        let candidate = work.path().join("tests/escape.py");
        assert!(
            !nearest_existing_ancestor_within_work_root(work.path(), &candidate),
            "PRR-001: symlink leaf canonicalizing outside work_root must be rejected"
        );
    }

    #[cfg(unix)]
    #[test]
    fn nearest_existing_ancestor_accepts_symlink_leaf_to_in_root_file() {
        use std::os::unix::fs::symlink;
        let work = tempdir().unwrap();
        std::fs::create_dir_all(work.path().join("tests")).unwrap();
        std::fs::write(work.path().join("tests/real.py"), "ok").unwrap();
        // Symlink leaf pointing to an in-root file — canonicalize lands
        // inside work_root, so this is safe to bind.
        symlink(
            work.path().join("tests/real.py"),
            work.path().join("tests/alias.py"),
        )
        .unwrap();
        let candidate = work.path().join("tests/alias.py");
        assert!(
            nearest_existing_ancestor_within_work_root(work.path(), &candidate),
            "PRR-001: in-root symlink leaf canonicalizing inside work_root must be accepted"
        );
    }

    #[test]
    fn nearest_existing_ancestor_accepts_real_existing_leaf_inside_root() {
        let work = tempdir().unwrap();
        std::fs::create_dir_all(work.path().join("tests")).unwrap();
        std::fs::write(work.path().join("tests/real.py"), "ok").unwrap();
        let candidate = work.path().join("tests/real.py");
        assert!(
            nearest_existing_ancestor_within_work_root(work.path(), &candidate),
            "real existing leaf inside work_root must be accepted (regression guard)"
        );
    }

    // ----------------------------------------------------------------
    // Issue #661 Task 2.1: NestedTestAdmission newtype contract.
    // DR1-003: public field を持たない named constructors により誤伝播を
    // 型レベルで防ぐ。`Default::default()` は `disabled()` を返し、既存
    // caller の意味論を破壊しない。
    // ----------------------------------------------------------------

    #[test]
    fn nested_test_admission_default_is_disabled() {
        let admission: NestedTestAdmission = NestedTestAdmission::default();
        assert!(
            !admission.allow_nested_test_subdirs(),
            "Default::default() must return disabled() = false"
        );
    }

    #[test]
    fn nested_test_admission_disabled_constructor_returns_false() {
        let admission = NestedTestAdmission::disabled();
        assert!(
            !admission.allow_nested_test_subdirs(),
            "disabled() must return false"
        );
    }

    #[test]
    fn nested_test_admission_enabled_constructor_returns_true() {
        let admission = NestedTestAdmission::enabled();
        assert!(
            admission.allow_nested_test_subdirs(),
            "enabled() must return true"
        );
    }

    #[test]
    fn nested_test_admission_is_copy_clone() {
        // Compile-time test: NestedTestAdmission must be Copy + Clone so it
        // can be passed by value to `OwnershipInputs` constructors without
        // ownership friction. Verified by relying on a `Copy` move pattern.
        let admission = NestedTestAdmission::enabled();
        let copy = admission;
        let clone = admission;
        assert!(copy.allow_nested_test_subdirs());
        assert!(clone.allow_nested_test_subdirs());
    }

    // ----------------------------------------------------------------
    // Issue #661 Task 3.1: `OwnershipInputs.nested_test_admission` field
    // shape contract. The field MUST exist, accept a `NestedTestAdmission`
    // by value, and default to `disabled()` so any caller that omits an
    // explicit setting (`..Default::default()` / field initializer
    // shorthand inside `OwnershipInputs::default()` test stubs) keeps the
    // legacy "reject nested test subdirs" behaviour.
    //
    // The Red phase for Task 3.3 (admission enables nested test Owned) is
    // co-located below this section so the two tasks share fixtures.
    // ----------------------------------------------------------------

    #[test]
    fn ownership_inputs_accepts_explicit_admission_field() {
        // Compile-time + runtime: the new field must exist on
        // `OwnershipInputs` and accept a `NestedTestAdmission` value. Pass
        // `enabled()` here so the test asserts the field is consumed (the
        // helper does not panic / does not silently drop the value).
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("README.md"), "# Hello\n").unwrap();
        let scope = single_root_scope();
        let out = classify_ownership(OwnershipInputs {
            work_root: dir.path(),
            relative_path: "README.md",
            scope: &scope,
            edited_this_session: true,
            scaffold_changed: false,
            verifier_passed_in_scope: false,
            nested_test_admission: NestedTestAdmission::enabled(),
        });
        // Non-test path: admission has no effect, the file is Owned via
        // `edited_this_session`. This pins the field's signature only.
        assert_eq!(out, ArtifactOwnership::Owned);
    }

    // ----------------------------------------------------------------
    // Issue #661 Task 3.3: `classify_ownership` honours the admission flag
    // for nested test subdirs. `app/tests/foo.py` matches
    // `util::file_classify::is_test_file` (contains `/tests/`).
    //   - `enabled()` + edit signal → Owned
    //   - `disabled()` (default) + edit signal → CandidateOnly (current
    //     behaviour: the path is in scope but the legacy code does NOT
    //     promote it as a verifier-bindable test artifact)
    // Workspace-relative / symlink containment / ignored_top_dir / role
    // re-confirm のすべては既存挙動を維持。
    // ----------------------------------------------------------------

    #[test]
    fn classify_ownership_admits_nested_test_with_admission_enabled() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("app/tests")).unwrap();
        std::fs::write(dir.path().join("app/tests/test_a.py"), "").unwrap();
        let scope = single_root_scope();
        let out = classify_ownership(OwnershipInputs {
            work_root: dir.path(),
            relative_path: "app/tests/test_a.py",
            scope: &scope,
            edited_this_session: true,
            scaffold_changed: false,
            verifier_passed_in_scope: false,
            nested_test_admission: NestedTestAdmission::enabled(),
        });
        assert_eq!(
            out,
            ArtifactOwnership::Owned,
            "enabled() + edit signal must Own nested test subdirs (app/tests/...)"
        );
    }

    #[test]
    fn classify_ownership_rejects_nested_test_when_admission_disabled_without_signal() {
        // default() = disabled(). A pre-existing test under app/tests/
        // without any edit/scaffold signal must NOT promote to Owned (it
        // stays CandidateOnly, preserving the existing semantics for the
        // non-verifier callers).
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("app/tests")).unwrap();
        std::fs::write(dir.path().join("app/tests/test_a.py"), "").unwrap();
        let scope = single_root_scope();
        let out = classify_ownership(OwnershipInputs {
            work_root: dir.path(),
            relative_path: "app/tests/test_a.py",
            scope: &scope,
            edited_this_session: false,
            scaffold_changed: false,
            verifier_passed_in_scope: false,
            nested_test_admission: NestedTestAdmission::default(),
        });
        assert_eq!(
            out,
            ArtifactOwnership::CandidateOnly,
            "default() (disabled()) keeps pre-existing nested test as CandidateOnly"
        );
    }

    #[test]
    fn classify_ownership_preserves_symlink_containment_under_admission_enabled() {
        // DR2-003 invariant: workspace-relative / symlink containment /
        // ignored_top_dir / role re-confirm のいずれも admission flag で
        // bypass しない。symlink escape は enabled() でも OutOfScope。
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            let outside = tempdir().unwrap();
            std::fs::write(outside.path().join("evil_test.py"), "").unwrap();
            let work = tempdir().unwrap();
            std::fs::create_dir_all(work.path().join("app/tests")).unwrap();
            symlink(
                outside.path().join("evil_test.py"),
                work.path().join("app/tests/test_a.py"),
            )
            .unwrap();
            let scope = single_root_scope();
            let out = classify_ownership(OwnershipInputs {
                work_root: work.path(),
                relative_path: "app/tests/test_a.py",
                scope: &scope,
                edited_this_session: true,
                scaffold_changed: false,
                verifier_passed_in_scope: false,
                nested_test_admission: NestedTestAdmission::enabled(),
            });
            assert_eq!(
                out,
                ArtifactOwnership::OutOfScope,
                "symlink escape must remain OutOfScope even with enabled()"
            );
        }
    }

    #[test]
    fn classify_ownership_preserves_ignored_top_dir_under_admission_enabled() {
        // node_modules is always rejected regardless of admission flag.
        let dir = tempdir().unwrap();
        let scope = single_root_scope();
        let out = classify_ownership(OwnershipInputs {
            work_root: dir.path(),
            relative_path: "node_modules/foo/tests/test_a.py",
            scope: &scope,
            edited_this_session: true,
            scaffold_changed: false,
            verifier_passed_in_scope: false,
            nested_test_admission: NestedTestAdmission::enabled(),
        });
        assert_eq!(
            out,
            ArtifactOwnership::OutOfScope,
            "node_modules ignored_top_dir must remain OutOfScope even with enabled()"
        );
    }

    /// CB-004 regression: ensure the new no-signal `nested_test_admission`
    /// branch (not the existing `has_promotion_signal` branch) is exercised.
    /// Without any edit/scaffold/verifier signal, an existing nested test
    /// path should still promote to `Owned` when admission is `enabled()`.
    #[test]
    fn classify_ownership_admits_nested_test_without_signal_under_enabled_admission() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("app/tests")).unwrap();
        std::fs::write(dir.path().join("app/tests/test_existing.py"), "").unwrap();
        let scope = single_root_scope();
        let out = classify_ownership(OwnershipInputs {
            work_root: dir.path(),
            relative_path: "app/tests/test_existing.py",
            scope: &scope,
            edited_this_session: false,
            scaffold_changed: false,
            verifier_passed_in_scope: false,
            nested_test_admission: NestedTestAdmission::enabled(),
        });
        assert_eq!(
            out,
            ArtifactOwnership::Owned,
            "enabled() should Own existing nested test even without explicit signal"
        );
    }

    /// CB-003 regression: a dangling-symlink parent directory must not let
    /// a not-yet-created nested test leaf pass `Owned` classification via
    /// the no-signal `nested_test_admission` branch. `canonical_escape`
    /// returns `false` for missing leaves, so we must additionally walk
    /// the nearest existing ancestor through
    /// `nearest_existing_ancestor_within_work_root`.
    #[test]
    fn classify_ownership_rejects_symlinked_parent_with_missing_leaf_under_enabled_admission() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            let outside = tempdir().unwrap();
            std::fs::create_dir_all(outside.path().join("evil_tests")).unwrap();
            let work = tempdir().unwrap();
            std::fs::create_dir_all(work.path().join("app")).unwrap();
            // Make `app/tests` a symlink pointing to outside-of-work_root.
            symlink(
                outside.path().join("evil_tests"),
                work.path().join("app/tests"),
            )
            .unwrap();
            let scope = single_root_scope();
            // Leaf does NOT exist; canonical_escape returns false.
            let out = classify_ownership(OwnershipInputs {
                work_root: work.path(),
                relative_path: "app/tests/test_missing.py",
                scope: &scope,
                edited_this_session: false,
                scaffold_changed: false,
                verifier_passed_in_scope: false,
                nested_test_admission: NestedTestAdmission::enabled(),
            });
            assert_eq!(
                out,
                ArtifactOwnership::OutOfScope,
                "symlinked parent + missing leaf must remain OutOfScope even with enabled()"
            );
        }
    }

    #[test]
    fn classify_ownership_disabled_admission_matches_legacy_non_test_path() {
        // Regression guard: for a non-test path the admission flag has no
        // effect — same Owned/CandidateOnly decision as before iteration-2.
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("README.md"), "").unwrap();
        let scope = single_root_scope();

        let with_disabled = classify_ownership(OwnershipInputs {
            work_root: dir.path(),
            relative_path: "README.md",
            scope: &scope,
            edited_this_session: true,
            scaffold_changed: false,
            verifier_passed_in_scope: false,
            nested_test_admission: NestedTestAdmission::disabled(),
        });
        let with_enabled = classify_ownership(OwnershipInputs {
            work_root: dir.path(),
            relative_path: "README.md",
            scope: &scope,
            edited_this_session: true,
            scaffold_changed: false,
            verifier_passed_in_scope: false,
            nested_test_admission: NestedTestAdmission::enabled(),
        });
        assert_eq!(with_disabled, ArtifactOwnership::Owned);
        assert_eq!(with_enabled, ArtifactOwnership::Owned);
    }
}
