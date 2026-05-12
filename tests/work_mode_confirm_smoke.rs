//! Issue #576: WorkMode second-pass confirmation E2E smoke suite.
//!
//! Drives `run_work_mode_confirm_with_strategy` (the closure-DI orchestrator
//! from `src/agent/loop_run/work_mode_confirm.rs`) end-to-end without an
//! Ollama dependency. All 14 cases finish in well under one second.

use anvil::agent::loop_run::{
    WORK_MODE_CONFIRM_PROMPT_INPUT_MAX_BYTES, WORK_MODE_CONFIRM_RESPONSE_MAX_BYTES,
    WorkModeConfirmInputs, WorkModeConfirmOutcome, WorkModeConfirmationSource,
    WorkModeFallbackReason, WorkModeSkipReason, build_work_mode_confirm_log_payload,
    build_work_mode_confirm_prompt, first_pass_has_explicit_no_edit_signal,
    run_work_mode_confirm_with_strategy, work_mode_confirm_disabled,
};
use anvil::modes::plan_act::{ModeClassification, WorkMode, WorkModeCandidate};

fn make_classification(
    mode: WorkMode,
    confidence: f32,
    ambiguity: bool,
    evidence: Vec<&'static str>,
) -> ModeClassification {
    ModeClassification {
        work_mode: mode,
        intent: "code",
        allows_file_edits: mode != WorkMode::AnswerOnly,
        requires_tests: false,
        confidence,
        ambiguity,
        alternative_gap: 0.10,
        reason: "test",
        evidence,
        alternatives: vec![],
    }
}

fn inputs<'a>(
    fp: &'a ModeClassification,
    raw: &'a str,
    model: Option<&'a str>,
) -> WorkModeConfirmInputs<'a> {
    WorkModeConfirmInputs {
        first_pass: fp,
        raw_input: raw,
        session_id: "sess-test",
        turn_index: 1,
        model,
    }
}

// ---------------------------------------------------------------------------
// WM-01: high confidence (0.95) + ambiguity=false → skipped
// ---------------------------------------------------------------------------
#[test]
fn wm_01_high_confidence_skips_second_pass() {
    let fp = make_classification(WorkMode::GenericCode, 0.95, false, vec![]);
    let outcome = run_work_mode_confirm_with_strategy(inputs(&fp, "fix bug", Some("m")), |_| {
        panic!("LLM must not be called for high-confidence classifications")
    });
    assert_eq!(
        outcome,
        WorkModeConfirmOutcome::Skipped {
            reason: WorkModeSkipReason::HighConfidence
        }
    );
}

