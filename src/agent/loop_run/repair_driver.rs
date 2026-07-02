use std::time::Duration;

use super::repair_attempt_outcome::RepairAttemptOutcome;
use super::verifier_failure_signature::compact_verifier_failure_text;

pub(super) const VERIFIER_REPAIR_PASS_TIMEOUT_SECS: u64 = 90;
pub(super) const VERIFIER_REPAIR_PASS_WALL_CLOCK_LIMIT_SECS: u64 = 180;
pub(super) const VERIFIER_REPAIR_PASS_MAX_PREDICT: usize = 2_048;
pub(super) const VERIFIER_REPAIR_PASS_ATTEMPT_LIMIT: usize = 3;
pub(super) const VERIFIER_REPAIR_PASS_MAX_OUTPUT_BYTES: usize = 12_288;
pub(super) const VERIFIER_REPAIR_PASS_MAX_FILE_BYTES: u64 = 256 * 1024;
pub(super) const VERIFIER_REPAIR_PASS_MAX_FILE_EXCERPT_BYTES: usize = 8_192;
pub(super) const VERIFIER_REPAIR_PASS_MAX_EDIT_BYTES: usize = 32_768;
pub(super) const VERIFIER_REPAIR_PASS_MAX_REASON_CHARS: usize = 180;
pub(super) const VERIFIER_REPAIR_PASS_MAX_EDITS: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum VerifierRepairPassOutcome {
    Applied {
        relative_path: String,
    },
    Invalid {
        error: String,
        repair_attempt_outcome: Option<RepairAttemptOutcome>,
    },
    /// No safe project verifier exists for the candidate path. The caller
    /// should defer to a full verifier rerun instead of treating the repair
    /// attempt as success or failure.
    Unavailable {
        relative_path: String,
    },
    Skipped,
}

pub(super) fn verifier_repair_pass_attempt_timeout_secs(elapsed: Duration) -> Option<u64> {
    let elapsed_secs = elapsed.as_secs();
    if elapsed_secs >= VERIFIER_REPAIR_PASS_WALL_CLOCK_LIMIT_SECS {
        return None;
    }
    Some(
        (VERIFIER_REPAIR_PASS_WALL_CLOCK_LIMIT_SECS - elapsed_secs)
            .clamp(1, VERIFIER_REPAIR_PASS_TIMEOUT_SECS),
    )
}

pub(super) fn verifier_repair_pass_timeout_error(elapsed: Duration) -> String {
    format!(
        "verifier_repair_pass_timeout: patch provider exceeded repair-pass wall-clock budget \
         after {}s (limit {}s)",
        elapsed.as_secs(),
        VERIFIER_REPAIR_PASS_WALL_CLOCK_LIMIT_SECS
    )
}

pub(super) fn verifier_repair_pass_retry_message(last_error: &str) -> String {
    let reason = compact_verifier_failure_text(last_error, 220);
    let lower = last_error.to_ascii_lowercase();
    let guidance = verifier_repair_pass_retry_guidance(&lower);
    format!("The previous repair intent was rejected: {reason}. {guidance}")
}

struct RetryGuidanceRule {
    needles: &'static [&'static str],
    guidance: &'static str,
}

