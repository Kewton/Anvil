//! Issue #990 (parent #988, Issue B): `NoProgressRecoveryPolicy`.
//!
//! Connects the `TargetReassessmentRequired` lifecycle event (#987) to a
//! deterministic, **failure-cluster-scoped** ban of repair targets and roles,
//! a forced role switch (`required_next_roles`), and an internal
//! classification of `repair_exhausted` (`same_target_same_diagnostic` /
//! `same_role_no_progress` / `operator_missing`).
//!
//! Design constraints (issue non-functional requirements):
//!   - The policy is controller state, **not** prompt-only control: it stores
//!     closed enum values and sanitized paths, never raw LLM prose.
//!   - Bans are scoped to a failure cluster identifier and never leak across
//!     clusters.
//!   - The policy is generic over [`ArtifactRole`]; it embeds no
//!     runtime-specific (Rust / Node / docs / data) branches.
//!
//! Visibility: every export is `pub(super)` and must not be re-exported from
//! `src/agent/loop_run/mod.rs` (CLAUDE.md DR3-001).

use crate::session::feedback::mask_secrets;

use super::task_contract::ArtifactRole;

/// FIFO cap on retained no-progress observations.
const MAX_NO_PROGRESS_OBSERVATIONS: usize = 32;
/// FIFO cap on banned targets.
const MAX_BANNED_TARGETS: usize = 32;
/// FIFO cap on banned roles.
const MAX_BANNED_ROLES: usize = 8;
/// A role is banned (and a role switch forced) after this many no-progress
/// observations under the same `(scope, role)`. The first observation bans the
/// *target*; the second — a second no-progress under the same role, on the same
/// or a different path — bans the *role*.
const ROLE_NO_PROGRESS_BAN_THRESHOLD: usize = 2;
/// Defense-in-depth char cap for ban scope identifiers (inputs are already
/// sanitized upstream).
const NO_PROGRESS_SCOPE_CHAR_CAP: usize = 240;
/// Defense-in-depth char cap for ban target paths.
const NO_PROGRESS_PATH_CHAR_CAP: usize = 240;

/// Closed internal reasons for the `repair_exhausted` terminal state. The
/// legacy `repair_exhausted` label is retained as a projection; this enum lets
/// the safe-stop report surface *why* convergence failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum NoProgressExhaustionReason {
    /// The same target kept producing the same diagnostic after repair.
    SameTargetSameDiagnostic,
    /// A role made no progress repeatedly and an alternate role still remained.
    SameRoleNoProgress,
    /// Every candidate role for the cluster is banned — no operator / role
    /// remains to make progress.
    OperatorMissing,
}

impl NoProgressExhaustionReason {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::SameTargetSameDiagnostic => "same_target_same_diagnostic",
            Self::SameRoleNoProgress => "same_role_no_progress",
            Self::OperatorMissing => "operator_missing",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NoProgressObservation {
    scope: String,
    role: ArtifactRole,
    path: String,
}

/// A repair target banned for a specific failure cluster scope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct BannedTarget {
    pub(super) scope: String,
    pub(super) role: ArtifactRole,
    pub(super) path: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BannedRole {
    scope: String,
    role: ArtifactRole,
}

/// Failure-cluster-scoped no-progress recovery state. Bounded, in-memory only
/// (not persisted into sessions).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct NoProgressRecoveryPolicy {
    observations: Vec<NoProgressObservation>,
    banned_targets: Vec<BannedTarget>,
    banned_roles: Vec<BannedRole>,
}

impl NoProgressRecoveryPolicy {
    pub(super) fn new() -> Self {
        Self::default()
    }

    fn sanitize_scope(scope: &str) -> String {
        mask_secrets(scope)
            .chars()
            .take(NO_PROGRESS_SCOPE_CHAR_CAP)
            .collect()
    }

    fn sanitize_path(path: &str) -> String {
        mask_secrets(path)
            .chars()
            .take(NO_PROGRESS_PATH_CHAR_CAP)
            .collect()
    }

