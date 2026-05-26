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
//! success.rs owns verifier selection and no-verifier feedback; turn.rs still
//! owns the AutoTest feedback builder until the remaining tool feedback
//! builders are split as a separate boundary.

use std::ffi::OsString;
use std::path::Path;

use super::auto_test::{
    AutoTestKind, AutoTestPlan, AutoTestResult, AutoTestRunner, OwnedTestVerifierPlan,
    auto_test_disabled, combined_output_for_classify,
};
use super::project_probe::ProjectUnit;
use super::task_workspace_scope::TaskWorkspaceScope;
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
    pub recent_successful_bash_commands: &'a [String],
    pub tester_candidate_some: bool,
    pub workspace_root: &'a Path,
    /// Issue #651 Task 3.1: owned test artifact paths the current task is
    /// allowed to bind to a structured verifier command. SSOT producer is
    /// `super::artifact_ownership::owned_test_artifacts(...)`.
    ///
    /// Contract (DR1-007):
    /// - The skills framework crate (`src/agent/skills/mod.rs`) MUST NOT
    ///   read this field; it is exposed across the `SkillInput::Verifier`
    ///   public boundary only because `VerifierInputs` itself is `pub`.
    ///   The field is consumed exclusively by `VerifierSkill::execute` in
    ///   this module.
    /// - An empty slice (`&[]`) is the back-compat sentinel — the
    ///   structured verifier path treats it as `Missing` when
    ///   `test_execution_required` is true, and as "no-op" otherwise.
    /// - The slice is **never persisted**: it is a per-turn borrow into
    ///   the planner-side artifact view.
    pub owned_test_artifacts: &'a [String],
    /// Project-unit facts selected by the controller for this turn. When
    /// verifier execution is required, verifier discovery must use this
    /// bounded project-unit view instead of root-level stack guessing.
    pub(crate) project_unit: Option<&'a ProjectUnit>,
    /// Issue #651 Task 3.1 / 3.3: gate that flips the structured Weak /
    /// Missing branch on. SSOT: `RequiredBehaviorContract.test_execution_required`,
    /// itself backed by `task_contract::request_asks_for_test_artifact`.
    ///
    /// `false` preserves the legacy `detect_with_recent_successes -> run`
    /// path verbatim (regression guard for every non-test-bearing
    /// request). `true` routes through
    /// `AutoTestRunner::detect_with_owned_test_artifacts` →
    /// `OwnedTestVerifierPlan::{Runnable, Weak, Missing}`.
    pub test_execution_required: bool,
    /// Issue #651 Task 3.3: workspace scope the planner is currently
    /// operating in. Required so `run_structured` can re-validate every
    /// `command.bound_test_artifacts` path at execution time
    /// (canonicalize + scope re-check, defends against TOCTOU symlink
    /// swap between planning and execution).
    ///
    /// Visibility is intentionally `pub(crate)`, narrower than the
    /// surrounding `pub struct VerifierInputs`, because
    /// `TaskWorkspaceScope` is itself `pub(crate)` (DR3-001 boundary).
    /// The skills framework crate does not need to read this field —
    /// `VerifierSkill::execute` is the sole in-crate consumer.
    pub(crate) workspace_scope: &'a TaskWorkspaceScope,
}

