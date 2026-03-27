//! Tests for AlternatingLoopDetector (Issue #172).

use anvil::app::alternating_loop_detector::{
    AlternatingLoopDetector, DEFAULT_BUFFER_SIZE, DEFAULT_CYCLE_THRESHOLD,
};
use anvil::app::loop_detector::LoopAction;

fn input_a() -> serde_json::Value {
    serde_json::json!({"pattern": "foo", "path": "src/a.rs"})
}

fn input_b() -> serde_json::Value {
    serde_json::json!({"pattern": "bar", "path": "src/b.rs"})
}

fn input_c() -> serde_json::Value {
    serde_json::json!({"pattern": "baz", "path": "src/c.rs"})
}

fn input_d() -> serde_json::Value {
    serde_json::json!({"pattern": "qux", "path": "src/d.rs"})
}

fn input_e() -> serde_json::Value {
    serde_json::json!({"pattern": "quux", "path": "src/e.rs"})
}

#[test]
fn alternating_no_loop() {
    let mut detector = AlternatingLoopDetector::new(DEFAULT_CYCLE_THRESHOLD);
    // All different calls — no cycle
    let a1 = detector.record_and_check("file.search", &input_a());
    let a2 = detector.record_and_check("file.read", &input_b());
    let a3 = detector.record_and_check("file.write", &input_c());
    let a4 = detector.record_and_check("shell.exec", &input_d());
    assert_eq!(a1, LoopAction::Continue);
    assert_eq!(a2, LoopAction::Continue);
    assert_eq!(a3, LoopAction::Continue);
    assert_eq!(a4, LoopAction::Continue);
}

#[test]
fn alternating_cycle_2() {
    let mut detector = AlternatingLoopDetector::new(DEFAULT_CYCLE_THRESHOLD);
    // Cycle of length 2: A-B-A-B-A-B (threshold=3 -> need 2*3=6 entries)
    for _ in 0..2 {
        assert_eq!(
            detector.record_and_check("file.search", &input_a()),
            LoopAction::Continue
        );
        assert_eq!(
            detector.record_and_check("file.read", &input_b()),
            LoopAction::Continue
        );
    }
    // 5th call (A): still building cycle
    assert_eq!(
        detector.record_and_check("file.search", &input_a()),
        LoopAction::Continue
    );
    // 6th call (B): completes the 3rd repetition of A-B pattern
    let action = detector.record_and_check("file.read", &input_b());
    assert!(
        matches!(action, LoopAction::Warn(_)),
        "Expected Warn for cycle-2 detection, got {:?}",
        action
    );
}

#[test]
fn alternating_cycle_3() {
    let mut detector = AlternatingLoopDetector::new(DEFAULT_CYCLE_THRESHOLD);
    // Cycle of length 3: A-B-C repeated 3 times (9 entries)
    for i in 0..9 {
        let action = match i % 3 {
            0 => detector.record_and_check("file.search", &input_a()),
            1 => detector.record_and_check("file.read", &input_b()),
            _ => detector.record_and_check("file.write", &input_c()),
        };
        if i < 8 {
            assert_eq!(
                action,
                LoopAction::Continue,
                "Unexpected action at step {i}"
            );
        } else {
            assert!(
                matches!(action, LoopAction::Warn(_)),
                "Expected Warn at step {i}, got {:?}",
                action
            );
        }
    }
}

#[test]
fn alternating_cycle_5() {
    let mut detector = AlternatingLoopDetector::new(DEFAULT_CYCLE_THRESHOLD);
    let tools = ["t1", "t2", "t3", "t4", "t5"];
    let inputs = [input_a(), input_b(), input_c(), input_d(), input_e()];
    // Cycle of length 5: need 5*3=15 entries
    for i in 0..15 {
        let action = detector.record_and_check(tools[i % 5], &inputs[i % 5]);
        if i < 14 {
            assert_eq!(
                action,
                LoopAction::Continue,
                "Unexpected action at step {i}"
            );
        } else {
            assert!(
                matches!(action, LoopAction::Warn(_)),
                "Expected Warn at step {i}, got {:?}",
                action
            );
        }
    }
}

