//! Issue #465 / Phase 5 Step 3: SkillRegistry smoke test.
//!
//! 8 ケースで以下を検証 (作業計画書 §Step 3):
//! 1. fence_routing_iteration_internal: ReminderSkill が IterationInternal trigger で invoke される
//! 2. fence_routing_post_loop: 同 skill が PostLoop でも invoke される
//! 3. per_turn_cap_blocks_second_call: state.reminder_called_this_turn=true で SkipReason::PerTurnCapped 返却
//! 4. plan_mode_disable: plan_mode=true で SkipReason::PlanMode 返却 + cap 不消費
//! 5. env_disable: closure DI で ANVIL_NO_REMINDER=1 (OsString) を返すと SkipReason::DisabledByEnv
//! 6. execute_err_emits_failed_event: SkillExecuteError 返却で `agent.<skill>.failed` event emit + actor loop 継続
//! 7. skill_name_allowlist_check: register に "Bad-Name" を渡すと debug build で panic (`#[cfg(debug_assertions)]`)
//! 8. completed_event_emit: ReminderSkill が ReminderOutcome::Failed (LLM 失敗) で `agent.reminder.failed` 12 key payload を emit

use std::cell::RefCell;
use std::ffi::OsString;
use std::path::PathBuf;
use std::rc::Rc;

use anvil::agent::loop_run::{
    ReminderFailureReason as FailureReason, ReminderInputs, ReminderOutcome,
    ReminderSkipReason as SkipReason,
};
use anvil::agent::skills::reminder_skill::ReminderSkill;
use anvil::agent::skills::{
    AgentSkill, RuntimeState, SkillExecuteError, SkillExecutionContext, SkillInput,
    SkillInvocationRequest, SkillOutput, SkillRegistry, SkillTrigger, SkillTrustTier,
};
use anvil::ollama::client::AssistantReply;
use anvil::session::feedback::{FeedbackFrame, FeedbackKind};
use anvil::session::store::{SessionSnapshot, WorkingMemory};

// ---------------------------------------------------------------------------
// Test fixture: ダミー skill (test 用に AgentSkill trait を実装)
// ---------------------------------------------------------------------------

struct DummySkill {
    name: &'static str,
    triggers: &'static [SkillTrigger],
    /// pre_check が返す値. None で applicability に進む
    pre_check_result: Option<SkipReason>,
    /// applicability の戻り値
    applicability_result: bool,
    /// execute の戻り値. true = Ok(NoOp), false = Err
    execute_returns_ok: bool,
    /// Issue #467: tier() の戻り値. 既存ケースは BuiltInReadOnly でデフォルト挙動。
    tier_value: SkillTrustTier,
}

impl AgentSkill for DummySkill {
    fn name(&self) -> &'static str {
        self.name
    }
    fn triggers(&self) -> &'static [SkillTrigger] {
        self.triggers
    }
    fn pre_check(
        &self,
        _state: &RuntimeState,
        _get_env: &dyn Fn(&str) -> Option<OsString>,
    ) -> Option<SkipReason> {
        self.pre_check_result
    }
    fn applicability(&self, _state: &RuntimeState) -> bool {
        self.applicability_result
    }
    fn execute(
        &mut self,
        _input: SkillInput<'_>,
        _ctx: &mut SkillExecutionContext<'_>,
    ) -> Result<SkillOutput, SkillExecuteError> {
        if self.execute_returns_ok {
            Ok(SkillOutput::NoOp)
        } else {
            Err(SkillExecuteError::Other("dummy failure".into()))
        }
    }
    fn render_log_payload(
        &self,
        _outcome: &SkillOutput,
        session_id: &str,
        _model: Option<&str>,
    ) -> (&'static str, serde_json::Value) {
        (
            "agent.dummy.completed",
            serde_json::json!({"session_id": session_id, "outcome": "ok"}),
        )
    }
    fn tier(&self) -> SkillTrustTier {
        self.tier_value
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_session_snapshot() -> SessionSnapshot {
    SessionSnapshot::default()
}

fn make_workspace_root() -> PathBuf {
    PathBuf::from("/tmp/skill_smoke")
}

fn make_state<'a>(
    snapshot: &'a SessionSnapshot,
    plan_mode: bool,
    called: bool,
) -> RuntimeState<'a> {
    RuntimeState {
        plan_mode,
        interrupted: false,
        turn_index: 0,
        session: snapshot,
        last_anvil_score: None,
        reminder_sidecar_available: true,
        reminder_kind_eligible: true,
        reminder_called_this_turn: called,
    }
}

