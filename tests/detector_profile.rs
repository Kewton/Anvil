/// Tests for the DetectorProfile system (Issue #371).
///
/// Covers: profile parsing, strict backward compatibility, minimal hint
/// disablement, relaxed threshold widening, Break-class preservation,
/// threshold ordering validation, and security invariant validation.
use anvil::config::{DetectorProfile, EffectiveConfig};

// ---------------------------------------------------------------------------
// 1. DetectorProfile parse tests
// ---------------------------------------------------------------------------

#[test]
fn test_detector_profile_parse_strict() {
    let profile: DetectorProfile = "strict".parse().unwrap();
    assert_eq!(profile, DetectorProfile::Strict);
}

#[test]
fn test_detector_profile_parse_relaxed() {
    let profile: DetectorProfile = "relaxed".parse().unwrap();
    assert_eq!(profile, DetectorProfile::Relaxed);
}

#[test]
fn test_detector_profile_parse_minimal() {
    let profile: DetectorProfile = "minimal".parse().unwrap();
    assert_eq!(profile, DetectorProfile::Minimal);
}

#[test]
fn test_detector_profile_parse_case_insensitive() {
    let profile: DetectorProfile = "STRICT".parse().unwrap();
    assert_eq!(profile, DetectorProfile::Strict);
    let profile: DetectorProfile = "Relaxed".parse().unwrap();
    assert_eq!(profile, DetectorProfile::Relaxed);
    let profile: DetectorProfile = "MINIMAL".parse().unwrap();
    assert_eq!(profile, DetectorProfile::Minimal);
}

#[test]
fn test_detector_profile_parse_unknown_falls_back_to_strict() {
    let profile: DetectorProfile = "unknown_value".parse().unwrap();
    assert_eq!(profile, DetectorProfile::Strict);
}

#[test]
fn test_detector_profile_parse_empty_falls_back_to_strict() {
    let profile: DetectorProfile = "".parse().unwrap();
    assert_eq!(profile, DetectorProfile::Strict);
}

#[test]
fn test_detector_profile_default_is_strict() {
    assert_eq!(DetectorProfile::default(), DetectorProfile::Strict);
}

#[test]
fn test_detector_profile_display() {
    assert_eq!(DetectorProfile::Strict.to_string(), "strict");
    assert_eq!(DetectorProfile::Relaxed.to_string(), "relaxed");
    assert_eq!(DetectorProfile::Minimal.to_string(), "minimal");
}

// ---------------------------------------------------------------------------
// 2. Strict profile matches current defaults
// ---------------------------------------------------------------------------

#[test]
fn test_strict_matches_current_defaults() {
    let config = EffectiveConfig::default_for_test().expect("config");
    let rt = &config.runtime;

    // Strict is default — all hint systems enabled
    assert_eq!(rt.detector_profile, DetectorProfile::Strict);
    assert!(rt.read_repeat_enabled);
    assert!(rt.write_repeat_enabled);
    assert!(rt.write_fail_enabled);
    assert!(rt.read_transition_enabled);
    assert!(rt.phase_force_transition_enabled);

    // Strict default thresholds match documented values
    assert_eq!(rt.loop_detection_threshold, 3);
    assert_eq!(rt.alternating_cycle_threshold, 3);
    assert!((rt.closure_jaccard_threshold - 0.7).abs() < f64::EPSILON);
    assert_eq!(rt.thrash_warn_threshold, 3);
    assert_eq!(rt.thrash_strong_warn_threshold, 4);
    assert_eq!(rt.thrash_break_threshold, 5);
    assert_eq!(rt.read_repeat_warn_threshold, 3);
    assert_eq!(rt.read_repeat_strong_warn_threshold, 6);
    assert_eq!(rt.write_repeat_warn_threshold, 3);
    assert_eq!(rt.write_repeat_strong_warn_threshold, 4);
    assert_eq!(rt.write_fail_threshold, 2);
    assert_eq!(rt.phase_force_transition_threshold, 15);
    assert_eq!(rt.read_transition_threshold, 8);
}

// ---------------------------------------------------------------------------
// 3. Minimal profile disables hint trackers
// ---------------------------------------------------------------------------

#[test]
fn test_minimal_disables_hint_trackers() {
    let mut config = EffectiveConfig::default_for_test().expect("config");
    config.runtime.apply_profile(DetectorProfile::Minimal);

    assert!(!config.runtime.read_repeat_enabled);
    assert!(!config.runtime.write_repeat_enabled);
    assert!(!config.runtime.write_fail_enabled);
    assert!(!config.runtime.read_transition_enabled);
    assert!(!config.runtime.phase_force_transition_enabled);
}

