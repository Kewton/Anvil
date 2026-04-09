//! Integration tests for Issue #321: fix_slice escalation path.
//!
//! Tests that repeated single-file edit failures escalate to
//! agent.fix_slice via EditFallbackAction::FixSliceEscalation.

use anvil::app::edit_fail_tracker::{
    EditFailTracker, EditFallbackAction, determine_fallback_action,
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
