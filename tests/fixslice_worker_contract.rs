//! Integration tests for Issue #345: fix_slice worker contract robustness.
//!
//! Issue #345 extends the Issue #343 failure taxonomy in two directions:
//!
//! 1. **Layer 1 — escalation bypass (A1 shape).** When `[Anvil ESCALATION]`
//!    fires but the LLM never actually calls `agent.fix_slice`, the parent's
//!    own `file.edit` can land a mutation and the session ends with
//!    `worker_observed=false` and no recorded failure reason. The tests in
//!    this file cover the new `EscalatedNotInvoked` classification that
//!    `finalize_fixslice_outcome` records at session wrap-up.
//!
//! 2. **Layer 2 — strict proposal contract.** The strict `serde_json` parse
//!    of the worker's ANVIL_FINAL block was too brittle: any wrapper prose,
//!    path drift, or extra field collapsed the whole proposal into a generic
//!    `no_proposal`. The tests here cover the salvage parser
//!    (`try_parse_fix_proposal`) and the split of `PathMismatch` out of
//!    `ProposalValidationFailed`, plus the runtime-configurable
//!    `fixslice_max_iterations` knob that replaces the hard-coded cap.

use anvil::agent::subagent::{
    DEFAULT_FIXSLICE_MAX_ITERATIONS, FixProposalParseOutcome, try_parse_fix_proposal,
};
use anvil::config::EffectiveConfig;
use anvil::contracts::{AgentTelemetry, FixSliceFailureReason, FixSliceProposal};

// ---------------------------------------------------------------------------
// Layer 1: EscalatedNotInvoked classification
// ---------------------------------------------------------------------------

#[test]
fn escalated_not_invoked_classifies_at_wrap_up() {
    // A1 shape: escalation fired, but agent.fix_slice was never invoked and
    // no worker success was ever recorded. finalize_fixslice_outcome must
    // record this as the EscalatedNotInvoked failure class.
    let mut tel = AgentTelemetry::new();
    tel.record_fixslice_escalation();
    assert_eq!(tel.fixslice_worker_invocation_count, 0);
    assert_eq!(tel.fixslice_worker_failure_count, 0);

    tel.finalize_fixslice_outcome();

    assert_eq!(tel.fixslice_worker_failure_count, 1);
    assert_eq!(
        tel.fixslice_failure_reason.as_deref(),
        Some("escalated_not_invoked")
    );
    assert!(
        !tel.worker_observed,
        "finalize must not flip worker_observed"
    );
}

#[test]
fn escalation_followed_by_invocation_does_not_classify_a1() {
    // Escalation fired and the worker was actually invoked (even if it
    // failed). finalize must be a no-op — the Layer-2 classification for
    // why the invocation failed is the caller's job.
    let mut tel = AgentTelemetry::new();
    tel.record_fixslice_escalation();
    tel.record_fixslice_worker_invocation();

    tel.finalize_fixslice_outcome();

    assert_eq!(
        tel.fixslice_worker_failure_count, 0,
        "finalize must not record failure when worker was invoked"
    );
    assert!(tel.fixslice_failure_reason.is_none());
}

#[test]
fn finalize_skips_a1_classification_when_worker_observed() {
    // Belt-and-braces: if worker_observed was already set, finalize must not
    // retroactively record A1.
    let mut tel = AgentTelemetry::new();
    tel.record_fixslice_escalation();
    tel.record_worker_success();

    tel.finalize_fixslice_outcome();

    assert_eq!(tel.fixslice_worker_failure_count, 0);
    assert!(tel.fixslice_failure_reason.is_none());
}

#[test]
fn finalize_is_idempotent() {
    // Multiple exit branches may call finalize_fixslice_outcome. Calling it
    // twice must not double-record the failure.
    let mut tel = AgentTelemetry::new();
    tel.record_fixslice_escalation();

    tel.finalize_fixslice_outcome();
    tel.finalize_fixslice_outcome();

    assert_eq!(tel.fixslice_worker_failure_count, 1);
}

#[test]
fn finalize_is_noop_without_escalation() {
    // A session that never escalated must not receive a spurious failure.
    let mut tel = AgentTelemetry::new();
    tel.finalize_fixslice_outcome();

    assert_eq!(tel.fixslice_worker_failure_count, 0);
    assert!(tel.fixslice_failure_reason.is_none());
}

#[test]
fn a1_classification_preserves_worker_required_repair_skip() {
    // After finalize records EscalatedNotInvoked, the existing Issue #343
    // guard must still fire under a requires_worker_observation pack.
    use anvil::contracts::PackExpectation;

    let mut tel = AgentTelemetry::new();
    tel.pack_expectation = Some(PackExpectation::RequiresWorkerObservation);
    tel.record_fixslice_escalation();
    tel.finalize_fixslice_outcome();

    assert!(
        tel.should_skip_pre_exit_repair_for_worker_failure(),
        "EscalatedNotInvoked must satisfy the worker-required repair skip"
    );
}

// ---------------------------------------------------------------------------
// Layer 2: try_parse_fix_proposal salvage
// ---------------------------------------------------------------------------

