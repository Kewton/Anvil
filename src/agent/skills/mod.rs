//! Issue #465: AgentSkill trait + SkillRegistry.
//!
//! Reminder / Tester / CaseRecord / CaseRetrieval などの内部補助処理を
//! `AgentSkill` trait + `SkillRegistry` で抽象化する基盤。
//!
//! 本 Issue では trait/registry 基盤と Reminder skill の trait 実装のみを
//! 対象とする。Tester / CaseRecord / CaseRetrieval / AntiPattern の trait
//! 移行は Epic D 配下の別 Issue で順次実施する (Out of Scope)。
//!
//! 設計方針書 §12 を正本として実装する:
//! - SkillExecuteError は project-local error type (anyhow に依存しない)
//! - Reminder の 6 段 priority は ReminderSkill::pre_check 内で既存
//!   `ReminderGate.skip_reason()` を呼ぶ (4 regression test 互換)
//! - render_log_payload は skill 側 SSOT (Reminder は既存 build_log_payload
//!   を呼ぶことで 12 key 完全互換)
//! - SkillRegistry は single-threaded actor loop に閉じる (Send/Sync 不要)
//! - catch_unwind は使わない (&mut WorkingMemory は UnwindSafe でない)

use std::ffi::OsString;
use std::path::Path;

use crate::agent::loop_run::reminder::{ReminderInputs, ReminderOutcome, SkipReason};
use crate::session::anvil_score::AnvilScore;
use crate::session::store::{SessionSnapshot, WorkingMemory};

pub mod reminder_skill;

/// 起動 fence の種別。Reminder は IterationInternal + PostLoop の両方に登録される。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillTrigger {
    IterationInternal,
    MessageBuild,
    PostLoop,
}

/// Skill 実行時に渡す runtime state (read-only).
///
/// Reminder 固有の field は `Agent::maybe_invoke_reminder` (turn.rs facade) で
/// 組み立てて渡す。
#[allow(dead_code)]
pub struct RuntimeState<'a> {
    pub plan_mode: bool,
    pub interrupted: bool,
    pub turn_index: usize,
    pub session: &'a SessionSnapshot,
    pub last_anvil_score: Option<&'a AnvilScore>,
    /// Reminder 固有 (turn.rs facade で組み立てる)
    pub reminder_sidecar_available: bool,
    pub reminder_kind_eligible: bool,
    pub reminder_called_this_turn: bool,
}

/// Skill 実行時の mutable sink (WorkingMemory への書き込み等).
#[allow(dead_code)]
pub struct SkillExecutionContext<'a> {
    pub working_memory: &'a mut WorkingMemory,
    pub workspace_root: &'a Path,
}

/// Skill 入力 (skill 種別ごとに variant).
///
/// 本 Issue では Reminder のみ。Tester / CaseRecord / CaseRetrieval は別 Issue
/// で variant 追加 (Out of Scope)。
pub enum SkillInput<'a> {
    Reminder(ReminderInputs<'a>),
    /// Test fixture / NoOp 用
    NoOp,
}

/// Skill 出力 (skill 種別ごとに variant).
///
/// Reminder の `Failed` / `Skipped` / `Completed` は ReminderOutcome 内 variant
/// として表現 (recoverable failure は Result::Err ではなく outcome variant)。
#[derive(Debug, Clone)]
pub enum SkillOutput {
    Reminder(ReminderOutcome),
    NoOp,
}

/// Project-local error type (anyhow 不使用).
///
/// `execute` の Err は invariant violation のみ (input mismatch など)。
/// recoverable failure は SkillOutput variant 内で表現する。
#[derive(Debug)]
pub enum SkillExecuteError {
    InputMismatch(&'static str),
    Other(String),
}

impl std::fmt::Display for SkillExecuteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SkillExecuteError::InputMismatch(s) => write!(f, "input mismatch: expected {s}"),
            SkillExecuteError::Other(s) => write!(f, "{s}"),
        }
    }
}

impl std::error::Error for SkillExecuteError {}