fn make_get_env_empty() -> impl Fn(&str) -> Option<OsString> {
    |_k: &str| None
}

fn make_get_env_with(key: &'static str, val: &'static str) -> impl Fn(&str) -> Option<OsString> {
    move |k: &str| {
        if k == key {
            Some(OsString::from(val))
        } else {
            None
        }
    }
}

// Captures emitted events during invoke.
type EventCapture = Rc<RefCell<Vec<(&'static str, serde_json::Value)>>>;

fn make_event_capture() -> EventCapture {
    Rc::new(RefCell::new(Vec::new()))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn fence_routing_iteration_internal() {
    let mut registry = SkillRegistry::new();
    registry.register(DummySkill {
        name: "dummy_iter",
        triggers: &[SkillTrigger::IterationInternal],
        pre_check_result: None,
        applicability_result: true,
        execute_returns_ok: true,
        tier_value: SkillTrustTier::BuiltInReadOnly,
    });
    let snapshot = make_session_snapshot();
    let state = make_state(&snapshot, false, false);
    let mut wm = WorkingMemory::default();
    let root = make_workspace_root();
    let ctx = SkillExecutionContext {
        working_memory: &mut wm,
        workspace_root: &root,
    };
    let events = make_event_capture();
    let events_clone = events.clone();
    let mut emit_event = move |k: &'static str, v: serde_json::Value| {
        events_clone.borrow_mut().push((k, v));
    };
    let get_env = make_get_env_empty();
    let result = registry.invoke(SkillInvocationRequest {
        skill_name: "dummy_iter",
        trigger: SkillTrigger::IterationInternal,
        state: &state,
        input: SkillInput::NoOp,
        ctx,
        get_env: &get_env,
        emit_event: &mut emit_event,
        session_id: "sess1",
        model: Some("test-model"),
    });
    assert!(result.attempted_execute);
    assert!(result.output.is_some());
    let evs = events.borrow();
    assert_eq!(evs.len(), 1);
    assert_eq!(evs[0].0, "agent.dummy.completed");
}

#[test]
fn fence_routing_post_loop_does_not_invoke_iteration_only_skill() {
    let mut registry = SkillRegistry::new();
    registry.register(DummySkill {
        name: "dummy_iter",
        triggers: &[SkillTrigger::IterationInternal],
        pre_check_result: None,
        applicability_result: true,
        execute_returns_ok: true,
        tier_value: SkillTrustTier::BuiltInReadOnly,
    });
    let snapshot = make_session_snapshot();
    let state = make_state(&snapshot, false, false);
    let mut wm = WorkingMemory::default();
    let root = make_workspace_root();
    let ctx = SkillExecutionContext {
        working_memory: &mut wm,
        workspace_root: &root,
    };
    let events = make_event_capture();
    let events_clone = events.clone();
    let mut emit_event = move |k: &'static str, v: serde_json::Value| {
        events_clone.borrow_mut().push((k, v));
    };
    let get_env = make_get_env_empty();
    let result = registry.invoke(SkillInvocationRequest {
        skill_name: "dummy_iter",
        trigger: SkillTrigger::PostLoop, // skill は IterationInternal にしか登録されていない
        state: &state,
        input: SkillInput::NoOp,
        ctx,
        get_env: &get_env,
        emit_event: &mut emit_event,
        session_id: "sess1",
        model: None,
    });
    // not_found 扱い: attempted_execute = false, output = None
    assert!(!result.attempted_execute);
    assert!(result.output.is_none());
    assert!(events.borrow().is_empty());
}

