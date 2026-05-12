//! Issue #579: FeedbackKind second-pass confirmation E2E smoke suite.
//!
//! Drives `run_feedback_kind_confirm_with_strategy` (the closure-DI
//! orchestrator from `src/agent/loop_run/feedback_kind_confirm.rs`) end-to-end
//! without an Ollama dependency. All 13 cases finish in well under one
//! second.
//!
//! Cases:
//!   FK-01: cargo strong match               → Skipped(HighConfidence)
//!   FK-02: pytest numbered summary          → Skipped(HighConfidence)
//!   FK-03: CompileError first-pass          → Skipped(HighConfidence)
//!   FK-04: UnknownFailure + LLM test_failure→ Confirmed(SecondPassOverridden)
//!   FK-05: weak TestFailure + LLM override  → Confirmed(SecondPassOverridden)
//!   FK-06: Plan mode (skip via wrapper API surface — modelled as
//!          Skipped(PlanMode) payload pin)
//!   FK-07: ANVIL_NO_FEEDBACK_KIND_CONFIRM   → wrapper Skipped(EnvDisabled)
//!          (modelled via Skipped payload pin since `env::var` is process state)
//!   FK-08: PerTurnCapConsumed payload pin
//!   FK-09: timeout                          → Fallback(Timeout)
//!   FK-10: malformed JSON                   → Fallback(Empty / Malformed)
//!   FK-11: allowlist-outside kind           → Fallback(Malformed)
//!   FK-12: response > 16 KiB                → Fallback(ResponseTooLarge)
//!   FK-13: payload pin (event names × keys × is_secret_like_key)

use anvil::agent::loop_run::{
    FEEDBACK_KIND_CONFIRM_RESPONSE_MAX_BYTES, FeedbackKindConfirmInputs,
    FeedbackKindConfirmOutcome, FeedbackKindConfirmationSource, FeedbackKindFallbackReason,
    FeedbackKindSkipReason, build_feedback_kind_confirm_log_payload,
    build_feedback_kind_confirm_prompt, feedback_kind_confirm_disabled,
    run_feedback_kind_confirm_with_strategy, should_request_feedback_confirmation,
};
use anvil::session::feedback::FeedbackKind;

fn inputs<'a>(
    first_pass: &'a FeedbackKind,
    combined_output: &'a str,
    model: Option<&'a str>,
) -> FeedbackKindConfirmInputs<'a> {
    FeedbackKindConfirmInputs {
        first_pass,
        combined_output,
        session_id: "sess-fk-test",
        turn_index: 3,
        model,
    }
}

// ---------------------------------------------------------------------------
// FK-01: cargo strong match → Skipped(HighConfidence)
// ---------------------------------------------------------------------------
#[test]
fn fk_01_cargo_strong_match_skips_second_pass() {
    let fp = FeedbackKind::TestFailure;
    let combined = "running 8 tests\ntest result: FAILED. 6 passed; 2 failed; 0 ignored\n";
    let outcome =
        run_feedback_kind_confirm_with_strategy(inputs(&fp, combined, Some("sidecar")), |_| {
            panic!("LLM must not be called for strong cargo summary")
        });
    assert_eq!(
        outcome,
        FeedbackKindConfirmOutcome::Skipped {
            reason: FeedbackKindSkipReason::HighConfidence,
        },
    );
}

// ---------------------------------------------------------------------------
// FK-02: pytest numbered summary → Skipped(HighConfidence)
// ---------------------------------------------------------------------------
#[test]
fn fk_02_pytest_numbered_summary_skips_second_pass() {
    let fp = FeedbackKind::TestFailure;
    let combined = "tests/test_x.py::test_a FAILED\n===== 3 failed, 12 passed in 2.1s =====\n";
    let outcome =
        run_feedback_kind_confirm_with_strategy(inputs(&fp, combined, Some("sidecar")), |_| {
            panic!("LLM must not be called for strong pytest summary")
        });
    assert_eq!(
        outcome,
        FeedbackKindConfirmOutcome::Skipped {
            reason: FeedbackKindSkipReason::HighConfidence,
        },
    );
}

