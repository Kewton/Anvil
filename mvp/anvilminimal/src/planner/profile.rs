use std::path::Path;

use crate::planner::verify::VerificationReport;

#[derive(Debug, Clone)]
pub enum ProfileSnapshot {
    Data(crate::planner::profiles::data::ProfileSnapshot),
    None,
}

pub fn verify_profile(root: &Path, profile: &str, goal: &str) -> VerificationReport {
    match profile {
        "nextjs" | "next-js" | "next.js" => crate::planner::profiles::nextjs::verify(root, goal),
        "data" | "data-analysis" | "data-pipeline" => crate::planner::profiles::data::verify(root),
        _ => VerificationReport::pass(),
    }
}

pub fn profile_before_phase(root: &Path, profile: &str) -> anyhow::Result<ProfileSnapshot> {
    match profile {
        "data" | "data-analysis" | "data-pipeline" => Ok(ProfileSnapshot::Data(
            crate::planner::profiles::data::before_phase(root)?,
        )),
        _ => Ok(ProfileSnapshot::None),
    }
}

pub fn profile_after_phase(
    root: &Path,
    profile: &str,
    snapshot: &ProfileSnapshot,
) -> VerificationReport {
    match (profile, snapshot) {
        ("data" | "data-analysis" | "data-pipeline", ProfileSnapshot::Data(snapshot)) => {
            crate::planner::profiles::data::after_phase(root, snapshot)
        }
        _ => VerificationReport::pass(),
    }
}

pub fn profile_guidance(profile: &str, goal: &str) -> Option<String> {
    match profile {
        "nextjs" | "next-js" | "next.js" => Some(crate::planner::profiles::nextjs::guidance(goal)),
        _ => None,
    }
}

pub fn profile_expected_paths(root: &Path, profile: &str, goal: &str) -> Vec<String> {
    match profile {
        "nextjs" | "next-js" | "next.js" => {
            crate::planner::profiles::nextjs::expected_paths(root, goal)
        }
        _ => Vec::new(),
    }
}

pub fn profile_repair_prompt(
    root: &Path,
    profile: &str,
    goal: &str,
    report: &VerificationReport,
) -> Option<String> {
    match profile {
        "nextjs" | "next-js" | "next.js" => Some(crate::planner::profiles::nextjs::repair_prompt(
            root, goal, report,
        )),
        _ => None,
    }
}

pub fn profile_auto_repair(
    root: &Path,
    profile: &str,
    goal: &str,
    report: &VerificationReport,
) -> anyhow::Result<bool> {
    match profile {
        "nextjs" | "next-js" | "next.js" => {
            crate::planner::profiles::nextjs::auto_repair(root, goal, report)
        }
        _ => Ok(false),
    }
}

pub fn profile_failure(reason: impl Into<String>) -> VerificationReport {
    VerificationReport::profile_failed(reason)
}
