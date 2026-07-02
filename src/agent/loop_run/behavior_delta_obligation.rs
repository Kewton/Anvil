//! Shadow behavior-delta obligation for existing-feature changes.
//!
//! This module projects from the sealed `TaskContract`; it does not rescan the
//! raw user prompt and does not grant completion authority in WP4.

use super::completion_evidence::{CompletionEvidence, EvidenceSet, RepoEditCategory};
use super::task_contract::{TaskContract, TaskIntent, TaskKind};
use crate::logging::log_llm_event;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BehaviorDeltaAuthority {
    ShadowOnly,
    Required,
}

impl BehaviorDeltaAuthority {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::ShadowOnly => "shadow_only",
            Self::Required => "required",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct BehaviorDeltaObligation {
    pub(super) behavior_goal: String,
    pub(super) affected_surface: Option<String>,
    pub(super) expected_observable_behavior: Vec<String>,
    pub(super) authority: BehaviorDeltaAuthority,
}

pub(super) fn project_behavior_delta_obligation(
    contract: &TaskContract,
) -> Option<BehaviorDeltaObligation> {
    if contract.task_kind != TaskKind::Coding {
        return None;
    }
    if !matches!(contract.intent, TaskIntent::Modify | TaskIntent::Fix) {
        return None;
    }
    let rb = &contract.required_behavior;
    if !rb.confidence.is_finite()
        || rb.confidence < super::required_behavior::LOW_CONFIDENCE_THRESHOLD
    {
        return None;
    }
    let behavior_goal = rb.behavior_goal.as_ref()?.label.clone();
    let affected_surface = rb
        .domain_terms
        .as_ref()
        .and_then(|terms| terms.iter().find(|term| !term.trim().is_empty()).cloned())
        .or_else(|| {
            rb.required_capabilities
                .as_ref()
                .and_then(|items| items.iter().map(|item| item.label.clone()).next())
        });
    let expected_observable_behavior = rb
        .verification_expectations
        .as_ref()
        .map(|items| items.iter().map(|item| item.label.clone()).collect())
        .unwrap_or_default();

    Some(BehaviorDeltaObligation {
        behavior_goal,
        affected_surface,
        expected_observable_behavior,
        authority: BehaviorDeltaAuthority::ShadowOnly,
    })
}

pub(super) fn project_required_behavior_delta_obligation(
    contract: &TaskContract,
) -> Option<BehaviorDeltaObligation> {
    let mut obligation = project_behavior_delta_obligation(contract)?;
    if contract.required_artifact_identities.is_empty() {
        return None;
    }
    obligation.authority = BehaviorDeltaAuthority::Required;
    Some(obligation)
}

pub(super) fn required_behavior_delta_missing_source_edit(
    contract: &TaskContract,
    evidence: &EvidenceSet,
) -> bool {
    project_required_behavior_delta_obligation(contract).is_some()
        && !evidence.iter().any(|item| {
            matches!(
                item,
                CompletionEvidence::RepoEdit {
                    category: RepoEditCategory::Impl,
                    ..
                }
            )
        })
}

pub(super) fn log_behavior_delta_shadow(
    session_id: &str,
    turn_index: usize,
    obligation: &BehaviorDeltaObligation,
) {
    log_llm_event(
        "agent.behavior_delta.shadow",
        serde_json::json!({
            "session_id": session_id,
            "turn_index": turn_index,
            "authority": obligation.authority.label(),
            "goal_present": !obligation.behavior_goal.is_empty(),
            "affected_surface_present": obligation.affected_surface.is_some(),
            "expected_observable_count": obligation.expected_observable_behavior.len(),
            "completion_authority": false,
        }),
    );
}

pub(super) fn log_behavior_delta_required(
    session_id: &str,
    turn_index: usize,
    obligation: &BehaviorDeltaObligation,
) {
    log_llm_event(
        "agent.behavior_delta.required",
        serde_json::json!({
            "session_id": session_id,
            "turn_index": turn_index,
            "authority": obligation.authority.label(),
            "goal_present": !obligation.behavior_goal.is_empty(),
            "affected_surface_present": obligation.affected_surface.is_some(),
            "expected_observable_count": obligation.expected_observable_behavior.len(),
            "requires_current_source_edit": true,
            "completion_authority": false,
        }),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::loop_run::task_contract::TaskContract;

    #[test]
    fn feature_modify_projects_shadow_behavior_delta() {
        let contract = TaskContract::from_request(
            "Improve existing discounts.py final_price behavior and update tests",
        );

        let obligation = project_behavior_delta_obligation(&contract)
            .expect("feature modification should project behavior delta");
        assert_eq!(obligation.authority, BehaviorDeltaAuthority::ShadowOnly);
        assert!(!obligation.behavior_goal.is_empty());
    }

    #[test]
    fn non_coding_tasks_do_not_project_behavior_delta() {
        let docs = TaskContract::from_request("Update README.md with usage documentation");
        let data = TaskContract::from_request("Create output/profile-summary.json");

        assert!(project_behavior_delta_obligation(&docs).is_none());
        assert!(project_behavior_delta_obligation(&data).is_none());
    }

    #[test]
    fn explain_request_does_not_project_behavior_delta() {
        let contract = TaskContract::from_request("Explain how the cache works");

        assert!(project_behavior_delta_obligation(&contract).is_none());
    }

    #[test]
    fn required_delta_requires_current_source_edit_for_feature_change() {
        let contract = TaskContract::from_request(
            "Improve the existing discounts.py function final_price(price, percent). It should return the price after applying the percentage discount, rounded to 2 decimal places. Keep the existing zero-discount behavior and update tests/test_discounts.py.",
        );
        assert!(project_required_behavior_delta_obligation(&contract).is_some());

        let evidence = EvidenceSet::new();
        assert!(required_behavior_delta_missing_source_edit(
            &contract, &evidence
        ));
    }

    #[test]
    fn test_only_edit_does_not_satisfy_required_delta_source_edit() {
        let contract = TaskContract::from_request(
            "Improve existing discounts.py final_price behavior and update tests/test_discounts.py",
        );
        let mut evidence = EvidenceSet::new();
        evidence.push(CompletionEvidence::RepoEdit {
            category: RepoEditCategory::Test,
            count: 1,
            path: Some("tests/test_discounts.py".to_string()),
        });

        assert!(required_behavior_delta_missing_source_edit(
            &contract, &evidence
        ));
    }

    #[test]
    fn implementation_edit_satisfies_required_delta_source_edit() {
        let contract = TaskContract::from_request(
            "Improve existing discounts.py final_price behavior and update tests/test_discounts.py",
        );
        let mut evidence = EvidenceSet::new();
        evidence.push(CompletionEvidence::RepoEdit {
            category: RepoEditCategory::Impl,
            count: 1,
            path: Some("discounts.py".to_string()),
        });

        assert!(!required_behavior_delta_missing_source_edit(
            &contract, &evidence
        ));
    }

    #[test]
    fn build_request_does_not_require_behavior_delta_source_edit() {
        let contract = TaskContract::from_request(
            "Create a Python CLI in main.py that prints JSON totals and add tests.",
        );
        let evidence = EvidenceSet::new();

        assert!(project_required_behavior_delta_obligation(&contract).is_none());
        assert!(!required_behavior_delta_missing_source_edit(
            &contract, &evidence
        ));
    }
}
