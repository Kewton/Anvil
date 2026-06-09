//! Typed recovery-planner input and output models for `TaskContract`.
//!
//! This module owns the data shape crossing from objective/evidence state into
//! recovery planning. It intentionally contains no prompt parsing or task-kind
//! inference, so future deliverable/evidence kinds can reuse the same lifecycle
//! boundary.

use super::completion_evidence::EvidenceSet;
use super::repair_job::VerifierRepairState;
use super::task_contract::{
    ArtifactRole, ArtifactState, CompletionDecision, RecoveryTargetHint, SafeStopReason,
    TaskContract,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ArtifactRecoveryAction {
    Continue {
        missing: Vec<ArtifactRole>,
        target_hint: Option<RecoveryTargetHint>,
    },
    RunVerifier,
    RepairArtifact {
        target_hint: Option<RecoveryTargetHint>,
    },
    Done,
    SafeStop {
        reason: SafeStopReason,
    },
}

pub(super) type ArtifactExcerpts = std::collections::HashMap<ArtifactRole, String>;

pub(super) const MAX_ARTIFACT_EXCERPT_BYTES: usize = 8 * 1024;

#[derive(Debug, Clone, Copy)]
pub(super) struct ArtifactRecoveryInputs<'a> {
    pub(super) contract: &'a TaskContract,
    pub(super) evidence: &'a EvidenceSet,
    pub(super) artifacts: &'a [ArtifactState],
    pub(super) repair_state: &'a VerifierRepairState,
    pub(super) artifact_excerpts: &'a ArtifactExcerpts,
    pub(super) missing_verifier_suppress_retry: bool,
    pub(super) owned_test_artifacts: &'a [String],
}

impl From<CompletionDecision> for ArtifactRecoveryAction {
    fn from(decision: CompletionDecision) -> Self {
        match decision {
            CompletionDecision::Continue { missing } => ArtifactRecoveryAction::Continue {
                missing,
                target_hint: None,
            },
            CompletionDecision::Verify => ArtifactRecoveryAction::RunVerifier,
            CompletionDecision::Done => ArtifactRecoveryAction::Done,
            CompletionDecision::SafeStop { reason } => ArtifactRecoveryAction::SafeStop { reason },
        }
    }
}
