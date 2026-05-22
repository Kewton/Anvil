//! Issue #652: `ArtifactCompletionJob` + role-specific retry budget.
//!
//! This module is the SSOT for tracking the in-flight "make the required
//! artifact appear" task for the current turn. It owns:
//!
//! - the canonicalized target path (validated via the `task_workspace_scope`
//!   + `artifact_ownership` SSOTs — never re-canonicalized here),
//! - the role-specific retry budget (`ARTIFACT_COMPLETION_ATTEMPT_LIMIT`),
//! - the failure attempt history (`WrongTarget` / `NoTool` / `ProseOnly` /
//!   `RolePolicyViolation`) — each attempt's user-controlled strings are run
//!   through the `mask_secrets` → length cap → control-char neutralization
//!   pipeline at construction time so consumers never observe raw input.
//!
//! ## Visibility (DR3-001)
//!
//! This module is intentionally **private** — `loop_run.rs` declares it as
//! `mod artifact_completion_job;` and never re-exports any name. `turn.rs`
//! is the only in-crate consumer via `super::artifact_completion_job::*`.
//!
//! ## Security (DR4-001 / DR4-002 / DR4-003)
//!
//! - All string fields are sanitized at construction time and never refreshed
//!   from raw LLM output afterwards.
//! - `failure_snapshot()` re-applies `mask_payload_inplace` as a defensive
//!   final-defence pass (multi-mask is harmless).
//! - The constructor refuses absolute paths, `..` components, control
//!   characters, ignored top dirs, symlink escapes, and any path the
//!   `task_workspace_scope` + `artifact_ownership` SSOTs reject — there is no
//!   custom `fs::canonicalize` here.
//! - The total budget is fixed at `ARTIFACT_COMPLETION_ATTEMPT_LIMIT` and
//!   cannot be inflated by user input or LLM output (DR4-003).
//!
//! ## SSOT cross-references
//!
//! - cluster key: `semantic_failure::build_failure_cluster_from_observation`
//!   produces a `FailureClusterKey` newtype that is stored verbatim — never
//!   downcast to a raw `String`.
//! - canonicalize / scope: `task_workspace_scope::TaskWorkspaceScope` +
//!   `artifact_ownership::classify_ownership`.
//! - role: `task_contract::ArtifactRole` (existing 4-variant SSOT, no new
//!   variant introduced here).

use std::path::Path;

use super::artifact_ownership::{
    ArtifactOwnership, OwnershipInputs, classify_ownership,
    nearest_existing_ancestor_within_work_root,
};
use super::semantic_failure::FailureClusterKey;
use super::task_contract::{ArtifactRole, RecoveryTarget, RecoveryTargetHint};
use super::task_workspace_scope::TaskWorkspaceScope;
use crate::logging::mask_payload_inplace;
use crate::session::feedback::mask_secrets;

/// Role-specific retry budget. Externally immutable; the constructor pins
/// this on every new `ArtifactCompletionJob` so user / LLM input can never
/// inflate it (DR4-003).
pub(super) const ARTIFACT_COMPLETION_ATTEMPT_LIMIT: usize = 3;

/// Upper bound on the number of `actual_actions` retained per recorded
/// attempt. Extra elements are dropped at `ArtifactAttemptOutcome::new()`
/// time — DoS / log-flood defence.
pub(super) const MAX_ARTIFACT_ACTIONS_PER_ATTEMPT: usize = 16;

/// Byte cap applied to each individual `actual_actions` element AND the
/// stored `expected_target` / `reason` (DR4-001 length cap). Truncation is
/// UTF-8-safe (char boundary preserved) and applied AFTER `mask_secrets` so
/// secret patterns can never sneak past via truncation.
pub(super) const MAX_ARTIFACT_ACTION_TEXT_BYTES: usize = 4096;

/// Write tool policy attached to an active job.
///
/// `target_only` is always `true` in this Issue's scope; the field exists so
/// future Issues (e.g. wider-scope refactor jobs) can flip it without
/// reshaping the struct. Constructors are `target_create_only()` /
/// `target_modify_only()` — there is no raw `bool` constructor on purpose.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct AllowedWriteActions {
    allow_create: bool,
    allow_modify: bool,
    target_only: bool,
}

impl AllowedWriteActions {
    /// Missing-target creation only (Write). `target_only` is `true`.
    pub(super) fn target_create_only() -> Self {
        Self {
            allow_create: true,
            allow_modify: false,
            target_only: true,
        }
    }

    /// Existing-target modification only (Edit / Write). `target_only` is
    /// `true`.
    pub(super) fn target_modify_only() -> Self {
        Self {
            allow_create: false,
            allow_modify: true,
            target_only: true,
        }
    }

    // ----------------------- accessors -----------------------
    //
    // Issue #652 surface: today turn.rs consumes the projection-derived
    // `EffectiveToolPolicy::artifact_directed` rather than reaching into the
    // job for individual bool flags. These accessors are pinned by the
    // unit test suite so the field layout stays under structural test
    // coverage; turn.rs / #653 will read them as the policy projection grows.
    #[allow(dead_code)]
    pub(super) fn allow_create(&self) -> bool {
        self.allow_create
    }

    #[allow(dead_code)]
    pub(super) fn allow_modify(&self) -> bool {
        self.allow_modify
    }

    #[allow(dead_code)]
    pub(super) fn target_only(&self) -> bool {
        self.target_only
    }
}

/// Read tool policy attached to an active job.
///
/// **Only `TargetOnly` is generated in this Issue.** `TargetAndDeps` exists
/// as a placeholder for a future lifecycle (cap / mutability / source) to be
/// fixed by #653 — see design judgement #10. The variant body
/// (`Vec<String>`) is sized at `TargetOnly` creation so we never reach the
/// other variant unless a future caller explicitly opts in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum AllowedReadScope {
    /// Only the job's `target_path` may be read.
    TargetOnly,
    /// Target plus an explicit list of dep paths. Reserved for #653+.
    #[allow(dead_code)]
    TargetAndDeps(Vec<String>),
}

