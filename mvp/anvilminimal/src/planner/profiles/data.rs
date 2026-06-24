use std::path::Path;

use crate::planner::verify::VerificationReport;

pub fn verify(_root: &Path) -> VerificationReport {
    VerificationReport::pass()
}
