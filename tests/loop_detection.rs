//! Tests for the LoopDetector module (Issue #145).

use anvil::app::loop_detector::{DEFAULT_MAX_HISTORY, LoopAction, LoopDetector, fingerprint};

// ============================================================
// LoopAction::merge() tests (Issue #172)
// ============================================================

#[test]
fn loop_action_merge_priority() {
    // Continue merges: other wins
    assert_eq!(
        LoopAction::Continue.merge(LoopAction::Continue),
        LoopAction::Continue
    );
    assert!(matches!(
        LoopAction::Continue.merge(LoopAction::Warn("w".into())),
        LoopAction::Warn(_)
    ));
    assert!(matches!(
        LoopAction::Continue.merge(LoopAction::StrongWarn("s".into())),
        LoopAction::StrongWarn(_)
    ));
    assert!(matches!(
        LoopAction::Continue.merge(LoopAction::Break("b".into())),
        LoopAction::Break(_)
    ));

    // Warn merges
    assert!(matches!(
        LoopAction::Warn("w".into()).merge(LoopAction::Continue),
        LoopAction::Warn(_)
    ));
    assert!(matches!(
        LoopAction::Warn("w1".into()).merge(LoopAction::Warn("w2".into())),
        LoopAction::Warn(_)
    )); // first wins
    assert!(matches!(
        LoopAction::Warn("w".into()).merge(LoopAction::StrongWarn("s".into())),
        LoopAction::StrongWarn(_)
    ));
    assert!(matches!(
        LoopAction::Warn("w".into()).merge(LoopAction::Break("b".into())),
        LoopAction::Break(_)
    ));

    // StrongWarn merges
    assert!(matches!(
        LoopAction::StrongWarn("s".into()).merge(LoopAction::Continue),
        LoopAction::StrongWarn(_)
    ));
    assert!(matches!(
        LoopAction::StrongWarn("s".into()).merge(LoopAction::Warn("w".into())),
        LoopAction::StrongWarn(_)
    ));
    assert!(matches!(
        LoopAction::StrongWarn("s1".into()).merge(LoopAction::StrongWarn("s2".into())),
        LoopAction::StrongWarn(_)
    ));
    assert!(matches!(
        LoopAction::StrongWarn("s".into()).merge(LoopAction::Break("b".into())),
        LoopAction::Break(_)
    ));

    // Break merges: always Break
    assert!(matches!(
        LoopAction::Break("b".into()).merge(LoopAction::Continue),
        LoopAction::Break(_)
    ));
    assert!(matches!(
        LoopAction::Break("b".into()).merge(LoopAction::Warn("w".into())),
        LoopAction::Break(_)
    ));
    assert!(matches!(
        LoopAction::Break("b".into()).merge(LoopAction::StrongWarn("s".into())),
        LoopAction::Break(_)
    ));
    assert!(matches!(
        LoopAction::Break("b1".into()).merge(LoopAction::Break("b2".into())),
        LoopAction::Break(_)
    ));
}

fn same_input() -> serde_json::Value {
    serde_json::json!({"path": "src/main.rs"})
}

#[test]
fn test_no_detection_below_threshold() {
    let mut detector = LoopDetector::new(3);
    // Only 2 calls — below threshold of 3
    let a1 = detector.record_and_check("file.read", &same_input());
    let a2 = detector.record_and_check("file.read", &same_input());
    assert_eq!(a1, LoopAction::Continue);
    assert_eq!(a2, LoopAction::Continue);
}

#[test]
fn test_detection_at_threshold() {
    let mut detector = LoopDetector::new(3);
    detector.record_and_check("file.read", &same_input());
    detector.record_and_check("file.read", &same_input());
    let action = detector.record_and_check("file.read", &same_input());
    assert!(
        matches!(action, LoopAction::Warn(_)),
        "Expected Warn at threshold, got {:?}",
        action
    );
}

#[test]
fn test_escalation_to_strong_warn() {
    let mut detector = LoopDetector::new(3);
    // First 3: Warn
    for _ in 0..3 {
        detector.record_and_check("file.read", &same_input());
    }
    // 4th: should still be Continue (not yet threshold again since escalation_count == 1)
    // Actually after Warn, the next call makes history 4 long, and the tail 3 are still identical
    let action = detector.record_and_check("file.read", &same_input());
    assert!(
        matches!(action, LoopAction::StrongWarn(_)),
        "Expected StrongWarn, got {:?}",
        action
    );
}

#[test]
fn test_escalation_to_break() {
    let mut detector = LoopDetector::new(3);
    // Calls 1-3: Warn at 3rd
    for _ in 0..3 {
        detector.record_and_check("file.read", &same_input());
    }
    // Call 4: StrongWarn
    detector.record_and_check("file.read", &same_input());
    // Call 5: Break
    let action = detector.record_and_check("file.read", &same_input());
    assert!(
        matches!(action, LoopAction::Break(_)),
        "Expected Break, got {:?}",
        action
    );
}

