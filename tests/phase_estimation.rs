//! Integration tests for Phase Estimator (Issue #159).
//!
//! Tests cover the PhaseEstimator's interaction with Config settings
//! and its overall behavior patterns.

use anvil::app::agentic::{TurnSummary, log_turn_summary};
use anvil::app::phase_estimator::{Phase, PhaseAction, PhaseEstimator};

#[test]
fn phase_estimator_default_config_values() {
    // Default thresholds from config: N=5, M=15, K=5 (Issue #187: M raised from 10 to 15)
    let est = PhaseEstimator::new(5, 15, 5);
    assert_eq!(est.current_phase(), Phase::Unknown);
}

#[test]
fn phase_estimator_full_lifecycle() {
    let mut est = PhaseEstimator::new(5, 10, 5);

    // Phase 1: Exploring — read files
    for _ in 0..5 {
        let action = est.record_tool_call("file.read", true);
        assert_eq!(action, PhaseAction::Continue);
    }
    assert_eq!(est.current_phase(), Phase::Exploring);

    // Phase 2: Implementing — write files
    est.record_tool_call("file.edit", true);
    assert_eq!(est.current_phase(), Phase::Implementing);

    // Phase 3: Verification reads
    for _ in 0..5 {
        est.record_tool_call("file.read", true);
    }

    // Fallback completion detected on empty response
    assert_eq!(est.check_empty_response(), PhaseAction::FallbackComplete);
}

#[test]
fn phase_estimator_anvil_final_disables_fallback() {
    let mut est = PhaseEstimator::new(5, 10, 5);

    // Write + K reads
    est.record_tool_call("file.write", true);
    for _ in 0..5 {
        est.record_tool_call("file.read", true);
    }

    // Before ANVIL_FINAL: fallback should trigger
    assert_eq!(est.check_empty_response(), PhaseAction::FallbackComplete);

    // After ANVIL_FINAL accepted: fallback disabled
    est.accept_anvil_final();
    assert_eq!(est.check_empty_response(), PhaseAction::Continue);
}

#[test]
fn phase_estimator_force_transition_at_threshold() {
    let mut est = PhaseEstimator::new(3, 6, 3);

    // Read up to M-1: no force transition
    for _ in 0..5 {
        let action = est.record_tool_call("file.search", true);
        assert_eq!(action, PhaseAction::Continue);
    }

    // M-th read: force transition
    let action = est.record_tool_call("file.read", true);
    assert!(matches!(action, PhaseAction::ForceTransition(_)));
}

#[test]
fn phase_estimator_reset_preserves_cross_turn_state() {
    let mut est = PhaseEstimator::new(5, 10, 5);

    // Turn 1: write a file
    est.record_tool_call("file.edit", true);
    assert!(est.current_phase() == Phase::Implementing);

    // New turn: reset
    est.reset();

    // has_written preserved, consecutive_reads reset
    assert_eq!(est.current_phase(), Phase::Implementing);

    // K reads → fallback should work (has_written preserved)
    for _ in 0..5 {
        est.record_tool_call("file.read", true);
    }
    assert_eq!(est.check_empty_response(), PhaseAction::FallbackComplete);
}

#[test]
fn phase_estimator_model_switch_resets_anvil_final() {
    let mut est = PhaseEstimator::new(5, 10, 5);

    est.accept_anvil_final();
    est.record_tool_call("file.write", true);
    for _ in 0..5 {
        est.record_tool_call("file.read", true);
    }
    // ANVIL_FINAL accepted → no fallback
    assert_eq!(est.check_empty_response(), PhaseAction::Continue);

    // Model switch
    est.reset_model_state();
    // Now fallback should work again
    assert_eq!(est.check_empty_response(), PhaseAction::FallbackComplete);
}

#[test]
fn phase_estimator_no_fallback_without_write() {
    let mut est = PhaseEstimator::new(5, 10, 5);

    // Many reads but no writes
    for _ in 0..10 {
        est.record_tool_call("file.read", true);
    }
    // No fallback completion without has_written
    assert_eq!(est.check_empty_response(), PhaseAction::Continue);
}

#[test]
fn phase_estimator_other_tools_ignored() {
    let mut est = PhaseEstimator::new(2, 4, 2);

    // Sub-agent and shell calls should not affect read count
    est.record_tool_call("agent.explore", true);
    est.record_tool_call("shell.exec", true);
    est.record_tool_call("agent.plan", true);
    assert_eq!(est.current_phase(), Phase::Unknown);
}

#[test]
fn turn_summary_includes_phase_field() {
    let summary = TurnSummary {
        turn: 1,
        max_turns: 10,
        elapsed: std::time::Duration::from_secs(1),
        tokens_used: 100,
        token_budget: 1000,
        tool_calls: 2,
        tool_names: &[],
        files_modified: 0,
        compact_info: None,
        phase: Phase::Exploring,
        mutations_this_turn: None,
        items_advanced_this_turn: None,
    };
    // Verify phase field exists and log_turn_summary is callable
    assert_eq!(format!("{}", summary.phase), "exploring");
    log_turn_summary(&summary);
}

// ---------------------------------------------------------------------------
// Task 0.4: observed_final / accepted_final separation
// ---------------------------------------------------------------------------

#[test]
fn suppressed_final_does_not_disable_fallback() {
    let mut est = PhaseEstimator::new(5, 10, 5);

    // Write + K reads → conditions for fallback
    est.record_tool_call("file.write", true);
    for _ in 0..5 {
        est.record_tool_call("file.read", true);
    }

    // Observe (but do NOT accept) ANVIL_FINAL → suppressed premature final
    est.observe_anvil_final();

    // Fallback should still work because accept_anvil_final() was not called
    assert_eq!(est.check_empty_response(), PhaseAction::FallbackComplete);
}
