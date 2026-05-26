//! Verifier-repair admission boundary.
//!
//! A diagnostic result is not executable authority by itself. This module
//! converts a bounded diagnostic brief into an accepted repair plan only after
//! validating it against the current failure packet and authority evidence.

use super::failure_packet::FailurePacket;
use super::repair_action::RepairActionRejection;
use super::repair_authority::{AuthorityEvidence, RepairPlanRejection};
use super::repair_brief::{LegacyDiagnosticBriefInput, SourceOfTruth};
use super::repair_job::{RepairJob, RepairJobEvent};
use super::repair_plan::{AcceptedRepairPlan, RepairPlanProposal, RepairPlanValidationError};
use super::spec_authority::SpecAuthority;

#[derive(Debug, Clone, PartialEq)]
pub(super) struct RepairPlanAdmissionInput<'a> {
    pub(super) context: &'a RepairJob,
    pub(super) active_request: &'a str,
    pub(super) behavior_contract_present: bool,
    pub(super) legacy_input: Option<LegacyDiagnosticBriefInput>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RepairPlanAdmissionError {
    MissingDiagnosticAssessment,
    MalformedDiagnosticBrief,
    Rejected(RepairPlanRejection),
}

impl RepairPlanAdmissionError {
    pub(super) fn message(self) -> String {
        match self {
            Self::MissingDiagnosticAssessment => {
                "repair plan rejected: missing diagnostic assessment".to_string()
            }
            Self::MalformedDiagnosticBrief => {
                "repair plan rejected: malformed diagnostic brief".to_string()
            }
            Self::Rejected(reason) => format!("repair plan rejected: {}", reason.as_str()),
        }
    }
}

pub(super) fn admission_error_event(err: &RepairPlanAdmissionError) -> RepairJobEvent {
    match err {
        RepairPlanAdmissionError::Rejected(
            RepairPlanRejection::AmbiguousAuthority
            | RepairPlanRejection::TestExpectationWithoutAuthority
            | RepairPlanRejection::TestExpectationContradictsAuthority,
        )
        | RepairPlanAdmissionError::Rejected(RepairPlanRejection::Action(
            RepairActionRejection::AmbiguousSpec
            | RepairActionRejection::TestExpectationBlockedByUserRequest
            | RepairActionRejection::TestExpectationWithoutAuthority,
        )) => RepairJobEvent::AmbiguousAuthority,
        RepairPlanAdmissionError::MissingDiagnosticAssessment
        | RepairPlanAdmissionError::MalformedDiagnosticBrief
        | RepairPlanAdmissionError::Rejected(_) => RepairJobEvent::DiagnosticMalformed,
    }
}

pub(super) fn validate_repair_plan_admission(
    input: RepairPlanAdmissionInput<'_>,
) -> Result<AcceptedRepairPlan, RepairPlanAdmissionError> {
    let packet = FailurePacket::from_repair_job(input.context);
    let mut brief = super::repair_brief::repair_brief_from_legacy_diagnostic(
        input
            .legacy_input
            .ok_or(RepairPlanAdmissionError::MissingDiagnosticAssessment)?,
    )
    .map_err(|_| RepairPlanAdmissionError::MalformedDiagnosticBrief)?;
    if let Some(plan) = input.context.semantic_plan.as_ref() {
        brief.source_of_truth =
            source_of_truth_from_spec_authority(plan.spec_authority, brief.source_of_truth);
    }
    let evidence = AuthorityEvidence::from_packet_and_context(
        &packet,
        input.active_request,
        input.behavior_contract_present,
    );
    let proposal = RepairPlanProposal::from_brief(brief);
    super::repair_plan::validate_repair_plan_proposal(&proposal, &packet, &evidence).map_err(
        |reason| match reason {
            RepairPlanValidationError::Rejected(reason) => {
                RepairPlanAdmissionError::Rejected(reason)
            }
        },
    )
}

fn source_of_truth_from_spec_authority(
    authority: SpecAuthority,
    fallback: SourceOfTruth,
) -> SourceOfTruth {
    match authority {
        SpecAuthority::UserRequest => SourceOfTruth::UserRequest,
        SpecAuthority::BehaviorContract => SourceOfTruth::BehaviorContract,
        SpecAuthority::VerifiedPublicInterface => SourceOfTruth::VerifiedPublicInterface,
        SpecAuthority::ImplementationContract => SourceOfTruth::ImplementationContract,
        SpecAuthority::LlmGeneratedTest => fallback,
    }
}

#[cfg(test)]
mod tests {
    use super::super::repair_action::RepairActionRejection;
    use super::super::repair_authority::RepairPlanRejection;
    use super::super::repair_job::{RepairJob, RepairJobEvent, RepairNextAction};
    use super::*;

    #[test]
    fn missing_assessment_forces_re_diagnostic() {
        let event = admission_error_event(&RepairPlanAdmissionError::MissingDiagnosticAssessment);
        assert!(matches!(event, RepairJobEvent::DiagnosticMalformed));

        let mut job = RepairJob::new_for_test();
        job.apply_event(event);

        assert_eq!(job.next_action(), RepairNextAction::RequestDiagnostic);
    }

    #[test]
    fn ambiguous_authority_maps_to_safe_stop_event() {
        let event = admission_error_event(&RepairPlanAdmissionError::Rejected(
            RepairPlanRejection::AmbiguousAuthority,
        ));

        assert!(matches!(event, RepairJobEvent::AmbiguousAuthority));
    }

    #[test]
    fn malformed_patch_action_maps_to_re_diagnostic_event() {
        let event = admission_error_event(&RepairPlanAdmissionError::Rejected(
            RepairPlanRejection::Action(RepairActionRejection::RoleMismatch),
        ));

        assert!(matches!(event, RepairJobEvent::DiagnosticMalformed));
    }
}