// ---------------------------------------------------------------------------
// FK-03: CompileError first-pass is outside the second-pass scope
// ---------------------------------------------------------------------------
#[test]
fn fk_03_compile_error_first_pass_skips_second_pass() {
    let fp = FeedbackKind::CompileError;
    let combined = "error[E0432]: unresolved import `foo`\n   --> src/lib.rs:1:5\n";
    let outcome =
        run_feedback_kind_confirm_with_strategy(inputs(&fp, combined, Some("sidecar")), |_| {
            panic!("LLM must not be called for non-failure-ambiguous kinds")
        });
    assert_eq!(
        outcome,
        FeedbackKindConfirmOutcome::Skipped {
            reason: FeedbackKindSkipReason::HighConfidence,
        },
    );
}

// ---------------------------------------------------------------------------
// FK-04: UnknownFailure + LLM returns test_failure → Confirmed(Overridden)
// ---------------------------------------------------------------------------
#[test]
fn fk_04_unknown_failure_overridden_to_test_failure() {
    let fp = FeedbackKind::UnknownFailure;
    let combined = "AssertionError: counter expected 1 got 2\n  at test_foo line 42";
    let invoked = std::cell::Cell::new(0u32);
    let outcome =
        run_feedback_kind_confirm_with_strategy(inputs(&fp, combined, Some("sidecar")), |_| {
            invoked.set(invoked.get() + 1);
            Ok(r#"{"kind":"test_failure","reason":"assertion error"}"#.to_string())
        });
    assert_eq!(
        invoked.get(),
        1,
        "LLM closure should be invoked exactly once"
    );
    match outcome {
        FeedbackKindConfirmOutcome::Confirmed(c) => {
            assert_eq!(c.kind, FeedbackKind::TestFailure);
            assert_eq!(
                c.source,
                FeedbackKindConfirmationSource::SecondPassOverridden,
            );
        }
        other => panic!("expected Confirmed(Overridden), got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// FK-05: weak TestFailure (assert-only) + LLM compile_error → Overridden
// ---------------------------------------------------------------------------
#[test]
fn fk_05_weak_test_failure_overridden_to_compile_error() {
    let fp = FeedbackKind::TestFailure;
    // Pytest-shaped assertion log WITHOUT a numbered summary line.
    let combined = "tests/test_x.py::test_a\n>       assert x == 1\nE       assert 2 == 1";
    let outcome =
        run_feedback_kind_confirm_with_strategy(inputs(&fp, combined, Some("sidecar")), |_| {
            Ok(r#"{"kind":"compile_error","reason":"E0382 borrow"}"#.to_string())
        });
    match outcome {
        FeedbackKindConfirmOutcome::Confirmed(c) => {
            assert_eq!(c.kind, FeedbackKind::CompileError);
            assert_eq!(
                c.source,
                FeedbackKindConfirmationSource::SecondPassOverridden,
            );
        }
        other => panic!("expected Confirmed(Overridden), got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// FK-06: Plan mode skip — exercised via the payload SSoT, since the
//        Plan-mode predicate lives in `Agent::classify_with_feedback_confirm`
//        (which requires a full Agent); the orchestrator surface itself does
//        not consult ExecutionMode. We pin the payload mapping here.
// ---------------------------------------------------------------------------
#[test]
fn fk_06_plan_mode_payload_pin() {
    let fp = FeedbackKind::UnknownFailure;
    let outcome = FeedbackKindConfirmOutcome::Skipped {
        reason: FeedbackKindSkipReason::PlanMode,
    };
    let (event, payload) = build_feedback_kind_confirm_log_payload(
        &outcome,
        "sess-fk-test",
        4,
        &fp,
        Some("sidecar"),
        128,
        None,
    );
    assert_eq!(event, "agent.feedback_kind.skipped");
    assert_eq!(
        payload.get("reason").and_then(|v| v.as_str()),
        Some("plan_mode")
    );
    assert_eq!(
        payload.get("parse_status").and_then(|v| v.as_str()),
        Some("not_invoked")
    );
    assert_eq!(payload.get("latency_ms"), Some(&serde_json::Value::Null));
}

// ---------------------------------------------------------------------------
// FK-07: ANVIL_NO_FEEDBACK_KIND_CONFIRM — disabled-via-env predicate
// ---------------------------------------------------------------------------
#[test]
fn fk_07_env_disabled_helper_returns_true() {
    // disabled-when-set patterns:
    assert!(feedback_kind_confirm_disabled(|k| {
        assert_eq!(k, "ANVIL_NO_FEEDBACK_KIND_CONFIRM");
        Ok("1".to_string())
    }));
    assert!(feedback_kind_confirm_disabled(|_| Ok("true".to_string())));
    // not-disabled cases:
    assert!(!feedback_kind_confirm_disabled(|_| Ok("".to_string())));
    assert!(!feedback_kind_confirm_disabled(|_| Ok("0".to_string())));
    assert!(!feedback_kind_confirm_disabled(|_| Err(
        std::env::VarError::NotPresent
    )));

    // Payload mapping for the EnvDisabled skip path.
    let fp = FeedbackKind::UnknownFailure;
    let outcome = FeedbackKindConfirmOutcome::Skipped {
        reason: FeedbackKindSkipReason::EnvDisabled,
    };
    let (event, payload) = build_feedback_kind_confirm_log_payload(
        &outcome,
        "sess",
        2,
        &fp,
        Some("sidecar"),
        64,
        None,
    );
    assert_eq!(event, "agent.feedback_kind.skipped");
    assert_eq!(
        payload.get("reason").and_then(|v| v.as_str()),
        Some("env_disabled")
    );
}

// ---------------------------------------------------------------------------
// FK-08: PerTurnCapConsumed payload pin
// ---------------------------------------------------------------------------
#[test]
fn fk_08_per_turn_cap_consumed_payload_pin() {
    let fp = FeedbackKind::UnknownFailure;
    let outcome = FeedbackKindConfirmOutcome::Skipped {
        reason: FeedbackKindSkipReason::PerTurnCapConsumed,
    };
    let (event, payload) = build_feedback_kind_confirm_log_payload(
        &outcome,
        "sess",
        9,
        &fp,
        Some("sidecar"),
        256,
        None,
    );
    assert_eq!(event, "agent.feedback_kind.skipped");
    assert_eq!(
        payload.get("reason").and_then(|v| v.as_str()),
        Some("per_turn_cap_consumed"),
    );
    assert_eq!(
        payload.get("source").and_then(|v| v.as_str()),
        Some("first_pass"),
    );
}

// ---------------------------------------------------------------------------
// FK-09: timeout → Fallback(Timeout)
// ---------------------------------------------------------------------------
#[test]
fn fk_09_timeout_falls_back() {
    let fp = FeedbackKind::UnknownFailure;
    let outcome = run_feedback_kind_confirm_with_strategy(
        inputs(&fp, "permission denied", Some("sidecar")),
        |_| Err("HTTP error: operation timed out after 5s".to_string()),
    );
    match outcome {
        FeedbackKindConfirmOutcome::Fallback {
            reason,
            confirmation,
        } => {
            assert_eq!(reason, FeedbackKindFallbackReason::Timeout);
            // first-pass kind is preserved in the confirmation.
            assert_eq!(confirmation.kind, FeedbackKind::UnknownFailure);
            assert_eq!(
                confirmation.source,
                FeedbackKindConfirmationSource::SecondPassFallback,
            );
        }
        other => panic!("expected Fallback(Timeout), got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// FK-10: malformed JSON → Fallback(Empty / Malformed)
// ---------------------------------------------------------------------------
#[test]
fn fk_10_malformed_response_falls_back() {
    let fp = FeedbackKind::UnknownFailure;
    // No braces at all → Empty.
    let outcome =
        run_feedback_kind_confirm_with_strategy(inputs(&fp, "x", Some("sidecar")), |_| {
            Ok("just plain prose".to_string())
        });
    match outcome {
        FeedbackKindConfirmOutcome::Fallback { reason, .. } => {
            assert_eq!(reason, FeedbackKindFallbackReason::Empty);
        }
        other => panic!("expected Fallback(Empty), got {other:?}"),
    }
    // Bad JSON → Malformed.
    let outcome2 =
        run_feedback_kind_confirm_with_strategy(inputs(&fp, "x", Some("sidecar")), |_| {
            Ok(r#"{kind: missing_quotes}"#.to_string())
        });
    match outcome2 {
        FeedbackKindConfirmOutcome::Fallback { reason, .. } => {
            assert_eq!(reason, FeedbackKindFallbackReason::Malformed);
        }
        other => panic!("expected Fallback(Malformed), got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// FK-11: allowlist-outside kind → Fallback(Malformed)
// ---------------------------------------------------------------------------
#[test]
fn fk_11_allowlist_outside_kind_falls_back() {
    let fp = FeedbackKind::UnknownFailure;
    let outcome = run_feedback_kind_confirm_with_strategy(
        inputs(&fp, "x", Some("sidecar")),
        // `timeout` is a valid FeedbackKind variant but NOT in the
        // second-pass allowlist.
        |_| Ok(r#"{"kind":"timeout","reason":"r"}"#.to_string()),
    );
    match outcome {
        FeedbackKindConfirmOutcome::Fallback {
            reason,
            confirmation,
        } => {
            assert_eq!(reason, FeedbackKindFallbackReason::Malformed);
            assert_eq!(confirmation.kind, FeedbackKind::UnknownFailure);
        }
        other => panic!("expected Fallback(Malformed), got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// FK-12: response > 16 KiB → Fallback(ResponseTooLarge)
// ---------------------------------------------------------------------------
#[test]
fn fk_12_oversized_response_falls_back() {
    let fp = FeedbackKind::UnknownFailure;
    let huge = "x".repeat(FEEDBACK_KIND_CONFIRM_RESPONSE_MAX_BYTES + 1);
    let outcome =
        run_feedback_kind_confirm_with_strategy(inputs(&fp, "x", Some("sidecar")), move |_| {
            Ok(huge)
        });
    match outcome {
        FeedbackKindConfirmOutcome::Fallback { reason, .. } => {
            assert_eq!(reason, FeedbackKindFallbackReason::ResponseTooLarge);
        }
        other => panic!("expected Fallback(ResponseTooLarge), got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// FK-13: payload pin — event names × source × parse_status × 10 keys × secret
// ---------------------------------------------------------------------------
#[test]
fn fk_13_payload_pin_covers_all_branches() {
    let fp = FeedbackKind::UnknownFailure;
    let session_id = "sess-fk-13";

    // ---- Confirmed (Overridden) ------------------------------------------
    let conf_outcome =
        FeedbackKindConfirmOutcome::Confirmed(anvil::agent::loop_run::FeedbackKindConfirmation {
            kind: FeedbackKind::TestFailure,
            reason: Some("r".to_string()),
            source: FeedbackKindConfirmationSource::SecondPassOverridden,
        });
    let (event, payload) = build_feedback_kind_confirm_log_payload(
        &conf_outcome,
        session_id,
        5,
        &fp,
        Some("sidecar"),
        512,
        Some(80),
    );
    assert_eq!(event, "agent.feedback_kind.confirmed");
    assert_eq!(
        payload.get("source").and_then(|v| v.as_str()),
        Some("second_pass_overridden"),
    );
    assert_eq!(
        payload.get("parse_status").and_then(|v| v.as_str()),
        Some("ok"),
    );
    assert_eq!(
        payload.get("second_pass_kind").and_then(|v| v.as_str()),
        Some("test_failure"),
    );

    // ---- Skipped (HighConfidence) ----------------------------------------
    let skip_outcome = FeedbackKindConfirmOutcome::Skipped {
        reason: FeedbackKindSkipReason::HighConfidence,
    };
    let (event_s, payload_s) =
        build_feedback_kind_confirm_log_payload(&skip_outcome, session_id, 5, &fp, None, 0, None);
    assert_eq!(event_s, "agent.feedback_kind.skipped");
    assert_eq!(
        payload_s.get("parse_status").and_then(|v| v.as_str()),
        Some("not_invoked"),
    );
    assert_eq!(
        payload_s.get("reason").and_then(|v| v.as_str()),
        Some("high_confidence"),
    );
    assert_eq!(payload_s.get("model"), Some(&serde_json::Value::Null));

    // ---- Fallback (Timeout) ---------------------------------------------
    let fb_timeout = FeedbackKindConfirmOutcome::Fallback {
        reason: FeedbackKindFallbackReason::Timeout,
        confirmation: anvil::agent::loop_run::FeedbackKindConfirmation {
            kind: fp.clone(),
            reason: None,
            source: FeedbackKindConfirmationSource::SecondPassFallback,
        },
    };
    let (event_t, payload_t) = build_feedback_kind_confirm_log_payload(
        &fb_timeout,
        session_id,
        5,
        &fp,
        Some("sidecar"),
        16,
        Some(5_000),
    );
    assert_eq!(event_t, "agent.feedback_kind.fallback");
    assert_eq!(
        payload_t.get("parse_status").and_then(|v| v.as_str()),
        Some("timeout"),
    );

    // ---- Fallback (SidecarUnavailable) — parse_status downgraded to NotInvoked
    let fb_unavail = FeedbackKindConfirmOutcome::Fallback {
        reason: FeedbackKindFallbackReason::SidecarUnavailable,
        confirmation: anvil::agent::loop_run::FeedbackKindConfirmation {
            kind: fp.clone(),
            reason: None,
            source: FeedbackKindConfirmationSource::SecondPassFallback,
        },
    };
    let (_event_u, payload_u) =
        build_feedback_kind_confirm_log_payload(&fb_unavail, session_id, 5, &fp, None, 0, None);
    assert_eq!(
        payload_u.get("parse_status").and_then(|v| v.as_str()),
        Some("not_invoked"),
    );

    // ---- Fallback (ResponseTooLarge) → parse_status `malformed` ---------
    let fb_oversize = FeedbackKindConfirmOutcome::Fallback {
        reason: FeedbackKindFallbackReason::ResponseTooLarge,
        confirmation: anvil::agent::loop_run::FeedbackKindConfirmation {
            kind: fp.clone(),
            reason: None,
            source: FeedbackKindConfirmationSource::SecondPassFallback,
        },
    };
    let (_event_o, payload_o) = build_feedback_kind_confirm_log_payload(
        &fb_oversize,
        session_id,
        5,
        &fp,
        Some("sidecar"),
        16,
        Some(2_000),
    );
    assert_eq!(
        payload_o.get("parse_status").and_then(|v| v.as_str()),
        Some("malformed"),
    );

    // ---- 10-key completeness assertion across every branch --------------
    for payload in [&payload, &payload_s, &payload_t, &payload_u, &payload_o] {
        for key in &[
            "session_id",
            "turn_index",
            "model",
            "first_pass_kind",
            "second_pass_kind",
            "combined_output_bytes",
            "latency_ms",
            "parse_status",
            "reason",
            "source",
        ] {
            assert!(
                payload.get(*key).is_some(),
                "missing key {key} in payload {payload:?}",
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Bonus: should_request_feedback_confirmation public predicate parity
// ---------------------------------------------------------------------------
#[test]
fn should_request_predicate_public_parity() {
    // re-pin the strong/weak matrix at the public API surface.
    assert!(!should_request_feedback_confirmation(
        &FeedbackKind::TestFailure,
        "test result: failed. 1 passed; 2 failed",
    ));
    assert!(!should_request_feedback_confirmation(
        &FeedbackKind::TestFailure,
        "===== 4 failed, 2 passed in 0.5s =====",
    ));
    assert!(should_request_feedback_confirmation(
        &FeedbackKind::TestFailure,
        "AssertionError: bad",
    ));
    assert!(should_request_feedback_confirmation(
        &FeedbackKind::UnknownFailure,
        "",
    ));
    assert!(!should_request_feedback_confirmation(
        &FeedbackKind::CompileError,
        "error[E0382]",
    ));
}

// ---------------------------------------------------------------------------
// Bonus: prompt builder integration smoke
// ---------------------------------------------------------------------------
#[test]
fn prompt_builder_embeds_first_pass_kind_and_caps_input() {
    let fp = FeedbackKind::UnknownFailure;
    let inputs = inputs(&fp, "permission denied: /var/log", Some("sidecar"));
    let prompt = build_feedback_kind_confirm_prompt(&inputs);
    assert!(prompt.contains("First-pass classification: unknown_failure"));
    assert!(prompt.contains("capped at 8192 bytes"));
}
