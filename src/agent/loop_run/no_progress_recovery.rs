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

/// Issue #1006: the deterministic *forward* strategy the controller should
/// switch to when the same failure key keeps recurring. Where
/// [`NoProgressExhaustionReason`] explains *why convergence failed*, this enum
/// decides *what to try next* — turning the policy from fail-fast into
/// strategy-switching. Closed, `Copy`, and generic over [`ArtifactRole`] (no
/// runtime-specific branches): the only inputs are the same-failure-key
/// repetition count, the failing role, the alternate roles, and whether the
/// failure is an inter-artifact contract conflict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RecoveryStrategy {
    /// `same_failure_once`: try the deterministic operator again (no ban yet).
    RetryDeterministicOperator,
    /// `same_failure_twice`: the previous target role is banned — switch to an
    /// alternate role rather than repeating the same role's repair.
    SwitchTargetRole,
    /// A setup-role no-progress never loops back into implementation repair; it
    /// routes to the evidence-binding / scaffold-rebuild side (AC2). Role-driven,
    /// not runtime-specific.
    RouteToEvidenceBinding,
    /// `same_failure_three_times` with an inter-artifact (e.g. test/impl)
    /// disagreement: escalate to contract arbitration (AC3).
    EscalateToContractConflict,
    /// `same_failure_three_times` with no inter-artifact conflict and an
    /// alternate role still available: rebuild via scaffold rather than keep
    /// patching.
    EscalateToScaffoldRebuild,
    /// No alternate role / operator remains for the cluster — the structured
    /// terminal reason (AC4). Keeps the `repair_exhausted` projection but records
    /// *why* no further strategy exists.
    OperatorMissing,
}

impl RecoveryStrategy {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::RetryDeterministicOperator => "retry_deterministic_operator",
            Self::SwitchTargetRole => "switch_target_role",
            Self::RouteToEvidenceBinding => "route_to_evidence_binding",
            Self::EscalateToContractConflict => "escalate_to_contract_conflict",
            Self::EscalateToScaffoldRebuild => "escalate_to_scaffold_rebuild",
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

    /// Issue #1006: how many no-progress observations have accrued for `scope`,
    /// across all roles. This is the repetition count of the *same failure key*
    /// (the cluster scope) and drives the strategy-switch escalation tier.
    pub(super) fn scope_no_progress_count(&self, scope: &str) -> usize {
        let scope = Self::sanitize_scope(scope);
        self.observations
            .iter()
            .filter(|obs| obs.scope == scope)
            .count()
    }

    /// Issue #1006: the role of the most recent no-progress observation for
    /// `scope`. Used to decide the role-driven strategy (e.g. a setup failure
    /// routes to evidence binding). `None` when no observation exists.
    pub(super) fn latest_no_progress_role(&self, scope: &str) -> Option<ArtifactRole> {
        let scope = Self::sanitize_scope(scope);
        self.observations
            .iter()
            .rev()
            .find(|obs| obs.scope == scope)
            .map(|obs| obs.role)
    }

