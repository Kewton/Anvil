//! Issue #466: VerifierSkill アダプタ.
//!
//! 既存 turn.rs の post-loop verifier 経路 (auto_test 起動 + AnvilScore 計算) を
//! `AgentSkill` trait の薄いアダプタとしてラップする。
//!
//! 設計方針書 §5 / §6-1 に対応:
//! - `pre_check`: None (Plan disable しない、DR-466-001)
//! - `applicability`: 常時 true (DR-466-002)
//! - `execute`: VerifierOutcome を返し、turn.rs facade が side-effect を適用する
//!   (facade applies outcome、DR-466-002 / Stage 5)
//! - `render_log_payload`: agent.verifier.completed (3 + 1 key、DR3-002)
//!
//! turn.rs から `select_success_verifier` / `build_feedback_for_no_verifier` /
//! `build_feedback_for_auto_test` の物理移動は本 Issue 範囲を超えるため、
//! `pub(super)` 経路で参照する。設計書 §11 で将来 Issue に申し送り。

use std::ffi::OsString;
use std::path::Path;

use super::auto_test::{
    AutoTestKind, AutoTestPlan, AutoTestResult, AutoTestRunner, auto_test_disabled,
};
use crate::agent::orchestration::RepoVerification;
use crate::session::anvil_score::{
    AnvilScore, AnvilScoreInputs, AnvilTestSummary, compute_anvil_score,
};
use crate::session::feedback::FeedbackFrame;

use crate::agent::skills::{
    AgentSkill, RuntimeState, SkillExecuteError, SkillExecutionContext, SkillInput, SkillOutput,
    SkillTrigger, SkillTrustTier,
};

const VERIFIER_TRIGGERS: &[SkillTrigger] = &[SkillTrigger::PostLoop];

/// AutoTest kind の skill 層 view (`auto_test::AutoTestKind` は `pub(super)` で
/// loop_run スコープに閉じる、DR-466-003 / #459 DR2-009 を維持するため).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoTestKindView {
    Build,
    Test,
}

impl From<AutoTestKind> for AutoTestKindView {
    fn from(k: AutoTestKind) -> Self {
        match k {
            AutoTestKind::Build => AutoTestKindView::Build,
            AutoTestKind::Test => AutoTestKindView::Test,
        }
    }
}

/// turn.rs facade が組み立てる VerifierSkill::execute への入力.
pub struct VerifierInputs<'a> {
    pub score_inputs: AnvilScoreInputs<'a>,
    pub repo_verification: Option<&'a RepoVerification>,
    /// facade で算出した success gate (DR-466-001).
    /// false なら AutoTest / Tester 分岐を起動せず Skipped を返す.
    pub should_dispatch_success_verifier: bool,
    /// 既存 should_run_auto_test_for_success() の結果.
    pub protocol_demands_verifier: bool,
    pub changed_files: &'a [String],
    pub tester_candidate_some: bool,
    pub workspace_root: &'a Path,
}

/// VerifierSkill::execute の出力. turn.rs facade が解釈して side-effect を適用する.
#[derive(Debug, Clone)]
pub enum VerifierOutcome {
    /// AutoTest が走り score まで計算できたケース
    AutoTestRan {
        score: AnvilScore,
        auto_test_kind: AutoTestKindView,
        auto_test_passed: bool,
        auto_test_command: String,
        auto_test_output: String,
        auto_test_reason: String,
        anvil_test_summary: AnvilTestSummary,
        feedback: Option<FeedbackFrame>,
    },
    /// AutoTestRunner::run が Err を返した (DR2-002)
    AutoTestTransportError { score: AnvilScore, error: String },
    /// Tester 委譲 (facade で try_invoke_tester を呼ぶ)
    TesterDelegated { score: AnvilScore },
    /// AutoTest / Tester いずれも無く NoVerifier を選んだ
    NoVerifier {
        score: AnvilScore,
        feedback: FeedbackFrame,
    },
    /// success gate false / verifier 候補無し のいずれも吸収
    Skipped { score: AnvilScore },
    /// ANVIL_NO_AUTO_TEST が設定されている場合. AnvilScore は計算済み.
    EnvDisabled { score: AnvilScore },
}

