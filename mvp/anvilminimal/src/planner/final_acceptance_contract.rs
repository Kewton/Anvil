use std::path::Path;

use crate::planner::contract_attribute_repair::{
    self, ContractAttributeIssue, guidance_for_issue, issue_from_hook_status,
};
use crate::planner::profile::profile_evidence_repair_target_paths;
use crate::planner::state_binding_scan::final_acceptance_actionable_diagnosis;
use crate::planner::verify::VerificationReport;

pub(crate) fn issue_for_hook_status(
    root: &Path,
    profile: &str,
    report: &VerificationReport,
    status: &str,
) -> Option<ContractAttributeIssue> {
    let path = final_acceptance_actionable_diagnosis(root, profile, report)
        .map(|diagnosis| diagnosis.path)
        .or_else(|| {
            profile_evidence_repair_target_paths(
                root,
                profile,
                &["browser_interaction_failed".to_string()],
            )
            .into_iter()
            .next()
        })?;
    issue_from_hook_status(status, path)
}

pub(crate) fn guidance_for_hook_status(
    root: &Path,
    profile: &str,
    report: &VerificationReport,
    status: &str,
    eval_events_path: Option<&Path>,
) -> String {
    issue_for_hook_status(root, profile, report, status)
        .map(|issue| guidance_for_issue(Some(root), &issue, eval_events_path))
        .unwrap_or_default()
}

pub(crate) fn target_paths(
    root: &Path,
    profile: &str,
    report: &VerificationReport,
    hook_status: Option<&str>,
) -> Vec<String> {
    contract_attribute_repair::detect(report)
        .or_else(|| {
            hook_status.and_then(|status| issue_for_hook_status(root, profile, report, status))
        })
        .map(|issue| vec![issue.path])
        .unwrap_or_default()
}