    /// Issue #1006: the deterministic forward strategy for `scope`, given the
    /// failing `role`, the cluster's `candidate_roles`, and whether the failure
    /// is an inter-artifact contract conflict.
    ///
    /// The decision is keyed purely on the same-failure-key repetition count and
    /// the closed role/conflict signals — there are no runtime-specific
    /// branches. The `OperatorMissing` arm stays coherent with
    /// [`Self::classify_exhaustion`] (both fire when no alternate role remains).
    pub(super) fn classify_strategy(
        &self,
        scope: &str,
        role: ArtifactRole,
        candidate_roles: &[ArtifactRole],
        has_contract_conflict: bool,
    ) -> RecoveryStrategy {
        let count = self.scope_no_progress_count(scope);
        if count == 0 {
            // No no-progress recorded yet: a deterministic operator attempt is
            // the first strategy (same_failure_once is the next tier).
            return RecoveryStrategy::RetryDeterministicOperator;
        }
        // AC2: a setup failure never loops back into implementation repair; it
        // routes to the evidence-binding / scaffold-rebuild side. Role-driven so
        // it stays runtime-agnostic.
        if role == ArtifactRole::Setup {
            return RecoveryStrategy::RouteToEvidenceBinding;
        }
        // AC4: no alternate role / operator remains — the structured terminal
        // reason, regardless of how many times we have retried.
        if self.required_next_roles(scope, candidate_roles).is_empty() {
            return RecoveryStrategy::OperatorMissing;
        }
        match count {
            1 => RecoveryStrategy::RetryDeterministicOperator,
            2 => RecoveryStrategy::SwitchTargetRole,
            _ => {
                if has_contract_conflict {
                    // AC3: an inter-artifact (test/impl) disagreement escalates
                    // to contract arbitration.
                    RecoveryStrategy::EscalateToContractConflict
                } else {
                    RecoveryStrategy::EscalateToScaffoldRebuild
                }
            }
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

    // --- Issue #1006: deterministic strategy-switch decision table -----------

    #[test]
    fn scope_count_and_latest_role_track_observations() {
        let mut policy = NoProgressRecoveryPolicy::new();
        assert_eq!(policy.scope_no_progress_count(SCOPE_A), 0);
        assert_eq!(policy.latest_no_progress_role(SCOPE_A), None);

        policy.record_no_progress(SCOPE_A, ArtifactRole::Implementation, "src/lib.rs");
        policy.record_no_progress(SCOPE_A, ArtifactRole::Test, "tests/a.rs");
        assert_eq!(policy.scope_no_progress_count(SCOPE_A), 2);
        // The most recent observation's role.
        assert_eq!(
            policy.latest_no_progress_role(SCOPE_A),
            Some(ArtifactRole::Test)
        );
        // A different cluster scope is unaffected.
        assert_eq!(policy.scope_no_progress_count(SCOPE_B), 0);
    }

    #[test]
    fn strategy_same_failure_once_retries_deterministic_operator() {
        let mut policy = NoProgressRecoveryPolicy::new();
        policy.record_no_progress(SCOPE_A, ArtifactRole::Implementation, "src/lib.rs");
        assert_eq!(
            policy.classify_strategy(
                SCOPE_A,
                ArtifactRole::Implementation,
                &[ArtifactRole::Implementation, ArtifactRole::Test],
                false,
            ),
            RecoveryStrategy::RetryDeterministicOperator
        );
    }

    #[test]
    fn strategy_same_failure_twice_switches_target_role() {
        // AC1: a second no-progress under the same role bans the role and the
        // strategy switches off it.
        let mut policy = NoProgressRecoveryPolicy::new();
        policy.record_no_progress(SCOPE_A, ArtifactRole::Implementation, "src/lib.rs");
        policy.record_no_progress(SCOPE_A, ArtifactRole::Implementation, "src/other.rs");
        assert!(policy.is_role_banned(SCOPE_A, ArtifactRole::Implementation));
        assert_eq!(
            policy.classify_strategy(
                SCOPE_A,
                ArtifactRole::Implementation,
                &[ArtifactRole::Implementation, ArtifactRole::Test],
                false,
            ),
            RecoveryStrategy::SwitchTargetRole
        );
    }

    #[test]
    fn strategy_setup_routes_to_evidence_binding() {
        // AC2: a setup-role no-progress routes to evidence binding / scaffold,
        // never back to implementation repair — even when implementation is an
        // available candidate role.
        let mut policy = NoProgressRecoveryPolicy::new();
        policy.record_no_progress(SCOPE_A, ArtifactRole::Setup, "Cargo.toml");
        let strategy = policy.classify_strategy(
            SCOPE_A,
            ArtifactRole::Setup,
            &[ArtifactRole::Setup, ArtifactRole::Implementation],
            false,
        );
        assert_eq!(strategy, RecoveryStrategy::RouteToEvidenceBinding);
        assert_ne!(strategy, RecoveryStrategy::SwitchTargetRole);
    }

    #[test]
    fn strategy_three_times_with_conflict_escalates_to_contract() {
        // AC3: a recurring inter-artifact (test/impl) conflict escalates to
        // contract arbitration.
        let mut policy = NoProgressRecoveryPolicy::new();
        policy.record_no_progress(SCOPE_A, ArtifactRole::Implementation, "src/lib.rs");
        policy.record_no_progress(SCOPE_A, ArtifactRole::Implementation, "src/lib.rs");
        policy.record_no_progress(SCOPE_A, ArtifactRole::Implementation, "src/lib.rs");
        assert_eq!(
            policy.classify_strategy(
                SCOPE_A,
                ArtifactRole::Implementation,
                &[ArtifactRole::Implementation, ArtifactRole::Test],
                true,
            ),
            RecoveryStrategy::EscalateToContractConflict
        );
    }

    #[test]
    fn strategy_three_times_without_conflict_rebuilds_scaffold() {
        let mut policy = NoProgressRecoveryPolicy::new();
        policy.record_no_progress(SCOPE_A, ArtifactRole::Implementation, "src/lib.rs");
        policy.record_no_progress(SCOPE_A, ArtifactRole::Implementation, "src/lib.rs");
        policy.record_no_progress(SCOPE_A, ArtifactRole::Implementation, "src/lib.rs");
        assert_eq!(
            policy.classify_strategy(
                SCOPE_A,
                ArtifactRole::Implementation,
                &[ArtifactRole::Implementation, ArtifactRole::Test],
                false,
            ),
            RecoveryStrategy::EscalateToScaffoldRebuild
        );
    }

    #[test]
    fn strategy_operator_missing_when_no_alternate_role() {
        // AC4: a single-role cluster exhausting its only role yields the
        // structured operator-missing strategy.
        let mut policy = NoProgressRecoveryPolicy::new();
        policy.record_no_progress(SCOPE_A, ArtifactRole::Implementation, "src/lib.rs");
        policy.record_no_progress(SCOPE_A, ArtifactRole::Implementation, "src/lib.rs");
        // Only Implementation is a candidate, and it is now role-banned.
        assert_eq!(
            policy.classify_strategy(
                SCOPE_A,
                ArtifactRole::Implementation,
                &[ArtifactRole::Implementation],
                false,
            ),
            RecoveryStrategy::OperatorMissing
        );
        // Coherent with the terminal classification.
        assert_eq!(
            policy.classify_exhaustion(SCOPE_A, &[ArtifactRole::Implementation]),
            Some(NoProgressExhaustionReason::OperatorMissing)
        );
    }

    #[test]
    fn strategy_labels_are_stable() {
        assert_eq!(
            RecoveryStrategy::RetryDeterministicOperator.as_str(),
            "retry_deterministic_operator"
        );
        assert_eq!(
            RecoveryStrategy::SwitchTargetRole.as_str(),
            "switch_target_role"
        );
        assert_eq!(
            RecoveryStrategy::RouteToEvidenceBinding.as_str(),
            "route_to_evidence_binding"
        );
        assert_eq!(
            RecoveryStrategy::EscalateToContractConflict.as_str(),
            "escalate_to_contract_conflict"
        );
        assert_eq!(
            RecoveryStrategy::EscalateToScaffoldRebuild.as_str(),
            "escalate_to_scaffold_rebuild"
        );
        assert_eq!(
            RecoveryStrategy::OperatorMissing.as_str(),
            "operator_missing"
        );
    }
}