const RETRY_GUIDANCE_PREFIX: &str = "Return exactly one corrected JSON object only. Do not include markdown, tool calls, shell commands, or prose. Reuse the selected target only.";
const RETRY_GUIDANCE_DEFAULT: &str = "Fix the validation issue directly and ensure every old_string is exact, safe, and unique unless replace_all=true is intentionally used.";
const RETRY_GUIDANCE_RULES: &[RetryGuidanceRule] = &[
    RetryGuidanceRule {
        needles: &["missing string field", "missing required field"],
        guidance: "The rejected reply was missing a required JSON string field. Return Schema A or Schema B exactly: every edit must include path, old_string, new_string, and reason, and Schema B edits must include old_string/new_string inside each edits[] object. Do not return diffs, instructions, or partial JSON.",
    },
    RetryGuidanceRule {
        needles: &["matched more than once"],
        guidance: "The rejected old_string matched multiple locations; do not repeat that same ambiguous old_string with replace_all=false. Either include surrounding class/function/section context so the old_string is unique after prior edits, or set replace_all=true only when every occurrence should be replaced.",
    },
    RetryGuidanceRule {
        needles: &["old_string must not be empty", "old_string was empty"],
        guidance: "The rejected old_string was empty. Choose a non-empty exact substring from the current selected target excerpt, or use Schema B with non-empty old_string values for each edit.",
    },
    RetryGuidanceRule {
        needles: &[
            "missing module-level binding",
            "undefined name",
            "missing symbol",
        ],
        guidance: "The rejected edit left a missing or inconsistent symbol binding. Use one consistent binding in the selected target: define the missing name in the same scope or update every read and write to the same namespace. Do not create an object attribute while leaving unqualified reads or writes behind.",
    },
    RetryGuidanceRule {
        needles: &["was not found", "missing"],
        guidance: "The rejected old_string was not found after earlier edits; use an exact substring from the current selected target excerpt and account for sequential edit order.",
    },
    RetryGuidanceRule {
        needles: &["too many edits"],
        guidance: "The rejected edit set had too many edits; combine adjacent changes into a single enclosing old_string/new_string replacement and stay within the bounded edit count.",
    },
    RetryGuidanceRule {
        needles: &[
            "duplicate binding",
            "defined multiple times",
            "already been declared",
            "redefined",
        ],
        guidance: "The verifier is reporting a duplicate binding. Do not add another copy of the same function/class/constant. Replace or remove one existing duplicate so the named binding appears only once in the selected target.",
    },
    RetryGuidanceRule {
        needles: &["duplicate repair edit intent"],
        guidance: "The rejected edit repeats a previously applied repair; choose the next remaining failure in the selected target and make a different minimal edit.",
    },
    RetryGuidanceRule {
        needles: &["old_string and new_string are identical", "identical"],
        guidance: "The rejected edit made no change. Return an old_string from the current selected target and a new_string that is different and directly addresses the verifier failure.",
    },
    RetryGuidanceRule {
        needles: &["cheap check failed", "syntaxerror"],
        guidance: "The rejected edit made the target fail a cheap syntax check. Return a smaller exact replacement around the affected function or block, preserve indentation and line breaks, and do not concatenate separate statements onto one line.",
    },
    RetryGuidanceRule {
        needles: &[
            "test/impl weakening detected",
            "assertiondeleted",
            "literalonlyexpectedchange",
        ],
        guidance: "The rejected edit weakened a test or implementation contract. Do not delete assertion lines or test cases. If the target is a test and the failure is a setup/isolation issue, repair setup/isolation/imports while preserving assertions. If setup assigns state on an imported object that the implementation does not read, reset the actual provider state or use independent public behavior instead of changing count literals to include leaked state. If a generated test imports a missing internal symbol, remove or replace that test-only import/setup and keep any affected test function by asserting public behavior instead. If an assertion observes a test-local fixture or fake state that is not connected to the system under test, replace it with a public-behavior assertion and keep or increase the assertion count. If the failure is an observed expected-literal mismatch, change only the expected literal of the existing assertion whose observed/expected pair appears in the verifier output, preserving the assertion subject and assertion count.",
    },
];

fn verifier_repair_pass_retry_guidance(lower_error: &str) -> String {
    let suffix = RETRY_GUIDANCE_RULES
        .iter()
        .find(|rule| {
            rule.needles
                .iter()
                .any(|needle| lower_error.contains(needle))
        })
        .map(|rule| rule.guidance)
        .unwrap_or(RETRY_GUIDANCE_DEFAULT);
    format!("{RETRY_GUIDANCE_PREFIX} {suffix}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repair_pass_attempt_timeout_is_capped_by_attempt_timeout() {
        assert_eq!(
            verifier_repair_pass_attempt_timeout_secs(Duration::from_secs(0)),
            Some(VERIFIER_REPAIR_PASS_TIMEOUT_SECS)
        );
    }

    #[test]
    fn repair_pass_attempt_timeout_uses_remaining_wall_clock_budget() {
        assert_eq!(
            verifier_repair_pass_attempt_timeout_secs(Duration::from_secs(
                VERIFIER_REPAIR_PASS_WALL_CLOCK_LIMIT_SECS - 10
            )),
            Some(10)
        );
    }

    #[test]
    fn repair_pass_attempt_timeout_expires_at_wall_clock_limit() {
        assert_eq!(
            verifier_repair_pass_attempt_timeout_secs(Duration::from_secs(
                VERIFIER_REPAIR_PASS_WALL_CLOCK_LIMIT_SECS
            )),
            None
        );
    }

    #[test]
    fn repair_pass_retry_message_guides_missing_symbol_binding() {
        let message = verifier_repair_pass_retry_message(
            "repair candidate cheap check failed: missing module-level binding(s): items_db",
        );

        assert!(message.contains("missing or inconsistent symbol binding"));
        assert!(message.contains("define the missing name in the same scope"));
    }
}
