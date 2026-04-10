//! Integration tests for Issue #321: fix_slice escalation path.
//!
//! Tests that repeated single-file edit failures escalate to
//! agent.fix_slice via EditFallbackAction::FixSliceEscalation.
//!
//! Issue #332 extensions test the broadened escalation trigger that
//! fires on stagnation + cumulative edit failures across multiple paths,
//! not only on same-path consecutive failures.

use anvil::app::edit_fail_tracker::{
    EditFailTracker, EditFallbackAction, determine_fallback_action, should_escalate_for_stagnation,
};
use anvil::app::stagnation_state::{StagnationState, compute_stagnation_score};
use anvil::contracts::AgentTelemetry;

// ============================================================
// Test 1: config default for edit_fixslice_threshold
// ============================================================

#[test]
fn test_fixslice_threshold_config_default() {
    let config = anvil::config::EffectiveConfig::default_for_test().unwrap();
    assert_eq!(
        config.runtime.edit_fixslice_threshold, 7,
        "default fixslice threshold should be 7"
    );
}

#[test]
fn test_fixslice_threshold_config_clamp_must_exceed_write_fallback() {
    let mut config = anvil::config::EffectiveConfig::default_for_test().unwrap();
    // Set fixslice to same as write_fallback (5) — should be bumped to write_fallback + 2
    config.runtime.edit_fixslice_threshold = 5;
    assert!(config.validate_for_test().is_ok());
    assert!(
        config.runtime.edit_fixslice_threshold > config.runtime.edit_write_fallback_threshold,
        "fixslice threshold ({}) must be > write_fallback threshold ({})",
        config.runtime.edit_fixslice_threshold,
        config.runtime.edit_write_fallback_threshold,
    );
}

#[test]
fn test_fixslice_threshold_config_zero_disables() {
    let mut config = anvil::config::EffectiveConfig::default_for_test().unwrap();
    config.runtime.edit_fixslice_threshold = 0;
    assert!(config.validate_for_test().is_ok());
    assert_eq!(
        config.runtime.edit_fixslice_threshold, 0,
        "0 should remain 0 (disabled)"
    );
}

// ============================================================
// Test 2: determine_fallback_action escalation chain
// ============================================================

#[test]
fn test_escalation_chain_with_fixslice() {
    // reread=3, write_fallback=5, fixslice=7
    // Verify the complete escalation chain
    assert_eq!(
        determine_fallback_action(1, 3, 5, 7),
        EditFallbackAction::Continue
    );
    assert_eq!(
        determine_fallback_action(3, 3, 5, 7),
        EditFallbackAction::ReRead
    );
    assert_eq!(
        determine_fallback_action(5, 3, 5, 7),
        EditFallbackAction::WriteFallback
    );
    assert_eq!(
        determine_fallback_action(7, 3, 5, 7),
        EditFallbackAction::FixSliceEscalation
    );
    // Beyond threshold, persists
    assert_eq!(
        determine_fallback_action(10, 3, 5, 7),
        EditFallbackAction::FixSliceEscalation
    );
}

#[test]
fn test_escalation_chain_fixslice_disabled() {
    // fixslice=0 (disabled) — WriteFallback is the ceiling
    assert_eq!(
        determine_fallback_action(7, 3, 5, 0),
        EditFallbackAction::WriteFallback
    );
    assert_eq!(
        determine_fallback_action(100, 3, 5, 0),
        EditFallbackAction::WriteFallback
    );
}

// ============================================================
// Test 3: EditFailTracker escalation flow
// ============================================================