/// Categorical outcome of a single attempted artifact-completion action.
///
/// `#[non_exhaustive]` is intentionally NOT applied — this enum is private
/// to the `artifact_completion_job` module and `turn.rs` is the only in-
/// crate consumer. Variant additions therefore compile-error every match
/// arm in-crate, which is a stronger safety guarantee than `_` arms inside
/// a `non_exhaustive` enum (design judgement #8 / DR1-004).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ArtifactAttemptOutcomeKind {
    /// Tool call landed on a path other than the job's `target_path`.
    WrongTarget,
    /// Assistant produced no tool call and no executable prose.
    NoTool,
    /// Assistant produced natural-language only (no tool call).
    ProseOnly,
    /// Policy violation that is not a wrong-target (e.g. out-of-scope read,
    /// repeat read after target was already read, multi-tool batch reject).
    /// Variant subdivision is intentionally kept in `actual_actions` rather
    /// than as separate enum variants (DR2-006 / design judgement #8).
    RolePolicyViolation,
}

/// One recorded attempt against an `ArtifactCompletionJob`.
///
/// All string fields are sanitized at `new()` / `with_failure_cluster()`
/// time — consumers (`failure_snapshot()`, `attempts()`) see masked, capped,
/// control-char-neutralized values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ArtifactAttemptOutcome {
    kind: ArtifactAttemptOutcomeKind,
    /// `semantic_failure::FailureClusterKey` newtype retained verbatim — the
    /// raw 16-hex string is never downcast (DR1-002 / design judgement #4).
    cluster_key: Option<FailureClusterKey>,
    /// Masked / capped / control-char-neutralized action descriptions.
    actual_actions: Vec<String>,
    /// Masked / capped expected target path.
    expected_target: String,
}

impl ArtifactAttemptOutcome {
    /// Build an attempt outcome from raw inputs. Applies the full sanitize
    /// pipeline (`mask_secrets` → length cap → control-char neutralization)
    /// to `actual_actions` and `expected_target` and caps the action vector
    /// at `MAX_ARTIFACT_ACTIONS_PER_ATTEMPT`.
    pub(super) fn new(
        kind: ArtifactAttemptOutcomeKind,
        actual_actions: Vec<String>,
        expected_target: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            cluster_key: None,
            actual_actions: sanitize_actions(actual_actions),
            expected_target: sanitize_single(expected_target.into()),
        }
    }

    /// Build an attempt outcome carrying a deterministic
    /// `FailureClusterKey` from the `semantic_failure` SSOT.
    #[allow(dead_code)] // surfaced for #653 lifecycle; pinned by unit tests today.
    pub(super) fn with_failure_cluster(
        kind: ArtifactAttemptOutcomeKind,
        cluster_key: FailureClusterKey,
        actual_actions: Vec<String>,
        expected_target: impl Into<String>,
    ) -> Self {
        let mut out = Self::new(kind, actual_actions, expected_target);
        out.cluster_key = Some(cluster_key);
        out
    }

    #[allow(dead_code)] // pinned by unit tests; #653 will consume this.
    pub(super) fn kind(&self) -> ArtifactAttemptOutcomeKind {
        self.kind
    }

    #[allow(dead_code)] // pinned by unit tests; #653 will consume this.
    pub(super) fn cluster_key(&self) -> Option<&FailureClusterKey> {
        self.cluster_key.as_ref()
    }

    #[allow(dead_code)] // pinned by unit tests; #653 will consume this.
    pub(super) fn actual_actions(&self) -> &[String] {
        &self.actual_actions
    }

    #[allow(dead_code)] // pinned by unit tests; #653 will consume this.
    pub(super) fn expected_target(&self) -> &str {
        &self.expected_target
    }
}

/// Lifecycle state of an `ArtifactCompletionJob`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ArtifactCompletionStatus {
    /// Budget not yet exhausted. `remaining_budget()` is computed from
    /// `total_budget - attempts.len()` — there is no separate counter
    /// field (DR1-008 / DR2-005).
    InFlight,
    /// Completion observed (admitted by `role_from_repo_edit` /
    /// `task_contract_evidence_set_this_turn` SSOT in turn.rs). The
    /// `record_completion` accessor is part of the state-machine surface
    /// pinned by unit tests; #654 will wire it explicitly once the
    /// persisted report contract lands.
    #[allow(dead_code)]
    // constructed only in tests today; #654 lands the production constructor.
    Completed,
    Exhausted {
        reason: ExhaustedReason,
    },
}

/// Reason an `ArtifactCompletionJob` exhausted its budget.
///
/// Issue #652 ships exactly one variant (design judgement #9 / YAGNI). New
/// variants must come with a designed detection condition; today's only
/// detector is "budget consumed without completion".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ExhaustedReason {
    BudgetExceeded,
}

/// Construction error for `ArtifactCompletionJob::new`.
///
/// Unit variants only — never carry the offending raw value (DR4-004).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ArtifactCompletionJobError {
    /// `RecoveryTargetHint.path` is empty, absolute, contains `..`, holds a
    /// control character, lives under an ignored top dir, escapes the work
    /// root, or is otherwise rejected by the `task_workspace_scope` +
    /// `artifact_ownership` SSOT pipeline.
    InvalidTarget,
}

/// SSOT for the in-flight artifact completion task on the current turn.
///
/// Constructed from `RecoveryTargetHint` (existing planner output) and
/// validated against `TaskWorkspaceScope` + `ArtifactOwnership`. All
/// downstream consumers (`turn.rs::EffectiveToolPolicy::artifact_directed`,
/// `RecoveryTarget`, `failure_snapshot()`) read this struct as the single
/// source of truth — there are no parallel counters or paths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ArtifactCompletionJob {
    role: ArtifactRole,
    /// Canonicalized workspace-relative path. The `String` representation
    /// is intentional (`task_workspace_scope::contains` SSOT operates on
    /// `&str`); `Path::new(&self.target_path)` is the only conversion site.
    target_path: String,
    /// `RecoveryTargetHint.reason` after `mask_secrets` + length cap +
    /// control-char neutralization. Empty allowed — never `Option`.
    reason: String,
    allowed_write_actions: AllowedWriteActions,
    allowed_read_scope: AllowedReadScope,
    attempts: Vec<ArtifactAttemptOutcome>,
    total_budget: usize,
    status: ArtifactCompletionStatus,
}