    /// Record one no-progress observation for `(scope, role, path)`.
    ///
    /// The target is banned immediately because the `TargetReassessmentRequired`
    /// event (#987) only fires after the same target produced the same
    /// diagnostic with no progress. The role is additionally banned (forcing a
    /// role switch) once `ROLE_NO_PROGRESS_BAN_THRESHOLD` no-progress
    /// observations accrue under the same `(scope, role)`.
    pub(super) fn record_no_progress(&mut self, scope: &str, role: ArtifactRole, path: &str) {
        let scope = Self::sanitize_scope(scope);
        if scope.is_empty() {
            return;
        }
        let path = Self::sanitize_path(path);
        self.push_banned_target(scope.clone(), role, path.clone());
        self.push_observation(scope.clone(), role, path);
        let role_no_progress = self
            .observations
            .iter()
            .filter(|obs| obs.scope == scope && obs.role == role)
            .count();
        if role_no_progress >= ROLE_NO_PROGRESS_BAN_THRESHOLD {
            self.push_banned_role(scope, role);
        }
    }

    fn push_observation(&mut self, scope: String, role: ArtifactRole, path: String) {
        if self.observations.len() >= MAX_NO_PROGRESS_OBSERVATIONS {
            self.observations.remove(0);
        }
        self.observations
            .push(NoProgressObservation { scope, role, path });
    }

    fn push_banned_target(&mut self, scope: String, role: ArtifactRole, path: String) {
        if self
            .banned_targets
            .iter()
            .any(|b| b.scope == scope && b.role == role && b.path == path)
        {
            return;
        }
        if self.banned_targets.len() >= MAX_BANNED_TARGETS {
            self.banned_targets.remove(0);
        }
        self.banned_targets.push(BannedTarget { scope, role, path });
    }

    fn push_banned_role(&mut self, scope: String, role: ArtifactRole) {
        if self
            .banned_roles
            .iter()
            .any(|b| b.scope == scope && b.role == role)
        {
            return;
        }
        if self.banned_roles.len() >= MAX_BANNED_ROLES {
            self.banned_roles.remove(0);
        }
        self.banned_roles.push(BannedRole { scope, role });
    }

    pub(super) fn is_empty(&self) -> bool {
        self.banned_targets.is_empty() && self.banned_roles.is_empty()
    }

    pub(super) fn is_role_banned(&self, scope: &str, role: ArtifactRole) -> bool {
        let scope = Self::sanitize_scope(scope);
        self.banned_roles
            .iter()
            .any(|b| b.scope == scope && b.role == role)
    }

    /// Whether a concrete `(role, path)` selection is banned for `scope` — true
    /// when the role is banned (forced switch) or the exact target is banned.
    pub(super) fn is_selection_banned(&self, scope: &str, role: ArtifactRole, path: &str) -> bool {
        if self.is_role_banned(scope, role) {
            return true;
        }
        let scope = Self::sanitize_scope(scope);
        let path = Self::sanitize_path(path);
        self.banned_targets
            .iter()
            .any(|b| b.scope == scope && b.role == role && b.path == path)
    }

    /// Banned targets for `scope`, in insertion order.
    pub(super) fn banned_targets_for(&self, scope: &str) -> Vec<&BannedTarget> {
        let scope = Self::sanitize_scope(scope);
        self.banned_targets
            .iter()
            .filter(|b| b.scope == scope)
            .collect()
    }

    /// Banned roles for `scope`, in insertion order.
    pub(super) fn banned_roles_for(&self, scope: &str) -> Vec<ArtifactRole> {
        let scope = Self::sanitize_scope(scope);
        self.banned_roles
            .iter()
            .filter(|b| b.scope == scope)
            .map(|b| b.role)
            .collect()
    }

    /// Candidate roles minus the banned roles, preserving the caller's order
    /// and de-duplicating. This is the forced role-switch ordering surfaced to
    /// the diagnostic payload.
    pub(super) fn required_next_roles(
        &self,
        scope: &str,
        candidate_roles: &[ArtifactRole],
    ) -> Vec<ArtifactRole> {
        let mut out: Vec<ArtifactRole> = Vec::new();
        for &role in candidate_roles {
            if !out.contains(&role) && !self.is_role_banned(scope, role) {
                out.push(role);
            }
        }
        out
    }

