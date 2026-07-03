use serde::Serialize;

use crate::minimal_loop::build_verifier;
use crate::minimal_loop::completion::CompletionContract;
use crate::minimal_loop::dependency_setup::NodeDependencySetupAuthority;
use crate::minimal_loop::evidence::{SatisfactionChannel, evidence_satisfaction_channel};
use crate::planner::verify::VerificationReport;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RepairActionClass {
    EditSourceArtifact,
    EditTestOrEvidence,
    EditManifestOrConfig,
    RunDependencySetup,
}

impl RepairActionClass {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::EditSourceArtifact => "edit_source_artifact",
            Self::EditTestOrEvidence => "edit_test_or_evidence",
            Self::EditManifestOrConfig => "edit_manifest_or_config",
            Self::RunDependencySetup => "run_dependency_setup",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepairReachability {
    pub reachable: bool,
    pub viable_actions: Vec<RepairActionClass>,
    pub blocked_requirements: Vec<String>,
}

pub fn assess_repair_reachability(
    report: &VerificationReport,
    contract: Option<&CompletionContract>,
    setup_authority: NodeDependencySetupAuthority,
    offline: bool,
) -> RepairReachability {
    let _ = contract;
    let mut assessment = RepairReachability {
        reachable: false,
        viable_actions: Vec::new(),
        blocked_requirements: Vec::new(),
    };

    if !report.verifier_command_false_negatives.is_empty() {
        push_blocked(&mut assessment, "deterministic_verify_command_bug");
        return assessment;
    }

    if !report.missing_paths.is_empty() {
        push_action(&mut assessment, RepairActionClass::EditSourceArtifact);
    }

    for reason in &report.dependency_missing {
        let _ = reason;
        assess_dependency_need(&mut assessment, setup_authority, offline);
    }

    for failure in &report.command_failures {
        if build_verifier::is_dependency_missing_output(&failure.reason)
            || build_verifier::is_dependency_missing_output(&failure.command)
        {
            assess_dependency_need(&mut assessment, setup_authority, offline);
        } else {
            push_action(&mut assessment, RepairActionClass::EditSourceArtifact);
        }
    }

    for reason in &report.profile_failures {
        assess_profile_failure(&mut assessment, reason);
    }

    assessment.reachable = !assessment.viable_actions.is_empty();
    assessment
}

pub fn reachability_failure_kind(reachability: &RepairReachability) -> &str {
    reachability
        .blocked_requirements
        .first()
        .map(String::as_str)
        .unwrap_or("repair_unreachable")
}

pub fn reachability_recovery_reason(reachability: &RepairReachability) -> String {
    let kind = reachability_failure_kind(reachability);
    if kind == "dependency_setup_blocked_offline" {
        return "dependency_setup_blocked_offline: dependency verification requires dependency setup lifecycle, but offline mode blocks install".to_string();
    }
    if kind == "dependency_setup_authority_required" {
        return "dependency_setup_authority_required: requires a Setup-authority step running dependency install before verification can pass".to_string();
    }
    if kind == "deterministic_verify_command_bug" {
        return "deterministic_verify_command_bug: the verify command is malformed; the artifact may already satisfy the requirement".to_string();
    }
    format!(
        "repair_unreachable: {}",
        reachability.blocked_requirements.join(", ")
    )
}

fn assess_dependency_need(
    assessment: &mut RepairReachability,
    setup_authority: NodeDependencySetupAuthority,
    offline: bool,
) {
    if setup_authority.allows_setup() && !offline {
        push_action(assessment, RepairActionClass::RunDependencySetup);
    } else if offline {
        push_blocked(assessment, "dependency_setup_blocked_offline");
    } else {
        push_blocked(assessment, "dependency_setup_authority_required");
    }
}

fn assess_profile_failure(assessment: &mut RepairReachability, reason: &str) {
    let mut matched_evidence = false;
    for evidence in evidence_keys_from_profile_failure(reason) {
        matched_evidence = true;
        push_evidence_action(assessment, &evidence);
    }
    if !matched_evidence && !looks_like_dependency_policy_block(reason) {
        push_action(assessment, RepairActionClass::EditSourceArtifact);
    }
}

fn evidence_keys_from_profile_failure(reason: &str) -> Vec<String> {
    let Some((prefix, rest)) = reason.split_once(':') else {
        return Vec::new();
    };
    if !matches!(
        prefix,
        "missing_required_evidence" | "weak_verification_evidence"
    ) {
        return Vec::new();
    }
    rest.split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect()
}

fn push_evidence_action(assessment: &mut RepairReachability, evidence: &str) {
    match evidence_satisfaction_channel(evidence) {
        SatisfactionChannel::SourceScan => {
            push_action(assessment, RepairActionClass::EditSourceArtifact)
        }
        SatisfactionChannel::TestArtifact => {
            push_action(assessment, RepairActionClass::EditTestOrEvidence)
        }
        SatisfactionChannel::RuntimeArtifact => {
            push_action(assessment, RepairActionClass::EditTestOrEvidence)
        }
    }
}

fn looks_like_dependency_policy_block(reason: &str) -> bool {
    let lower = reason.to_ascii_lowercase();
    lower.contains("dependency_setup_authority_required")
        || lower.contains("dependency_setup_blocked_offline")
        || lower.contains("dependency setup authority missing")
}

fn push_action(assessment: &mut RepairReachability, action: RepairActionClass) {
    if !assessment.viable_actions.contains(&action) {
        assessment.viable_actions.push(action);
    }
}

fn push_blocked(assessment: &mut RepairReachability, requirement: &str) {
    if !assessment
        .blocked_requirements
        .iter()
        .any(|existing| existing == requirement)
    {
        assessment
            .blocked_requirements
            .push(requirement.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dependency_missing_without_authority_is_unreachable() {
        let mut report = VerificationReport::pass();
        report.push_dependency_missing("Cannot find module 'next/package.json'");

        let reachability =
            assess_repair_reachability(&report, None, NodeDependencySetupAuthority::None, false);

        assert!(!reachability.reachable);
        assert_eq!(
            reachability.blocked_requirements,
            vec!["dependency_setup_authority_required".to_string()]
        );
        assert!(reachability.viable_actions.is_empty());
    }

    #[test]
    fn dependency_missing_with_setup_authority_online_is_reachable() {
        let mut report = VerificationReport::pass();
        report.push_dependency_missing("Cannot find module 'next/package.json'");

        let reachability = assess_repair_reachability(
            &report,
            None,
            NodeDependencySetupAuthority::PlanSetupStep,
            false,
        );

        assert!(reachability.reachable);
        assert_eq!(
            reachability.viable_actions,
            vec![RepairActionClass::RunDependencySetup]
        );
        assert!(reachability.blocked_requirements.is_empty());
    }

    #[test]
    fn source_scan_evidence_failure_is_reachable_by_source_edit() {
        let mut report = VerificationReport::pass();
        report.push_profile_failure("missing_required_evidence:failure_or_collision_evidence");

        let reachability =
            assess_repair_reachability(&report, None, NodeDependencySetupAuthority::None, false);

        assert!(reachability.reachable);
        assert_eq!(
            reachability.viable_actions,
            vec![RepairActionClass::EditSourceArtifact]
        );
    }

    #[test]
    fn mixed_source_evidence_and_blocked_dependency_stays_reachable() {
        let mut report = VerificationReport::pass();
        report.push_profile_failure("missing_required_evidence:failure_or_collision_evidence");
        report.push_dependency_missing("Cannot find module 'next/package.json'");

        let reachability =
            assess_repair_reachability(&report, None, NodeDependencySetupAuthority::None, false);

        assert!(reachability.reachable);
        assert_eq!(
            reachability.viable_actions,
            vec![RepairActionClass::EditSourceArtifact]
        );
        assert_eq!(
            reachability.blocked_requirements,
            vec!["dependency_setup_authority_required".to_string()]
        );
    }

    #[test]
    fn verifier_command_false_negative_is_unreachable() {
        let mut report = VerificationReport::pass();
        report.push_verifier_command_false_negative(
            "python3 usage_error.py",
            "verify_command_false_negative: usage: fake",
        );

        let reachability =
            assess_repair_reachability(&report, None, NodeDependencySetupAuthority::None, false);

        assert!(!reachability.reachable);
        assert!(reachability.viable_actions.is_empty());
        assert_eq!(
            reachability.blocked_requirements,
            vec!["deterministic_verify_command_bug".to_string()]
        );
        assert_eq!(
            reachability_recovery_reason(&reachability),
            "deterministic_verify_command_bug: the verify command is malformed; the artifact may already satisfy the requirement"
        );
    }
}
