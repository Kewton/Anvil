use std::path::Path;

/// Owned admission context for every `RecoveryTargetHint` promotion path.
/// Path-local predicates ensure ownership is evaluated for the candidate path,
/// not by a turn-global signal that could promote unrelated files.
pub(super) struct RepairTargetAdmissionContext<'a> {
    pub(super) work_root: &'a Path,
    pub(super) scope: &'a super::task_workspace_scope::TaskWorkspaceScope,
    pub(super) edited_this_session_for: &'a dyn Fn(&str) -> bool,
    pub(super) scaffold_changed_for: &'a dyn Fn(&str) -> bool,
}

#[cfg(test)]
fn admission_always_true(_: &str) -> bool {
    true
}

#[cfg(test)]
pub(super) fn admission_always_false(_: &str) -> bool {
    false
}

impl<'a> RepairTargetAdmissionContext<'a> {
    /// Test-only helper for fixtures that need owned-path admission while still
    /// routing through the same SSOT as production.
    #[cfg(test)]
    pub(super) fn owned_for_test(
        work_root: &'a Path,
        scope: &'a super::task_workspace_scope::TaskWorkspaceScope,
    ) -> Self {
        Self {
            work_root,
            scope,
            edited_this_session_for: &admission_always_true,
            scaffold_changed_for: &admission_always_false,
        }
    }
}

/// Owned admission gate for any `RecoveryTargetHint` heading downstream into
/// the repair-job pipeline.
pub(super) fn admit_repair_target_hint(
    hint: super::task_contract::RecoveryTargetHint,
    ctx: &RepairTargetAdmissionContext<'_>,
) -> Option<super::task_contract::RecoveryTargetHint> {
    let path = hint.path.clone();
    let inputs = super::artifact_ownership::OwnershipInputs {
        work_root: ctx.work_root,
        relative_path: &path,
        scope: ctx.scope,
        edited_this_session: (ctx.edited_this_session_for)(&path),
        scaffold_changed: (ctx.scaffold_changed_for)(&path),
        verifier_passed_in_scope: false,
        nested_test_admission: super::artifact_ownership::NestedTestAdmission::default(),
    };
    match super::artifact_ownership::classify_ownership(inputs) {
        super::artifact_ownership::ArtifactOwnership::Owned => Some(hint),
        super::artifact_ownership::ArtifactOwnership::CandidateOnly
        | super::artifact_ownership::ArtifactOwnership::OutOfScope => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn hint(path: &str) -> super::super::task_contract::RecoveryTargetHint {
        super::super::task_contract::RecoveryTargetHint {
            role: super::super::task_contract::ArtifactRole::Implementation,
            path: path.to_string(),
            reason: "test".to_string(),
        }
    }

    #[test]
    fn admission_accepts_in_scope_edited_hint() {
        let temp = tempdir().unwrap();
        let work_root = temp.path();
        std::fs::create_dir_all(work_root.join("app")).unwrap();
        std::fs::write(work_root.join("app/main.py"), "x = 1\n").unwrap();
        let scope = super::super::task_workspace_scope::TaskWorkspaceScope::detect(work_root, "");
        let edited = |path: &str| path == "app/main.py";
        let unchanged = |_: &str| false;
        let ctx = RepairTargetAdmissionContext {
            work_root,
            scope: &scope,
            edited_this_session_for: &edited,
            scaffold_changed_for: &unchanged,
        };

        assert_eq!(
            admit_repair_target_hint(hint("app/main.py"), &ctx)
                .map(|hint| hint.path)
                .as_deref(),
            Some("app/main.py")
        );
    }

    #[test]
    fn admission_rejects_candidate_only_hint() {
        let temp = tempdir().unwrap();
        let work_root = temp.path();
        std::fs::create_dir_all(work_root.join("app")).unwrap();
        std::fs::write(work_root.join("app/main.py"), "x = 1\n").unwrap();
        let scope = super::super::task_workspace_scope::TaskWorkspaceScope::detect(work_root, "");
        let unchanged = |_: &str| false;
        let ctx = RepairTargetAdmissionContext {
            work_root,
            scope: &scope,
            edited_this_session_for: &unchanged,
            scaffold_changed_for: &unchanged,
        };

        assert!(admit_repair_target_hint(hint("app/main.py"), &ctx).is_none());
    }

    #[test]
    fn admission_rejects_edited_hint_outside_ambiguous_parent_scope() {
        let temp = tempdir().unwrap();
        let work_root = temp.path();
        std::fs::create_dir_all(work_root.join("packages/a/src")).unwrap();
        std::fs::create_dir_all(work_root.join("packages/b/src")).unwrap();
        std::fs::write(work_root.join("packages/a/Cargo.toml"), "[package]\n").unwrap();
        std::fs::write(work_root.join("packages/b/Cargo.toml"), "[package]\n").unwrap();
        std::fs::write(work_root.join("packages/a/src/lib.rs"), "").unwrap();
        let scope = super::super::task_workspace_scope::TaskWorkspaceScope::detect(work_root, "");
        let edited = |path: &str| path == "packages/a/src/lib.rs";
        let unchanged = |_: &str| false;
        let ctx = RepairTargetAdmissionContext {
            work_root,
            scope: &scope,
            edited_this_session_for: &edited,
            scaffold_changed_for: &unchanged,
        };

        assert!(admit_repair_target_hint(hint("packages/a/src/lib.rs"), &ctx).is_none());
    }
}