#[test]
fn test_minimal_preserves_break_class_thresholds() {
    let mut config = EffectiveConfig::default_for_test().expect("config");
    let baseline_loop = config.runtime.loop_detection_threshold;
    let baseline_alt = config.runtime.alternating_cycle_threshold;
    let baseline_jaccard = config.runtime.closure_jaccard_threshold;
    let baseline_thrash_break = config.runtime.thrash_break_threshold;

    config.runtime.apply_profile(DetectorProfile::Minimal);

    // Minimal must NOT change Break-class detector thresholds
    assert_eq!(config.runtime.loop_detection_threshold, baseline_loop);
    assert_eq!(config.runtime.alternating_cycle_threshold, baseline_alt);
    assert!((config.runtime.closure_jaccard_threshold - baseline_jaccard).abs() < f64::EPSILON);
    assert_eq!(config.runtime.thrash_break_threshold, baseline_thrash_break);
}

// ---------------------------------------------------------------------------
// 4. Relaxed profile widens thresholds
// ---------------------------------------------------------------------------

#[test]
fn test_relaxed_widens_loop_threshold() {
    let mut config = EffectiveConfig::default_for_test().expect("config");
    config.runtime.apply_profile(DetectorProfile::Relaxed);

    assert_eq!(config.runtime.loop_detection_threshold, 5);
    assert_eq!(config.runtime.alternating_cycle_threshold, 4);
    assert!((config.runtime.closure_jaccard_threshold - 0.8).abs() < f64::EPSILON);
}

#[test]
fn test_relaxed_widens_thrash_thresholds() {
    let mut config = EffectiveConfig::default_for_test().expect("config");
    config.runtime.apply_profile(DetectorProfile::Relaxed);

    assert_eq!(config.runtime.thrash_warn_threshold, 4);
    assert_eq!(config.runtime.thrash_strong_warn_threshold, 5);
    assert_eq!(config.runtime.thrash_break_threshold, 6);
}

#[test]
fn test_relaxed_widens_read_repeat_thresholds() {
    let mut config = EffectiveConfig::default_for_test().expect("config");
    config.runtime.apply_profile(DetectorProfile::Relaxed);

    assert_eq!(config.runtime.read_repeat_warn_threshold, 5);
    assert_eq!(config.runtime.read_repeat_strong_warn_threshold, 8);
}

#[test]
fn test_relaxed_widens_write_repeat_thresholds() {
    let mut config = EffectiveConfig::default_for_test().expect("config");
    config.runtime.apply_profile(DetectorProfile::Relaxed);

    assert_eq!(config.runtime.write_repeat_warn_threshold, 5);
    assert_eq!(config.runtime.write_repeat_strong_warn_threshold, 6);
}

#[test]
fn test_relaxed_widens_write_fail_threshold() {
    let mut config = EffectiveConfig::default_for_test().expect("config");
    config.runtime.apply_profile(DetectorProfile::Relaxed);

    assert_eq!(config.runtime.write_fail_threshold, 3);
}

#[test]
fn test_relaxed_widens_phase_force_transition() {
    let mut config = EffectiveConfig::default_for_test().expect("config");
    config.runtime.apply_profile(DetectorProfile::Relaxed);

    assert_eq!(config.runtime.phase_force_transition_threshold, 20);
    assert_eq!(config.runtime.read_transition_threshold, 12);
}

#[test]
fn test_relaxed_keeps_hint_systems_enabled() {
    let mut config = EffectiveConfig::default_for_test().expect("config");
    config.runtime.apply_profile(DetectorProfile::Relaxed);

    assert!(config.runtime.read_repeat_enabled);
    assert!(config.runtime.write_repeat_enabled);
    assert!(config.runtime.write_fail_enabled);
    assert!(config.runtime.read_transition_enabled);
    assert!(config.runtime.phase_force_transition_enabled);
}

// ---------------------------------------------------------------------------
// 5. All profiles preserve Break-class detectors
// ---------------------------------------------------------------------------

#[test]
fn test_all_profiles_preserve_break_detectors() {
    for profile in [
        DetectorProfile::Strict,
        DetectorProfile::Relaxed,
        DetectorProfile::Minimal,
    ] {
        let mut config = EffectiveConfig::default_for_test().expect("config");
        config.runtime.apply_profile(profile);

        // Loop detection always enabled and within safe range
        assert!(
            config.runtime.loop_detection_threshold >= 2,
            "{profile}: loop_detection_threshold too low"
        );
        // Alternating loop always enabled
        assert!(
            config.runtime.alternating_cycle_threshold >= 2,
            "{profile}: alternating_cycle_threshold too low"
        );
        // Closure loop Jaccard threshold always meaningful
        assert!(
            config.runtime.closure_jaccard_threshold >= 0.3,
            "{profile}: closure_jaccard_threshold too low"
        );
        // PostFailureThrashDetector break threshold always present
        assert!(
            config.runtime.thrash_break_threshold >= 2,
            "{profile}: thrash_break_threshold too low"
        );
    }
}

