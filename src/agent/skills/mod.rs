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
use crate::agent::loop_run::verifier_skill::{VerifierInputs, VerifierOutcome};
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

/// Issue #467: skill 単位の宣言的 permission tier.
///
/// `AgentSkill::tier()` の必須戻り値として各 skill が宣言する。
/// `SkillRegistry::invoke()` は `applicability()` 直後・`run()` 直前で
/// `evaluate_trust_tier()` を呼び、`ExternalDisabled` を無条件 deny、
/// Plan mode + 非 `BuiltInReadOnly` を deny する (VerifierSkill は
/// allowlist で bypass、DR-466-001 互換)。
///
/// `#[non_exhaustive]` は将来 variant 追加時の `match` 互換性を保つ
/// (DR-466-004)。 `#[serde(other)]` は付けず、未知 variant は parse error と
/// する (DR4-003: trusted in-process registration 経由のため fallback 不要)。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum SkillTrustTier {
    /// Read-only (Read のみ、Write/Edit/Bash 不可).
    BuiltInReadOnly,
    /// `tmp-tests/` 配下への Write のみ許可.
    BuiltInCanWriteTemp,
    /// `tmp-tests/` への Write + 限定 Bash (固定テンプレート) 要求可.
    BuiltInCanRequestBash,
    /// 外部 skill。invoke 入口で early return + permission_denied event.
    ExternalDisabled,
}