#[test]
fn alternating_below_threshold() {
    let mut detector = AlternatingLoopDetector::new(DEFAULT_CYCLE_THRESHOLD);
    // Only 2 repetitions of A-B (4 entries), threshold is 3
    for _ in 0..2 {
        assert_eq!(
            detector.record_and_check("file.search", &input_a()),
            LoopAction::Continue
        );
        assert_eq!(
            detector.record_and_check("file.read", &input_b()),
            LoopAction::Continue
        );
    }
    // No detection
}

#[test]
fn alternating_escalation() {
    let mut detector = AlternatingLoopDetector::new(DEFAULT_CYCLE_THRESHOLD);

    // First detection at 6th call: Warn (cycle-2 * threshold-3 = 6 entries)
    for i in 0..5 {
        let action = match i % 2 {
            0 => detector.record_and_check("file.search", &input_a()),
            _ => detector.record_and_check("file.read", &input_b()),
        };
        assert_eq!(action, LoopAction::Continue, "Step {i} should be Continue");
    }
    // 6th call completes cycle -> Warn (escalation_count=1)
    let action = detector.record_and_check("file.read", &input_b());
    assert!(
        matches!(action, LoopAction::Warn(_)),
        "Expected Warn at step 5, got {:?}",
        action
    );

    // 7th call (A): tail 6 entries are still A-B cycle -> StrongWarn (escalation_count=2)
    let action = detector.record_and_check("file.search", &input_a());
    assert!(
        matches!(action, LoopAction::StrongWarn(_)),
        "Expected StrongWarn at step 6, got {:?}",
        action
    );

    // 8th call (B): tail 6 entries are still A-B cycle -> Break (escalation_count=3)
    let action = detector.record_and_check("file.read", &input_b());
    assert!(
        matches!(action, LoopAction::Break(_)),
        "Expected Break at step 7, got {:?}",
        action
    );
}

#[test]
fn alternating_reset() {
    let mut detector = AlternatingLoopDetector::new(DEFAULT_CYCLE_THRESHOLD);

    // Build up some history
    for _ in 0..3 {
        detector.record_and_check("file.search", &input_a());
        detector.record_and_check("file.read", &input_b());
    }

    // Reset
    detector.reset();

    // After reset, 4 calls (2 cycles) should not trigger
    for _ in 0..2 {
        assert_eq!(
            detector.record_and_check("file.search", &input_a()),
            LoopAction::Continue
        );
        assert_eq!(
            detector.record_and_check("file.read", &input_b()),
            LoopAction::Continue
        );
    }
}

#[test]
fn alternating_non_cycle_repetition() {
    let mut detector = AlternatingLoopDetector::new(DEFAULT_CYCLE_THRESHOLD);

    // Non-periodic pattern: A, B, C, A, B, D, A, B, C
    // This does not form a perfect cycle
    detector.record_and_check("file.search", &input_a());
    detector.record_and_check("file.read", &input_b());
    detector.record_and_check("file.write", &input_c());
    detector.record_and_check("file.search", &input_a());
    detector.record_and_check("file.read", &input_b());
    detector.record_and_check("shell.exec", &input_d()); // D instead of C
    detector.record_and_check("file.search", &input_a());
    detector.record_and_check("file.read", &input_b());
    let action = detector.record_and_check("file.write", &input_c());
    assert_eq!(
        action,
        LoopAction::Continue,
        "Non-cyclic pattern should not trigger detection"
    );
}

#[test]
fn alternating_buffer_overflow() {
    let mut detector = AlternatingLoopDetector::new(DEFAULT_CYCLE_THRESHOLD);

    // Fill buffer beyond capacity with unique entries
    for i in 0..(DEFAULT_BUFFER_SIZE + 5) {
        let input = serde_json::json!({"path": format!("file_{}.rs", i)});
        detector.record_and_check("file.read", &input);
    }

    // Now establish a cycle of length 2: should still detect after buffer overflow
    // Need 6 entries for cycle-2 * threshold-3
    for _ in 0..2 {
        detector.record_and_check("file.search", &input_a());
        detector.record_and_check("file.read", &input_b());
    }
    detector.record_and_check("file.search", &input_a());
    let action = detector.record_and_check("file.read", &input_b());
    assert!(
        matches!(action, LoopAction::Warn(_)),
        "Expected Warn after buffer overflow, got {:?}",
        action
    );
}
