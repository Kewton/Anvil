use std::path::Path;

use serde_json::json;

use crate::eval_events;
use crate::minimal_loop::dependency_setup::NodeDependencySetupAuthority;
use crate::minimal_loop::reachability::RepairActionClass;
use crate::minimal_loop::reachability::{RepairReachability, assess_repair_reachability};
use crate::planner::verify::VerificationReport;

pub(crate) const DEPENDENCY_DENIAL_GUIDANCE: &str = "Dependency installation is forbidden for this profile. Rewrite with the Python 3 standard library only (csv/json/statistics); do not run pip install or add dependencies.";

pub(crate) struct StepRepairPolicy<'a> {
    profile: &'a str,
    step_id: &'a str,
    max_attempts: usize,
    eval_events_path: Option<&'a Path>,
}

impl<'a> StepRepairPolicy<'a> {
    pub(crate) fn new(
        profile: &'a str,
        step_id: &'a str,
        max_attempts: usize,
        eval_events_path: Option<&'a Path>,
    ) -> Self {
        Self {
            profile,
            step_id,
            max_attempts,
            eval_events_path,
        }
    }

    pub(crate) fn assess(
        &self,
        report: &VerificationReport,
        setup_authority: NodeDependencySetupAuthority,
        offline: bool,
        attempt: usize,
    ) -> RepairReachability {
        let mut reachability = assess_repair_reachability(report, None, setup_authority, offline);
        apply_dependency_denial(
            self.profile,
            self.step_id,
            &mut reachability,
            attempt,
            self.max_attempts,
            self.eval_events_path,
        );
        reachability
    }
}

fn apply_dependency_denial(
    profile: &str,
    step_id: &str,
    reachability: &mut RepairReachability,
    attempt: usize,
    max_attempts: usize,
    eval_events_path: Option<&Path>,
) {
    if !is_data_profile(Some(profile))
        || !reachability
            .blocked_requirements
            .iter()
            .any(|item| item == "dependency_setup_authority_required")
    {
        return;
    }

    let repair_exhausted = attempt >= max_attempts;
    if repair_exhausted {
        reachability.reachable = false;
        reachability.viable_actions.clear();
    } else {
        reachability
            .blocked_requirements
            .retain(|item| item != "dependency_setup_authority_required");
        if !reachability
            .viable_actions
            .contains(&RepairActionClass::EditSourceArtifact)
        {
            reachability
                .viable_actions
                .push(RepairActionClass::EditSourceArtifact);
        }
        reachability.reachable = true;
    }

    eval_events::emit(
        eval_events_path,
        json!({
            "event": "dependency_denial_guidance",
            "step_id": step_id,
            "profile": "data",
            "attempt": attempt,
            "max_attempts": max_attempts,
            "guidance": DEPENDENCY_DENIAL_GUIDANCE,
            "repair_exhausted": repair_exhausted,
            "repair_reachable": reachability.reachable,
        }),
    );
}

pub(crate) fn profile_guidance(
    profile: Option<&str>,
    report: &VerificationReport,
) -> Option<String> {
    if !is_data_profile(profile) {
        return None;
    }
    let mut guidance = Vec::new();
    if report_mentions_dependency_denial(report) {
        guidance.push(DEPENDENCY_DENIAL_GUIDANCE.to_string());
    }
    if report.profile_failures.iter().any(|reason| {
        super::manifest::check_ids()
            .iter()
            .any(|id| reason == id || reason.starts_with(&format!("{id}:")))
    }) {
        guidance.push(
            super::manifest::get()
                .guidance
                .contracts
                .contract_attribute_guidance
                .clone(),
        );
    }
    (!guidance.is_empty()).then(|| guidance.join("\n"))
}

fn is_data_profile(profile: Option<&str>) -> bool {
    profile.is_some_and(|profile| crate::planner::profile::domain_profile(profile).id() == "data")
}

fn report_mentions_dependency_denial(report: &VerificationReport) -> bool {
    report
        .dependency_missing
        .iter()
        .chain(report.profile_failures.iter())
        .any(|reason| reason.contains("dependency_setup_authority_required"))
        || report.command_failures.iter().any(|failure| {
            failure
                .reason
                .contains("dependency_setup_authority_required")
                || failure
                    .command
                    .contains("dependency_setup_authority_required")
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::PromptLayout;
    use crate::minimal_loop::reachability::RepairActionClass;
    use crate::planner::repair::{RepairContext, build_repair_prompt_with_context};

    fn dependency_denial_report() -> VerificationReport {
        let mut report = VerificationReport::pass();
        report.push_dependency_missing(
            "dependency_setup_authority_required: python3 -m pip install pandas",
        );
        report
    }

    fn reachability(report: &VerificationReport) -> RepairReachability {
        assess_repair_reachability(report, None, NodeDependencySetupAuthority::None, false)
    }

    #[test]
    fn dependency_denial_becomes_bounded_source_repair_guidance() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let report = dependency_denial_report();
        let mut assessment = reachability(&report);

        apply_dependency_denial(
            "data",
            "install-pandas",
            &mut assessment,
            0,
            4,
            Some(&events),
        );

        assert!(assessment.reachable);
        assert_eq!(
            assessment.viable_actions,
            [RepairActionClass::EditSourceArtifact]
        );
        assert!(assessment.blocked_requirements.is_empty());
        assert_eq!(
            profile_guidance(Some("data"), &report).as_deref(),
            Some(DEPENDENCY_DENIAL_GUIDANCE)
        );
        let event = std::fs::read_to_string(events).unwrap();
        assert!(event.contains("\"event\":\"dependency_denial_guidance\""));
        assert!(event.contains("\"attempt\":0"));
        assert!(event.contains("\"repair_exhausted\":false"));
        assert!(event.contains("Python 3 standard library only"));
    }

    #[test]
    fn repeated_dependency_denial_returns_to_honest_exhaustion_at_budget() {
        let report = dependency_denial_report();
        for attempt in 0..4 {
            let mut assessment = reachability(&report);
            apply_dependency_denial("data", "install-pandas", &mut assessment, attempt, 4, None);
            assert!(assessment.reachable, "attempt {attempt}");
        }

        let mut exhausted = reachability(&report);
        apply_dependency_denial("data", "install-pandas", &mut exhausted, 4, 4, None);
        assert!(!exhausted.reachable);
        assert!(exhausted.viable_actions.is_empty());
        assert_eq!(
            exhausted.blocked_requirements,
            ["dependency_setup_authority_required"]
        );
    }

    #[test]
    fn data_contract_failure_injects_manifest_guidance_into_repair_prompt() {
        let mut report = VerificationReport::pass();
        report.push_profile_failure(
            "data_results_schema:missing required key `reconciliation`".to_string(),
        );
        let context = RepairContext {
            profile: Some("data".to_string()),
            prompt_layout: PromptLayout::Stable,
            ..RepairContext::default()
        };

        let prompt = build_repair_prompt_with_context("verify-results", &report, &context);

        assert!(prompt.contains("Profile repair guidance:"));
        assert!(prompt.contains("output/results.json exactly as"));
        assert!(prompt.contains("\"reconciliation\""));
        assert!(prompt.contains("Never weaken or replace the manifest-bound evidence checks"));
    }

    #[test]
    fn dependency_policy_does_not_change_nextjs_reachability_or_prompt() {
        let report = dependency_denial_report();
        let mut assessment = reachability(&report);
        let before = assessment.clone();

        apply_dependency_denial("nextjs", "setup-nextjs", &mut assessment, 0, 4, None);

        assert_eq!(assessment, before);
        assert_eq!(profile_guidance(Some("nextjs"), &report), None);
    }
}