#[test]
fn test_tracker_fixslice_escalation_flow() {
    // reread=3, write_fallback=5, fixslice=7
    let mut tracker = EditFailTracker::new(3, 5, 7);
    let path = "src/lib/auto-yes-poller.ts";

    // Failures 1-2: Continue
    assert_eq!(tracker.record_failure(path), EditFallbackAction::Continue);
    assert_eq!(tracker.record_failure(path), EditFallbackAction::Continue);

    // Failures 3-4: ReRead
    assert_eq!(tracker.record_failure(path), EditFallbackAction::ReRead);
    assert_eq!(tracker.record_failure(path), EditFallbackAction::ReRead);

    // Failures 5-6: WriteFallback
    assert_eq!(
        tracker.record_failure(path),
        EditFallbackAction::WriteFallback
    );
    assert_eq!(
        tracker.record_failure(path),
        EditFallbackAction::WriteFallback
    );

    // Failure 7+: FixSliceEscalation
    assert_eq!(
        tracker.record_failure(path),
        EditFallbackAction::FixSliceEscalation
    );
    assert_eq!(tracker.failure_count(path), 7);

    // Persists
    assert_eq!(
        tracker.record_failure(path),
        EditFallbackAction::FixSliceEscalation
    );
}

#[test]
fn test_tracker_fixslice_independent_paths() {
    let mut tracker = EditFailTracker::new(3, 5, 7);

    // Drive path A to fixslice threshold
    for _ in 0..7 {
        tracker.record_failure("a.ts");
    }
    assert_eq!(
        tracker.record_failure("a.ts"),
        EditFallbackAction::FixSliceEscalation
    );

    // Path B should still be at Continue
    assert_eq!(tracker.record_failure("b.ts"), EditFallbackAction::Continue);
}

#[test]
fn test_tracker_fixslice_reset_on_success() {
    let mut tracker = EditFailTracker::new(3, 5, 7);

    // Drive to fixslice
    for _ in 0..7 {
        tracker.record_failure("a.ts");
    }
    assert_eq!(
        tracker.record_failure("a.ts"),
        EditFallbackAction::FixSliceEscalation
    );

    // Success resets
    tracker.record_success("a.ts");
    assert_eq!(tracker.failure_count("a.ts"), 0);
    assert_eq!(tracker.record_failure("a.ts"), EditFallbackAction::Continue);
}

// ============================================================
// Test 4: Stagnation + single-file edit failure reproduces Issue #321
// ============================================================

#[test]
fn test_stagnation_with_single_file_edit_drift() {
    // Reproduce the observed pattern from Issue #321:
    // - Same file edited repeatedly with failures
    // - Stagnation score rises
    // - But fix_slice was never invoked
    //
    // After the fix, EditFailTracker returns FixSliceEscalation at count >= 7

    let mut stagnation = StagnationState::new();
    let mut tracker = EditFailTracker::new(3, 5, 7);
    let path = "src/lib/auto-yes-poller.ts";

    // Simulate 5 turns of same-file edit failures (no mutation)
    let workset = vec![0usize];
    for _ in 0..5 {
        stagnation.begin_turn(&workset);
        stagnation.end_turn(false); // no mutation
    }
    let score = compute_stagnation_score(&stagnation);
    assert!(score >= 1, "Expected stagnation score >= 1, got {score}");

    // After 7 consecutive edit failures, escalation should trigger
    for _ in 0..6 {
        tracker.record_failure(path);
    }
    let action = tracker.record_failure(path);
    assert_eq!(
        action,
        EditFallbackAction::FixSliceEscalation,
        "7th consecutive edit failure should trigger FixSliceEscalation"
    );
}

// ============================================================
// Test 5: AgentTelemetry records escalation
// ============================================================

#[test]
fn test_agent_telemetry_fixslice_escalation() {
    let mut tel = AgentTelemetry::new();
    assert_eq!(tel.fixslice_escalation_count, 0);

    tel.record_fixslice_escalation();
    assert_eq!(tel.fixslice_escalation_count, 1);

    tel.record_fixslice_escalation();
    assert_eq!(tel.fixslice_escalation_count, 2);
}

#[test]
fn test_agent_telemetry_fixslice_serialization() {
    let mut tel = AgentTelemetry::new();
    tel.record_fixslice_escalation();

    let json = serde_json::to_string(&tel).unwrap();
    assert!(
        json.contains("\"fixslice_escalation_count\":1"),
        "fixslice_escalation_count should appear in serialized telemetry"
    );

    // Deserialize back
    let tel2: AgentTelemetry = serde_json::from_str(&json).unwrap();
    assert_eq!(tel2.fixslice_escalation_count, 1);
}