/// Issue #465: skill 共通 interface.
///
/// 設計方針書 §12-1 に対応。各メソッドの責務:
/// - `name`: logging key の一部 (lower-snake-case allowlist)
/// - `triggers`: 複数 fence への登録を許す (Reminder は 2 fence)
/// - `pre_check`: skill 固有の優先度判定 (Reminder の 6 段 priority など)
/// - `applicability`: pre_check が None の場合の最終判定
/// - `execute`: 本体実行 (recoverable failure は SkillOutput variant で表現)
/// - `render_log_payload`: skill 側で log payload を SSOT として組み立てる
pub trait AgentSkill {
    fn name(&self) -> &'static str;
    fn triggers(&self) -> &'static [SkillTrigger];

    fn pre_check(
        &self,
        _state: &RuntimeState,
        _get_env: &dyn Fn(&str) -> Option<OsString>,
    ) -> Option<SkipReason> {
        None
    }

    fn applicability(&self, _state: &RuntimeState) -> bool {
        true
    }

    fn execute(
        &mut self,
        input: SkillInput<'_>,
        ctx: &mut SkillExecutionContext<'_>,
    ) -> Result<SkillOutput, SkillExecuteError>;

    fn render_log_payload(
        &self,
        outcome: &SkillOutput,
        session_id: &str,
        model: Option<&str>,
    ) -> (&'static str, serde_json::Value);
}

/// Skill 実行リクエスト. invoke 引数を struct に集約する (clippy too_many_arguments 回避).
pub struct SkillInvocationRequest<'a> {
    pub skill_name: &'static str,
    pub trigger: SkillTrigger,
    pub state: &'a RuntimeState<'a>,
    pub input: SkillInput<'a>,
    pub ctx: SkillExecutionContext<'a>,
    pub get_env: &'a dyn Fn(&str) -> Option<OsString>,
    pub emit_event: &'a mut dyn FnMut(&'static str, serde_json::Value),
    pub session_id: &'a str,
    pub model: Option<&'a str>,
}

/// Skill 実行結果. `attempted_execute=true` のとき turn.rs facade が
/// per-turn cap を立てる (Skipped/Disabled は false → cap 不消費).
#[derive(Debug)]
pub struct SkillInvocationResult {
    pub attempted_execute: bool,
    pub output: Option<SkillOutput>,
    pub skipped_reason: Option<SkipReason>,
}

impl SkillInvocationResult {
    pub fn completed(output: SkillOutput) -> Self {
        Self {
            attempted_execute: true,
            output: Some(output),
            skipped_reason: None,
        }
    }
    pub fn failed() -> Self {
        Self {
            attempted_execute: true,
            output: None,
            skipped_reason: None,
        }
    }
    pub fn skipped(reason: SkipReason) -> Self {
        Self {
            attempted_execute: false,
            output: None,
            skipped_reason: Some(reason),
        }
    }
    pub fn skipped_silent() -> Self {
        Self {
            attempted_execute: false,
            output: None,
            skipped_reason: None,
        }
    }
    pub fn not_found() -> Self {
        Self {
            attempted_execute: false,
            output: None,
            skipped_reason: None,
        }
    }
}

