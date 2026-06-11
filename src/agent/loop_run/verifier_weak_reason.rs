//! Typed reason for a verifier that exists but is not strong enough to claim
//! completion.
//!
//! This module deliberately classifies structural evidence only. It does not
//! inspect benchmark names, user prompt substrings, or failure-log fragments.

use super::completion_evidence::{CompletionEvidence, EvidenceSet};
use crate::tools::bash::BashCommandClass;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum VerifierWeakReason {
    /// A structured verifier plan found a runner, but it could not bind the
    /// current task-owned test artifacts to the runner argv.
    StructuredSelectionUnbound,
    /// A BuildTest command exited zero through the structured path, but carried
    /// zero bound owned test artifacts.
    ExitZeroWithoutOwnedBinding,
    /// The verifier planner reported weak metadata, but the completion
    /// evidence itself did not carry bound structured evidence.
    WeakPlanMetadataOnly,
}

impl VerifierWeakReason {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::StructuredSelectionUnbound => "structured_selection_unbound",
            Self::ExitZeroWithoutOwnedBinding => "exit_zero_without_owned_binding",
            Self::WeakPlanMetadataOnly => "weak_plan_metadata_only",
        }
    }

    pub(super) fn repairability_hint(self) -> &'static str {
        match self {
            Self::StructuredSelectionUnbound
            | Self::ExitZeroWithoutOwnedBinding
            | Self::WeakPlanMetadataOnly => "repairable_evidence_binding",
        }
    }
}

pub(super) fn structured_selection_unbound_reason() -> VerifierWeakReason {
    VerifierWeakReason::StructuredSelectionUnbound
}

pub(super) fn done_gate_weak_reason(
    evidence: &EvidenceSet,
    weak_metadata: Option<usize>,
) -> Option<VerifierWeakReason> {
    let has_bound_zero = evidence.iter().any(|item| {
        matches!(
            item,
            CompletionEvidence::VerifierExitZero {
                class: BashCommandClass::BuildTest,
                bound_test_artifacts_count: Some(0),
                ..
            }
        )
    });
    if has_bound_zero {
        return Some(VerifierWeakReason::ExitZeroWithoutOwnedBinding);
    }
    if matches!(weak_metadata, Some(n) if n > 0) {
        return Some(VerifierWeakReason::WeakPlanMetadataOnly);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn done_gate_reason_detects_zero_bound_structured_evidence() {
        let mut evidence = EvidenceSet::new();
        evidence.push(CompletionEvidence::VerifierExitZero {
            class: BashCommandClass::BuildTest,
            command: "cargo test".to_string(),
            bound_test_artifacts_count: Some(0),
        });

        assert_eq!(
            done_gate_weak_reason(&evidence, None),
            Some(VerifierWeakReason::ExitZeroWithoutOwnedBinding)
        );
    }

    #[test]
    fn done_gate_reason_detects_weak_metadata_without_bound_evidence() {
        let mut evidence = EvidenceSet::new();
        evidence.push(CompletionEvidence::VerifierExitZero {
            class: BashCommandClass::BuildTest,
            command: "cargo test".to_string(),
            bound_test_artifacts_count: None,
        });

        assert_eq!(
            done_gate_weak_reason(&evidence, Some(1)),
            Some(VerifierWeakReason::WeakPlanMetadataOnly)
        );
    }

    #[test]
    fn done_gate_reason_absent_when_missing_not_weak() {
        let mut evidence = EvidenceSet::new();
        evidence.push(CompletionEvidence::VerifierExitZero {
            class: BashCommandClass::BuildTest,
            command: "cargo test".to_string(),
            bound_test_artifacts_count: None,
        });

        assert_eq!(done_gate_weak_reason(&evidence, None), None);
    }
}
