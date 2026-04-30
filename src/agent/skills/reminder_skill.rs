//! Issue #465: Reminder skill アダプタ.
//!
//! 既存の `crate::agent::loop_run::reminder` 実装を `AgentSkill` trait の
//! 薄いアダプタとしてラップする。中身は変えず、registry 経由での呼び出しを
//! 可能にすることが目的:
//!
//! - `pre_check`: 既存 `ReminderGate.skip_reason()` を呼び 6 段 priority を維持
//! - `execute`: 既存 `run_reminder_with_strategy(...)` をそのまま呼ぶ
//! - `render_log_payload`: 既存 `build_log_payload` を呼ぶ (12 key 完全互換)
//!
//! Reminder の 4 regression test (`reminder.rs:1207-1536`) と ReminderGate
//! 7 unit test (`reminder.rs:631-691`) は **改修なしで green** を維持する。

use std::ffi::OsString;

use crate::agent::loop_run::reminder::{
    ReminderGate, ReminderOutcome, SkipReason, build_log_payload, reminder_disabled,
    run_reminder_with_strategy,
};
use crate::ollama::client::AssistantReply;

use super::{
    AgentSkill, RuntimeState, SkillExecuteError, SkillExecutionContext, SkillInput, SkillOutput,
    SkillTrigger, SkillTrustTier,
};

const REMINDER_TRIGGERS: &[SkillTrigger] =
    &[SkillTrigger::IterationInternal, SkillTrigger::PostLoop];

/// 既存 `run_reminder_with_strategy` の closure シグネチャ互換型。
pub type ReminderLlmCall = Box<dyn FnOnce(&str) -> Result<AssistantReply, String>>;

/// 上記を毎回新規生成する factory 型 (FnMut)。Tester 同様の per-call factory パターン。
pub type ReminderLlmCallFactory = Box<dyn FnMut() -> ReminderLlmCall>;

/// Reminder skill アダプタ.
///
/// `make_llm_call` は per-call factory closure. 既存
/// `run_reminder_with_strategy<F: FnOnce(&str) -> Result<AssistantReply, String>>`
/// シグネチャ互換のため `Box<dyn FnOnce>` を毎回新規生成する形にする。
pub struct ReminderSkill {
    pub make_llm_call: ReminderLlmCallFactory,
}

impl ReminderSkill {
    /// テスト用 / 通常用のコンストラクタ.
    /// `make_llm_call` factory を渡すと、毎回新しい closure を返す.
    pub fn new<M>(make_llm_call: M) -> Self
    where
        M: FnMut() -> ReminderLlmCall + 'static,
    {
        Self {
            make_llm_call: Box::new(make_llm_call),
        }
    }
}

impl AgentSkill for ReminderSkill {
    fn name(&self) -> &'static str {
        "reminder"
    }

    fn triggers(&self) -> &'static [SkillTrigger] {
        REMINDER_TRIGGERS
    }

    fn pre_check(
        &self,
        state: &RuntimeState,
        get_env: &dyn Fn(&str) -> Option<OsString>,
    ) -> Option<SkipReason> {
        // 既存 ReminderGate を組み立てて skip_reason() を呼ぶ.
        // priority: disabled_by_env > sidecar_unavailable > feedback_kind_excluded
        //           > plan_mode > interrupted > per_turn_capped
        let gate = ReminderGate {
            disabled_by_env: reminder_disabled(|k| get_env(k)),
            sidecar_available: state.reminder_sidecar_available,
            kind_eligible: state.reminder_kind_eligible,
            plan_mode: state.plan_mode,
            interrupted: state.interrupted,
            per_turn_already_called: state.reminder_called_this_turn,
        };
        gate.skip_reason()
    }

    fn applicability(&self, _state: &RuntimeState) -> bool {
        // 既存 ReminderGate で skip 判定が完結するため applicability は常に true.
        true
    }

    fn execute(
        &mut self,
        input: SkillInput<'_>,
        ctx: &mut SkillExecutionContext<'_>,
    ) -> Result<SkillOutput, SkillExecuteError> {
        let inputs = match input {
            SkillInput::Reminder(i) => i,
            _ => return Err(SkillExecuteError::InputMismatch("reminder")),
        };
        let llm_call = (self.make_llm_call)();
        // run_reminder_with_strategy は -> ReminderOutcome を直接返す (Result ではない).
        // recoverable failure は ReminderOutcome::Failed variant で表現される.
        let outcome: ReminderOutcome =
            run_reminder_with_strategy(inputs, ctx.working_memory, ctx.workspace_root, llm_call);
        Ok(SkillOutput::Reminder(outcome))
    }

    fn render_log_payload(
        &self,
        outcome: &SkillOutput,
        session_id: &str,
        model: Option<&str>,
    ) -> (&'static str, serde_json::Value) {
        match outcome {
            SkillOutput::Reminder(o) => {
                // Issue #473: skill registry 経路は production の Reminder で
                // 通らない (DR3-002: turn.rs::maybe_invoke_reminder direct call
                // 経路を維持)。turn_index は registry 経由 path が将来稼働する
                // までは 0 fallback、inputs も None で payload schema を well-formed
                // に保つ (新 5 field は null/[] で render される)。
                build_log_payload(o, session_id, model, 0, None)
            }
            _ => ("agent.reminder.failed", serde_json::Value::Null),
        }
    }

    /// Issue #467: ReminderSkill は Read-only (sidecar Ollama 呼び出し +
    /// WorkingMemory 書込のみ、Tool 経由の repo Write はしない).
    ///
    /// **DR3-002 注記**: 本 Issue では ReminderSkill は production 経路で
    /// `SkillRegistry::invoke` を通らない (`Agent::maybe_invoke_reminder` の
    /// direct call 経路、turn.rs:1856)。本値は将来 registry 経由化 Issue で
    /// 実効稼働する宣言値、および設計判断 #1 の沈黙の権限付与防止のための
    /// 型定義上の必須要件として位置付ける。
    fn tier(&self) -> SkillTrustTier {
        SkillTrustTier::BuiltInReadOnly
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::store::WorkingMemory;
    use std::path::PathBuf;

    fn make_skill_with_dummy() -> ReminderSkill {
        ReminderSkill::new(|| {
            Box::new(|_prompt: &str| -> Result<AssistantReply, String> {
                Err("dummy llm".to_string())
            })
        })
    }

    #[test]
    fn skill_name_is_reminder() {
        let s = make_skill_with_dummy();
        assert_eq!(s.name(), "reminder");
    }

    #[test]
    fn skill_triggers_are_iteration_internal_and_post_loop() {
        let s = make_skill_with_dummy();
        let triggers = s.triggers();
        assert_eq!(triggers.len(), 2);
        assert!(triggers.contains(&SkillTrigger::IterationInternal));
        assert!(triggers.contains(&SkillTrigger::PostLoop));
    }

    /// Issue #467: ReminderSkill::tier() == BuiltInReadOnly を pin.
    #[test]
    fn reminder_skill_tier_is_read_only() {
        let s = make_skill_with_dummy();
        assert_eq!(s.tier(), SkillTrustTier::BuiltInReadOnly);
    }

    #[test]
    fn execute_rejects_non_reminder_input() {
        let mut s = make_skill_with_dummy();
        let mut wm = WorkingMemory::default();
        let root = PathBuf::from("/tmp");
        let mut ctx = SkillExecutionContext {
            working_memory: &mut wm,
            workspace_root: &root,
        };
        let result = s.execute(SkillInput::NoOp, &mut ctx);
        assert!(matches!(
            result,
            Err(SkillExecuteError::InputMismatch("reminder"))
        ));
    }
}
