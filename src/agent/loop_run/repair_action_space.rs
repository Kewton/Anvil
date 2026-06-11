//! Typed action space for verifier repair prompts.
//!
//! This projection is deliberately small: it does not choose targets, parse
//! diagnostics, or validate patches. It turns the already-selected target and
//! optional controller-admitted `RepairAction` into a stable payload the repair
//! editor can follow, especially after a malformed proposal is rejected.

use super::repair_action::RepairAction;
use super::task_contract::{ArtifactRole, RecoveryTargetHint};
use crate::session::feedback::mask_secrets;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RepairActionPlan {
    pub(super) status: RepairActionPlanStatus,
    pub(super) target_role: Option<ArtifactRole>,
    pub(super) target_path: Option<String>,
    pub(super) allowed_change_kind: Option<String>,
    pub(super) allowed_tool_category: &'static str,
    pub(super) expected_evidence_delta: &'static str,
    pub(super) rejection_reason: Option<&'static str>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RepairActionPlanStatus {
    Admissible,
    Rejected,
}

impl RepairActionPlanStatus {
    fn label(self) -> &'static str {
        match self {
            Self::Admissible => "admissible",
            Self::Rejected => "rejected",
        }
    }
}

impl RepairActionPlan {
    pub(super) fn to_json_value(&self) -> serde_json::Value {
        let target_path = self.target_path.as_deref().map(mask_secrets);
        serde_json::json!({
            "status": self.status.label(),
            "target_artifact": {
                "role": self.target_role.map(ArtifactRole::label),
                "path": target_path,
            },
            "allowed_change_kind": self.allowed_change_kind,
            "allowed_tool_category": self.allowed_tool_category,
            "expected_evidence_delta": self.expected_evidence_delta,
            "rejection_reason": self.rejection_reason,
        })
    }
}

pub(super) fn repair_action_plan_for_selected_target(
    target_hint: Option<&RecoveryTargetHint>,
    accepted_action: Option<&RepairAction>,
) -> RepairActionPlan {
    let Some(target_hint) = target_hint else {
        return rejected_action_plan("no_selected_target");
    };
    if let Some(action) = accepted_action {
        if action.target_role != target_hint.role || action.target_path != target_hint.path {
            return RepairActionPlan {
                status: RepairActionPlanStatus::Rejected,
                target_role: Some(target_hint.role),
                target_path: Some(target_hint.path.clone()),
                allowed_change_kind: Some(action.allowed_change_kind.as_str().to_string()),
                allowed_tool_category: "none",
                expected_evidence_delta: "none",
                rejection_reason: Some("accepted_action_target_mismatch"),
            };
        }
        return RepairActionPlan {
            status: RepairActionPlanStatus::Admissible,
            target_role: Some(target_hint.role),
            target_path: Some(target_hint.path.clone()),
            allowed_change_kind: Some(action.allowed_change_kind.as_str().to_string()),
            allowed_tool_category: "edit_selected_artifact",
            expected_evidence_delta: expected_evidence_delta_for_role(target_hint.role),
            rejection_reason: None,
        };
    }
    RepairActionPlan {
        status: RepairActionPlanStatus::Admissible,
        target_role: Some(target_hint.role),
        target_path: Some(target_hint.path.clone()),
        allowed_change_kind: Some("selected_target_edit".to_string()),
        allowed_tool_category: "edit_selected_artifact",
        expected_evidence_delta: expected_evidence_delta_for_role(target_hint.role),
        rejection_reason: None,
    }
}

fn rejected_action_plan(reason: &'static str) -> RepairActionPlan {
    RepairActionPlan {
        status: RepairActionPlanStatus::Rejected,
        target_role: None,
        target_path: None,
        allowed_change_kind: None,
        allowed_tool_category: "none",
        expected_evidence_delta: "none",
        rejection_reason: Some(reason),
    }
}

fn expected_evidence_delta_for_role(role: ArtifactRole) -> &'static str {
    match role {
        ArtifactRole::Implementation | ArtifactRole::Test => "verifier_result_should_change",
        ArtifactRole::Setup => "evidence_runner_binding_or_setup_should_change",
        ArtifactRole::UsageDocs => "content_or_command_observation_should_change",
        ArtifactRole::DataOutput => "data_schema_or_output_evidence_should_change",
    }
}

#[cfg(test)]
mod tests {
    use super::super::repair_brief::{AllowedChangeKind, SourceOfTruth};
    use super::*;

    fn hint(role: ArtifactRole, path: &str) -> RecoveryTargetHint {
        RecoveryTargetHint {
            role,
            path: path.to_string(),
            reason: "selected target".to_string(),
        }
    }

    fn action(role: ArtifactRole, path: &str, kind: AllowedChangeKind) -> RepairAction {
        RepairAction {
            target_role: role,
            target_path: path.to_string(),
            allowed_change_kind: kind,
            source_of_truth: SourceOfTruth::BehaviorContract,
            budget: 2,
            brief_confidence: 0.8,
        }
    }

    #[test]
    fn data_schema_repair_projects_data_evidence_delta() {
        let plan = repair_action_plan_for_selected_target(
            Some(&hint(ArtifactRole::DataOutput, "output/summary.csv")),
            None,
        );

        assert_eq!(plan.status, RepairActionPlanStatus::Admissible);
        assert_eq!(
            plan.expected_evidence_delta,
            "data_schema_or_output_evidence_should_change"
        );
        assert_eq!(plan.allowed_tool_category, "edit_selected_artifact");
    }

    #[test]
    fn api_body_mismatch_can_admit_implementation_action() {
        let target = hint(ArtifactRole::Implementation, "app.py");
        let accepted = action(
            ArtifactRole::Implementation,
            "app.py",
            AllowedChangeKind::FixImplementationBehavior,
        );

        let plan = repair_action_plan_for_selected_target(Some(&target), Some(&accepted));

        assert_eq!(plan.status, RepairActionPlanStatus::Admissible);
        assert_eq!(
            plan.allowed_change_kind.as_deref(),
            Some("fix_implementation_behavior")
        );
        assert_eq!(
            plan.expected_evidence_delta,
            "verifier_result_should_change"
        );
    }

    #[test]
    fn verifier_binding_repair_projects_setup_delta() {
        let plan = repair_action_plan_for_selected_target(
            Some(&hint(ArtifactRole::Setup, "package.json")),
            None,
        );

        assert_eq!(
            plan.expected_evidence_delta,
            "evidence_runner_binding_or_setup_should_change"
        );
    }

    #[test]
    fn no_target_is_not_admissible() {
        let plan = repair_action_plan_for_selected_target(None, None);

        assert_eq!(plan.status, RepairActionPlanStatus::Rejected);
        assert_eq!(plan.rejection_reason, Some("no_selected_target"));
    }

    #[test]
    fn accepted_action_must_match_selected_target() {
        let target = hint(ArtifactRole::Implementation, "app.py");
        let accepted = action(
            ArtifactRole::Test,
            "tests/test_app.py",
            AllowedChangeKind::FixGeneratedTestExpectation,
        );

        let plan = repair_action_plan_for_selected_target(Some(&target), Some(&accepted));

        assert_eq!(plan.status, RepairActionPlanStatus::Rejected);
        assert_eq!(
            plan.rejection_reason,
            Some("accepted_action_target_mismatch")
        );
    }
}