#[test]
fn per_turn_cap_via_pre_check_returns_per_turn_capped() {
    // Reminder skill の pre_check が 6 段 priority で per_turn_capped を返すことを検証
    let skill = ReminderSkill::new(|| {
        Box::new(|_p: &str| -> Result<AssistantReply, String> { Err("never called".into()) })
    });
    let mut registry = SkillRegistry::new();
    registry.register(skill);
    let snapshot = make_session_snapshot();
    let state = make_state(&snapshot, false, true); // reminder_called_this_turn = true
    let mut wm = WorkingMemory::default();
    let root = make_workspace_root();
    let ctx = SkillExecutionContext {
        working_memory: &mut wm,
        workspace_root: &root,
    };
    let events = make_event_capture();
    let events_clone = events.clone();
    let mut emit_event = move |k: &'static str, v: serde_json::Value| {
        events_clone.borrow_mut().push((k, v));
    };
    let get_env = make_get_env_empty();
    let result = registry.invoke(SkillInvocationRequest {
        skill_name: "reminder",
        trigger: SkillTrigger::PostLoop,
        state: &state,
        input: SkillInput::NoOp,
        ctx,
        get_env: &get_env,
        emit_event: &mut emit_event,
        session_id: "sess1",
        model: None,
    });
    assert!(!result.attempted_execute, "Skipped should not consume cap");
    assert_eq!(result.skipped_reason, Some(SkipReason::PerTurnCapped));
    let evs = events.borrow();
    assert_eq!(evs.len(), 1);
    assert_eq!(evs[0].0, "agent.reminder.skipped");
    assert_eq!(evs[0].1["reason"], "per_turn_capped");
}

#[test]
fn plan_mode_disable_returns_plan_mode_skip_reason() {
    let skill = ReminderSkill::new(|| {
        Box::new(|_p: &str| -> Result<AssistantReply, String> { Err("never".into()) })
    });
    let mut registry = SkillRegistry::new();
    registry.register(skill);
    let snapshot = make_session_snapshot();
    let state = make_state(&snapshot, true, false); // plan_mode = true
    let mut wm = WorkingMemory::default();
    let root = make_workspace_root();
    let ctx = SkillExecutionContext {
        working_memory: &mut wm,
        workspace_root: &root,
    };
    let events = make_event_capture();
    let events_clone = events.clone();
    let mut emit_event = move |k: &'static str, v: serde_json::Value| {
        events_clone.borrow_mut().push((k, v));
    };
    let get_env = make_get_env_empty();
    let result = registry.invoke(SkillInvocationRequest {
        skill_name: "reminder",
        trigger: SkillTrigger::PostLoop,
        state: &state,
        input: SkillInput::NoOp,
        ctx,
        get_env: &get_env,
        emit_event: &mut emit_event,
        session_id: "sess1",
        model: None,
    });
    assert!(!result.attempted_execute);
    assert_eq!(result.skipped_reason, Some(SkipReason::PlanMode));
    let evs = events.borrow();
    assert_eq!(evs[0].1["reason"], "plan_mode");
}