    /// Structured, already-masked diagnostic payload carrying
    /// `banned_targets` / `banned_roles` / `required_next_roles` for `scope`.
    /// Masking happens here (not at the caller) so recovery renderers never
    /// touch raw paths.
    pub(super) fn diagnostic_payload(
        &self,
        scope: &str,
        candidate_roles: &[ArtifactRole],
    ) -> serde_json::Value {
        let banned_targets = self
            .banned_targets_for(scope)
            .into_iter()
            .map(|b| {
                serde_json::json!({
                    "role": b.role.label(),
                    "path": mask_secrets(&b.path),
                })
            })
            .collect::<Vec<_>>();
        let banned_roles = self
            .banned_roles_for(scope)
            .into_iter()
            .map(|role| role.label())
            .collect::<Vec<_>>();
        let required_next_roles = self
            .required_next_roles(scope, candidate_roles)
            .into_iter()
            .map(|role| role.label())
            .collect::<Vec<_>>();
        serde_json::json!({
            "banned_targets": banned_targets,
            "banned_roles": banned_roles,
            "required_next_roles": required_next_roles,
        })
    }

    /// Classify why `repair_exhausted` was reached from the no-progress signals
    /// recorded for `scope`. Returns `None` when the no-progress policy did not
    /// contribute (the caller keeps the legacy classification).
    pub(super) fn classify_exhaustion(
        &self,
        scope: &str,
        candidate_roles: &[ArtifactRole],
    ) -> Option<NoProgressExhaustionReason> {
        let sanitized = Self::sanitize_scope(scope);
        let role_banned = self.banned_roles.iter().any(|b| b.scope == sanitized);
        let target_banned = self.banned_targets.iter().any(|b| b.scope == sanitized);
        if !role_banned && !target_banned {
            return None;
        }
        if self.required_next_roles(scope, candidate_roles).is_empty() {
            return Some(NoProgressExhaustionReason::OperatorMissing);
        }
        if role_banned {
            Some(NoProgressExhaustionReason::SameRoleNoProgress)
        } else {
            Some(NoProgressExhaustionReason::SameTargetSameDiagnostic)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCOPE_A: &str = "cluster-a";
    const SCOPE_B: &str = "cluster-b";

    #[test]
    fn same_target_same_diagnostic_bans_target_on_first_no_progress() {
        // AC1: a same-target / same-diagnostic no-progress observation fires a
        // target ban immediately (the #987 event already encodes repetition).
        let mut policy = NoProgressRecoveryPolicy::new();
        policy.record_no_progress(SCOPE_A, ArtifactRole::Implementation, "src/lib.rs");

        assert!(policy.is_selection_banned(SCOPE_A, ArtifactRole::Implementation, "src/lib.rs"));
        assert!(!policy.is_role_banned(SCOPE_A, ArtifactRole::Implementation));
        let banned = policy.banned_targets_for(SCOPE_A);
        assert_eq!(banned.len(), 1);
        assert_eq!(banned[0].path, "src/lib.rs");
    }

    #[test]
    fn second_no_progress_under_same_role_bans_role_and_forces_switch() {
        // AC2: two no-progress observations under the same role ban the role and
        // force a switch — `required_next_roles` drops the banned role.
        let mut policy = NoProgressRecoveryPolicy::new();
        policy.record_no_progress(SCOPE_A, ArtifactRole::Test, "tests/a.rs");
        policy.record_no_progress(SCOPE_A, ArtifactRole::Test, "tests/b.rs");

        assert!(policy.is_role_banned(SCOPE_A, ArtifactRole::Test));
        let next = policy
            .required_next_roles(SCOPE_A, &[ArtifactRole::Test, ArtifactRole::Implementation]);
        assert_eq!(next, vec![ArtifactRole::Implementation]);
        // The banned role's targets are all rejected via the role ban.
        assert!(policy.is_selection_banned(SCOPE_A, ArtifactRole::Test, "tests/never_seen.rs"));
    }

    #[test]
    fn bans_do_not_leak_across_clusters() {
        let mut policy = NoProgressRecoveryPolicy::new();
        policy.record_no_progress(SCOPE_A, ArtifactRole::Implementation, "src/lib.rs");
        policy.record_no_progress(SCOPE_A, ArtifactRole::Implementation, "src/lib.rs");

        assert!(policy.is_role_banned(SCOPE_A, ArtifactRole::Implementation));
        // A different cluster scope is unaffected.
        assert!(!policy.is_role_banned(SCOPE_B, ArtifactRole::Implementation));
        assert!(!policy.is_selection_banned(SCOPE_B, ArtifactRole::Implementation, "src/lib.rs"));
        assert!(policy.banned_targets_for(SCOPE_B).is_empty());
    }

    #[test]
    fn diagnostic_payload_carries_structured_ban_data() {
        let mut policy = NoProgressRecoveryPolicy::new();
        policy.record_no_progress(SCOPE_A, ArtifactRole::Test, "tests/a.rs");
        policy.record_no_progress(SCOPE_A, ArtifactRole::Test, "tests/b.rs");

        let payload =
            policy.diagnostic_payload(SCOPE_A, &[ArtifactRole::Test, ArtifactRole::Implementation]);
        let banned_targets = payload["banned_targets"].as_array().unwrap();
        assert_eq!(banned_targets.len(), 2);
        assert_eq!(banned_targets[0]["role"], "test");
        assert_eq!(banned_targets[0]["path"], "tests/a.rs");
        let banned_roles = payload["banned_roles"].as_array().unwrap();
        assert_eq!(banned_roles, &vec![serde_json::json!("test")]);
        let required = payload["required_next_roles"].as_array().unwrap();
        assert_eq!(required, &vec![serde_json::json!("implementation")]);
    }

    #[test]
    fn classify_exhaustion_distinguishes_reasons() {
        // target-only ban -> same_target_same_diagnostic (alternate role remains)
        let mut target_only = NoProgressRecoveryPolicy::new();
        target_only.record_no_progress(SCOPE_A, ArtifactRole::Implementation, "src/lib.rs");
        assert_eq!(
            target_only
                .classify_exhaustion(SCOPE_A, &[ArtifactRole::Implementation, ArtifactRole::Test]),
            Some(NoProgressExhaustionReason::SameTargetSameDiagnostic)
        );

        // role ban with an alternate role -> same_role_no_progress
        let mut role_ban = NoProgressRecoveryPolicy::new();
        role_ban.record_no_progress(SCOPE_A, ArtifactRole::Test, "tests/a.rs");
        role_ban.record_no_progress(SCOPE_A, ArtifactRole::Test, "tests/b.rs");
        assert_eq!(
            role_ban
                .classify_exhaustion(SCOPE_A, &[ArtifactRole::Test, ArtifactRole::Implementation]),
            Some(NoProgressExhaustionReason::SameRoleNoProgress)
        );

        // role ban with no alternate role -> operator_missing
        assert_eq!(
            role_ban.classify_exhaustion(SCOPE_A, &[ArtifactRole::Test]),
            Some(NoProgressExhaustionReason::OperatorMissing)
        );

        // no bans -> None (legacy classification stands)
        let empty = NoProgressRecoveryPolicy::new();
        assert_eq!(
            empty.classify_exhaustion(SCOPE_A, &[ArtifactRole::Implementation]),
            None
        );
    }

    #[test]
    fn reason_labels_are_stable() {
        assert_eq!(
            NoProgressExhaustionReason::SameTargetSameDiagnostic.as_str(),
            "same_target_same_diagnostic"
        );
        assert_eq!(
            NoProgressExhaustionReason::SameRoleNoProgress.as_str(),
            "same_role_no_progress"
        );
        assert_eq!(
            NoProgressExhaustionReason::OperatorMissing.as_str(),
            "operator_missing"
        );
    }

    #[test]
    fn empty_scope_is_ignored() {
        let mut policy = NoProgressRecoveryPolicy::new();
        policy.record_no_progress("", ArtifactRole::Implementation, "src/lib.rs");
        assert!(policy.is_empty());
    }

    #[test]
    fn banned_target_records_are_deduplicated() {
        let mut policy = NoProgressRecoveryPolicy::new();
        policy.record_no_progress(SCOPE_A, ArtifactRole::Implementation, "src/lib.rs");
        policy.record_no_progress(SCOPE_A, ArtifactRole::Implementation, "src/lib.rs");
        // Same (scope, role, path) does not duplicate the ban entry.
        assert_eq!(policy.banned_targets_for(SCOPE_A).len(), 1);
        // ... but two observations under the same role still ban the role.
        assert!(policy.is_role_banned(SCOPE_A, ArtifactRole::Implementation));
    }
}
