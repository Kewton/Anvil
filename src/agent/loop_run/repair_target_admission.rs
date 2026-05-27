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