impl ArtifactCompletionJob {
    /// Construct a new job for `role` targeting the path inside `target_hint`.
    ///
    /// Validates the target via `task_workspace_scope::contains` +
    /// `artifact_ownership::classify_ownership` (the existing SSOT pipeline
    /// covers absolute paths, `..` components, control characters, ignored
    /// top dirs, and symlink escapes). On success the job starts `InFlight`
    /// with an empty attempt history and the budget pinned at
    /// `ARTIFACT_COMPLETION_ATTEMPT_LIMIT`.
    pub(super) fn new(
        work_root: &Path,
        scope: &TaskWorkspaceScope,
        target_hint: RecoveryTargetHint,
        edited_this_session: bool,
        scaffold_changed: bool,
    ) -> Result<Self, ArtifactCompletionJobError> {
        let raw_path = target_hint.path.trim().to_string();
        if raw_path.is_empty() {
            return Err(ArtifactCompletionJobError::InvalidTarget);
        }
        let ownership = classify_ownership(OwnershipInputs {
            work_root,
            relative_path: &raw_path,
            scope,
            edited_this_session,
            scaffold_changed,
            verifier_passed_in_scope: false,
            // Issue #661 (Task 3.1): ArtifactCompletionJob is not one of
            // the 4 verifier-path SSOT sites — keep `disabled()` so this
            // caller's nested-test-subdir semantics stay unchanged.
            nested_test_admission: super::artifact_ownership::NestedTestAdmission::default(),
        });
        // CB-001: accept only `Owned` for an existing target, or treat
        // a missing leaf as a creation target after canonicalizing the
        // nearest existing parent against `work_root`. CandidateOnly
        // (in-scope but un-owned existing file) and OutOfScope are
        // both rejected as artifact-completion targets.
        let target_full = Path::new(work_root).join(&raw_path);
        let target_exists = target_full.is_file() || target_full.is_dir();
        let allowed_write_actions = match ownership {
            ArtifactOwnership::OutOfScope => {
                return Err(ArtifactCompletionJobError::InvalidTarget);
            }
            ArtifactOwnership::CandidateOnly => {
                if target_exists {
                    // Existing unowned file: must not be claimed as a
                    // completion target by the current task (DR4-002).
                    return Err(ArtifactCompletionJobError::InvalidTarget);
                }
                // Missing leaf, in-scope: verify the nearest existing
                // ancestor canonicalizes inside `work_root` so a symlink
                // parent cannot redirect a Write/Edit outside the
                // workspace (DR4-002 / CB-001 symlink-parent escape).
                // PR-002 SSOT: defer to `artifact_ownership`'s
                // `nearest_existing_ancestor_within_work_root` so the
                // canonicalize policy lives in one module only.
                if !nearest_existing_ancestor_within_work_root(work_root, &target_full) {
                    return Err(ArtifactCompletionJobError::InvalidTarget);
                }
                AllowedWriteActions::target_create_only()
            }
            ArtifactOwnership::Owned => {
                if target_full.is_file() {
                    AllowedWriteActions::target_modify_only()
                } else {
                    // Owned-but-missing (e.g. scaffold-changed without
                    // file persisted). Still verify the parent does not
                    // escape via symlink — `classify_ownership` already
                    // checked canonicalized existing path, but a missing
                    // leaf takes the early-return path inside
                    // `canonical_escape`.
                    if !nearest_existing_ancestor_within_work_root(work_root, &target_full) {
                        return Err(ArtifactCompletionJobError::InvalidTarget);
                    }
                    AllowedWriteActions::target_create_only()
                }
            }
        };
        Ok(Self {
            role: target_hint.role,
            target_path: raw_path,
            reason: sanitize_single(target_hint.reason),
            allowed_write_actions,
            allowed_read_scope: AllowedReadScope::TargetOnly,
            attempts: Vec::new(),
            total_budget: ARTIFACT_COMPLETION_ATTEMPT_LIMIT,
            status: ArtifactCompletionStatus::InFlight,
        })
    }

    pub(super) fn role(&self) -> ArtifactRole {
        self.role
    }

    pub(super) fn target_path(&self) -> &str {
        &self.target_path
    }

    #[allow(dead_code)] // pinned by unit tests + projection_recovery_target.
    pub(super) fn reason(&self) -> &str {
        &self.reason
    }

    #[allow(dead_code)] // pinned by unit tests; turn.rs reads via projection.
    pub(super) fn allowed_write_actions(&self) -> &AllowedWriteActions {
        &self.allowed_write_actions
    }

    #[allow(dead_code)] // pinned by unit tests; turn.rs reads via projection.
    pub(super) fn allowed_read_scope(&self) -> &AllowedReadScope {
        &self.allowed_read_scope
    }

    #[allow(dead_code)] // pinned by unit tests; turn.rs reads via
    // `record_artifact_completion_attempt` return signal instead of
    // status() directly.
    pub(super) fn status(&self) -> &ArtifactCompletionStatus {
        &self.status
    }

    #[allow(dead_code)] // pinned by unit tests; #653 will consume this slice.
    pub(super) fn attempts(&self) -> &[ArtifactAttemptOutcome] {
        &self.attempts
    }

    /// Remaining attempts before exhaustion. Returns `0` once the job has
    /// reached `Completed` or `Exhausted` regardless of `attempts.len()`.
    #[allow(dead_code)] // pinned by unit tests; turn.rs reads `status()` directly.
    pub(super) fn remaining_budget(&self) -> usize {
        match self.status {
            ArtifactCompletionStatus::InFlight => {
                self.total_budget.saturating_sub(self.attempts.len())
            }
            _ => 0,
        }
    }

    /// Append `outcome` to the attempt history and possibly transition the
    /// status. Returns the new status (`InFlight` or
    /// `Exhausted { BudgetExceeded }`).
    ///
    /// A no-op on `Completed` / `Exhausted` jobs (returns the current
    /// status). This keeps the state machine total / monotonic.
    pub(super) fn record_attempt(
        &mut self,
        outcome: ArtifactAttemptOutcome,
    ) -> ArtifactCompletionStatus {
        if !matches!(self.status, ArtifactCompletionStatus::InFlight) {
            return self.status.clone();
        }
        self.attempts.push(outcome);
        if self.attempts.len() >= self.total_budget {
            self.status = ArtifactCompletionStatus::Exhausted {
                reason: ExhaustedReason::BudgetExceeded,
            };
        }
        self.status.clone()
    }

    /// Mark the job `Completed`. No-op on `Completed` / `Exhausted`.
    ///
    /// Today `turn.rs` does not call this directly — completion is observed
    /// implicitly via the existing `task_contract_evidence_set_this_turn`
    /// SSOT (`role_from_repo_edit` admission). The method is part of the
    /// state-machine API so #654 can move to an explicit "ack completion"
    /// signal once the persisted report contract lands.
    #[allow(dead_code)] // pinned by unit tests; #654 will consume this entry.
    pub(super) fn record_completion(&mut self) {
        if matches!(self.status, ArtifactCompletionStatus::InFlight) {
            self.status = ArtifactCompletionStatus::Completed;
        }
    }