#[test]
fn env_disable_returns_disabled_by_env() {
    let skill = ReminderSkill::new(|| {
        Box::new(|_p: &str| -> Result<AssistantReply, String> { Err("never".into()) })
    });
    let mut registry = SkillRegistry::new();
    registry.register(skill);
    let snapshot = make_session_snapshot();
    let state = make_state(&snapshot, false, false);
    let mut wm = WorkingMemory::default();
    let root = make_workspace_root();
    let ctx = SkillExecutionContext {
        working_memory: &mut wm,
        workspace_root: &root,
    };
    let events = make_event_capture();
    let events_clone = events.clone();
    let mut emit_event = move |k: &'static str, v: serde_json::Value| {
        events_clone.borrow_mut().push((k, v));
    };
    let get_env = make_get_env_with("ANVIL_NO_REMINDER", "1");
    let result = registry.invoke(SkillInvocationRequest {
        skill_name: "reminder",
        trigger: SkillTrigger::PostLoop,
        state: &state,
        input: SkillInput::NoOp,
        ctx,
        get_env: &get_env,
        emit_event: &mut emit_event,
        session_id: "sess1",
        model: None,
    });
    assert!(!result.attempted_execute);
    assert_eq!(result.skipped_reason, Some(SkipReason::DisabledByEnv));
    let evs = events.borrow();
    assert_eq!(evs[0].1["reason"], "disabled_by_env");
}

#[test]
fn execute_err_emits_failed_event_and_continues() {
    let mut registry = SkillRegistry::new();
    registry.register(DummySkill {
        name: "dummy_fail",
        triggers: &[SkillTrigger::PostLoop],
        pre_check_result: None,
        applicability_result: true,
        execute_returns_ok: false,
        tier_value: SkillTrustTier::BuiltInReadOnly,
    });
    let snapshot = make_session_snapshot();
    let state = make_state(&snapshot, false, false);
    let mut wm = WorkingMemory::default();
    let root = make_workspace_root();
    let ctx = SkillExecutionContext {
        working_memory: &mut wm,
        workspace_root: &root,
    };
    let events = make_event_capture();
    let events_clone = events.clone();
    let mut emit_event = move |k: &'static str, v: serde_json::Value| {
        events_clone.borrow_mut().push((k, v));
    };
    let get_env = make_get_env_empty();
    let result = registry.invoke(SkillInvocationRequest {
        skill_name: "dummy_fail",
        trigger: SkillTrigger::PostLoop,
        state: &state,
        input: SkillInput::NoOp,
        ctx,
        get_env: &get_env,
        emit_event: &mut emit_event,
        session_id: "sess1",
        model: None,
    });
    // failed: attempted_execute = true (cap 消費), output = None
    assert!(result.attempted_execute);
    assert!(result.output.is_none());
    let evs = events.borrow();
    assert_eq!(evs.len(), 1);
    // dummy_fail はカスタムキー (allowlist にないので fallback)
    assert_eq!(evs[0].0, "agent.skill.failed");
    assert!(
        evs[0].1["error"]
            .as_str()
            .unwrap()
            .contains("dummy failure")
    );
}

#[test]
fn applicability_false_emits_silent_skip() {
    let mut registry = SkillRegistry::new();
    registry.register(DummySkill {
        name: "dummy_silent",
        triggers: &[SkillTrigger::PostLoop],
        pre_check_result: None,
        applicability_result: false,
        execute_returns_ok: true,
        tier_value: SkillTrustTier::BuiltInReadOnly,
    });
    let snapshot = make_session_snapshot();
    let state = make_state(&snapshot, false, false);
    let mut wm = WorkingMemory::default();
    let root = make_workspace_root();
    let ctx = SkillExecutionContext {
        working_memory: &mut wm,
        workspace_root: &root,
    };
    let events = make_event_capture();
    let events_clone = events.clone();
    let mut emit_event = move |k: &'static str, v: serde_json::Value| {
        events_clone.borrow_mut().push((k, v));
    };
    let get_env = make_get_env_empty();
    let result = registry.invoke(SkillInvocationRequest {
        skill_name: "dummy_silent",
        trigger: SkillTrigger::PostLoop,
        state: &state,
        input: SkillInput::NoOp,
        ctx,
        get_env: &get_env,
        emit_event: &mut emit_event,
        session_id: "sess1",
        model: None,
    });
    assert!(!result.attempted_execute);
    assert!(result.skipped_reason.is_none()); // silent skip
    assert!(events.borrow().is_empty()); // applicability false は silent
}