fn minimal_proposal_json() -> &'static str {
    r#"{"target_path":"src/main.rs","start_line":1,"end_line":2,"replacement_content":"fn main(){}\n","rationale":"fix"}"#
}

#[test]
fn try_parse_fix_proposal_strict() {
    let outcome = try_parse_fix_proposal(minimal_proposal_json());
    match outcome {
        FixProposalParseOutcome::Parsed(p) => {
            assert_eq!(p.target_path, "src/main.rs");
            assert_eq!(p.start_line, 1);
            assert_eq!(p.end_line, 2);
        }
        other => panic!("expected Parsed, got {other:?}"),
    }
}

#[test]
fn try_parse_fix_proposal_salvages_wrapped_json() {
    // B1 shape: the worker produces a JSON proposal but surrounds it with
    // prose or code-fence artifacts. Salvage must extract the first balanced
    // JSON object and parse it.
    let wrapped = format!(
        "Here is the proposal as requested:\n```json\n{}\n```\nThanks!",
        minimal_proposal_json()
    );
    let outcome = try_parse_fix_proposal(&wrapped);
    match outcome {
        FixProposalParseOutcome::Salvaged(p) => {
            assert_eq!(p.target_path, "src/main.rs");
        }
        other => panic!("expected Salvaged, got {other:?}"),
    }
}

#[test]
fn try_parse_fix_proposal_empty_final() {
    let outcome = try_parse_fix_proposal("");
    assert!(matches!(outcome, FixProposalParseOutcome::EmptyFinal));

    let outcome = try_parse_fix_proposal("   \n\t  ");
    assert!(matches!(outcome, FixProposalParseOutcome::EmptyFinal));
}

#[test]
fn try_parse_fix_proposal_non_json_text() {
    let outcome = try_parse_fix_proposal("I was unable to produce a valid proposal.");
    assert!(matches!(outcome, FixProposalParseOutcome::ParseFailed));
}

#[test]
fn try_parse_fix_proposal_handles_escaped_braces_in_strings() {
    // The salvage depth scanner must track JSON string state so that a brace
    // inside `replacement_content` does not throw off the bracket count.
    let proposal_with_braces = r#"{
        "target_path": "src/main.rs",
        "start_line": 1,
        "end_line": 3,
        "replacement_content": "if x { y } else { z }",
        "rationale": "fix"
    }"#;
    let wrapped = format!("prose before\n{proposal_with_braces}\nprose after");
    let outcome = try_parse_fix_proposal(&wrapped);
    match outcome {
        FixProposalParseOutcome::Salvaged(p) => {
            assert_eq!(p.replacement_content, "if x { y } else { z }");
        }
        other => panic!("expected Salvaged, got {other:?}"),
    }
}

#[test]
fn try_parse_fix_proposal_rejects_unbalanced_json() {
    // Unbalanced braces (e.g. a truncated payload) must not mis-salvage.
    let outcome = try_parse_fix_proposal("{ \"target_path\": \"src/main.rs\"");
    assert!(matches!(outcome, FixProposalParseOutcome::ParseFailed));
}

#[test]
fn try_parse_fix_proposal_rejects_missing_required_fields() {
    // A well-formed JSON object that lacks required proposal fields must be
    // reported as ParseFailed (not Salvaged).
    let outcome = try_parse_fix_proposal("prose {\"unrelated\": 1} more prose");
    assert!(matches!(outcome, FixProposalParseOutcome::ParseFailed));
}

// ---------------------------------------------------------------------------
// Layer 2: new FixSliceFailureReason variants
// ---------------------------------------------------------------------------

#[test]
fn new_failure_reason_variants_have_distinct_snake_case_display() {
    for (variant, expected) in [
        (FixSliceFailureReason::NoProposal, "no_proposal"),
        (
            FixSliceFailureReason::ProposalParseFailed,
            "proposal_parse_failed",
        ),
        (FixSliceFailureReason::PathMismatch, "path_mismatch"),
        (
            FixSliceFailureReason::ProposalValidationFailed,
            "proposal_validation_failed",
        ),
        (FixSliceFailureReason::RewriteFailed, "rewrite_failed"),
        (
            FixSliceFailureReason::MaxIterationsReached,
            "max_iterations_reached",
        ),
        (
            FixSliceFailureReason::EscalatedNotInvoked,
            "escalated_not_invoked",
        ),
    ] {
        assert_eq!(variant.to_string(), expected);
    }
}

#[test]
fn path_mismatch_and_parse_failed_count_as_worker_failures() {
    // Pre-exit repair skip and the failure counter must accept the new
    // variants as first-class worker failures.
    let mut tel = AgentTelemetry::new();
    tel.record_fixslice_worker_failure(FixSliceFailureReason::ProposalParseFailed);
    tel.record_fixslice_worker_failure(FixSliceFailureReason::PathMismatch);

    assert_eq!(tel.fixslice_worker_failure_count, 2);
    assert!(tel.has_fixslice_worker_failure());
    assert_eq!(
        tel.fixslice_failure_reason.as_deref(),
        Some("path_mismatch")
    );
}