#[test]
fn test_agent_telemetry_fixslice_default_zero() {
    // Backward compatibility: missing field defaults to 0
    let json = r#"{"premature_final_count":0,"total_final_requests":0,"plan_registration_count":0,"plan_update_count":0,"sync_from_touched_files_count":0,"completion_kind":null}"#;
    let tel: AgentTelemetry = serde_json::from_str(json).unwrap();
    assert_eq!(tel.fixslice_escalation_count, 0);
}

// ============================================================
// Issue #332: broadened escalation (stagnation + cross-path failures)
// ============================================================

#[test]
fn test_config_defaults_for_stagnation_escalation_thresholds() {
    let config = anvil::config::EffectiveConfig::default_for_test().unwrap();
    assert_eq!(
        config.runtime.edit_fixslice_stagnation_total_threshold, 4,
        "default total-failures threshold should be 4"
    );
    assert_eq!(
        config.runtime.edit_fixslice_stagnation_score_threshold, 2,
        "default stagnation-score threshold should be 2"
    );
}

#[test]
fn test_tracker_total_failures_accumulates_across_paths() {
    let mut tracker = EditFailTracker::new(3, 5, 7);
    assert_eq!(tracker.total_failures(), 0);

    tracker.record_failure("a.ts");
    tracker.record_failure("b.ts");
    tracker.record_failure("c.ts");
    assert_eq!(tracker.total_failures(), 3);

    // Success on one path does not decrease the cumulative total.
    tracker.record_success("a.ts");
    assert_eq!(tracker.total_failures(), 3);

    tracker.record_failure("a.ts");
    assert_eq!(tracker.total_failures(), 4);
}

#[test]
fn test_should_escalate_for_stagnation_pure_function() {
    // Disabled threshold: never escalate.
    assert!(!should_escalate_for_stagnation(10, 4, 0, 2));
    // Below failure threshold: no escalation.
    assert!(!should_escalate_for_stagnation(3, 4, 4, 2));
    // Below stagnation score: no escalation.
    assert!(!should_escalate_for_stagnation(4, 1, 4, 2));
    // Both conditions met: escalate.
    assert!(should_escalate_for_stagnation(4, 2, 4, 2));
    assert!(should_escalate_for_stagnation(10, 4, 4, 2));
}

#[test]
fn test_cross_path_drift_triggers_stagnation_escalation() {
    // Issue #332: model spreads edits across multiple files and no path
    // reaches the same-path threshold, but cumulative failures + stagnation
    // are high enough to escalate.
    let mut tracker = EditFailTracker::new(3, 5, 7);
    // 4 failures across 4 different files — no single path hits 3 reread,
    // so EditFallbackAction never returns FixSliceEscalation.
    for path in ["a.ts", "b.ts", "c.ts", "d.ts"] {
        let action = tracker.record_failure(path);
        assert_eq!(action, EditFallbackAction::Continue);
    }
    assert_eq!(tracker.total_failures(), 4);

    // Simulate stagnation score = 2 (mutation drought + workset staleness).
    let mut stag = StagnationState::new();
    let workset = vec![0usize];
    for _ in 0..5 {
        stag.begin_turn(&workset);
        stag.end_turn(false);
    }
    let score = compute_stagnation_score(&stag);
    assert!(score >= 2, "expected score >= 2, got {score}");

    // Broadened policy: should fire even though no same-path count >= 7.
    assert!(should_escalate_for_stagnation(
        tracker.total_failures(),
        score as u32,
        4,
        2,
    ));
}

#[test]
fn test_telemetry_same_path_counter_bumped_by_legacy_method() {
    let mut tel = AgentTelemetry::new();
    tel.record_fixslice_escalation();
    assert_eq!(tel.fixslice_escalation_count, 1);
    assert_eq!(tel.fixslice_escalation_same_path_count, 1);
    assert_eq!(tel.fixslice_escalation_stagnation_count, 0);
    assert_eq!(tel.fixslice_escalation_repair_salvage_count, 0);
    assert!(tel.worker_observed);
}

