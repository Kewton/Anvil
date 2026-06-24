use std::path::Path;

use crate::planner::verify::{VerificationReport, VerifyStatus};

pub fn verify_profile(root: &Path, profile: &str, goal: &str) -> VerificationReport {
    match profile {
        "nextjs" | "next-js" | "next.js" => crate::planner::profiles::nextjs::verify(root, goal),
        "data" | "data-analysis" | "data-pipeline" => crate::planner::profiles::data::verify(root),
        _ => VerificationReport::pass(),
    }
}

pub fn profile_failure(reason: impl Into<String>) -> VerificationReport {
    VerificationReport {
        status: VerifyStatus::ProfileContractFailed(reason.into()),
    }
}