/// Skill 名 allowlist: `^[a-z][a-z0-9_]{2,32}$` (3〜33 文字、lower-snake-case).
fn is_valid_skill_name(name: &str) -> bool {
    if name.len() < 3 || name.len() > 33 {
        return false;
    }
    let mut chars = name.chars();
    let first = match chars.next() {
        Some(c) => c,
        None => return false,
    };
    if !first.is_ascii_lowercase() {
        return false;
    }
    chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// Skill registry. single-threaded actor loop に閉じる (Send/Sync 不要).
pub struct SkillRegistry {
    skills: Vec<Box<dyn AgentSkill>>,
}

impl Default for SkillRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl SkillRegistry {
    pub fn new() -> Self {
        Self { skills: Vec::new() }
    }

    /// Skill を登録する. debug build では name allowlist + 重複検出を assert.
    pub fn register<S: AgentSkill + 'static>(&mut self, skill: S) {
        debug_assert!(
            is_valid_skill_name(skill.name()),
            "skill name must match ^[a-z][a-z0-9_]{{2,32}}$ : {:?}",
            skill.name()
        );
        debug_assert!(
            !self.skills.iter().any(|s| s.name() == skill.name()),
            "duplicate skill name: {:?}",
            skill.name()
        );
        self.skills.push(Box::new(skill));
    }

    /// 1 skill 1 invoke. fence は呼び出し側 (turn.rs facade) が指定.
    ///
    /// 判定順:
    /// 1. lookup (skill_name + trigger 一致)
    /// 2. pre_check (skill 固有の優先度判定 / Reminder の 6 段 priority)
    /// 3. applicability (silent skip)
    /// 4. execute → render_log_payload → emit_event
    ///
    /// catch_unwind は使わない (D4-001: &mut WorkingMemory は UnwindSafe でない).
    pub fn invoke(&mut self, request: SkillInvocationRequest<'_>) -> SkillInvocationResult {
        let SkillInvocationRequest {
            skill_name,
            trigger,
            state,
            input,
            mut ctx,
            get_env,
            emit_event,
            session_id,
            model,
        } = request;
        let skill = match self
            .skills
            .iter_mut()
            .find(|s| s.name() == skill_name && s.triggers().contains(&trigger))
        {
            Some(s) => s,
            None => return SkillInvocationResult::not_found(),
        };
        if let Some(reason) = skill.pre_check(state, get_env) {
            let event_key = skipped_event_key(skill.name());
            let payload = serde_json::json!({
                "session_id": session_id,
                "model": model,
                "reason": reason.as_str(),
            });
            emit_event(event_key, payload);
            return SkillInvocationResult::skipped(reason);
        }
        if !skill.applicability(state) {
            return SkillInvocationResult::skipped_silent();
        }
        match skill.execute(input, &mut ctx) {
            Ok(out) => {
                let (event_key, payload) = skill.render_log_payload(&out, session_id, model);
                emit_event(event_key, payload);
                SkillInvocationResult::completed(out)
            }
            Err(e) => {
                let event_key = failed_event_key(skill.name());
                let payload = serde_json::json!({
                    "session_id": session_id,
                    "model": model,
                    "error": e.to_string(),
                });
                emit_event(event_key, payload);
                SkillInvocationResult::failed()
            }
        }
    }
}

/// `agent.<skill_name>.skipped` event key (`<skill_name>` ごとに `&'static str`).
fn skipped_event_key(skill_name: &str) -> &'static str {
    match skill_name {
        "reminder" => "agent.reminder.skipped",
        _ => "agent.skill.skipped",
    }
}

/// `agent.<skill_name>.failed` event key.
fn failed_event_key(skill_name: &str) -> &'static str {
    match skill_name {
        "reminder" => "agent.reminder.failed",
        _ => "agent.skill.failed",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skill_name_allowlist_accepts_lower_snake_case() {
        assert!(is_valid_skill_name("reminder"));
        assert!(is_valid_skill_name("case_record"));
        assert!(is_valid_skill_name("a1b"));
        assert!(is_valid_skill_name("anti_pattern_2"));
    }

    #[test]
    fn skill_name_allowlist_rejects_invalid() {
        assert!(!is_valid_skill_name(""));
        assert!(!is_valid_skill_name("ab"));
        assert!(!is_valid_skill_name("Reminder")); // Upper case
        assert!(!is_valid_skill_name("reminder!"));
        assert!(!is_valid_skill_name("9start"));
        assert!(!is_valid_skill_name("with-dash"));
        assert!(!is_valid_skill_name(&"a".repeat(34))); // too long
    }

    #[test]
    fn registry_default_is_empty() {
        let r = SkillRegistry::default();
        assert!(r.skills.is_empty());
    }

    #[test]
    fn invocation_result_factories() {
        let s = SkillInvocationResult::skipped(SkipReason::PerTurnCapped);
        assert!(!s.attempted_execute);
        assert_eq!(s.skipped_reason, Some(SkipReason::PerTurnCapped));

        let f = SkillInvocationResult::failed();
        assert!(f.attempted_execute);
        assert!(f.output.is_none());

        let n = SkillInvocationResult::not_found();
        assert!(!n.attempted_execute);
    }
}
