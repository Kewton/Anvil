//! v0.4.16: patch-provider admission boundary.
//!
//! A provider proposes concrete edits only after the controller has accepted a
//! repair plan. The provider does not choose the target, role, authority, or
//! allowed change kind.

#![allow(dead_code)]

use super::patch_proposal::{
    PatchProposal, PatchProposalRejection, validate_patch_proposal_for_action,
};
use super::repair_brief::AllowedChangeKind;
use super::repair_plan::AcceptedRepairPlan;
use super::task_contract::ArtifactRole;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PatchProviderKind {
    MainLlmEdit,
    DiagnosticLlmAssisted,
    DeterministicFallback,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct PatchProviderRequest<'a> {
    pub(super) accepted_plan: &'a AcceptedRepairPlan,
    pub(super) target_contents: &'a str,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct PatchProviderOutput<'a> {
    pub(super) provider_kind: PatchProviderKind,
    pub(super) proposal: &'a PatchProposal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PatchAdmission {
    pub(super) provider_kind: PatchProviderKind,
    pub(super) target_role: ArtifactRole,
    pub(super) target_path: String,
    pub(super) allowed_change_kind: AllowedChangeKind,
    pub(super) edit_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum PatchAdmissionError {
    Proposal(PatchProposalRejection),
}

impl PatchAdmissionError {
    pub(super) fn as_str(&self) -> &'static str {
        match self {
            Self::Proposal(reason) => reason.as_str(),
        }
    }
}

pub(super) fn admit_patch_provider_output(
    request: PatchProviderRequest<'_>,
    output: PatchProviderOutput<'_>,
) -> Result<PatchAdmission, PatchAdmissionError> {
    let action = &request.accepted_plan.action;
    validate_patch_proposal_for_action(output.proposal, action, request.target_contents)
        .map_err(PatchAdmissionError::Proposal)?;

    Ok(PatchAdmission {
        provider_kind: output.provider_kind,
        target_role: action.target_role,
        target_path: action.target_path.clone(),
        allowed_change_kind: action.allowed_change_kind,
        edit_count: output.proposal.edits.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::super::patch_proposal::{PatchEdit, PatchProposal};
    use super::super::repair_action::RepairAction;
    use super::super::repair_brief::{RepairBriefSource, SourceOfTruth};
    use super::*;

    fn accepted_plan() -> AcceptedRepairPlan {
        AcceptedRepairPlan {
            action: RepairAction {
                target_role: ArtifactRole::Implementation,
                target_path: "app/main.py".to_string(),
                allowed_change_kind: AllowedChangeKind::FixImplementationBehavior,
                source_of_truth: SourceOfTruth::UserRequest,
                budget: 2,
                brief_confidence: 0.8,
            },
            proposal_source: RepairBriefSource::DiagnosticLlm,
        }
    }

    fn proposal(path: &str) -> PatchProposal {
        PatchProposal {
            target_path: path.to_string(),
            edits: vec![PatchEdit {
                old_string: "return 201".to_string(),
                new_string: "return 200".to_string(),
                reason: "align response status".to_string(),
                replace_all: false,
            }],
            explanation: String::new(),
            risk: String::new(),
        }
    }

    #[test]
    fn provider_output_is_admitted_for_accepted_plan_target() {
        let plan = accepted_plan();
        let proposal = proposal("app/main.py");

        let admission = admit_patch_provider_output(
            PatchProviderRequest {
                accepted_plan: &plan,
                target_contents: "def create():\n    return 201\n",
            },
            PatchProviderOutput {
                provider_kind: PatchProviderKind::DiagnosticLlmAssisted,
                proposal: &proposal,
            },
        )
        .expect("matching provider proposal should be admitted");

        assert_eq!(
            admission.provider_kind,
            PatchProviderKind::DiagnosticLlmAssisted
        );
        assert_eq!(admission.target_path, "app/main.py");
        assert_eq!(admission.target_role, ArtifactRole::Implementation);
        assert_eq!(admission.edit_count, 1);
    }

    #[test]
    fn provider_cannot_override_controller_selected_target() {
        let plan = accepted_plan();
        let proposal = proposal("tests/test_main.py");

        let error = admit_patch_provider_output(
            PatchProviderRequest {
                accepted_plan: &plan,
                target_contents: "def create():\n    return 201\n",
            },
            PatchProviderOutput {
                provider_kind: PatchProviderKind::DiagnosticLlmAssisted,
                proposal: &proposal,
            },
        )
        .unwrap_err();

        assert_eq!(error.as_str(), "target_mismatch");
    }

    #[test]
    fn provider_cannot_skip_exact_patch_validation() {
        let plan = accepted_plan();
        let proposal = proposal("app/main.py");

        let error = admit_patch_provider_output(
            PatchProviderRequest {
                accepted_plan: &plan,
                target_contents: "def create():\n    return 204\n",
            },
            PatchProviderOutput {
                provider_kind: PatchProviderKind::DeterministicFallback,
                proposal: &proposal,
            },
        )
        .unwrap_err();

        assert_eq!(error.as_str(), "old_string_not_found");
    }
}
