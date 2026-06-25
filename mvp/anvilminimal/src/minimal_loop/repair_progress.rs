use crate::planner::verify::VerificationReport;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerificationSignature {
    pub missing_paths: Vec<String>,
    pub dependency_missing: Vec<String>,
    pub command_failures: Vec<String>,
    pub profile_failures: Vec<String>,
}

impl VerificationSignature {
    pub fn from_report(report: &VerificationReport) -> Self {
        let mut missing_paths = report.missing_paths.clone();
        missing_paths.sort();
        let mut dependency_missing = report.dependency_missing.clone();
        dependency_missing.sort();
        let mut command_failures = report
            .command_failures
            .iter()
            .map(|failure| {
                format!(
                    "{}:{}",
                    failure.command,
                    normalize_failure_reason(&failure.reason)
                )
            })
            .collect::<Vec<_>>();
        command_failures.sort();
        let mut profile_failures = report.profile_failures.clone();
        profile_failures.sort();
        Self {
            missing_paths,
            dependency_missing,
            command_failures,
            profile_failures,
        }
    }

    pub fn total_failures(&self) -> usize {
        self.missing_paths.len()
            + self.dependency_missing.len()
            + self.command_failures.len()
            + self.profile_failures.len()
    }

    pub fn label(&self) -> String {
        format!(
            "missing={};dependency={};commands={};profile={}",
            self.missing_paths.join("|"),
            self.dependency_missing.join("|"),
            self.command_failures.join("|"),
            self.profile_failures.join("|")
        )
    }

    pub fn has_test_discovery_failure(&self) -> bool {
        self.command_failures
            .iter()
            .any(|failure| failure.contains("test_discovery_failure"))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepairProgressVerdict {
    Passed,
    Improved,
    Unchanged,
    Regressed,
    Invalid,
}

impl RepairProgressVerdict {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Passed => "passed",
            Self::Improved => "improved",
            Self::Unchanged => "unchanged",
            Self::Regressed => "regressed",
            Self::Invalid => "invalid",
        }
    }
}

pub fn classify_repair_progress(
    previous: Option<&VerificationSignature>,
    current_report: &VerificationReport,
    had_edit: bool,
) -> (VerificationSignature, RepairProgressVerdict) {
    if current_report.is_pass() {
        return (
            VerificationSignature::from_report(current_report),
            RepairProgressVerdict::Passed,
        );
    }
    if !had_edit && previous.is_some() {
        return (
            VerificationSignature::from_report(current_report),
            RepairProgressVerdict::Invalid,
        );
    }
    let current = VerificationSignature::from_report(current_report);
    let Some(previous) = previous else {
        return (current, RepairProgressVerdict::Unchanged);
    };
    let verdict = if &current == previous {
        RepairProgressVerdict::Unchanged
    } else if current.total_failures() < previous.total_failures() {
        RepairProgressVerdict::Improved
    } else if current.total_failures() > previous.total_failures() {
        RepairProgressVerdict::Regressed
    } else {
        RepairProgressVerdict::Unchanged
    };
    (current, verdict)
}

fn normalize_failure_reason(reason: &str) -> String {
    let lower = reason.to_ascii_lowercase();
    if lower.contains("no tests ran") || lower.contains("ran 0 tests") {
        return "test_discovery_failure:no_tests_ran".to_string();
    }
    if lower.contains("module not found") || lower.contains("no module named") {
        return "dependency_missing:module_not_found".to_string();
    }
    if lower.contains("assert") || lower.contains("assertion") {
        return "assertion_failure".to_string();
    }
    reason
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or(reason)
        .trim()
        .chars()
        .take(160)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planner::verify::VerificationReport;

    #[test]
    fn repair_progress_improved_when_command_failures_decrease() {
        let mut previous = VerificationReport::pass();
        previous.push_missing_path("a.py");
        previous.push_command_failure("python3 -m unittest test_a.py", "AssertionError: bad");
        let mut current = VerificationReport::pass();
        current.push_command_failure("python3 -m unittest test_a.py", "AssertionError: bad");
        let previous = VerificationSignature::from_report(&previous);
        let (_, verdict) = classify_repair_progress(Some(&previous), &current, true);
        assert_eq!(verdict, RepairProgressVerdict::Improved);
    }

    #[test]
    fn repair_progress_unchanged_for_same_signature() {
        let mut previous = VerificationReport::pass();
        previous.push_command_failure("python3 -m unittest test_a.py", "AssertionError: bad");
        let current = previous.clone();
        let previous = VerificationSignature::from_report(&previous);
        let (_, verdict) = classify_repair_progress(Some(&previous), &current, true);
        assert_eq!(verdict, RepairProgressVerdict::Unchanged);
    }

    #[test]
    fn repair_progress_invalid_without_edit() {
        let mut previous = VerificationReport::pass();
        previous.push_command_failure("python3 -m unittest test_a.py", "AssertionError: bad");
        let current = previous.clone();
        let previous = VerificationSignature::from_report(&previous);
        let (_, verdict) = classify_repair_progress(Some(&previous), &current, false);
        assert_eq!(verdict, RepairProgressVerdict::Invalid);
    }

    #[test]
    fn signature_detects_no_tests_ran() {
        let mut report = VerificationReport::pass();
        report.push_command_failure("python3 -m unittest test_a.py", "NO TESTS RAN");
        let signature = VerificationSignature::from_report(&report);
        assert!(signature.has_test_discovery_failure());
    }
}