/// VerifierSkill アダプタ. unit struct (状態を持たない).
pub struct VerifierSkill;

#[inline]
fn wrap_skill_output(o: VerifierOutcome) -> SkillOutput {
    SkillOutput::Verifier(Box::new(o))
}

impl AgentSkill for VerifierSkill {
    fn name(&self) -> &'static str {
        "verifier"
    }

    fn triggers(&self) -> &'static [SkillTrigger] {
        VERIFIER_TRIGGERS
    }

    fn pre_check(
        &self,
        _state: &RuntimeState,
        _get_env: &dyn Fn(&str) -> Option<OsString>,
    ) -> Option<crate::agent::loop_run::reminder::SkipReason> {
        // DR-466-001 / DR1-004: VerifierSkill は意図的に Plan disable しない.
        // success gate は facade 側 should_dispatch_success_verifier で判定する.
        None
    }

    fn applicability(&self, _state: &RuntimeState) -> bool {
        // DR-466-002 / DR1-004: 常時 true. compute_anvil_score を必ず走らせる.
        true
    }

    fn execute(
        &mut self,
        input: SkillInput<'_>,
        _ctx: &mut SkillExecutionContext<'_>,
    ) -> Result<SkillOutput, SkillExecuteError> {
        let inputs = match input {
            SkillInput::Verifier(i) => i,
            _ => return Err(SkillExecuteError::InputMismatch("verifier")),
        };

        // [1] success gate false → AutoTest / Tester を起動せず Skipped で compute のみ
        if !inputs.should_dispatch_success_verifier {
            let score = compute_anvil_score(&inputs.score_inputs, inputs.repo_verification, None);
            return Ok(wrap_skill_output(VerifierOutcome::Skipped { score }));
        }

        // [1.5] ANVIL_NO_AUTO_TEST gate: success_gate=true かつ env disabled のみここに入る
        // DR-466-001 不変条件: AnvilScore は必ず compute してから EnvDisabled を返す
        let score_before_env_check =
            compute_anvil_score(&inputs.score_inputs, inputs.repo_verification, None);
        if auto_test_disabled(|k| std::env::var(k)) {
            return Ok(wrap_skill_output(VerifierOutcome::EnvDisabled {
                score: score_before_env_check,
            }));
        }

        // [2] gate true → SuccessVerifier 三値で分岐 (DR2-001/007: AutoTestRunner は
        // unit struct + associated fn で changed_files が必須引数)
        let detected_plan = AutoTestRunner::detect(inputs.workspace_root, inputs.changed_files);
        let auto_test_some = detected_plan.is_some();

        let decision = super::turn::select_success_verifier(
            inputs.protocol_demands_verifier,
            auto_test_some,
            inputs.tester_candidate_some,
        );

        match (decision, detected_plan) {
            (super::turn::SuccessVerifier::AutoTest, Some(plan)) => {
                // DR2-002: Result::Err を AutoTestTransportError variant に分岐
                match AutoTestRunner::run(inputs.workspace_root, &plan) {
                    Ok(result) => {
                        let summary = build_anvil_test_summary_for_skill(&plan, &result);
                        let score = compute_anvil_score(
                            &inputs.score_inputs,
                            inputs.repo_verification,
                            Some(&summary),
                        );
                        let feedback = Some(super::turn::build_feedback_for_auto_test(
                            &plan,
                            &result,
                            inputs.workspace_root,
                            inputs.changed_files,
                        ));
                        Ok(wrap_skill_output(VerifierOutcome::AutoTestRan {
                            score,
                            auto_test_kind: AutoTestKindView::from(plan.auto_test_kind()),
                            auto_test_passed: result.passed,
                            auto_test_command: result.command.clone(),
                            auto_test_output: result.output.clone(),
                            auto_test_reason: plan.reason.clone(),
                            anvil_test_summary: summary,
                            feedback,
                        }))
                    }
                    Err(error) => {
                        let score = compute_anvil_score(
                            &inputs.score_inputs,
                            inputs.repo_verification,
                            None,
                        );
                        Ok(wrap_skill_output(VerifierOutcome::AutoTestTransportError {
                            score,
                            error,
                        }))
                    }
                }
            }
            (super::turn::SuccessVerifier::Tester, _) => {
                let score =
                    compute_anvil_score(&inputs.score_inputs, inputs.repo_verification, None);
                Ok(wrap_skill_output(VerifierOutcome::TesterDelegated {
                    score,
                }))
            }
            (super::turn::SuccessVerifier::NoVerifier, _) => {
                let score =
                    compute_anvil_score(&inputs.score_inputs, inputs.repo_verification, None);
                let feedback = super::turn::build_feedback_for_no_verifier(inputs.workspace_root);
                Ok(wrap_skill_output(VerifierOutcome::NoVerifier {
                    score,
                    feedback,
                }))
            }
            (super::turn::SuccessVerifier::Skip, _)
            | (super::turn::SuccessVerifier::AutoTest, None) => {
                let score =
                    compute_anvil_score(&inputs.score_inputs, inputs.repo_verification, None);
                Ok(wrap_skill_output(VerifierOutcome::Skipped { score }))
            }
        }
    }

    fn render_log_payload(
        &self,
        outcome: &SkillOutput,
        session_id: &str,
        model: Option<&str>,
    ) -> (&'static str, serde_json::Value) {
        let dispatched = match outcome {
            SkillOutput::Verifier(o) => match o.as_ref() {
                VerifierOutcome::AutoTestRan { .. } => "autotest",
                VerifierOutcome::AutoTestTransportError { .. } => "transport_error",
                VerifierOutcome::TesterDelegated { .. } => "tester",
                VerifierOutcome::NoVerifier { .. } => "no_verifier",
                VerifierOutcome::Skipped { .. } => "skip",
                VerifierOutcome::EnvDisabled { .. } => "env_disabled",
            },
            // DR1-007: 将来 variant 追加で更新漏れがあると debug build で panic、release では
            // 安全 fallback。
            _ => {
                debug_assert!(
                    false,
                    "unhandled SkillOutput variant in VerifierSkill::render_log_payload"
                );
                "unknown"
            }
        };
        (
            "agent.verifier.completed",
            serde_json::json!({
                "session_id": session_id,
                "model": model,
                "dispatched": dispatched,
            }),
        )
    }

    /// Issue #467: VerifierSkill は AutoTestRunner 経由で `cargo test` 等の
    /// 固定テンプレート Bash を要求するため `BuiltInCanRequestBash`.
    ///
    /// Plan mode では DR-466-001 の例外として tier_check を bypass し、
    /// 既存の `agent.verifier.completed { dispatched: "skip" }` 経路 (run() 内で
    /// success gate false 時に Skipped 返却) を維持する。bypass は
    /// `skill_bypasses_plan_mode("verifier") == true` で表現される。
    fn tier(&self) -> SkillTrustTier {
        SkillTrustTier::BuiltInCanRequestBash
    }
}