#[test]
fn test_telemetry_stagnation_escalation_distinct_from_same_path() {
    let mut tel = AgentTelemetry::new();
    tel.record_fixslice_escalation_stagnation();
    assert_eq!(tel.fixslice_escalation_count, 1);
    assert_eq!(tel.fixslice_escalation_stagnation_count, 1);
    assert_eq!(tel.fixslice_escalation_same_path_count, 0);
    assert!(tel.worker_observed);

    // A subsequent same-path escalation bumps only the same-path counter.
    tel.record_fixslice_escalation();
    assert_eq!(tel.fixslice_escalation_count, 2);
    assert_eq!(tel.fixslice_escalation_same_path_count, 1);
    assert_eq!(tel.fixslice_escalation_stagnation_count, 1);
}

#[test]
fn test_telemetry_repair_salvage_classification_does_not_flip_worker() {
    // Salvage is a retrospective label — it means repair turn produced
    // a mutation without ever reaching the worker path.
    let mut tel = AgentTelemetry::new();
    tel.record_mutation_turn(3, Some(1.0), "file.edit");
    tel.record_pre_exit_repair_injected();
    tel.record_pre_exit_repair_consumed();
    assert!(!tel.worker_observed);

    tel.classify_repair_salvage();
    assert_eq!(tel.fixslice_escalation_repair_salvage_count, 1);
    assert!(
        !tel.worker_observed,
        "salvage must not flip worker_observed — it is a distinct classification"
    );
    // Salvage classification is idempotent.
    tel.classify_repair_salvage();
    assert_eq!(tel.fixslice_escalation_repair_salvage_count, 1);
}

#[test]
fn test_telemetry_no_salvage_when_worker_already_observed() {
    let mut tel = AgentTelemetry::new();
    tel.record_mutation_turn(3, Some(1.0), "file.edit");
    tel.record_pre_exit_repair_injected();
    tel.record_fixslice_escalation_stagnation();

    tel.classify_repair_salvage();
    assert_eq!(tel.fixslice_escalation_repair_salvage_count, 0);
}

#[test]
fn test_telemetry_no_salvage_without_mutation() {
    let mut tel = AgentTelemetry::new();
    tel.record_pre_exit_repair_injected();
    tel.record_pre_exit_repair_consumed();

    tel.classify_repair_salvage();
    assert_eq!(tel.fixslice_escalation_repair_salvage_count, 0);
}

#[test]
fn test_telemetry_serializes_new_counters() {
    let mut tel = AgentTelemetry::new();
    tel.record_fixslice_escalation();
    tel.record_fixslice_escalation_stagnation();

    let json = serde_json::to_string(&tel).unwrap();
    assert!(json.contains("\"fixslice_escalation_same_path_count\":1"));
    assert!(json.contains("\"fixslice_escalation_stagnation_count\":1"));
    assert!(json.contains("\"fixslice_escalation_repair_salvage_count\":0"));

    let round_trip: AgentTelemetry = serde_json::from_str(&json).unwrap();
    assert_eq!(round_trip.fixslice_escalation_same_path_count, 1);
    assert_eq!(round_trip.fixslice_escalation_stagnation_count, 1);
    assert_eq!(round_trip.fixslice_escalation_repair_salvage_count, 0);
}

#[test]
fn test_telemetry_new_counters_backward_compat_default_zero() {
    // Old artifacts missing the new fields should deserialize cleanly.
    let json = r#"{"premature_final_count":0,"total_final_requests":0,"plan_registration_count":0,"plan_update_count":0,"sync_from_touched_files_count":0,"completion_kind":null}"#;
    let tel: AgentTelemetry = serde_json::from_str(json).unwrap();
    assert_eq!(tel.fixslice_escalation_same_path_count, 0);
    assert_eq!(tel.fixslice_escalation_stagnation_count, 0);
    assert_eq!(tel.fixslice_escalation_repair_salvage_count, 0);
}