// ---------------------------------------------------------------------------
// 6. Threshold ordering validation
// ---------------------------------------------------------------------------

#[test]
fn test_validate_detector_thresholds_strict_ok() {
    let config = EffectiveConfig::default_for_test().expect("config");
    assert!(config.runtime.validate_detector_thresholds().is_ok());
}

#[test]
fn test_validate_detector_thresholds_relaxed_ok() {
    let mut config = EffectiveConfig::default_for_test().expect("config");
    config.runtime.apply_profile(DetectorProfile::Relaxed);
    assert!(config.runtime.validate_detector_thresholds().is_ok());
}

#[test]
fn test_validate_detector_thresholds_minimal_ok() {
    let mut config = EffectiveConfig::default_for_test().expect("config");
    config.runtime.apply_profile(DetectorProfile::Minimal);
    assert!(config.runtime.validate_detector_thresholds().is_ok());
}

#[test]
fn test_validate_detector_thresholds_rejects_bad_thrash_ordering() {
    let mut config = EffectiveConfig::default_for_test().expect("config");
    config.runtime.thrash_warn_threshold = 5;
    config.runtime.thrash_strong_warn_threshold = 3; // bad: strong < warn
    assert!(config.runtime.validate_detector_thresholds().is_err());
}

#[test]
fn test_validate_detector_thresholds_rejects_bad_write_repeat_ordering() {
    let mut config = EffectiveConfig::default_for_test().expect("config");
    config.runtime.write_repeat_warn_threshold = 6;
    config.runtime.write_repeat_strong_warn_threshold = 4; // bad
    assert!(config.runtime.validate_detector_thresholds().is_err());
}

// ---------------------------------------------------------------------------
// 7. Security invariant validation
// ---------------------------------------------------------------------------

#[test]
fn test_security_invariants_strict_ok() {
    let config = EffectiveConfig::default_for_test().expect("config");
    assert!(
        config
            .runtime
            .validate_detector_security_invariants()
            .is_ok()
    );
}

#[test]
fn test_security_invariants_relaxed_ok() {
    let mut config = EffectiveConfig::default_for_test().expect("config");
    config.runtime.apply_profile(DetectorProfile::Relaxed);
    assert!(
        config
            .runtime
            .validate_detector_security_invariants()
            .is_ok()
    );
}

#[test]
fn test_security_invariants_minimal_ok() {
    let mut config = EffectiveConfig::default_for_test().expect("config");
    config.runtime.apply_profile(DetectorProfile::Minimal);
    assert!(
        config
            .runtime
            .validate_detector_security_invariants()
            .is_ok()
    );
}

#[test]
fn test_security_invariants_rejects_loop_threshold_too_low() {
    let mut config = EffectiveConfig::default_for_test().expect("config");
    config.runtime.loop_detection_threshold = 1; // below security floor
    assert!(
        config
            .runtime
            .validate_detector_security_invariants()
            .is_err()
    );
}

#[test]
fn test_security_invariants_rejects_alternating_threshold_too_low() {
    let mut config = EffectiveConfig::default_for_test().expect("config");
    config.runtime.alternating_cycle_threshold = 1; // below security floor
    assert!(
        config
            .runtime
            .validate_detector_security_invariants()
            .is_err()
    );
}

#[test]
fn test_security_invariants_rejects_jaccard_out_of_range() {
    let mut config = EffectiveConfig::default_for_test().expect("config");
    config.runtime.closure_jaccard_threshold = 0.1; // too low
    assert!(
        config
            .runtime
            .validate_detector_security_invariants()
            .is_err()
    );
}

#[test]
fn test_security_invariants_rejects_thrash_break_too_low() {
    let mut config = EffectiveConfig::default_for_test().expect("config");
    config.runtime.thrash_break_threshold = 1; // below security floor
    assert!(
        config
            .runtime
            .validate_detector_security_invariants()
            .is_err()
    );
}

// ---------------------------------------------------------------------------
// 8. PhaseEstimator ForceTransition separation
// ---------------------------------------------------------------------------