    /// One-way derivation to the legacy `RecoveryTarget` projection (design
    /// judgement #2 / DR1-003). The four fields are derived from this job
    /// only — no independent storage, no mutable accessor on the projection.
    #[allow(dead_code)] // pinned by unit tests; turn.rs still keeps a parallel
    // `current_artifact_recovery_target` field for incremental land — see
    // §11 of the design policy.
    pub(super) fn projection_recovery_target(&self) -> RecoveryTarget {
        RecoveryTarget {
            role: self.role,
            path: self.target_path.clone(),
            reason: self.reason.clone(),
            attempt: self.attempts.len(),
        }
    }

    /// Read-only snapshot for #654 (bounded safe stop report). Returns
    /// `Some` only while the status is `Exhausted` — `InFlight` /
    /// `Completed` short-circuit to `None`.
    ///
    /// The returned snapshot has every string field sanitized at attempt
    /// construction time and is additionally walked through
    /// `mask_payload_inplace` as a defensive final pass (multi-mask is
    /// harmless — DR4-001).
    pub(super) fn failure_snapshot(&self) -> Option<ArtifactCompletionFailureSnapshot> {
        if !matches!(self.status, ArtifactCompletionStatus::Exhausted { .. }) {
            return None;
        }
        let mut snapshot = ArtifactCompletionFailureSnapshot {
            current_role: self.role,
            expected_target: self.target_path.clone(),
            actual_actions: self
                .attempts
                .iter()
                .flat_map(|a| a.actual_actions.iter().cloned())
                .collect(),
            attempts: self.attempts.clone(),
            status: self.status.clone(),
        };
        // Defensive: re-mask the snapshot's JSON shape. The original fields
        // were already sanitized at attempt-construction time; this pass
        // exists so any future field added without going through `new()`
        // still gets the final-defence treatment (DR4-001).
        snapshot.defensive_mask();
        Some(snapshot)
    }
}

/// Read-only snapshot for #654's bounded safe-stop report (design
/// judgement #7). Strings are sanitized at construction; the helper
/// `defensive_mask()` re-applies `mask_payload_inplace` after copying so
/// any future field added without going through `new()` still gets the
/// final-defence treatment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ArtifactCompletionFailureSnapshot {
    pub(super) current_role: ArtifactRole,
    pub(super) expected_target: String,
    pub(super) actual_actions: Vec<String>,
    pub(super) attempts: Vec<ArtifactAttemptOutcome>,
    pub(super) status: ArtifactCompletionStatus,
}

impl ArtifactCompletionFailureSnapshot {
    /// Defensive re-application of `mask_payload_inplace` (DR4-001
    /// final-defence line). Strings here are already sanitized at
    /// attempt-construction time; this pass is intentionally idempotent so
    /// the snapshot can be rendered into system note / working-memory /
    /// agent-controlled-failure sinks without any of them re-implementing
    /// the redactor.
    fn defensive_mask(&mut self) {
        self.expected_target = sanitize_single(std::mem::take(&mut self.expected_target));
        let taken = std::mem::take(&mut self.actual_actions);
        self.actual_actions = sanitize_actions(taken);
        // Final JSON-shape mask: walk a serialized form so any nested
        // secret-like key is redacted as well. This is intentionally
        // belt-and-braces — sanitize_single / sanitize_actions already
        // handles the leaf strings.
        let mut payload = serde_json::json!({
            "expected_target": self.expected_target,
            "actual_actions": self.actual_actions,
        });
        mask_payload_inplace(&mut payload);
        if let Some(s) = payload
            .get("expected_target")
            .and_then(serde_json::Value::as_str)
        {
            self.expected_target = s.to_string();
        }
        if let Some(arr) = payload
            .get("actual_actions")
            .and_then(serde_json::Value::as_array)
        {
            self.actual_actions = arr
                .iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect();
        }
    }
}

// ----------------------- internal helpers (sanitize SSOT) -------------------

/// Apply the full sanitize pipeline (`mask_secrets` → length cap →
/// control-char neutralization) to a single string. SSOT used by every
/// constructor that ingests caller-controlled text.
fn sanitize_single(raw: impl Into<String>) -> String {
    let raw: String = raw.into();
    let masked = mask_secrets(&raw);
    let truncated = truncate_utf8(&masked, MAX_ARTIFACT_ACTION_TEXT_BYTES);
    neutralize_control_chars(&truncated)
}

/// Apply `sanitize_single` to every element and cap the vector at
/// `MAX_ARTIFACT_ACTIONS_PER_ATTEMPT`.
pub(super) fn sanitize_actions(actions: Vec<String>) -> Vec<String> {
    actions
        .into_iter()
        .take(MAX_ARTIFACT_ACTIONS_PER_ATTEMPT)
        .map(sanitize_single)
        .collect()
}

fn truncate_utf8(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s.to_string();
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_string()
}