/// Issue #467: tier_check 違反詳細.
///
/// `SkillOutput::PermissionDenied` の payload としても使われるが、registry-internal
/// の構造的保証 (DR1-002 1 次防御) により turn.rs facade には届かない設計。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillPermissionDenial {
    pub skill_name: String,
    /// "read_only" | "write_repo" | "bash" | "external"
    pub requested_capability: String,
    /// "plan_mode_violation" | "tier_mismatch" | "external_disabled"
    pub reason: String,
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
/// 本 Issue (#466) で `Verifier` variant を追加。Tester / CaseRecord / CaseRetrieval
/// は別 Issue で variant 追加 (Out of Scope)。
///
/// `#[non_exhaustive]` は将来 variant 追加時の `match` 互換性を保つため (DR-466-004)。
#[non_exhaustive]
pub enum SkillInput<'a> {
    Reminder(ReminderInputs<'a>),
    Verifier(VerifierInputs<'a>),
    /// Test fixture / NoOp 用
    NoOp,
}

/// Skill 出力 (skill 種別ごとに variant).
///
/// Reminder の `Failed` / `Skipped` / `Completed` は ReminderOutcome 内 variant
/// として表現 (recoverable failure は Result::Err ではなく outcome variant)。
///
/// `#[non_exhaustive]` は将来 variant 追加時の `match` 互換性を保つため (DR-466-004)。
/// `Verifier` は payload が大きい (AutoTestRan 内 FeedbackFrame / String 多数) ため
/// `Box` で indirection し、`large_enum_variant` clippy lint を満たす。
///
/// Issue #467: `PermissionDenied` は `SkillRegistry::invoke` 内の tier_check で生成され、
/// turn.rs facade には届かない設計 (DR1-002 二重防御の 1 次防御 = registry-internal、
/// 2 次防御 = facade の `_ => debug_assert!(false, ...)` arm)。
#[non_exhaustive]
#[derive(Debug, Clone)]
pub enum SkillOutput {
    Reminder(ReminderOutcome),
    Verifier(Box<VerifierOutcome>),
    NoOp,
    PermissionDenied(SkillPermissionDenial),
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

    /// Issue #467: skill 単位の宣言的 permission tier (default なし、必須).
    ///
    /// 設計判断 #1: default 実装を持たせない (沈黙の権限付与 / 沈黙の権限剥奪を防ぐ)。
    /// 未実装はコンパイルエラーで検知される。
    fn tier(&self) -> SkillTrustTier;
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
    /// Issue #467: tier_check 違反時の factory.
    ///
    /// `attempted_execute = false` (per-turn cap 不消費) として扱い、
    /// `SkillOutput::PermissionDenied` を output に格納する。
    pub fn permission_denied(denial: SkillPermissionDenial) -> Self {
        Self {
            attempted_execute: false,
            output: Some(SkillOutput::PermissionDenied(denial)),
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
    /// 4. **tier_check (Issue #467 / DR1-002 1 次防御)**: deny 時は run() 不実行で
    ///    `SkillOutput::PermissionDenied` を返し、`agent.skill.permission_denied`
    ///    event を emit (per-turn cap 不消費)
    /// 5. execute → render_log_payload → emit_event
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
        // Issue #467: tier_check (applicability 直後・execute 直前)
        if let Some(denial) = evaluate_trust_tier(skill.as_ref(), state) {
            let (event_key, payload) =
                build_permission_denied_payload(session_id, model, &denial, skill.tier());
            emit_event(event_key, payload);
            return SkillInvocationResult::permission_denied(denial);
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

// ---------------------------------------------------------------------------
// Issue #467: trust tier check (registry-internal, private helpers)
// ---------------------------------------------------------------------------

/// 純関数: tier_check の deny 条件のみを判定する。副作用なし。
///
/// 判定:
/// 1. `ExternalDisabled` は無条件 deny
/// 2. Plan mode + 非 `BuiltInReadOnly` → deny (`skill_bypasses_plan_mode` allowlist 例外)
fn evaluate_trust_tier(
    skill: &dyn AgentSkill,
    state: &RuntimeState,
) -> Option<SkillPermissionDenial> {
    let tier = skill.tier();

    if tier == SkillTrustTier::ExternalDisabled {
        return Some(SkillPermissionDenial {
            skill_name: skill.name().to_string(),
            requested_capability: tier_capability_label(tier),
            reason: "external_disabled".to_string(),
        });
    }

    if state.plan_mode
        && tier != SkillTrustTier::BuiltInReadOnly
        && !skill_bypasses_plan_mode(skill.name())
    {
        return Some(SkillPermissionDenial {
            skill_name: skill.name().to_string(),
            requested_capability: tier_capability_label(tier),
            reason: "plan_mode_violation".to_string(),
        });
    }

    None
}

/// 純関数: tier → capability label 写像 (DR1-003 SSOT).
fn tier_capability_label(tier: SkillTrustTier) -> String {
    match tier {
        SkillTrustTier::BuiltInReadOnly => "read_only".to_string(),
        SkillTrustTier::BuiltInCanWriteTemp => "write_repo".to_string(),
        SkillTrustTier::BuiltInCanRequestBash => "bash".to_string(),
        SkillTrustTier::ExternalDisabled => "external".to_string(),
    }
}

/// 純関数: event payload を組み立てる (event_key literal を 1 箇所に閉じ込め、DR1-004 SSOT).
///
/// payload key 6 値 (`session_id` / `model` / `skill_name` / `tier` /
/// `requested_capability` / `reason`) は `is_secret_like_key=false` を満たすよう
/// 命名されている (DR1-005 / DR2-006 / DR4-003)。
fn build_permission_denied_payload(
    session_id: &str,
    model: Option<&str>,
    denial: &SkillPermissionDenial,
    tier: SkillTrustTier,
) -> (&'static str, serde_json::Value) {
    let event_key = "agent.skill.permission_denied";
    let payload = serde_json::json!({
        "session_id": session_id,
        "model": model,
        "skill_name": denial.skill_name,
        "tier": tier,
        "requested_capability": denial.requested_capability,
        "reason": denial.reason,
    });
    (event_key, payload)
}

/// 純関数: VerifierSkill のみ Plan mode の tier_check を bypass (DR-466-001 互換).
///
/// 設計判断 #4: 1 件追加で済む間は allowlist、3 件目で trait method に昇格を再評価。
fn skill_bypasses_plan_mode(skill_name: &str) -> bool {
    matches!(skill_name, "verifier")
}

/// `agent.<skill_name>.skipped` event key (`<skill_name>` ごとに `&'static str`).
fn skipped_event_key(skill_name: &str) -> &'static str {
    match skill_name {
        "reminder" => "agent.reminder.skipped",
        "verifier" => "agent.verifier.skipped",
        _ => "agent.skill.skipped",
    }
}

/// `agent.<skill_name>.failed` event key.
fn failed_event_key(skill_name: &str) -> &'static str {
    match skill_name {
        "reminder" => "agent.reminder.failed",
        "verifier" => "agent.verifier.failed",
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

    // ---------------------------------------------------------------------
    // Issue #467: SkillTrustTier / tier_check / payload tests
    // ---------------------------------------------------------------------

    use crate::session::store::{SessionSnapshot, WorkingMemory};
    use std::path::PathBuf;

    fn make_state_for_tier_test(plan_mode: bool, snapshot: &SessionSnapshot) -> RuntimeState<'_> {
        RuntimeState {
            plan_mode,
            interrupted: false,
            turn_index: 0,
            session: snapshot,
            last_anvil_score: None,
            reminder_sidecar_available: true,
            reminder_kind_eligible: true,
            reminder_called_this_turn: false,
        }
    }

    /// Tier 値を直接指定して tier_check 判定だけを検証するテスト用 skill.
    struct TierTestSkill {
        name_static: &'static str,
        tier_value: SkillTrustTier,
    }
    impl AgentSkill for TierTestSkill {
        fn name(&self) -> &'static str {
            self.name_static
        }
        fn triggers(&self) -> &'static [SkillTrigger] {
            &[SkillTrigger::PostLoop]
        }
        fn execute(
            &mut self,
            _input: SkillInput<'_>,
            _ctx: &mut SkillExecutionContext<'_>,
        ) -> Result<SkillOutput, SkillExecuteError> {
            Ok(SkillOutput::NoOp)
        }
        fn render_log_payload(
            &self,
            _outcome: &SkillOutput,
            _session_id: &str,
            _model: Option<&str>,
        ) -> (&'static str, serde_json::Value) {
            ("agent.tier_test.completed", serde_json::Value::Null)
        }
        fn tier(&self) -> SkillTrustTier {
            self.tier_value
        }
    }

    // -- P1 enum / serde tests ---------------------------------------------

    #[test]
    fn tier_enum_serde_round_trip() {
        let tiers = [
            SkillTrustTier::BuiltInReadOnly,
            SkillTrustTier::BuiltInCanWriteTemp,
            SkillTrustTier::BuiltInCanRequestBash,
            SkillTrustTier::ExternalDisabled,
        ];
        for t in tiers {
            let json = serde_json::to_string(&t).unwrap();
            let decoded: SkillTrustTier = serde_json::from_str(&json).unwrap();
            assert_eq!(decoded, t);
        }
    }

    #[test]
    fn tier_serializes_as_snake_case_string() {
        assert_eq!(
            serde_json::to_value(SkillTrustTier::BuiltInReadOnly).unwrap(),
            serde_json::json!("built_in_read_only")
        );
        assert_eq!(
            serde_json::to_value(SkillTrustTier::BuiltInCanWriteTemp).unwrap(),
            serde_json::json!("built_in_can_write_temp")
        );
        assert_eq!(
            serde_json::to_value(SkillTrustTier::BuiltInCanRequestBash).unwrap(),
            serde_json::json!("built_in_can_request_bash")
        );
        assert_eq!(
            serde_json::to_value(SkillTrustTier::ExternalDisabled).unwrap(),
            serde_json::json!("external_disabled")
        );
    }

    /// DR4-003: 未知 variant は parse error を Err で上げる (`#[serde(other)]` を付けない設計).
    #[test]
    fn tier_serde_unknown_variant_returns_err() {
        let result: Result<SkillTrustTier, _> = serde_json::from_str("\"future_tier\"");
        assert!(
            result.is_err(),
            "expected Err for unknown variant, got {result:?}"
        );
    }

    // -- P3 evaluate_trust_tier / tier_capability_label / payload tests ----

    #[test]
    fn tier_capability_label_matrix() {
        assert_eq!(
            tier_capability_label(SkillTrustTier::BuiltInReadOnly),
            "read_only"
        );
        assert_eq!(
            tier_capability_label(SkillTrustTier::BuiltInCanWriteTemp),
            "write_repo"
        );
        assert_eq!(
            tier_capability_label(SkillTrustTier::BuiltInCanRequestBash),
            "bash"
        );
        assert_eq!(
            tier_capability_label(SkillTrustTier::ExternalDisabled),
            "external"
        );
    }

    #[test]
    fn tier_evaluate_external_disabled_denies() {
        let snapshot = SessionSnapshot::default();
        let state = make_state_for_tier_test(false, &snapshot);
        let skill = TierTestSkill {
            name_static: "ext_skill",
            tier_value: SkillTrustTier::ExternalDisabled,
        };
        let denial = evaluate_trust_tier(&skill, &state).expect("expected deny");
        assert_eq!(denial.skill_name, "ext_skill");
        assert_eq!(denial.requested_capability, "external");
        assert_eq!(denial.reason, "external_disabled");
    }

    #[test]
    fn tier_evaluate_plan_mode_built_in_read_only_allows() {
        let snapshot = SessionSnapshot::default();
        let state = make_state_for_tier_test(true, &snapshot);
        let skill = TierTestSkill {
            name_static: "ro_skill",
            tier_value: SkillTrustTier::BuiltInReadOnly,
        };
        assert!(evaluate_trust_tier(&skill, &state).is_none());
    }

    #[test]
    fn tier_evaluate_plan_mode_can_write_temp_denies() {
        let snapshot = SessionSnapshot::default();
        let state = make_state_for_tier_test(true, &snapshot);
        let skill = TierTestSkill {
            name_static: "wt_skill",
            tier_value: SkillTrustTier::BuiltInCanWriteTemp,
        };
        let denial = evaluate_trust_tier(&skill, &state).expect("expected deny");
        assert_eq!(denial.requested_capability, "write_repo");
        assert_eq!(denial.reason, "plan_mode_violation");
    }

    /// VerifierSkill は Plan mode で tier_check を bypass (DR-466-001 互換).
    #[test]
    fn tier_evaluate_plan_mode_verifier_bypasses() {
        let snapshot = SessionSnapshot::default();
        let state = make_state_for_tier_test(true, &snapshot);
        let skill = TierTestSkill {
            name_static: "verifier",
            tier_value: SkillTrustTier::BuiltInCanRequestBash,
        };
        assert!(
            evaluate_trust_tier(&skill, &state).is_none(),
            "verifier should bypass Plan mode tier_check"
        );
    }

    #[test]
    fn tier_evaluate_act_mode_all_tiers_allow() {
        let snapshot = SessionSnapshot::default();
        let state = make_state_for_tier_test(false, &snapshot);
        for tier in [
            SkillTrustTier::BuiltInReadOnly,
            SkillTrustTier::BuiltInCanWriteTemp,
            SkillTrustTier::BuiltInCanRequestBash,
        ] {
            let skill = TierTestSkill {
                name_static: "act_skill",
                tier_value: tier,
            };
            assert!(
                evaluate_trust_tier(&skill, &state).is_none(),
                "act mode + {tier:?} should allow"
            );
        }
    }

    /// DR1-004 SSOT: payload は 6 key のみ含む (是正テスト).
    #[test]
    fn permission_denied_payload_key_allowlist() {
        let denial = SkillPermissionDenial {
            skill_name: "verifier".to_string(),
            requested_capability: "bash".to_string(),
            reason: "plan_mode_violation".to_string(),
        };
        let (event_key, payload) = build_permission_denied_payload(
            "session-x",
            Some("model-y"),
            &denial,
            SkillTrustTier::BuiltInCanRequestBash,
        );
        assert_eq!(event_key, "agent.skill.permission_denied");
        let obj = payload.as_object().expect("payload must be object");
        let mut keys: Vec<&String> = obj.keys().collect();
        keys.sort();
        let expected = vec![
            "model".to_string(),
            "reason".to_string(),
            "requested_capability".to_string(),
            "session_id".to_string(),
            "skill_name".to_string(),
            "tier".to_string(),
        ];
        let actual: Vec<String> = keys.into_iter().cloned().collect();
        assert_eq!(actual, expected);
        // tier は snake_case 文字列 (DR1-005 / 永続化方針)
        assert_eq!(obj["tier"], serde_json::json!("built_in_can_request_bash"));
    }

    /// DR1-005 / DR2-006: payload 6 key 全件で is_secret_like_key=false.
    #[test]
    fn permission_denied_event_payload_no_secret_like_keys() {
        let denial = SkillPermissionDenial {
            skill_name: "reminder".to_string(),
            requested_capability: "external".to_string(),
            reason: "external_disabled".to_string(),
        };
        let (_event_key, payload) =
            build_permission_denied_payload("s", None, &denial, SkillTrustTier::ExternalDisabled);
        let obj = payload.as_object().expect("object");
        for key in obj.keys() {
            assert!(
                !crate::logging::is_secret_like_key(key),
                "payload key {key:?} must not be secret-like"
            );
        }
    }

    /// DR3-007: tier_check は env を読まない (always-on).
    #[test]
    fn tier_check_does_not_consult_env() {
        let snapshot = SessionSnapshot::default();
        // env を「呼ばれたら panic」する closure として渡し、evaluate_trust_tier が env を読まないことを pin
        let state = make_state_for_tier_test(true, &snapshot);
        let skill = TierTestSkill {
            name_static: "tier_env_skill",
            tier_value: SkillTrustTier::BuiltInCanWriteTemp,
        };
        // get_env は evaluate_trust_tier には渡さない (state のみで判定)
        let denial = evaluate_trust_tier(&skill, &state);
        assert!(denial.is_some(), "plan_mode + can_write_temp should deny");
    }

    /// P4: tier_check 違反は per-turn cap を消費しない (attempted_execute=false).
    #[test]
    fn permission_denied_per_turn_cap_unconsumed() {
        let denial = SkillPermissionDenial {
            skill_name: "ext".to_string(),
            requested_capability: "external".to_string(),
            reason: "external_disabled".to_string(),
        };
        let result = SkillInvocationResult::permission_denied(denial);
        assert!(
            !result.attempted_execute,
            "per-turn cap must NOT be consumed"
        );
        assert!(matches!(
            result.output,
            Some(SkillOutput::PermissionDenied(_))
        ));
    }

    /// P4: tier_check が deny した時、run() (execute) は呼ばれない.
    #[test]
    fn permission_denied_does_not_reach_run() {
        use std::cell::Cell;
        struct ExecCountSkill {
            count: Cell<usize>,
        }
        impl AgentSkill for ExecCountSkill {
            fn name(&self) -> &'static str {
                "exec_count"
            }
            fn triggers(&self) -> &'static [SkillTrigger] {
                &[SkillTrigger::PostLoop]
            }
            fn execute(
                &mut self,
                _input: SkillInput<'_>,
                _ctx: &mut SkillExecutionContext<'_>,
            ) -> Result<SkillOutput, SkillExecuteError> {
                self.count.set(self.count.get() + 1);
                Ok(SkillOutput::NoOp)
            }
            fn render_log_payload(
                &self,
                _outcome: &SkillOutput,
                _session_id: &str,
                _model: Option<&str>,
            ) -> (&'static str, serde_json::Value) {
                ("agent.exec_count.completed", serde_json::Value::Null)
            }
            fn tier(&self) -> SkillTrustTier {
                SkillTrustTier::ExternalDisabled
            }
        }
        let mut registry = SkillRegistry::new();
        registry.register(ExecCountSkill {
            count: Cell::new(0),
        });
        let snapshot = SessionSnapshot::default();
        let state = make_state_for_tier_test(false, &snapshot);
        let mut wm = WorkingMemory::default();
        let root = PathBuf::from("/tmp/tier_run_test");
        let ctx = SkillExecutionContext {
            working_memory: &mut wm,
            workspace_root: &root,
        };
        let mut events: Vec<(&'static str, serde_json::Value)> = Vec::new();
        let mut emit_event = |k: &'static str, v: serde_json::Value| events.push((k, v));
        let get_env = |_k: &str| None;
        let result = registry.invoke(SkillInvocationRequest {
            skill_name: "exec_count",
            trigger: SkillTrigger::PostLoop,
            state: &state,
            input: SkillInput::NoOp,
            ctx,
            get_env: &get_env,
            emit_event: &mut emit_event,
            session_id: "s",
            model: None,
        });
        // ExternalDisabled → deny; execute は呼ばれない、event は agent.skill.permission_denied
        assert!(matches!(
            result.output,
            Some(SkillOutput::PermissionDenied(_))
        ));
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].0, "agent.skill.permission_denied");
    }
}