/// turn.rs の private fn `build_anvil_test_summary` を VerifierSkill 側でも参照するため
/// 内部で同じロジックを再現する (本 Issue では turn.rs 側を pub(super) に昇格しない方針、
/// 設計方針書 §11 で将来 Issue に申し送り)。
fn build_anvil_test_summary_for_skill(
    plan: &AutoTestPlan,
    result: &AutoTestResult,
) -> AnvilTestSummary {
    use super::auto_test::{count_compile_errors, count_test_failures};
    match (plan.auto_test_kind(), result.passed) {
        (AutoTestKind::Build, true) => AnvilTestSummary {
            build_passed: Some(true),
            tests_passed: None,
            compile_error_count: Some(0),
            test_failure_count: None,
        },
        (AutoTestKind::Build, false) => AnvilTestSummary {
            build_passed: Some(false),
            tests_passed: None,
            compile_error_count: count_compile_errors(result),
            test_failure_count: None,
        },
        (AutoTestKind::Test, true) => AnvilTestSummary {
            build_passed: None,
            tests_passed: Some(true),
            compile_error_count: None,
            test_failure_count: Some(0),
        },
        (AutoTestKind::Test, false) => AnvilTestSummary {
            build_passed: None,
            tests_passed: Some(false),
            compile_error_count: count_compile_errors(result),
            test_failure_count: count_test_failures(result),
        },
    }
}

