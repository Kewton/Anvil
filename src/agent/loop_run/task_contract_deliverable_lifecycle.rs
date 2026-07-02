//! Objective deliverable lifecycle stage planning.
//!
//! This module is deliberately narrower than the full recovery planner: it only
//! decides whether declared deliverables are satisfied and, if not, which
//! missing role/path should drive the next recovery step.

use super::task_contract::{
    ArtifactRecoveryAction, ArtifactRecoveryInputs, ArtifactRole, RecoveryTargetHint,
    required_role_satisfied,
};
use super::task_contract_recovery_planning::{
    order_missing_deliverables_for_recovery, recovery_target_hint_for_missing_with_contract,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ObjectiveLifecycleStage {
    MissingDeliverable {
        missing: Vec<ArtifactRole>,
        target_hint: Option<RecoveryTargetHint>,
    },
    DeliverablesSatisfied,
}

impl ObjectiveLifecycleStage {
    pub(super) fn into_recovery_action(self) -> Option<ArtifactRecoveryAction> {
        match self {
            ObjectiveLifecycleStage::MissingDeliverable {
                missing,
                target_hint,
            } => Some(ArtifactRecoveryAction::Continue {
                missing,
                target_hint,
            }),
            ObjectiveLifecycleStage::DeliverablesSatisfied => None,
        }
    }
}

pub(super) fn objective_deliverable_stage(
    inputs: &ArtifactRecoveryInputs<'_>,
    observed: &[ArtifactRole],
) -> ObjectiveLifecycleStage {
    let objective = inputs.contract.objective_contract();
    let mut missing = Vec::new();
    for role in objective.required_deliverables() {
        if required_role_satisfied(
            inputs.contract,
            inputs.evidence,
            inputs.artifacts,
            inputs.artifact_excerpts,
            observed,
            *role,
        ) {
            continue;
        }
        missing.push(*role);
    }
    order_missing_deliverables_for_recovery(&mut missing);

    if missing.is_empty() {
        return ObjectiveLifecycleStage::DeliverablesSatisfied;
    }

    ObjectiveLifecycleStage::MissingDeliverable {
        target_hint: recovery_target_hint_for_missing_with_contract(
            inputs.contract,
            inputs.artifacts,
            inputs.artifact_excerpts,
            &missing,
        ),
        missing,
    }
}