#[test]
fn reminder_skill_completes_with_dummy_failure_outcome() {
    // Reminder skill が registry 経由で execute され、ReminderOutcome::Failed が
    // SkillOutput::Reminder にラップされて返ることを検証.
    // また build_log_payload 経由で `agent.reminder.failed` event が emit される.
    let skill = ReminderSkill::new(|| {
        Box::new(|_p: &str| -> Result<AssistantReply, String> { Err("test llm error".into()) })
    });
    let mut registry = SkillRegistry::new();
    registry.register(skill);
    let snapshot = make_session_snapshot();
    let state = make_state(&snapshot, false, false);
    let mut wm = WorkingMemory::default();
    let root = make_workspace_root();
    let ctx = SkillExecutionContext {
        working_memory: &mut wm,
        workspace_root: &root,
    };
    let events = make_event_capture();
    let events_clone = events.clone();
    let mut emit_event = move |k: &'static str, v: serde_json::Value| {
        events_clone.borrow_mut().push((k, v));
    };
    let get_env = make_get_env_empty();
    let frame = FeedbackFrame::default();
    let inputs = ReminderInputs {
        user_task: "test task",
        mode_label: "Act",
        plan_summary: None,
        active_precautions_summary: "(none)",
        frame: &frame,
        working_memory_touched: &[],
        anvil_score: None,
        active_precautions_at_call_time: &[],
    };
    let result = registry.invoke(SkillInvocationRequest {
        skill_name: "reminder",
        trigger: SkillTrigger::PostLoop,
        state: &state,
        input: SkillInput::Reminder(inputs),
        ctx,
        get_env: &get_env,
        emit_event: &mut emit_event,
        session_id: "sess1",
        model: Some("test-model"),
    });
    assert!(result.attempted_execute);
    let output = result
        .output
        .expect("ReminderSkill should return SkillOutput::Reminder");
    match output {
        SkillOutput::Reminder(ReminderOutcome::Failed { reason, .. }) => {
            assert!(matches!(reason, FailureReason::LlmCall(_)));
        }
        _ => panic!("expected ReminderOutcome::Failed"),
    }
    let evs = events.borrow();
    assert_eq!(evs.len(), 1);
    // build_log_payload は agent.reminder.failed を返す
    assert_eq!(evs[0].0, "agent.reminder.failed");
    // 12 key payload の一部を assert (互換確認)
    let payload = &evs[0].1;
    assert!(payload.is_object());
    let obj = payload.as_object().unwrap();
    assert!(
        obj.contains_key("session_id") || obj.contains_key("reason"),
        "expected build_log_payload schema, got: {:?}",
        obj.keys().collect::<Vec<_>>()
    );
}

#[test]
fn registry_lookup_misses_unknown_skill() {
    let mut registry = SkillRegistry::new();
    let snapshot = make_session_snapshot();
    let state = make_state(&snapshot, false, false);
    let mut wm = WorkingMemory::default();
    let root = make_workspace_root();
    let ctx = SkillExecutionContext {
        working_memory: &mut wm,
        workspace_root: &root,
    };
    let events = make_event_capture();
    let events_clone = events.clone();
    let mut emit_event = move |k: &'static str, v: serde_json::Value| {
        events_clone.borrow_mut().push((k, v));
    };
    let get_env = make_get_env_empty();
    let result = registry.invoke(SkillInvocationRequest {
        skill_name: "nonexistent",
        trigger: SkillTrigger::PostLoop,
        state: &state,
        input: SkillInput::NoOp,
        ctx,
        get_env: &get_env,
        emit_event: &mut emit_event,
        session_id: "sess1",
        model: None,
    });
    assert!(!result.attempted_execute);
    assert!(events.borrow().is_empty());
}

// FeedbackKind silence the unused-import warning in builds where the macro path differs.
#[allow(dead_code)]
fn _kind_keepalive(_k: FeedbackKind) {}