/// VerifierSkill::execute の出力. turn.rs facade が解釈して side-effect を適用する.
#[derive(Debug, Clone)]
pub enum VerifierOutcome {
    /// AutoTest が走り score まで計算できたケース.
    ///
    /// `auto_test_output` (現存) と `auto_test_combined_output` (新規, Issue #579) は
    /// 由来が異なるので混同しないこと (DR2-001 / DR2-015):
    /// - `auto_test_output`: `AutoTestResult.output` の clone。truncated 済みの
    ///   stdout/stderr 結合ログ。facade で `error_text` 等に再露出される表示用テキスト。
    /// - `auto_test_combined_output`: `combined_output_for_classify(&result)` の戻り値。
    ///   `classify_auto_test` が見た文字列と**同一** (DR1-001 SSoT 保証)。second-pass
    ///   confirmation の strong-match 判定はこの文字列で行うため、output 構築ロジックに
    ///   ズレが生じても classification と判定とが食い違わない。
    AutoTestRan {
        score: AnvilScore,
        auto_test_kind: AutoTestKindView,
        auto_test_passed: bool,
        auto_test_command: String,
        auto_test_output: String,
        auto_test_reason: String,
        anvil_test_summary: AnvilTestSummary,
        feedback: Option<FeedbackFrame>,
        /// Issue #579 / DR1-001: snapshot of the exact string
        /// `classify_auto_test` consumed. Carried through to the facade so the
        /// second-pass `should_request_feedback_confirmation` predicate runs on
        /// the same input as the first-pass classifier.
        auto_test_combined_output: String,
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
    /// Issue #651: a structured verifier candidate was detected but the
    /// owned test artifacts could not be bound to it (free-form shell
    /// from ProjectInstruction / RecentSuccessfulBash, unsupported
    /// toolchain, etc.). Caller (success.rs / turn.rs) maps this to
    /// `CompletionDecision::SafeStop { reason: VerifierWeak }`.
    ///
    /// Payload is intentionally narrow (no `display_command` / path
    /// list / output) so the per-turn log emit stays well under the
    /// 1 KiB payload budget (design 8-A) and never leaks raw command
    /// text past the `redact_verifier_command_for_storage` SSOT.
    Weak {
        score: AnvilScore,
        owned_test_artifacts_count: usize,
        command_runner: &'static str,
    },
    /// Issue #651: no runnable verifier could be detected at all (and
    /// `test_execution_required == true`). Caller maps this to
    /// `CompletionDecision::SafeStop { reason: VerifierMissing }`.
    Missing {
        score: AnvilScore,
        owned_test_artifacts_count: usize,
    },
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

        // [2] Issue #651: structured verifier gate. Activates only when the
        // request literally asks for test execution evidence
        // (`RequiredBehaviorContract.test_execution_required`) AND the
        // protocol demands a verifier this turn. In that mode the legacy
        // `detect_with_recent_successes -> run` path is bypassed because
        // a free-form shell command could otherwise execute without the
        // current task's owned test artifact appearing in argv (Issue #651
        // root cause).
        //
        // When `test_execution_required == false` we deliberately keep the
        // legacy path untouched so every non-test-bearing request preserves
        // its existing behaviour (Phase 4.1 regression guard).
        if inputs.test_execution_required && inputs.protocol_demands_verifier {
            let owned_plan = if let Some(project_unit) = inputs.project_unit {
                AutoTestRunner::detect_with_owned_test_artifacts_and_project_unit(
                    inputs.workspace_root,
                    inputs.changed_files,
                    inputs.recent_successful_bash_commands,
                    inputs.owned_test_artifacts,
                    Some(project_unit),
                )
            } else {
                OwnedTestVerifierPlan::Missing
            };
            match owned_plan {
                OwnedTestVerifierPlan::Runnable { plan, command } => {
                    let display_command = command.to_display_string();
                    match AutoTestRunner::run_structured(
                        inputs.workspace_root,
                        inputs.workspace_scope,
                        &command,
                        &display_command,
                    ) {
                        Ok(result) => {
                            let summary = build_anvil_test_summary_for_skill(&plan, &result);
                            let score = compute_anvil_score(
                                &inputs.score_inputs,
                                inputs.repo_verification,
                                Some(&summary),
                            );
                            let combined_output = combined_output_for_classify(&result);
                            let feedback = Some(super::turn::build_feedback_for_auto_test(
                                &plan,
                                &result,
                                inputs.workspace_root,
                                inputs.changed_files,
                            ));
                            return Ok(wrap_skill_output(VerifierOutcome::AutoTestRan {
                                score,
                                auto_test_kind: AutoTestKindView::from(plan.auto_test_kind()),
                                auto_test_passed: result.passed,
                                auto_test_command: result.command.clone(),
                                auto_test_output: result.output.clone(),
                                auto_test_reason: plan.reason.clone(),
                                anvil_test_summary: summary,
                                feedback,
                                auto_test_combined_output: combined_output,
                            }));
                        }
                        Err(error) => {
                            let score = compute_anvil_score(
                                &inputs.score_inputs,
                                inputs.repo_verification,
                                None,
                            );
                            return Ok(wrap_skill_output(
                                VerifierOutcome::AutoTestTransportError { score, error },
                            ));
                        }
                    }
                }
                OwnedTestVerifierPlan::Weak {
                    detected_source, ..
                } => {
                    let score =
                        compute_anvil_score(&inputs.score_inputs, inputs.repo_verification, None);
                    return Ok(wrap_skill_output(VerifierOutcome::Weak {
                        score,
                        owned_test_artifacts_count: inputs.owned_test_artifacts.len(),
                        command_runner: detected_source,
                    }));
                }
                OwnedTestVerifierPlan::Missing => {
                    let score =
                        compute_anvil_score(&inputs.score_inputs, inputs.repo_verification, None);
                    return Ok(wrap_skill_output(VerifierOutcome::Missing {
                        score,
                        owned_test_artifacts_count: inputs.owned_test_artifacts.len(),
                    }));
                }
            }
        }

        // [3] gate true → SuccessVerifier 三値で分岐 (DR2-001/007: AutoTestRunner は
        // unit struct + associated fn で changed_files が必須引数)
        let detected_plan = AutoTestRunner::detect_with_project_unit(
            inputs.workspace_root,
            inputs.changed_files,
            inputs.recent_successful_bash_commands,
            inputs.project_unit,
        );
        let auto_test_some = detected_plan.is_some();

        let decision = super::success::select_success_verifier(
            inputs.protocol_demands_verifier,
            auto_test_some,
            inputs.tester_candidate_some,
        );

        match (decision, detected_plan) {
            (super::success::SuccessVerifier::AutoTest, Some(plan)) => {
                // DR2-002: Result::Err を AutoTestTransportError variant に分岐
                match AutoTestRunner::run(inputs.workspace_root, &plan) {
                    Ok(result) => {
                        let summary = build_anvil_test_summary_for_skill(&plan, &result);
                        let score = compute_anvil_score(
                            &inputs.score_inputs,
                            inputs.repo_verification,
                            Some(&summary),
                        );
                        // Issue #579 / DR1-001: evaluate `combined_output_for_classify`
                        // exactly once so the same string flows into both
                        // `classify_auto_test` (via `build_feedback_for_auto_test`)
                        // and the second-pass confirmation in `success.rs`.
                        let combined_output = combined_output_for_classify(&result);
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
                            auto_test_combined_output: combined_output,
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
            (super::success::SuccessVerifier::Tester, _) => {
                let score =
                    compute_anvil_score(&inputs.score_inputs, inputs.repo_verification, None);
                Ok(wrap_skill_output(VerifierOutcome::TesterDelegated {
                    score,
                }))
            }
            (super::success::SuccessVerifier::NoVerifier, _) => {
                let score =
                    compute_anvil_score(&inputs.score_inputs, inputs.repo_verification, None);
                let feedback =
                    super::success::build_feedback_for_no_verifier(inputs.workspace_root);
                Ok(wrap_skill_output(VerifierOutcome::NoVerifier {
                    score,
                    feedback,
                }))
            }
            (super::success::SuccessVerifier::Skip, _)
            | (super::success::SuccessVerifier::AutoTest, None) => {
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
                // Issue #651: `_ =>` is forbidden so future
                // VerifierOutcome variants light up compile errors.
                VerifierOutcome::Weak { .. } => "weak",
                VerifierOutcome::Missing { .. } => "missing",
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

impl VerifierSkill {
    /// Issue #661 iteration-4 Task 5.3 (DR1-005 / DR2-001 regression guard):
    /// pre-spawn observer-aware variant of the structured verifier path.
    ///
    /// Order of operations (DR1-005 emit ownership):
    ///   1. Detect the `OwnedTestVerifierPlan` via
    ///      `AutoTestRunner::detect_with_owned_test_artifacts`.
    ///   2. For `Runnable`: build the pre-spawn `VerifierInvokedSnapshot`
    ///      and call `on_pre_spawn(snapshot)` BEFORE `run_structured`
    ///      spawns the verifier process. The callback owns the emit +
    ///      dedup (`turn.rs::emit_agent_verifier_invoked_if_new`).
    ///   3. Spawn via `AutoTestRunner::run_structured` and map the result
    ///      into `VerifierOutcome`.
    ///
    /// `AgentSkill::execute` keeps its existing signature unchanged
    /// (DR2-001). The trait method bypasses this observer hook entirely
    /// so the skills framework callers (skills mod) preserve their
    /// pre-#661 behavior.
    ///
    /// Preconditions: caller must have validated `should_dispatch_success_verifier`
    /// and ensured `inputs.test_execution_required && inputs.protocol_demands_verifier`
    /// hold. The legacy path (`test_execution_required = false`) MUST go
    /// through `AgentSkill::execute` so this method does not need to fall
    /// back to it.
    #[allow(dead_code)] // wired by `turn.rs::run_post_loop_success_verifier` callers in iteration-4 Task 5.4 if structured post-loop emit is enabled
    pub(super) fn execute_with_invocation_observer(
        &mut self,
        inputs: VerifierInputs<'_>,
        on_pre_spawn: &mut dyn FnMut(&super::auto_test::VerifierInvokedSnapshot),
    ) -> VerifierOutcome {
        debug_assert!(
            inputs.test_execution_required && inputs.protocol_demands_verifier,
            "execute_with_invocation_observer must only be entered on the structured path"
        );
        // CB-005 runtime gate (release build でも有効): facade dispatch を
        // bypass されても、AgentSkill::execute と同じ早期 return 条件を
        // 必ず強制する。これにより `should_dispatch_success_verifier=false`
        // / `auto_test_disabled` / `test_execution_required=false` のいずれ
        // でも `Skipped` を返し、observer callback を発火させない。
        let bypass_score =
            || compute_anvil_score(&inputs.score_inputs, inputs.repo_verification, None);
        if !inputs.test_execution_required || !inputs.protocol_demands_verifier {
            return VerifierOutcome::Skipped {
                score: bypass_score(),
            };
        }
        if !inputs.should_dispatch_success_verifier {
            return VerifierOutcome::Skipped {
                score: bypass_score(),
            };
        }
        if auto_test_disabled(|k| std::env::var(k)) {
            return VerifierOutcome::Skipped {
                score: bypass_score(),
            };
        }
        let owned_plan = if let Some(project_unit) = inputs.project_unit {
            AutoTestRunner::detect_with_owned_test_artifacts_and_project_unit(
                inputs.workspace_root,
                inputs.changed_files,
                inputs.recent_successful_bash_commands,
                inputs.owned_test_artifacts,
                Some(project_unit),
            )
        } else {
            OwnedTestVerifierPlan::Missing
        };
        match owned_plan {
            OwnedTestVerifierPlan::Runnable { plan, command } => {
                let display_command = command.to_display_string();
                // DR1-005: pre-spawn snapshot + callback BEFORE
                // `run_structured`. The callback owns the dedup-aware
                // `agent.verifier.invoked` emit; this method never calls
                // `log_llm_event` directly so the emit ownership stays in
                // `turn.rs` (Agent-side dedup state).
                //
                // Issue #661 iteration-5 Task 6.2: pass runner-specific
                // extras (VERIFIER_ENV_PYTHON_EXTRA for python3) so the
                // pre-spawn snapshot mirrors the execution-time env plan
                // built inside `run_structured`.
                let extras_for_emit: &[(&'static str, &'static str)] = match command.runner() {
                    "python3" => super::auto_test::VERIFIER_ENV_PYTHON_EXTRA,
                    _ => &[],
                };
                let env_plan = super::auto_test::build_hermetic_env_plan(
                    inputs.workspace_root,
                    extras_for_emit,
                );
                if let Some(snapshot) =
                    super::auto_test::VerifierInvokedSnapshot::from_command_and_env(
                        &command, &env_plan,
                    )
                {
                    on_pre_spawn(&snapshot);
                }
                match AutoTestRunner::run_structured(
                    inputs.workspace_root,
                    inputs.workspace_scope,
                    &command,
                    &display_command,
                ) {
                    Ok(result) => {
                        let summary = build_anvil_test_summary_for_skill(&plan, &result);
                        let score = compute_anvil_score(
                            &inputs.score_inputs,
                            inputs.repo_verification,
                            Some(&summary),
                        );
                        let combined_output = combined_output_for_classify(&result);
                        let feedback = Some(super::turn::build_feedback_for_auto_test(
                            &plan,
                            &result,
                            inputs.workspace_root,
                            inputs.changed_files,
                        ));
                        VerifierOutcome::AutoTestRan {
                            score,
                            auto_test_kind: AutoTestKindView::from(plan.auto_test_kind()),
                            auto_test_passed: result.passed,
                            auto_test_command: result.command.clone(),
                            auto_test_output: result.output.clone(),
                            auto_test_reason: plan.reason.clone(),
                            anvil_test_summary: summary,
                            feedback,
                            auto_test_combined_output: combined_output,
                        }
                    }
                    Err(error) => {
                        let score = compute_anvil_score(
                            &inputs.score_inputs,
                            inputs.repo_verification,
                            None,
                        );
                        VerifierOutcome::AutoTestTransportError { score, error }
                    }
                }
            }
            OwnedTestVerifierPlan::Weak {
                detected_source, ..
            } => {
                let score =
                    compute_anvil_score(&inputs.score_inputs, inputs.repo_verification, None);
                VerifierOutcome::Weak {
                    score,
                    owned_test_artifacts_count: inputs.owned_test_artifacts.len(),
                    command_runner: detected_source,
                }
            }
            OwnedTestVerifierPlan::Missing => {
                let score =
                    compute_anvil_score(&inputs.score_inputs, inputs.repo_verification, None);
                VerifierOutcome::Missing {
                    score,
                    owned_test_artifacts_count: inputs.owned_test_artifacts.len(),
                }
            }
        }
    }
}

/// CB-012 (Codex iteration-5 medium): observation carried to the external-
/// import callback by the post-loop structured verifier path. Each variant
/// matches one of the `agent.verifier.external_import_rejected` emit
/// reasons used by `turn.rs::run_task_contract_verifier_once`. The
/// callback contract is "raw paths never leak past the SSOT
/// `mask_secrets` + `stable_path_hash` boundary"; the variants therefore
/// already carry pre-hashed `path_hash` strings rather than raw paths.
#[derive(Debug, Clone)]
#[allow(dead_code)] // wired by `execute_with_full_observers` callers
pub(super) enum ExternalImportObservation {
    /// Pre-execution PYTHONPATH check found at least one work_root-external
    /// component. `path_hash` is the SSOT-hashed raw substring.
    PythonpathRejected { path_hash: String },
    /// Post-execution stdout/stderr scan found one-or-more external
    /// workspace imports. `path_hashes` are SSOT-hashed (mask_secrets +
    /// stable_path_hash) raw substrings, capped at
    /// `EXTERNAL_IMPORT_DETECTED_CAP`. `total_count` and `truncated`
    /// reflect the pre-cap full count (CB-009 contract).
    StdoutStderrDetected {
        path_hashes: Vec<String>,
        total_count: usize,
        truncated: bool,
    },
}

impl VerifierSkill {
    /// CB-012 (Codex iteration-5 medium): observer-aware variant of
    /// `execute_with_invocation_observer` that ALSO fires an external-
    /// import callback for the same two signals `turn.rs` emits inline
    /// (`external_pythonpath_rejected` + `external_import_detected`).
    /// `execute_with_invocation_observer` continues to exist with its
    /// existing signature for backward compatibility, but it delegates
    /// here with a no-op external-import observer.
    ///
    /// Order of operations (mirrors `execute_with_invocation_observer`):
    ///   1. Bypass gates (test_execution_required / dispatch / disabled).
    ///   2. Detect `OwnedTestVerifierPlan`.
    ///   3. For Runnable: build env_plan, fire pre-spawn callback, fire
    ///      external-import callback for any PYTHONPATH rejection, spawn
    ///      `run_structured`, then fire external-import callback for any
    ///      stdout/stderr detection. The callback owns the
    ///      `Agent::emit_agent_verifier_external_import_rejected_if_first`
    ///      per-turn cap on the receiving side.
    #[allow(dead_code)] // wired by post-loop callers that need external-import emit parity
    pub(super) fn execute_with_full_observers(
        &mut self,
        inputs: VerifierInputs<'_>,
        on_pre_spawn: &mut dyn FnMut(&super::auto_test::VerifierInvokedSnapshot),
        on_external_import: &mut dyn FnMut(&str, &ExternalImportObservation),
    ) -> VerifierOutcome {
        debug_assert!(
            inputs.test_execution_required && inputs.protocol_demands_verifier,
            "execute_with_full_observers must only be entered on the structured path"
        );
        let bypass_score =
            || compute_anvil_score(&inputs.score_inputs, inputs.repo_verification, None);
        if !inputs.test_execution_required || !inputs.protocol_demands_verifier {
            return VerifierOutcome::Skipped {
                score: bypass_score(),
            };
        }
        if !inputs.should_dispatch_success_verifier {
            return VerifierOutcome::Skipped {
                score: bypass_score(),
            };
        }
        if auto_test_disabled(|k| std::env::var(k)) {
            return VerifierOutcome::Skipped {
                score: bypass_score(),
            };
        }
        let owned_plan = if let Some(project_unit) = inputs.project_unit {
            AutoTestRunner::detect_with_owned_test_artifacts_and_project_unit(
                inputs.workspace_root,
                inputs.changed_files,
                inputs.recent_successful_bash_commands,
                inputs.owned_test_artifacts,
                Some(project_unit),
            )
        } else {
            OwnedTestVerifierPlan::Missing
        };
        match owned_plan {
            OwnedTestVerifierPlan::Runnable { plan, command } => {
                let display_command = command.to_display_string();
                let extras_for_emit: &[(&'static str, &'static str)] = match command.runner() {
                    "python3" => super::auto_test::VERIFIER_ENV_PYTHON_EXTRA,
                    _ => &[],
                };
                let env_plan = super::auto_test::build_hermetic_env_plan(
                    inputs.workspace_root,
                    extras_for_emit,
                );
                if let Some(snapshot) =
                    super::auto_test::VerifierInvokedSnapshot::from_command_and_env(
                        &command, &env_plan,
                    )
                {
                    on_pre_spawn(&snapshot);
                }
                // CB-012 step (3a): pre-execution PYTHONPATH rejection.
                if let Some(hash) = env_plan.rejected_pythonpath_hash() {
                    on_external_import(
                        command.runner(),
                        &ExternalImportObservation::PythonpathRejected { path_hash: hash },
                    );
                }
                match AutoTestRunner::run_structured(
                    inputs.workspace_root,
                    inputs.workspace_scope,
                    &command,
                    &display_command,
                ) {
                    Ok(result) => {
                        // CB-012 step (3b): post-execution stdout/stderr scan.
                        let detected = super::auto_test::detect_external_imports_in_output(
                            inputs.workspace_root,
                            &result.stdout,
                            &result.stderr,
                        );
                        if !detected.entries.is_empty() {
                            let path_hashes: Vec<String> = detected
                                .entries
                                .iter()
                                .map(|raw| {
                                    crate::logging::stable_path_hash(
                                        &crate::session::feedback::mask_secrets(raw),
                                    )
                                })
                                .collect();
                            on_external_import(
                                command.runner(),
                                &ExternalImportObservation::StdoutStderrDetected {
                                    path_hashes,
                                    total_count: detected.total_count,
                                    truncated: detected.truncated,
                                },
                            );
                        }
                        let summary = build_anvil_test_summary_for_skill(&plan, &result);
                        let score = compute_anvil_score(
                            &inputs.score_inputs,
                            inputs.repo_verification,
                            Some(&summary),
                        );
                        let combined_output = combined_output_for_classify(&result);
                        let feedback = Some(super::turn::build_feedback_for_auto_test(
                            &plan,
                            &result,
                            inputs.workspace_root,
                            inputs.changed_files,
                        ));
                        VerifierOutcome::AutoTestRan {
                            score,
                            auto_test_kind: AutoTestKindView::from(plan.auto_test_kind()),
                            auto_test_passed: result.passed,
                            auto_test_command: result.command.clone(),
                            auto_test_output: result.output.clone(),
                            auto_test_reason: plan.reason.clone(),
                            anvil_test_summary: summary,
                            feedback,
                            auto_test_combined_output: combined_output,
                        }
                    }
                    Err(error) => {
                        let score = compute_anvil_score(
                            &inputs.score_inputs,
                            inputs.repo_verification,
                            None,
                        );
                        VerifierOutcome::AutoTestTransportError { score, error }
                    }
                }
            }
            OwnedTestVerifierPlan::Weak {
                detected_source, ..
            } => {
                let score =
                    compute_anvil_score(&inputs.score_inputs, inputs.repo_verification, None);
                VerifierOutcome::Weak {
                    score,
                    owned_test_artifacts_count: inputs.owned_test_artifacts.len(),
                    command_runner: detected_source,
                }
            }
            OwnedTestVerifierPlan::Missing => {
                let score =
                    compute_anvil_score(&inputs.score_inputs, inputs.repo_verification, None);
                VerifierOutcome::Missing {
                    score,
                    owned_test_artifacts_count: inputs.owned_test_artifacts.len(),
                }
            }
        }
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
