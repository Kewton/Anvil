//! v0.4.13 Phase 6: verifier-rerun progress classification.
//!
//! This is intentionally small and data-oriented. It does not decide the next
//! repair target; it gives the controller a stable verdict after a patch and a
//! verifier rerun.

#![allow(dead_code)]

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RepairProgressVerdict {
    Passed,
    Improved,
    Unchanged,
    Regressed,
    Invalid,
}

impl RepairProgressVerdict {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Passed => "passed",
            Self::Improved => "improved",
            Self::Unchanged => "unchanged",
            Self::Regressed => "regressed",
            Self::Invalid => "invalid",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RepairProgress {
    pub(super) verdict: RepairProgressVerdict,
    pub(super) failure_signature_changed: bool,
    pub(super) failed_case_count_delta: i32,
    pub(super) new_failure_introduced: bool,
}

pub(super) fn classify_repair_progress(
    before_signature: &str,
    before_failed_count: Option<usize>,
    after_signature: Option<&str>,
    after_failed_count: Option<usize>,
    patch_applied: bool,
    verifier_passed: bool,
) -> RepairProgress {
    if verifier_passed {
        return RepairProgress {
            verdict: RepairProgressVerdict::Passed,
            failure_signature_changed: true,
            failed_case_count_delta: negative_delta(before_failed_count.unwrap_or(0)),
            new_failure_introduced: false,
        };
    }
    if !patch_applied {
        return RepairProgress {
            verdict: RepairProgressVerdict::Invalid,
            failure_signature_changed: false,
            failed_case_count_delta: 0,
            new_failure_introduced: false,
        };
    }

    let after_signature = after_signature.unwrap_or_default();
    let failure_signature_changed =
        !after_signature.is_empty() && after_signature != before_signature;
    let before = before_failed_count.unwrap_or(0);
    let after = after_failed_count.unwrap_or(before);
    let failed_case_count_delta = after as i32 - before as i32;
    let new_failure_introduced = failure_signature_changed && after >= before;
    let verdict = if after < before {
        RepairProgressVerdict::Improved
    } else if after > before || new_failure_introduced {
        RepairProgressVerdict::Regressed
    } else {
        RepairProgressVerdict::Unchanged
    };

    RepairProgress {
        verdict,
        failure_signature_changed,
        failed_case_count_delta,
        new_failure_introduced,
    }
}

fn negative_delta(value: usize) -> i32 {
    -(value.min(i32::MAX as usize) as i32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_passed_wins() {
        let progress = classify_repair_progress("a", Some(3), None, None, true, true);

        assert_eq!(progress.verdict, RepairProgressVerdict::Passed);
        assert_eq!(progress.verdict.as_str(), "passed");
        assert_eq!(progress.failed_case_count_delta, -3);
    }

    #[test]
    fn progress_invalid_when_no_patch_applied() {
        let progress = classify_repair_progress("a", Some(3), Some("a"), Some(3), false, false);

        assert_eq!(progress.verdict, RepairProgressVerdict::Invalid);
    }

    #[test]
    fn progress_improved_when_failure_count_drops() {
        let progress = classify_repair_progress("a", Some(3), Some("b"), Some(1), true, false);

        assert_eq!(progress.verdict, RepairProgressVerdict::Improved);
        assert!(progress.failure_signature_changed);
        assert_eq!(progress.failed_case_count_delta, -2);
    }

    #[test]
    fn progress_unchanged_when_signature_and_count_same() {
        let progress = classify_repair_progress("a", Some(3), Some("a"), Some(3), true, false);

        assert_eq!(progress.verdict, RepairProgressVerdict::Unchanged);
    }

    #[test]
    fn progress_regressed_when_new_signature_without_count_drop() {
        let progress = classify_repair_progress("a", Some(3), Some("b"), Some(3), true, false);

        assert_eq!(progress.verdict, RepairProgressVerdict::Regressed);
        assert!(progress.new_failure_introduced);
    }
}