fn neutralize_control_chars(s: &str) -> String {
    // CB-004: diagnostic strings flow into single-line system notes /
    // log payloads — newlines and tabs would either break multi-line
    // injection guarantees or wreck log column alignment, so they are
    // collapsed to a single ASCII space alongside all other control
    // characters (NUL, BEL, VT, FF, CR, ESC, DEL, …).
    s.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

// PR-002 (Issue #652) SSOT: the missing-tail parent / dangling-symlink
// validator is now `artifact_ownership::nearest_existing_ancestor_within_work_root`.
// The previous local copy in this module is removed — see the import block.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::loop_run::semantic_failure::build_failure_cluster_from_observation;
    use crate::agent::loop_run::task_workspace_scope::ScopeMode;

    fn single_root_scope() -> TaskWorkspaceScope {
        TaskWorkspaceScope {
            mode: ScopeMode::SingleProjectRoot,
        }
    }

    fn make_hint(path: &str) -> RecoveryTargetHint {
        RecoveryTargetHint {
            role: ArtifactRole::Test,
            path: path.to_string(),
            reason: "missing test artifact".to_string(),
        }
    }

    /// Drive `job` to `Exhausted` by recording `ARTIFACT_COMPLETION_ATTEMPT_LIMIT`
    /// attempts of `kind`. Each attempt carries the same `actual_actions` and
    /// the job's `target_path` as the expected target.
    fn drive_to_exhaustion(
        job: &mut ArtifactCompletionJob,
        kind: ArtifactAttemptOutcomeKind,
        actual_actions: Vec<String>,
    ) {
        let expected = job.target_path().to_string();
        for _ in 0..ARTIFACT_COMPLETION_ATTEMPT_LIMIT {
            job.record_attempt(ArtifactAttemptOutcome::new(
                kind,
                actual_actions.clone(),
                expected.clone(),
            ));
        }
    }

    #[test]
    fn test_new_returns_inflight_job_for_valid_target() {
        let dir = tempfile::tempdir().unwrap();
        let scope = single_root_scope();
        let job = ArtifactCompletionJob::new(
            dir.path(),
            &scope,
            make_hint("tests/test_foo.py"),
            true,
            false,
        )
        .expect("valid target must be accepted");
        assert_eq!(job.role(), ArtifactRole::Test);
        assert_eq!(job.target_path(), "tests/test_foo.py");
        assert!(matches!(job.status(), ArtifactCompletionStatus::InFlight));
        assert_eq!(job.remaining_budget(), ARTIFACT_COMPLETION_ATTEMPT_LIMIT);
    }

    #[test]
    fn test_new_rejects_absolute_path() {
        let dir = tempfile::tempdir().unwrap();
        let scope = single_root_scope();
        let err =
            ArtifactCompletionJob::new(dir.path(), &scope, make_hint("/etc/passwd"), true, false)
                .unwrap_err();
        assert_eq!(err, ArtifactCompletionJobError::InvalidTarget);
    }

    #[test]
    fn test_new_rejects_parent_traversal() {
        let dir = tempfile::tempdir().unwrap();
        let scope = single_root_scope();
        let err = ArtifactCompletionJob::new(
            dir.path(),
            &scope,
            make_hint("../sibling/file.py"),
            true,
            false,
        )
        .unwrap_err();
        assert_eq!(err, ArtifactCompletionJobError::InvalidTarget);
    }

    #[test]
    fn test_new_rejects_empty_path() {
        let dir = tempfile::tempdir().unwrap();
        let scope = single_root_scope();
        let err =
            ArtifactCompletionJob::new(dir.path(), &scope, make_hint(""), true, false).unwrap_err();
        assert_eq!(err, ArtifactCompletionJobError::InvalidTarget);
    }

    #[test]
    fn test_new_rejects_ignored_top_dir() {
        let dir = tempfile::tempdir().unwrap();
        let scope = single_root_scope();
        let err = ArtifactCompletionJob::new(
            dir.path(),
            &scope,
            make_hint("node_modules/foo/index.js"),
            true,
            false,
        )
        .unwrap_err();
        assert_eq!(err, ArtifactCompletionJobError::InvalidTarget);
    }

    #[test]
    fn test_new_rejects_control_chars_in_path() {
        let dir = tempfile::tempdir().unwrap();
        let scope = single_root_scope();
        let err = ArtifactCompletionJob::new(
            dir.path(),
            &scope,
            make_hint("tests/\x00bad.py"),
            true,
            false,
        )
        .unwrap_err();
        assert_eq!(err, ArtifactCompletionJobError::InvalidTarget);
    }

    #[test]
    fn test_new_masks_reason_and_truncates_long_text() {
        let dir = tempfile::tempdir().unwrap();
        let scope = single_root_scope();
        let mut hint = make_hint("tests/test_foo.py");
        // 20+ char secret matching `sk-[A-Za-z0-9_-]{20,}` so mask_secrets
        // replaces it with `***`.
        let secret = "sk-AAAAAAAAAAAAAAAAAAAAAAAAA";
        hint.reason = format!("leaking secret {secret} and {}", "a".repeat(8000));
        let job = ArtifactCompletionJob::new(dir.path(), &scope, hint, true, false).unwrap();
        assert!(
            !job.reason().contains(secret),
            "reason must be mask_secrets-redacted"
        );
        assert!(
            job.reason().len() <= MAX_ARTIFACT_ACTION_TEXT_BYTES,
            "reason must be cap-truncated"
        );
    }

    #[test]
    fn test_record_attempt_three_wrong_targets_exhausts_with_budget_exceeded() {
        let dir = tempfile::tempdir().unwrap();
        let scope = single_root_scope();
        let mut job = ArtifactCompletionJob::new(
            dir.path(),
            &scope,
            make_hint("tests/test_foo.py"),
            true,
            false,
        )
        .unwrap();
        drive_to_exhaustion(
            &mut job,
            ArtifactAttemptOutcomeKind::WrongTarget,
            vec!["src/x.py".to_string()],
        );
        assert!(matches!(
            job.status(),
            ArtifactCompletionStatus::Exhausted {
                reason: ExhaustedReason::BudgetExceeded
            }
        ));
        assert_eq!(job.remaining_budget(), 0);
    }

    #[test]
    fn test_record_attempt_three_no_tools_exhausts_with_budget_exceeded() {
        let dir = tempfile::tempdir().unwrap();
        let scope = single_root_scope();
        let mut job = ArtifactCompletionJob::new(
            dir.path(),
            &scope,
            make_hint("tests/test_foo.py"),
            true,
            false,
        )
        .unwrap();
        drive_to_exhaustion(&mut job, ArtifactAttemptOutcomeKind::NoTool, vec![]);
        assert!(matches!(
            job.status(),
            ArtifactCompletionStatus::Exhausted { .. }
        ));
    }

    #[test]
    fn test_record_attempt_three_prose_only_exhausts_with_budget_exceeded() {
        let dir = tempfile::tempdir().unwrap();
        let scope = single_root_scope();
        let mut job = ArtifactCompletionJob::new(
            dir.path(),
            &scope,
            make_hint("tests/test_foo.py"),
            true,
            false,
        )
        .unwrap();
        drive_to_exhaustion(
            &mut job,
            ArtifactAttemptOutcomeKind::ProseOnly,
            vec!["natural language response".to_string()],
        );
        assert!(matches!(
            job.status(),
            ArtifactCompletionStatus::Exhausted { .. }
        ));
    }

    #[test]
    fn test_record_attempt_role_policy_violation_holds_subdivision_in_actions() {
        let dir = tempfile::tempdir().unwrap();
        let scope = single_root_scope();
        let mut job = ArtifactCompletionJob::new(
            dir.path(),
            &scope,
            make_hint("tests/test_foo.py"),
            true,
            false,
        )
        .unwrap();
        job.record_attempt(ArtifactAttemptOutcome::new(
            ArtifactAttemptOutcomeKind::RolePolicyViolation,
            vec!["multi_tool_batch".to_string()],
            "tests/test_foo.py",
        ));
        assert_eq!(job.attempts().len(), 1);
        assert_eq!(
            job.attempts()[0].kind(),
            ArtifactAttemptOutcomeKind::RolePolicyViolation
        );
        assert_eq!(job.attempts()[0].actual_actions(), &["multi_tool_batch"]);
    }

    #[test]
    fn test_record_completion_moves_to_completed_state() {
        let dir = tempfile::tempdir().unwrap();
        let scope = single_root_scope();
        let mut job = ArtifactCompletionJob::new(
            dir.path(),
            &scope,
            make_hint("tests/test_foo.py"),
            true,
            false,
        )
        .unwrap();
        job.record_completion();
        assert!(matches!(job.status(), ArtifactCompletionStatus::Completed));
        assert_eq!(job.remaining_budget(), 0);
    }

    #[test]
    fn test_record_attempt_after_exhausted_is_noop() {
        let dir = tempfile::tempdir().unwrap();
        let scope = single_root_scope();
        let mut job = ArtifactCompletionJob::new(
            dir.path(),
            &scope,
            make_hint("tests/test_foo.py"),
            true,
            false,
        )
        .unwrap();
        drive_to_exhaustion(&mut job, ArtifactAttemptOutcomeKind::NoTool, vec![]);
        let exhausted_len = job.attempts().len();
        job.record_attempt(ArtifactAttemptOutcome::new(
            ArtifactAttemptOutcomeKind::NoTool,
            vec![],
            "tests/test_foo.py",
        ));
        assert_eq!(job.attempts().len(), exhausted_len);
    }

    #[test]
    fn test_sanitize_actions_masks_secrets_caps_length_neutralizes_control() {
        let secret = "sk-AAAAAAAAAAAAAAAAAAAAAAAAA";
        let raw = vec![
            format!("Authorization: Bearer {secret}"),
            "control char \x00 embedded".to_string(),
            "a".repeat(8000),
        ];
        let out = sanitize_actions(raw);
        assert_eq!(out.len(), 3);
        assert!(!out[0].contains(secret));
        assert!(!out[1].contains('\x00'));
        assert!(out[2].len() <= MAX_ARTIFACT_ACTION_TEXT_BYTES);
    }

    #[test]
    fn test_sanitize_actions_caps_vector_length() {
        let raw: Vec<String> = (0..(MAX_ARTIFACT_ACTIONS_PER_ATTEMPT + 5))
            .map(|i| format!("action {i}"))
            .collect();
        let out = sanitize_actions(raw);
        assert_eq!(out.len(), MAX_ARTIFACT_ACTIONS_PER_ATTEMPT);
    }

    #[test]
    fn test_failure_snapshot_returns_some_only_on_exhausted() {
        let dir = tempfile::tempdir().unwrap();
        let scope = single_root_scope();
        let mut job = ArtifactCompletionJob::new(
            dir.path(),
            &scope,
            make_hint("tests/test_foo.py"),
            true,
            false,
        )
        .unwrap();
        assert!(job.failure_snapshot().is_none());
        let secret = "sk-AAAAAAAAAAAAAAAAAAAAAAAAA";
        drive_to_exhaustion(
            &mut job,
            ArtifactAttemptOutcomeKind::WrongTarget,
            vec![format!("{secret} in action")],
        );
        let snap = job
            .failure_snapshot()
            .expect("exhausted produces a snapshot");
        assert_eq!(snap.current_role, ArtifactRole::Test);
        assert_eq!(snap.expected_target, "tests/test_foo.py");
        assert!(!snap.actual_actions.is_empty());
        for action in &snap.actual_actions {
            assert!(
                !action.contains(secret),
                "snapshot must surface mask-applied actions only"
            );
        }
    }

    #[test]
    fn test_projection_recovery_target_derives_from_attempts() {
        let dir = tempfile::tempdir().unwrap();
        let scope = single_root_scope();
        let mut job = ArtifactCompletionJob::new(
            dir.path(),
            &scope,
            make_hint("tests/test_foo.py"),
            true,
            false,
        )
        .unwrap();
        let pr0 = job.projection_recovery_target();
        assert_eq!(pr0.role, ArtifactRole::Test);
        assert_eq!(pr0.path, "tests/test_foo.py");
        assert_eq!(pr0.attempt, 0);
        job.record_attempt(ArtifactAttemptOutcome::new(
            ArtifactAttemptOutcomeKind::WrongTarget,
            vec!["src/x.py".to_string()],
            "tests/test_foo.py",
        ));
        let pr1 = job.projection_recovery_target();
        assert_eq!(pr1.attempt, 1);
        assert_eq!(pr1.reason, job.reason());
    }

    #[test]
    fn test_attempt_outcome_holds_failure_cluster_key_newtype() {
        let cluster = build_failure_cluster_from_observation(
            "AssertionError",
            "expected 200",
            "shape",
            "shape",
            &[ArtifactRole::Test],
            Vec::new(),
        );
        let key = cluster.cluster_key.clone();
        let outcome = ArtifactAttemptOutcome::with_failure_cluster(
            ArtifactAttemptOutcomeKind::RolePolicyViolation,
            key.clone(),
            vec!["multi_tool_batch".to_string()],
            "tests/test_foo.py",
        );
        // FailureClusterKey newtype retained verbatim — equality is via the
        // newtype, never a downcast to raw String.
        assert_eq!(outcome.cluster_key(), Some(&key));
    }

    #[test]
    fn test_allowed_write_actions_least_privilege_constructors() {
        let create = AllowedWriteActions::target_create_only();
        assert!(create.allow_create());
        assert!(!create.allow_modify());
        assert!(create.target_only());
        let modify = AllowedWriteActions::target_modify_only();
        assert!(!modify.allow_create());
        assert!(modify.allow_modify());
        assert!(modify.target_only());
    }

    #[test]
    fn test_default_allowed_read_scope_is_target_only() {
        let dir = tempfile::tempdir().unwrap();
        let scope = single_root_scope();
        let job = ArtifactCompletionJob::new(
            dir.path(),
            &scope,
            make_hint("tests/test_foo.py"),
            true,
            false,
        )
        .unwrap();
        assert!(matches!(
            job.allowed_read_scope(),
            AllowedReadScope::TargetOnly
        ));
    }

    #[test]
    fn test_artifact_attempt_outcome_kind_enum_is_4_variants_closed() {
        // Exhaustive match — adding a new variant requires updating this
        // arm (design judgement #8 / DR1-004 compile-time enforcement).
        for kind in [
            ArtifactAttemptOutcomeKind::WrongTarget,
            ArtifactAttemptOutcomeKind::NoTool,
            ArtifactAttemptOutcomeKind::ProseOnly,
            ArtifactAttemptOutcomeKind::RolePolicyViolation,
        ] {
            let label: &'static str = match kind {
                ArtifactAttemptOutcomeKind::WrongTarget => "wrong_target",
                ArtifactAttemptOutcomeKind::NoTool => "no_tool",
                ArtifactAttemptOutcomeKind::ProseOnly => "prose_only",
                ArtifactAttemptOutcomeKind::RolePolicyViolation => "role_policy_violation",
            };
            assert!(!label.is_empty());
        }
    }

    // ----------------------------------------------------------------
    // CB-001 regression: ArtifactCompletionJob::new must reject targets
    // that are not `ArtifactOwnership::Owned` and missing-leaf targets
    // whose nearest existing parent escapes work_root via a symlink.
    // ----------------------------------------------------------------

    #[test]
    fn test_new_rejects_candidate_only_existing_file() {
        // CB-001: in-scope existing file with NO ownership signal (i.e.
        // `CandidateOnly`) must be rejected — the artifact-directed
        // recovery target must be owned by the current task to be a
        // legitimate completion target.
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("tests")).unwrap();
        std::fs::write(dir.path().join("tests/test_foo.py"), "# unowned\n").unwrap();
        let scope = single_root_scope();
        let err = ArtifactCompletionJob::new(
            dir.path(),
            &scope,
            make_hint("tests/test_foo.py"),
            /*edited_this_session=*/ false,
            /*scaffold_changed=*/ false,
        )
        .unwrap_err();
        assert_eq!(err, ArtifactCompletionJobError::InvalidTarget);
    }

    #[test]
    fn test_new_accepts_owned_existing_file() {
        // CB-001 regression positive case: an existing file recorded as
        // edited this session is `Owned` and remains an acceptable target.
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("tests")).unwrap();
        std::fs::write(dir.path().join("tests/test_foo.py"), "# owned\n").unwrap();
        let scope = single_root_scope();
        let job = ArtifactCompletionJob::new(
            dir.path(),
            &scope,
            make_hint("tests/test_foo.py"),
            /*edited_this_session=*/ true,
            /*scaffold_changed=*/ false,
        )
        .expect("owned existing file must be accepted");
        assert_eq!(job.target_path(), "tests/test_foo.py");
    }

    #[test]
    fn test_new_accepts_missing_leaf_under_existing_parent() {
        // CB-001 regression positive case: the typical "create the
        // missing test file" workflow — leaf does not exist, but the
        // nearest existing parent (`tests/`) canonicalizes inside the
        // work_root.
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("tests")).unwrap();
        let scope = single_root_scope();
        let job = ArtifactCompletionJob::new(
            dir.path(),
            &scope,
            make_hint("tests/test_new.py"),
            /*edited_this_session=*/ false,
            /*scaffold_changed=*/ false,
        )
        .expect("missing leaf with in-scope existing parent must be accepted");
        assert_eq!(job.target_path(), "tests/test_new.py");
    }

    #[cfg(unix)]
    #[test]
    fn test_new_rejects_missing_leaf_under_symlinked_parent_outside_work_root() {
        // CB-001: when the leaf is missing but the nearest existing
        // parent is a symlink whose canonical resolution escapes the
        // work_root, the job must reject — otherwise an attacker can
        // craft a `tests/` symlink to /etc/ and have the recovery write
        // a file under /etc/ by simply naming a missing leaf.
        use std::os::unix::fs::symlink;
        let outside = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        // outside/tests/ exists; work/tests -> outside/tests
        std::fs::create_dir_all(outside.path().join("tests")).unwrap();
        symlink(outside.path().join("tests"), work.path().join("tests")).unwrap();
        let scope = single_root_scope();
        let err = ArtifactCompletionJob::new(
            work.path(),
            &scope,
            make_hint("tests/test_new.py"),
            /*edited_this_session=*/ false,
            /*scaffold_changed=*/ false,
        )
        .unwrap_err();
        assert_eq!(err, ArtifactCompletionJobError::InvalidTarget);
    }

    #[cfg(unix)]
    #[test]
    fn test_new_rejects_missing_leaf_under_symlinked_root_to_outside() {
        // CB-001: a more subtle variant — nearest existing parent is
        // work_root itself, but `nested/` is missing, while a parent
        // *symlinked* dir within work_root resolves outside. Verify the
        // first existing ancestor's canonical form is bounded by
        // canonical(work_root).
        use std::os::unix::fs::symlink;
        let outside = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(outside.path().join("evil")).unwrap();
        let work = tempfile::tempdir().unwrap();
        symlink(outside.path().join("evil"), work.path().join("evil")).unwrap();
        let scope = single_root_scope();
        let err = ArtifactCompletionJob::new(
            work.path(),
            &scope,
            make_hint("evil/missing.py"),
            /*edited_this_session=*/ false,
            /*scaffold_changed=*/ false,
        )
        .unwrap_err();
        assert_eq!(err, ArtifactCompletionJobError::InvalidTarget);
    }

    // ----------------------------------------------------------------
    // CB2-002 regression: a dangling-symlink ancestor must NOT be
    // skipped by the missing-leaf parent walk. The original CB-001
    // implementation used `Path::exists()` which follows symlinks —
    // so a dangling symlink ancestor counted as "missing" and the
    // walker proceeded to a parent (typically work_root itself) that
    // canonicalized cleanly inside work_root. Between the validation
    // and the actual Write/Edit, an attacker could materialize the
    // link target outside the workspace and have the recovery write
    // there. Now the walk uses `symlink_metadata` so the dangling
    // link is detected, then `canonicalize` rejects it.
    // ----------------------------------------------------------------

    #[cfg(unix)]
    #[test]
    fn test_new_rejects_missing_leaf_under_dangling_symlink_parent() {
        // CB2-002: `tests/` is a symlink whose target does NOT exist
        // (a dangling symlink). With `Path::exists()` the resolver
        // walked past `tests/` to `work_root`, accepted the missing
        // leaf, and opened the TOCTOU window for an attacker to
        // create the link target before the Write. With
        // `symlink_metadata` the dangling link is seen, but
        // `canonicalize` fails on it, so the job is rejected.
        use std::os::unix::fs::symlink;
        let work = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        // Build a dangling link: outside/nonexistent does not exist.
        symlink(
            outside.path().join("nonexistent"),
            work.path().join("tests"),
        )
        .unwrap();
        assert!(
            !work.path().join("tests").exists(),
            "fixture invariant: tests/ must be a dangling symlink (exists()=false)"
        );
        assert!(
            work.path().join("tests").symlink_metadata().is_ok(),
            "fixture invariant: tests/ must exist as a symlink per symlink_metadata"
        );
        let scope = single_root_scope();
        let err = ArtifactCompletionJob::new(
            work.path(),
            &scope,
            make_hint("tests/test_new.py"),
            /*edited_this_session=*/ false,
            /*scaffold_changed=*/ false,
        )
        .unwrap_err();
        assert_eq!(
            err,
            ArtifactCompletionJobError::InvalidTarget,
            "CB2-002: dangling-symlink ancestor MUST be rejected, not walked past"
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_nearest_existing_parent_returns_false_for_dangling_symlink_parent() {
        // CB2-002: direct unit-level verification of the SSOT helper
        // (now hosted by `artifact_ownership::
        // nearest_existing_ancestor_within_work_root` — PR-002 SSOT).
        // A dangling symlink ancestor must be detected (so the loop
        // stops at the link) and then rejected by the canonicalize
        // step. This is the same code path the artifact-directed
        // target match relies on indirectly (via
        // `workspace_relative_path_for_tool_arg` ->
        // `resolve_user_path`), so closing it here closes both classes.
        use std::os::unix::fs::symlink;
        let work = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        symlink(
            outside.path().join("nonexistent"),
            work.path().join("dangle"),
        )
        .unwrap();
        let target = work.path().join("dangle").join("file.py");
        assert!(
            !nearest_existing_ancestor_within_work_root(work.path(), &target),
            "dangling symlink parent must NOT canonicalize inside work_root"
        );
    }

    // ----------------------------------------------------------------
    // PRR-001 (re-review v2): the **final leaf** itself must be checked,
    // not just the ancestor chain. A dangling symlink leaf (e.g.
    // `tests/test_artifact.py -> /outside/missing.py`) used to be
    // accepted because the previous helper advanced to `candidate.parent()`
    // before any `symlink_metadata` check, and the parent was inside
    // `work_root`. A subsequent `std::fs::write` on the leaf would
    // dereference the link and escape the workspace.
    // ----------------------------------------------------------------

    #[cfg(unix)]
    #[test]
    fn test_new_rejects_dangling_symlink_leaf() {
        use std::os::unix::fs::symlink;
        let work = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(work.path().join("tests")).unwrap();
        let outside = tempfile::tempdir().unwrap();
        // Final leaf is a dangling symlink — parent `tests/` is inside
        // `work_root`. Pre-PRR-001 the helper walked to `tests/`, which
        // canonicalized cleanly inside work_root, and the job installed.
        symlink(
            outside.path().join("missing.py"),
            work.path().join("tests/test_artifact.py"),
        )
        .unwrap();
        let scope = single_root_scope();
        let err = ArtifactCompletionJob::new(
            work.path(),
            &scope,
            make_hint("tests/test_artifact.py"),
            /*edited_this_session=*/ false,
            /*scaffold_changed=*/ false,
        )
        .unwrap_err();
        assert_eq!(
            err,
            ArtifactCompletionJobError::InvalidTarget,
            "PRR-001: dangling symlink leaf MUST be rejected even when parent is inside work_root"
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_new_rejects_symlink_leaf_pointing_outside_work_root() {
        use std::os::unix::fs::symlink;
        let work = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(work.path().join("tests")).unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret.py"), "secret").unwrap();
        // Leaf symlink resolves to an existing file outside work_root.
        symlink(
            outside.path().join("secret.py"),
            work.path().join("tests/escape.py"),
        )
        .unwrap();
        let scope = single_root_scope();
        let err = ArtifactCompletionJob::new(
            work.path(),
            &scope,
            make_hint("tests/escape.py"),
            /*edited_this_session=*/ true,
            /*scaffold_changed=*/ false,
        )
        .unwrap_err();
        assert_eq!(
            err,
            ArtifactCompletionJobError::InvalidTarget,
            "PRR-001: symlink leaf that canonicalizes outside work_root MUST be rejected"
        );
    }

    // ----------------------------------------------------------------
    // CB-004 regression: sanitize_single must neutralize \n / \t / NUL
    // simultaneously, along with mask_secrets + 4096 byte cap.
    // ----------------------------------------------------------------

    #[test]
    fn test_sanitize_single_neutralizes_newlines_tabs_and_nul_together() {
        // CB-004: newlines and tabs must NOT survive into diagnostic
        // sinks — they can cause multi-line injection or log column
        // misalignment. NUL must also be neutralized. mask_secrets +
        // length cap must compose with the control-char pass.
        let secret = "sk-AAAAAAAAAAAAAAAAAAAAAAAAA";
        let raw = format!(
            "line1\nline2\twith\ttabs\nand {secret} and \x00 plus {}",
            "x".repeat(8192)
        );
        let out = sanitize_single(raw);
        assert!(
            !out.contains('\n'),
            "sanitize_single must strip newlines, got: {out:?}"
        );
        assert!(
            !out.contains('\t'),
            "sanitize_single must strip tabs, got: {out:?}"
        );
        assert!(!out.contains('\x00'), "sanitize_single must strip NUL");
        assert!(
            !out.contains(secret),
            "sanitize_single must mask secrets, got: {out:?}"
        );
        assert!(
            out.len() <= MAX_ARTIFACT_ACTION_TEXT_BYTES,
            "sanitize_single must cap at MAX_ARTIFACT_ACTION_TEXT_BYTES"
        );
    }

    #[test]
    fn test_sanitize_actions_neutralizes_newlines_and_tabs() {
        // CB-004: the vector pipeline must inherit the per-element
        // newline/tab neutralization so every diagnostic element is
        // single-line.
        let raw = vec![
            "first\nsecond".to_string(),
            "tab\there".to_string(),
            "carriage\rreturn".to_string(),
        ];
        let out = sanitize_actions(raw);
        for action in &out {
            assert!(
                !action.contains('\n'),
                "action contains newline: {action:?}"
            );
            assert!(!action.contains('\t'), "action contains tab: {action:?}");
            assert!(!action.contains('\r'), "action contains CR: {action:?}");
        }
    }
}
