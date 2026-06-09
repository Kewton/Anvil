//! Core task-contract state types.
//!
//! This module contains plain state/data shapes that are shared by completion
//! evaluation, recovery targeting, controller policy, and artifact projection.
//! It deliberately does not infer requests, inspect evidence, or decide
//! terminal state.

use super::task_contract_taxonomy::{
    ArtifactRole, DeliverableFormat, DeliverableKind, DeliverableSchema, ProjectLanguage,
    ProjectShape, StructuredRecordSchema, TaskIntent, TaskKind, VerificationRequirement,
};

/// Issue #917 (P0.5): per-turn classification head, projected from
/// `TaskContract` via `TaskContract::classification`. The P0.5 frozen shape is
/// `{ task_kind, confidence }`; `needs_confirm()` is derived (no independent
/// bool) so `confidence` is the single source of truth (DR1-004).
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub(super) struct TaskClassification {
    pub(super) task_kind: TaskKind,
    /// 0.0..=1.0. P0.5 is a 2-value approximation (matched -> 1.0, no-match ->
    /// 0.0); the f32 type leaves room for additive refinement toward
    /// `ModeClassification.confidence`'s continuous scale.
    pub(super) confidence: f32,
}

impl TaskClassification {
    /// "Unknown" signal (D1/D5): a no-keyword-match request (`confidence` below
    /// the confirm threshold) routes to the confirm path instead of silently
    /// staying `Coding`.
    pub(super) fn needs_confirm(&self) -> bool {
        self.confidence < crate::modes::plan_act::WORK_MODE_CONFIRM_CONFIDENCE_THRESHOLD
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct DeliverableObligation {
    pub(super) role: ArtifactRole,
    pub(super) kind: DeliverableKind,
    pub(super) path: String,
    pub(super) format: Option<DeliverableFormat>,
    pub(super) schema: Option<DeliverableSchema>,
    pub(super) required_sections: Vec<String>,
    pub(super) acceptance_criteria: Vec<String>,
    pub(super) structured_record_schema: Option<StructuredRecordSchema>,
}

pub(super) type ArtifactObligation = DeliverableObligation;

#[derive(Debug, Clone, PartialEq)]
pub(super) struct ProjectIntent {
    pub(super) intent: TaskIntent,
    pub(super) language: Option<ProjectLanguage>,
    pub(super) shape: Option<ProjectShape>,
    pub(super) verification: VerificationRequirement,
    pub(super) confidence: f32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum CompletionDecision {
    Continue {
        missing: Vec<ArtifactRole>,
    },
    Verify,
    Done,
    /// Issue #651: verifier was attempted but the required-test invariant
    /// (`test_execution_required && owned_test_artifacts bound to runner`)
    /// could not be satisfied. The agent must stop without claiming `Done`
    /// to prevent false-positive completion. The reason is preserved at
    /// type level so caller match sites stay exhaustive (no `_ =>`).
    ///
    /// Phase 4.1 is the first producer of this variant. The arm also
    /// keeps `_ =>` fallback out of `turn.rs` match sites (design
    /// judgement #2).
    SafeStop {
        reason: SafeStopReason,
    },
}

/// Issue #651: deterministic reason for `CompletionDecision::SafeStop`.
///
/// `Weak`: a structurally runnable verifier was found, but the owned test
/// artifacts could not be bound to its arguments (e.g. ProjectInstruction
/// / RecentSuccessfulBash / shell-only compound command).
///
/// `Missing`: no allowlisted test runner could be detected at all.
///
/// The variants are kept narrow on purpose. Adding a new reason (e.g.
/// `VerifierTimedOut`) must be a type-level extension so `_ =>` fallback
/// stays out of the codebase (CLAUDE.md unwritten rule for new enums).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SafeStopReason {
    /// A structurally runnable verifier was found, but owned test
    /// artifacts could not be bound to its arguments.
    ///
    /// `#[allow(dead_code)]` is intentional today: `VerifierOutcome::Weak`
    /// in `verifier_skill.rs` is observed by `success.rs` /
    /// `turn.rs::run_task_contract_verifier_once`, which translate it
    /// directly to `ExitReason::SafeStopVerifierWeak` without going
    /// through the planner-side `CompletionDecision::SafeStop`. The
    /// variant is retained so the `_ =>` ban (design judgement #2)
    /// holds at every match site and so a future planner-driven
    /// "Weak-from-evaluate" path lights up here at compile time.
    #[allow(dead_code)]
    VerifierWeak,
    /// No allowlisted test runner could be detected at all (or
    /// `evaluate_with_owned_test_artifacts` saw an empty owned slice
    /// while `test_execution_required` was true).
    VerifierMissing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RecoveryTargetHint {
    pub(super) role: ArtifactRole,
    pub(super) path: String,
    pub(super) reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RecoveryTarget {
    pub(super) role: ArtifactRole,
    pub(super) path: String,
    pub(super) reason: String,
    pub(super) attempt: usize,
}

impl RecoveryTarget {
    pub(super) fn from_hint(hint: RecoveryTargetHint, attempt: usize) -> Self {
        Self {
            role: hint.role,
            path: hint.path,
            reason: hint.reason,
            attempt,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ArtifactStateKind {
    ExistsButUnverified,
    ChangedThisTurn,
    ScaffoldUnchanged,
    #[allow(dead_code)]
    Verified,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ArtifactState {
    pub(super) role: ArtifactRole,
    pub(super) path: Option<String>,
    pub(super) kind: ArtifactStateKind,
}

impl ArtifactState {
    pub(super) fn exists(role: ArtifactRole, path: impl Into<String>) -> Self {
        Self {
            role,
            path: Some(path.into()),
            kind: ArtifactStateKind::ExistsButUnverified,
        }
    }

    pub(super) fn scaffold(role: ArtifactRole, path: impl Into<String>) -> Self {
        Self {
            role,
            path: Some(path.into()),
            kind: ArtifactStateKind::ScaffoldUnchanged,
        }
    }

    pub(super) fn changed(role: ArtifactRole) -> Self {
        Self {
            role,
            path: None,
            kind: ArtifactStateKind::ChangedThisTurn,
        }
    }

    pub(super) fn changed_at(role: ArtifactRole, path: impl Into<String>) -> Self {
        Self {
            role,
            path: Some(path.into()),
            kind: ArtifactStateKind::ChangedThisTurn,
        }
    }
}
