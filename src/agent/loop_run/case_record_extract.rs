//! Case-record extraction helpers + supporting agent-layer projections
//! (language stack derivation + `AnvilTestSummary` builder) extracted
//! from `turn.rs` (parent #680).
//!
//! Hosts:
//!
//! * `case_record_auto_test_active(&AnvilScore) -> bool` — is auto-test
//!   data present in the score?
//! * `case_record_extraction_succeeded(&AnvilScore, repo_edit_succeeded,
//!   unsafe_blocks) -> bool` — Issue #450 success predicate.
//! * `case_record_initial_feedback(Option<&FeedbackFrame>) -> Vec<FeedbackKind>`
//!   — derive the initial feedback list for `RepoFingerprint`.
//! * `derive_language_stack(work_root) -> Vec<String>` — Issue #462,
//!   reuses `auto_test::has_*` so DR3-002 (agent → session is one-way)
//!   is preserved.
//! * `build_anvil_test_summary(&AutoTestPlan, &AutoTestResult)
//!   -> AnvilTestSummary` — Issue #466 (VerifierSkill 経路でも利用).
//!
//! `pub(super)` limited / no facade re-export (DR3-001).
//! `turn.rs` is the only in-crate consumer.

use std::path::Path;

use super::auto_test::{
    AutoTestKind, AutoTestPlan, AutoTestResult, count_compile_errors, count_test_failures,
    has_cargo_manifest, has_python_surface, package_json_has_test_script,
};
use crate::session::anvil_score::{AnvilScore, AnvilTestSummary};
use crate::session::feedback::{FeedbackFrame, FeedbackKind};

pub(super) fn case_record_auto_test_active(score: &AnvilScore) -> bool {
    score.build_passed.is_some() || score.tests_passed.is_some()
}

pub(super) fn case_record_extraction_succeeded(
    score: &AnvilScore,
    repo_edit_succeeded_this_turn: bool,
    unsafe_blocks_this_turn: usize,
) -> bool {
    if case_record_auto_test_active(score) {
        score.build_passed == Some(true)
            && score.tests_passed == Some(true)
            && score.user_visible_artifact
            && score.unsafe_actions_blocked == 0
            && score.consecutive_no_progress_turns == 0
    } else {
        repo_edit_succeeded_this_turn
            && unsafe_blocks_this_turn == 0
            && score.consecutive_no_progress_turns == 0
    }
}

pub(super) fn case_record_initial_feedback(
    last_feedback: Option<&FeedbackFrame>,
) -> Vec<FeedbackKind> {
    last_feedback
        .map(|feedback| vec![feedback.kind.clone()])
        .unwrap_or_default()
}

/// Issue #462: derive the `language_stack` Vec for `RepoFingerprint`.
/// Reuses `auto_test::has_*` helpers (also in the agent layer) so DR3-002 —
/// agent → session is one-way — is preserved: the session-layer
/// `case_record::extract` accepts the slice as input rather than calling back.
pub(super) fn derive_language_stack(work_root: &Path) -> Vec<String> {
    let mut stack: Vec<String> = Vec::new();
    if has_cargo_manifest(work_root) {
        stack.push("rust".into());
    }
    if package_json_has_test_script(work_root) {
        stack.push("node".into());
    }
    if has_python_surface(work_root, &[]) {
        stack.push("python".into());
    }
    stack.iter_mut().for_each(|s| *s = s.to_ascii_lowercase());
    stack.sort();
    stack.dedup();
    stack
}

// Issue #466: build_anvil_test_summary は VerifierSkill 経路でも利用するため残置。
// VerifierSkill 側 (verifier_skill.rs::build_anvil_test_summary_for_skill) は同等の
// ロジックを内部 helper として保持する。将来 Issue で SSOT を一本化する。
#[cfg_attr(not(test), allow(dead_code))]
pub(super) fn build_anvil_test_summary(
    plan: &AutoTestPlan,
    result: &AutoTestResult,
) -> AnvilTestSummary {
    match (plan.auto_test_kind(), result.passed) {
        (AutoTestKind::Build, true) => AnvilTestSummary {
            build_passed: Some(true),
            tests_passed: None,
            compile_error_count: Some(0),
            test_failure_count: None,
        },
        (AutoTestKind::Build, false) => AnvilTestSummary {
            build_passed: Some(false),
            tests_passed: None,
            compile_error_count: count_compile_errors(result),
            test_failure_count: None,
        },
        (AutoTestKind::Test, true) => AnvilTestSummary {
            build_passed: None,
            tests_passed: Some(true),
            compile_error_count: None,
            test_failure_count: Some(0),
        },
        (AutoTestKind::Test, false) => AnvilTestSummary {
            build_passed: None,
            tests_passed: Some(false),
            compile_error_count: count_compile_errors(result),
            test_failure_count: count_test_failures(result),
        },
    }
}