// ---------------------------------------------------------------------------
// WM-02: confidence=0.87 (< threshold) → confirmed
// ---------------------------------------------------------------------------
#[test]
fn wm_02_low_confidence_invokes_second_pass_and_confirms() {
    let fp = make_classification(WorkMode::GenericCode, 0.87, false, vec![]);
    let outcome = run_work_mode_confirm_with_strategy(inputs(&fp, "do thing", Some("m")), |_| {
        Ok(r#"{"mode":"generic-code","confidence":0.93,"reason":"agrees"}"#.to_string())
    });
    match outcome {
        WorkModeConfirmOutcome::Confirmed(c) => {
            assert!(matches!(
                c.source,
                WorkModeConfirmationSource::SecondPassConfirmed
                    | WorkModeConfirmationSource::SecondPassOverridden
            ));
        }
        other => panic!("expected Confirmed, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// WM-03: ambiguity=true (confidence not relevant) → confirmed
// ---------------------------------------------------------------------------
#[test]
fn wm_03_ambiguity_invokes_second_pass() {
    let fp = make_classification(WorkMode::GenericCode, 0.95, true, vec![]);
    let invoked = std::cell::Cell::new(false);
    let outcome = run_work_mode_confirm_with_strategy(inputs(&fp, "do x", Some("m")), |_| {
        invoked.set(true);
        Ok(r#"{"mode":"generic-code","confidence":0.95,"reason":"r"}"#.to_string())
    });
    assert!(
        invoked.get(),
        "LLM should have been called for ambiguous classifications"
    );
    assert!(matches!(outcome, WorkModeConfirmOutcome::Confirmed(_)));
}

// ---------------------------------------------------------------------------
// WM-04: LLM response malformed → fallback
// ---------------------------------------------------------------------------
#[test]
fn wm_04_malformed_response_falls_back() {
    let fp = make_classification(WorkMode::GenericCode, 0.50, true, vec![]);
    let outcome = run_work_mode_confirm_with_strategy(inputs(&fp, "x", Some("m")), |_| {
        Ok("this is not JSON".to_string())
    });
    match outcome {
        WorkModeConfirmOutcome::Fallback {
            reason,
            confirmation,
        } => {
            assert_eq!(reason, WorkModeFallbackReason::Empty);
            assert_eq!(
                confirmation.source,
                WorkModeConfirmationSource::SecondPassFallback
            );
        }
        other => panic!("expected Fallback, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// WM-05: ANVIL_NO_MODE_CONFIRM=1 → skipped (env disable)
// ---------------------------------------------------------------------------
#[test]
fn wm_05_env_disable_skips_via_helper() {
    // We can verify the env helper directly. The wrapper logic for env disable
    // lives in `turn.rs::maybe_invoke_work_mode_confirm`, but the helper SSoT
    // is reachable from integration tests.
    assert!(work_mode_confirm_disabled(|_| Ok("1".to_string())));
    assert!(work_mode_confirm_disabled(|_| Ok("yes".to_string())));
    assert!(!work_mode_confirm_disabled(|_| Ok("0".to_string())));
    assert!(!work_mode_confirm_disabled(|_| Ok("".to_string())));
    assert!(!work_mode_confirm_disabled(|_| Err(
        std::env::VarError::NotPresent
    )));
}

// ---------------------------------------------------------------------------
// WM-06: sidecar=None → fallback (fail-open)
// ---------------------------------------------------------------------------
#[test]
fn wm_06_no_sidecar_falls_back_fail_open() {
    let fp = make_classification(WorkMode::GenericCode, 0.50, true, vec![]);
    let outcome =
        run_work_mode_confirm_with_strategy(inputs(&fp, "x", None), |_| Ok("ignored".to_string()));
    match outcome {
        WorkModeConfirmOutcome::Fallback { reason, .. } => {
            assert_eq!(reason, WorkModeFallbackReason::SidecarUnavailable);
        }
        other => panic!("expected Fallback(SidecarUnavailable), got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// WM-07: second-pass result is preserved in WorkModeConfirmation.mode so the
// caller can write it back into `session.mode_state.work_mode` for
// `answer_only_mode_active` to honour. This test exercises the orchestrator
// output shape that `answer_only_mode_active` depends on.
// ---------------------------------------------------------------------------
#[test]
fn wm_07_second_pass_result_carries_mode_for_session_writeback() {
    // First-pass says AnswerOnly (low confidence). Second pass overrides to
    // GenericCode — the orchestrator returns that as the resolved confirmation,
    // which the wrapper would then assign into session.mode_state.work_mode.
    let fp = make_classification(WorkMode::AnswerOnly, 0.60, true, vec!["answer-request"]);
    let outcome = run_work_mode_confirm_with_strategy(inputs(&fp, "edit src", Some("m")), |_| {
        Ok(r#"{"mode":"generic-code","confidence":0.92,"reason":"actually edits"}"#.to_string())
    });
    match outcome {
        WorkModeConfirmOutcome::Confirmed(c) => {
            assert_eq!(c.mode, WorkMode::GenericCode);
            assert_eq!(c.source, WorkModeConfirmationSource::SecondPassOverridden);
        }
        other => panic!("expected Confirmed, got {other:?}"),
    }

    // Reverse direction: first pass GenericCode, second pass corrects to AnswerOnly
    // (would make `answer_only_mode_active` return true after writeback).
    let fp2 = make_classification(WorkMode::GenericCode, 0.50, true, vec![]);
    let outcome2 =
        run_work_mode_confirm_with_strategy(inputs(&fp2, "explain x", Some("m")), |_| {
            Ok(r#"{"mode":"answer-only","confidence":0.94,"reason":"read-only"}"#.to_string())
        });
    match outcome2 {
        WorkModeConfirmOutcome::Confirmed(c) => {
            assert_eq!(c.mode, WorkMode::AnswerOnly);
            assert_eq!(c.source, WorkModeConfirmationSource::SecondPassOverridden);
        }
        other => panic!("expected Confirmed, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// WM-08: per-turn cap shape verification (orchestrator itself does not own the
// cap — the wrapper does — but the closure is FnOnce so we verify the
// invocation count is constrained to max one LLM dispatch per call).
// ---------------------------------------------------------------------------
#[test]
fn wm_08_orchestrator_invokes_llm_at_most_once() {
    let fp = make_classification(WorkMode::GenericCode, 0.50, true, vec![]);
    let call_count = std::cell::Cell::new(0_usize);
    let _ = run_work_mode_confirm_with_strategy(inputs(&fp, "x", Some("m")), |_| {
        call_count.set(call_count.get() + 1);
        Ok(r#"{"mode":"generic-code","confidence":0.91,"reason":"r"}"#.to_string())
    });
    assert_eq!(
        call_count.get(),
        1,
        "orchestrator must dispatch exactly once"
    );
}

// ---------------------------------------------------------------------------
// WM-09: turn_index appears in the log payload as `turn_index`, distinct from
// session_id, so dataset export can join events by (session_id, turn_index).
// ---------------------------------------------------------------------------
#[test]
fn wm_09_log_payload_includes_turn_index_join_key() {
    let fp = make_classification(WorkMode::GenericCode, 0.50, true, vec![]);
    let outcome = WorkModeConfirmOutcome::Confirmed(anvil::agent::loop_run::WorkModeConfirmation {
        mode: WorkMode::Docs,
        confidence: 0.91,
        source: WorkModeConfirmationSource::SecondPassOverridden,
        reason: Some("test".to_string()),
    });
    let (event, payload) = build_work_mode_confirm_log_payload(
        &outcome,
        "sess-X",
        Some("m"),
        42,
        &fp,
        Some(120),
        anvil::agent::loop_run::WorkModeConfirmParseStatus::Ok,
    );
    assert_eq!(event, "agent.work_mode.confirmed");
    assert_eq!(payload["turn_index"], serde_json::json!(42));
    assert_eq!(payload["session_id"], serde_json::json!("sess-X"));
}

// ---------------------------------------------------------------------------
// WM-10: Plan mode skip is handled by the wrapper (turn.rs). At the
// orchestrator boundary we verify the wrapper's contract: when the wrapper
// determines Plan mode it builds a `Skipped(PlanMode)` outcome via the SSoT
// payload helper and emits `agent.work_mode.skipped`.
// ---------------------------------------------------------------------------
#[test]
fn wm_10_plan_mode_skip_is_well_formed() {
    let fp = make_classification(WorkMode::GenericCode, 0.50, true, vec![]);
    let outcome = WorkModeConfirmOutcome::Skipped {
        reason: WorkModeSkipReason::PlanMode,
    };
    let (event, payload) = build_work_mode_confirm_log_payload(
        &outcome,
        "sess",
        None,
        1,
        &fp,
        None,
        anvil::agent::loop_run::WorkModeConfirmParseStatus::NotInvoked,
    );
    assert_eq!(event, "agent.work_mode.skipped");
    assert_eq!(payload["reason"], serde_json::json!("plan_mode"));
    assert_eq!(payload["parse_status"], serde_json::json!("not_invoked"));
}

// ---------------------------------------------------------------------------
// WM-11: timeout error → Fallback(Timeout) + parse_status maps appropriately
// in the wrapper. We verify orchestrator + log payload shape here; the
// wrapper-side `parse_status=timeout` mapping is exercised by the orchestrator
// returning Fallback(Timeout).
// ---------------------------------------------------------------------------
#[test]
fn wm_11_timeout_response_falls_back_with_timeout_reason() {
    let fp = make_classification(WorkMode::GenericCode, 0.50, true, vec![]);
    let outcome = run_work_mode_confirm_with_strategy(inputs(&fp, "x", Some("m")), |_| {
        Err("read timed out after 10s".to_string())
    });
    match outcome {
        WorkModeConfirmOutcome::Fallback { reason, .. } => {
            assert_eq!(reason, WorkModeFallbackReason::Timeout);
        }
        other => panic!("expected Fallback(Timeout), got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// WM-12: explicit-no-edit signal must NOT be overridden by the LLM, even if
// the LLM tries to return `generic-code`.
// ---------------------------------------------------------------------------
#[test]
fn wm_12_explicit_no_edit_prevents_llm_privilege_escalation() {
    // Top-level evidence path.
    let fp = make_classification(WorkMode::AnswerOnly, 0.95, false, vec!["explicit-no-edit"]);
    assert!(first_pass_has_explicit_no_edit_signal(&fp));
    let outcome = run_work_mode_confirm_with_strategy(
        inputs(&fp, "do not modify; explain", Some("m")),
        |_| {
            // LLM tries to escalate to GenericCode.
            Ok(r#"{"mode":"generic-code","confidence":0.99,"reason":"override"}"#.to_string())
        },
    );
    // Must be Skipped (ExplicitReadOnly) — LLM never called.
    assert_eq!(
        outcome,
        WorkModeConfirmOutcome::Skipped {
            reason: WorkModeSkipReason::ExplicitReadOnly
        }
    );

    // Alternative-candidate path: even if top-level evidence doesn't include
    // explicit-no-edit but an AnswerOnly alternative does, the same guard fires.
    let mut fp2 = make_classification(WorkMode::AnswerOnly, 0.95, false, vec![]);
    fp2.alternatives.push(WorkModeCandidate {
        work_mode: WorkMode::AnswerOnly,
        intent: "answer",
        confidence: 0.95,
        evidence: vec!["explicit-no-edit"],
    });
    assert!(first_pass_has_explicit_no_edit_signal(&fp2));
}

// ---------------------------------------------------------------------------
// WM-13: prompt secret-mask + byte cap (DR4-002).
// ---------------------------------------------------------------------------
#[test]
fn wm_13_prompt_masks_secrets_and_caps_input() {
    // Input contains a secret-looking literal and is much larger than the
    // 4 KiB cap. `mask_secrets` should remove the literal; the cap should be
    // referenced in the prompt as part of the "(secret-masked and capped at
    // 4096 bytes)" instruction text.
    let secret = "API_KEY=sk-abcdef0123456789abcdef0123456789";
    let big = format!(
        "{secret} {}",
        "x".repeat(WORK_MODE_CONFIRM_PROMPT_INPUT_MAX_BYTES + 100)
    );
    let fp = make_classification(WorkMode::GenericCode, 0.50, true, vec![]);
    let prompt = build_work_mode_confirm_prompt(&inputs(&fp, &big, Some("m")));
    // Secret literal must not survive masking.
    assert!(
        !prompt.contains("sk-abcdef0123456789abcdef0123456789"),
        "secret leaked into prompt"
    );
    // Cap label is in the prompt instructions.
    assert!(prompt.contains("capped at 4096 bytes"));
    // Even with a giant raw input, the prompt total length is bounded — the
    // user-request slice is at most 4 KiB, plus a fixed-size instruction
    // header (~1 KiB). Allow generous slack.
    assert!(
        prompt.len() < WORK_MODE_CONFIRM_PROMPT_INPUT_MAX_BYTES + 4096,
        "prompt length {} exceeded reasonable upper bound",
        prompt.len()
    );
}

// ---------------------------------------------------------------------------
// WM-14: oversized response → Fallback(ResponseTooLarge), and consecutive
// per-user-input dispatches do not retry sidecar (per-turn cap is the
// wrapper's job; the orchestrator just ensures one-and-done semantics through
// the FnOnce closure type — captured here as a property of the closure
// signature).
// ---------------------------------------------------------------------------
#[test]
fn wm_14_oversized_response_falls_back_and_orchestrator_uses_fnonce() {
    let fp = make_classification(WorkMode::GenericCode, 0.50, true, vec![]);
    let huge = "x".repeat(WORK_MODE_CONFIRM_RESPONSE_MAX_BYTES + 1);
    let outcome =
        run_work_mode_confirm_with_strategy(inputs(&fp, "x", Some("m")), move |_| Ok(huge));
    match outcome {
        WorkModeConfirmOutcome::Fallback { reason, .. } => {
            assert_eq!(reason, WorkModeFallbackReason::ResponseTooLarge);
        }
        other => panic!("expected Fallback(ResponseTooLarge), got {other:?}"),
    }
}
