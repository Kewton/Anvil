//! Shadow projection from weak verifier reasons to typed repair targets.
//!
//! WP2 keeps this advisory-only: it emits observability for the controller
//! path WP3 may adopt, but it does not change terminal behavior.

use super::verifier_weak_reason::VerifierWeakReason;
use crate::logging::log_llm_event;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum VerifierWeakRepairTargetKind {
    MissingEvidenceJob,
    EvidenceFailedJob,
}

impl VerifierWeakRepairTargetKind {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::MissingEvidenceJob => "missing_evidence_job",
            Self::EvidenceFailedJob => "evidence_failed_job",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct VerifierWeakRepairTarget {
    pub(super) reason: VerifierWeakReason,
    pub(super) job_kind: VerifierWeakRepairTargetKind,
    pub(super) expected_delta: &'static str,
    pub(super) adoption_state: &'static str,
}

pub(super) fn shadow_repair_target(reason: VerifierWeakReason) -> VerifierWeakRepairTarget {
    match reason {
        VerifierWeakReason::StructuredSelectionUnbound
        | VerifierWeakReason::WeakPlanMetadataOnly => VerifierWeakRepairTarget {
            reason,
            job_kind: VerifierWeakRepairTargetKind::MissingEvidenceJob,
            expected_delta: "produce_bound_owned_test_evidence",
            adoption_state: "shadow_only",
        },
        VerifierWeakReason::ExitZeroWithoutOwnedBinding => VerifierWeakRepairTarget {
            reason,
            job_kind: VerifierWeakRepairTargetKind::EvidenceFailedJob,
            expected_delta: "rerun_verifier_with_owned_test_binding",
            adoption_state: "shadow_only",
        },
    }
}

pub(super) fn log_shadow_repair_target(
    session_id: &str,
    turn_index: usize,
    iter_index: usize,
    source: &'static str,
    reason: VerifierWeakReason,
) {
    let target = shadow_repair_target(reason);
    log_llm_event(
        "agent.verifier.weak_repair_shadow",
        serde_json::json!({
            "session_id": session_id,
            "turn_index": turn_index,
            "iter_index": iter_index,
            "source": source,
            "weak_reason": target.reason.label(),
            "target_job": target.job_kind.label(),
            "expected_delta": target.expected_delta,
            "adoption_state": target.adoption_state,
            "terminal_behavior": "unchanged_safe_stop",
        }),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn structured_selection_projects_to_missing_evidence_shadow_target() {
        let target = shadow_repair_target(VerifierWeakReason::StructuredSelectionUnbound);

        assert_eq!(
            target.job_kind,
            VerifierWeakRepairTargetKind::MissingEvidenceJob
        );
        assert_eq!(target.expected_delta, "produce_bound_owned_test_evidence");
        assert_eq!(target.adoption_state, "shadow_only");
    }

    #[test]
    fn zero_bound_exit_projects_to_failed_evidence_shadow_target() {
        let target = shadow_repair_target(VerifierWeakReason::ExitZeroWithoutOwnedBinding);

        assert_eq!(
            target.job_kind,
            VerifierWeakRepairTargetKind::EvidenceFailedJob
        );
        assert_eq!(
            target.expected_delta,
            "rerun_verifier_with_owned_test_binding"
        );
        assert_eq!(target.adoption_state, "shadow_only");
    }
}