/// DR4-001: facade が `verify_commands_collected.push` する前に通す sanitize helper.
/// `mask_secrets` (logging.rs SSOT) → control char flatten → length cap (4096 byte).
/// 空なら None、ガード対象なら Some(s) を返す.
pub(crate) fn sanitize_verify_command_for_case_record(s: &str) -> Option<String> {
    const MAX_BYTES: usize = 4096;
    if s.is_empty() {
        return None;
    }
    let masked = crate::session::feedback::mask_secrets(s);
    let flattened: String = masked
        .chars()
        .map(|c| {
            if c == '\t' || c == ' ' {
                c
            } else if c.is_control() {
                ' '
            } else {
                c
            }
        })
        .collect();
    let trimmed = flattened.trim();
    if trimmed.is_empty() {
        return None;
    }
    let capped = if trimmed.len() > MAX_BYTES {
        trimmed.chars().take(MAX_BYTES).collect()
    } else {
        trimmed.to_string()
    };
    Some(capped)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skill_name_is_verifier() {
        let s = VerifierSkill;
        assert_eq!(s.name(), "verifier");
    }

    #[test]
    fn skill_triggers_post_loop_only() {
        let s = VerifierSkill;
        let triggers = s.triggers();
        assert_eq!(triggers.len(), 1);
        assert!(triggers.contains(&SkillTrigger::PostLoop));
    }

    /// Issue #467: VerifierSkill::tier() == BuiltInCanRequestBash を pin.
    #[test]
    fn verifier_skill_tier_is_can_request_bash() {
        let s = VerifierSkill;
        assert_eq!(s.tier(), SkillTrustTier::BuiltInCanRequestBash);
    }

    #[test]
    fn applicability_always_true() {
        let s = VerifierSkill;
        let snap = crate::session::store::SessionSnapshot::default();
        let state = RuntimeState {
            plan_mode: true, // Plan でも true (DR1-004)
            interrupted: false,
            turn_index: 0,
            session: &snap,
            last_anvil_score: None,
            reminder_sidecar_available: false,
            reminder_kind_eligible: false,
            reminder_called_this_turn: false,
        };
        assert!(s.applicability(&state));
    }

    #[test]
    fn pre_check_returns_none() {
        let s = VerifierSkill;
        let snap = crate::session::store::SessionSnapshot::default();
        let state = RuntimeState {
            plan_mode: true,
            interrupted: true,
            turn_index: 99,
            session: &snap,
            last_anvil_score: None,
            reminder_sidecar_available: false,
            reminder_kind_eligible: false,
            reminder_called_this_turn: true,
        };
        assert!(s.pre_check(&state, &|_| None).is_none());
    }

    #[test]
    fn execute_rejects_non_verifier_input() {
        let mut s = VerifierSkill;
        let mut wm = crate::session::store::WorkingMemory::default();
        let root = std::path::PathBuf::from("/tmp");
        let mut ctx = SkillExecutionContext {
            working_memory: &mut wm,
            workspace_root: &root,
        };
        let result = s.execute(SkillInput::NoOp, &mut ctx);
        assert!(matches!(
            result,
            Err(SkillExecuteError::InputMismatch("verifier"))
        ));
    }

    #[test]
    fn sanitize_verify_command_handles_empty_and_whitespace() {
        assert_eq!(sanitize_verify_command_for_case_record(""), None);
        assert_eq!(sanitize_verify_command_for_case_record("   "), None);
        assert_eq!(
            sanitize_verify_command_for_case_record("cargo build"),
            Some("cargo build".to_string())
        );
    }

    #[test]
    fn sanitize_verify_command_flattens_control_chars() {
        let dirty = "cargo\x00build\x07";
        let cleaned = sanitize_verify_command_for_case_record(dirty);
        assert!(cleaned.is_some());
        let s = cleaned.unwrap();
        assert!(!s.contains('\x00'));
        assert!(!s.contains('\x07'));
    }
}