#[test]
fn test_different_tool_names_no_detection() {
    let mut detector = LoopDetector::new(3);
    let input = same_input();
    let a1 = detector.record_and_check("file.read", &input);
    let a2 = detector.record_and_check("file.write", &input);
    let a3 = detector.record_and_check("shell.exec", &input);
    assert_eq!(a1, LoopAction::Continue);
    assert_eq!(a2, LoopAction::Continue);
    assert_eq!(a3, LoopAction::Continue);
}

#[test]
fn test_same_tool_different_args_no_detection() {
    let mut detector = LoopDetector::new(3);
    let a1 = detector.record_and_check("file.read", &serde_json::json!({"path": "a.rs"}));
    let a2 = detector.record_and_check("file.read", &serde_json::json!({"path": "b.rs"}));
    let a3 = detector.record_and_check("file.read", &serde_json::json!({"path": "c.rs"}));
    assert_eq!(a1, LoopAction::Continue);
    assert_eq!(a2, LoopAction::Continue);
    assert_eq!(a3, LoopAction::Continue);
}

#[test]
fn test_custom_threshold() {
    let mut detector = LoopDetector::new(5);
    let input = same_input();
    // 4 calls: below threshold
    for _ in 0..4 {
        let action = detector.record_and_check("file.read", &input);
        assert_eq!(action, LoopAction::Continue);
    }
    // 5th call: detection
    let action = detector.record_and_check("file.read", &input);
    assert!(matches!(action, LoopAction::Warn(_)));
}

#[test]
fn test_ring_buffer_capacity() {
    let mut detector = LoopDetector::new(3);
    let input = same_input();
    // Fill buffer well beyond max_history
    for i in 0..DEFAULT_MAX_HISTORY + 5 {
        let different_input = serde_json::json!({"path": format!("file_{}.rs", i)});
        detector.record_and_check("file.read", &different_input);
    }
    // Now add threshold identical calls — should detect
    for _ in 0..2 {
        detector.record_and_check("file.read", &input);
    }
    let action = detector.record_and_check("file.read", &input);
    assert!(
        matches!(action, LoopAction::Warn(_)),
        "Expected Warn after ring buffer overflow, got {:?}",
        action
    );
}

#[test]
fn test_reset_clears_state() {
    let mut detector = LoopDetector::new(3);
    let input = same_input();
    // Build up to detection
    for _ in 0..3 {
        detector.record_and_check("file.read", &input);
    }
    // Reset
    detector.reset();
    // Should be back to clean state
    for _ in 0..2 {
        let action = detector.record_and_check("file.read", &input);
        assert_eq!(action, LoopAction::Continue);
    }
    // 3rd call after reset should be Warn (escalation_count reset)
    let action = detector.record_and_check("file.read", &input);
    assert!(
        matches!(action, LoopAction::Warn(_)),
        "Expected Warn after reset, got {:?}",
        action
    );
}

#[test]
fn test_fingerprint_deterministic() {
    let input = serde_json::json!({"path": "src/main.rs", "content": "hello"});
    let h1 = fingerprint("file.write", &input);
    let h2 = fingerprint("file.write", &input);
    assert_eq!(h1, h2, "fingerprint should be deterministic");

    // Different tool name → different hash
    let h3 = fingerprint("file.read", &input);
    assert_ne!(
        h1, h3,
        "different tool names should produce different hashes"
    );

    // Different input → different hash
    let other_input = serde_json::json!({"path": "src/lib.rs"});
    let h4 = fingerprint("file.write", &other_input);
    assert_ne!(h1, h4, "different inputs should produce different hashes");
}

#[test]
fn test_warning_message_does_not_contain_input() {
    let mut detector = LoopDetector::new(3);
    let malicious_input = serde_json::json!({"path": "INJECTED_PAYLOAD_$(evil_command)", "content": "<script>alert(1)</script>"});

    for _ in 0..3 {
        detector.record_and_check("file.read", &malicious_input);
    }
    // The Warn message should contain the tool name but not the raw input
    // We already consumed the Warn above. Reset and try again to inspect.
    detector.reset();
    for _ in 0..2 {
        detector.record_and_check("file.read", &malicious_input);
    }
    let action = detector.record_and_check("file.read", &malicious_input);
    if let LoopAction::Warn(msg) = action {
        assert!(
            !msg.contains("INJECTED_PAYLOAD"),
            "Warning should not echo untrusted input"
        );
        assert!(
            !msg.contains("<script>"),
            "Warning should not echo untrusted input"
        );
        assert!(
            msg.contains("file.read"),
            "Warning should contain the tool name"
        );
    } else {
        panic!("Expected Warn, got {:?}", action);
    }
}