// ---------------------------------------------------------------------------
// Layer 2: Proposal path mismatch classification (helper-level)
// ---------------------------------------------------------------------------
//
// handle_fixslice_result lives on AppState so reproducing the full call in
// a unit test is heavy. Instead, we exercise the telemetry path directly:
// a session that saw a worker invocation + a PathMismatch must carry both
// the invocation count and the distinct failure reason, and
// finalize_fixslice_outcome must NOT overwrite it with EscalatedNotInvoked.

#[test]
fn path_mismatch_after_invocation_does_not_degrade_to_a1() {
    let mut tel = AgentTelemetry::new();
    tel.record_fixslice_escalation();
    tel.record_fixslice_worker_invocation();
    tel.record_fixslice_worker_failure(FixSliceFailureReason::PathMismatch);

    tel.finalize_fixslice_outcome();

    assert_eq!(
        tel.fixslice_worker_failure_count, 1,
        "finalize must not add a second failure"
    );
    assert_eq!(
        tel.fixslice_failure_reason.as_deref(),
        Some("path_mismatch"),
        "finalize must not overwrite the concrete failure reason"
    );
}

// ---------------------------------------------------------------------------
// Configurable fixslice_max_iterations
// ---------------------------------------------------------------------------

#[test]
fn fixslice_max_iterations_defaults_to_three() {
    let config = EffectiveConfig::default_for_test().expect("config");
    assert_eq!(config.runtime.fixslice_max_iterations, 3);
    assert_eq!(DEFAULT_FIXSLICE_MAX_ITERATIONS, 3);
}

#[test]
fn fixslice_max_iterations_honored_by_validate() {
    let mut config = EffectiveConfig::default_for_test().expect("config");
    config.runtime.fixslice_max_iterations = 5;
    config.validate_for_test().expect("validate");
    assert_eq!(config.runtime.fixslice_max_iterations, 5);
}

#[test]
fn fixslice_max_iterations_clamped_upper_bound() {
    let mut config = EffectiveConfig::default_for_test().expect("config");
    config.runtime.fixslice_max_iterations = 50;
    config.validate_for_test().expect("validate");
    assert_eq!(
        config.runtime.fixslice_max_iterations, 10,
        "values above 10 must be clamped"
    );
}

#[test]
fn fixslice_max_iterations_clamped_lower_bound() {
    let mut config = EffectiveConfig::default_for_test().expect("config");
    config.runtime.fixslice_max_iterations = 0;
    config.validate_for_test().expect("validate");
    assert_eq!(
        config.runtime.fixslice_max_iterations, 1,
        "zero must be clamped up to 1 so the worker always gets one shot"
    );
}

// ---------------------------------------------------------------------------
// Telemetry artifact JSON carries the new counter
// ---------------------------------------------------------------------------

#[test]
fn telemetry_artifact_includes_worker_invocation_count() {
    use std::fs;

    let tmp = tempfile::tempdir().expect("tempdir");
    // SAFETY: tests do not race on the env var in this crate; the telemetry
    // dir write is guarded by absolute-path validation.
    unsafe {
        std::env::set_var("ANVIL_TELEMETRY_DIR", tmp.path());
    }

    let mut tel = AgentTelemetry::new();
    tel.record_fixslice_escalation();
    tel.record_fixslice_worker_invocation();
    tel.record_fixslice_worker_failure(FixSliceFailureReason::ProposalParseFailed);

    tel.write_artifact("issue345_smoke")
        .expect("artifact should write");

    let file_path = tmp.path().join("issue345_smoke_telemetry.json");
    let json = fs::read_to_string(&file_path).expect("read artifact");
    let value: serde_json::Value = serde_json::from_str(&json).expect("parse artifact");

    assert_eq!(value["fixslice_worker_invocation_count"], 1);
    assert_eq!(value["fixslice_worker_failure_count"], 1);
    assert_eq!(value["fixslice_failure_reason"], "proposal_parse_failed");

    unsafe {
        std::env::remove_var("ANVIL_TELEMETRY_DIR");
    }
}

// ---------------------------------------------------------------------------
// Sanity: the salvage parser round-trips through a FixSliceProposal
// ---------------------------------------------------------------------------

#[test]
fn salvaged_proposal_is_usable_by_downstream_tools() {
    // Ensure the salvaged struct carries all fields downstream tools expect.
    let wrapped = format!("wrap\n{}\nend", minimal_proposal_json());
    let outcome = try_parse_fix_proposal(&wrapped);
    let proposal: FixSliceProposal = match outcome {
        FixProposalParseOutcome::Salvaged(p) => p,
        other => panic!("expected Salvaged, got {other:?}"),
    };
    assert_eq!(proposal.target_path, "src/main.rs");
    assert_eq!(proposal.start_line, 1);
    assert_eq!(proposal.end_line, 2);
    assert!(!proposal.replacement_content.is_empty());
    assert_eq!(proposal.rationale, "fix");
}