#[test]
fn test_phase_estimator_force_transition_disabled() {
    use anvil::app::phase_estimator::{PhaseAction, PhaseEstimator};

    let mut est = PhaseEstimator::new(5, 10, 5);
    est.set_force_transition_enabled(false);

    // Even after enough reads, should NOT produce ForceTransition
    for _ in 0..15 {
        let action = est.record_tool_call("file.read", true);
        assert_eq!(action, PhaseAction::Continue);
    }
}

#[test]
fn test_phase_estimator_force_transition_enabled_default() {
    use anvil::app::phase_estimator::{PhaseAction, PhaseEstimator};

    let mut est = PhaseEstimator::new(5, 10, 5);
    // Default: force_transition_enabled = true

    for i in 0..10 {
        let action = est.record_tool_call("file.read", true);
        if i < 9 {
            assert_eq!(action, PhaseAction::Continue);
        } else {
            assert!(matches!(action, PhaseAction::ForceTransition(_)));
        }
    }
}

#[test]
fn test_phase_estimator_fallback_complete_preserved_when_force_disabled() {
    use anvil::app::phase_estimator::{PhaseAction, PhaseEstimator};

    let mut est = PhaseEstimator::new(5, 10, 5);
    est.set_force_transition_enabled(false);

    // Write, then enough reads should still produce FallbackComplete
    est.record_tool_call("file.write", true);
    for _ in 0..5 {
        est.record_tool_call("file.read", true);
    }
    assert_eq!(est.check_empty_response(), PhaseAction::FallbackComplete);
}

// ---------------------------------------------------------------------------
// 9. ANVIL_DETECTOR_PROFILE env var integration
// ---------------------------------------------------------------------------

#[test]
fn test_env_var_detector_profile_relaxed() {
    let mut config = EffectiveConfig::default_for_test().expect("config");
    let mut env = std::collections::HashMap::new();
    env.insert("ANVIL_DETECTOR_PROFILE".to_string(), "relaxed".to_string());
    config
        .apply_env_overrides_from_map_for_test(&env)
        .expect("apply");

    assert_eq!(config.runtime.detector_profile, DetectorProfile::Relaxed);
    assert_eq!(config.runtime.loop_detection_threshold, 5);
}

#[test]
fn test_env_var_detector_profile_minimal() {
    let mut config = EffectiveConfig::default_for_test().expect("config");
    let mut env = std::collections::HashMap::new();
    env.insert("ANVIL_DETECTOR_PROFILE".to_string(), "minimal".to_string());
    config
        .apply_env_overrides_from_map_for_test(&env)
        .expect("apply");

    assert_eq!(config.runtime.detector_profile, DetectorProfile::Minimal);
    assert!(!config.runtime.read_repeat_enabled);
    assert!(!config.runtime.phase_force_transition_enabled);
}

#[test]
fn test_env_var_detector_profile_unknown_falls_back_to_strict() {
    let mut config = EffectiveConfig::default_for_test().expect("config");
    let mut env = std::collections::HashMap::new();
    env.insert("ANVIL_DETECTOR_PROFILE".to_string(), "foobar".to_string());
    config
        .apply_env_overrides_from_map_for_test(&env)
        .expect("apply");

    assert_eq!(config.runtime.detector_profile, DetectorProfile::Strict);
    assert!(config.runtime.read_repeat_enabled);
}

// ---------------------------------------------------------------------------
// 10. ClosureLoopDetector with custom threshold
// ---------------------------------------------------------------------------

#[test]
fn test_closure_loop_detector_with_threshold() {
    use anvil::app::closure_loop_detector::ClosureLoopDetector;
    use anvil::app::loop_detector::LoopAction;

    // A strict (0.7) detector
    let mut strict_det = ClosureLoopDetector::new();
    // A relaxed (0.8) detector
    let mut relaxed_det = ClosureLoopDetector::with_threshold(0.8);

    let content_a = "The resolveAutoAnswer helper appears truncated. \
        Because the worker produced no valid proposal, the fix_slice invocation \
        failed. The late-stage closure branch should therefore abort instead \
        of looping. resolveAutoAnswer, truncated, closure, invalid, proposal, \
        worker, failure, fix_slice, target_path, rewrite, iterations, session.";

    // Record same content twice on both
    strict_det.record_and_check(content_a);
    relaxed_det.record_and_check(content_a);

    // Exact match should trigger both
    let strict_action = strict_det.record_and_check(content_a);
    let relaxed_action = relaxed_det.record_and_check(content_a);

    assert!(
        matches!(strict_action, LoopAction::Warn(_)),
        "strict should warn on exact duplicate"
    );
    assert!(
        matches!(relaxed_action, LoopAction::Warn(_)),
        "relaxed should warn on exact duplicate"
    );
}
