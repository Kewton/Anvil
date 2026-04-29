//! Issue #467 / Phase P6: SkillTrustTier E2E smoke test.
//!
//! 8 ケースで以下を検証:
//! 1. ExternalDisabled 宣言 dummy skill が deny される
//! 2. Plan mode + BuiltInCanWriteTemp dummy skill が deny される
//! 3. Plan mode + BuiltInReadOnly skill は tier_check 通過 (deny されない)
//! 4. Plan mode + VerifierSkill 名 (BuiltInCanRequestBash) は tier_check bypass で run() 到達
//! 5. Act mode + すべての tier が allow される (ExternalDisabled は別経路で deny)
//! 6. permission_denied 後、per-turn cap が立たないことを 2 連続 invoke で確認
//! 7. session.json round-trip で SkillPermissionDenied が UnknownFailure に潰れない
//! 8. event payload 全体組み立てで snake_case 部分文字列を含む

use std::cell::{Cell, RefCell};
use std::ffi::OsString;
use std::path::PathBuf;
use std::rc::Rc;

use anvil::agent::loop_run::ReminderSkipReason as SkipReason;
use anvil::agent::skills::{
    AgentSkill, RuntimeState, SkillExecuteError, SkillExecutionContext, SkillInput,
    SkillInvocationRequest, SkillOutput, SkillRegistry, SkillTrigger, SkillTrustTier,
};
use anvil::session::feedback::FeedbackKind;
use anvil::session::store::{SessionSnapshot, WorkingMemory};

// ---------------------------------------------------------------------------
// E2E fixture: tier 値と execute count を持つ skill
// ---------------------------------------------------------------------------

struct TierE2ESkill {
    name_static: &'static str,
    triggers: &'static [SkillTrigger],
    tier_value: SkillTrustTier,
    execute_count: Cell<usize>,
}

impl TierE2ESkill {
    fn new(name: &'static str, tier: SkillTrustTier) -> Self {
        Self {
            name_static: name,
            triggers: &[SkillTrigger::PostLoop],
            tier_value: tier,
            execute_count: Cell::new(0),
        }
    }
}

impl AgentSkill for TierE2ESkill {
    fn name(&self) -> &'static str {
        self.name_static
    }
    fn triggers(&self) -> &'static [SkillTrigger] {
        self.triggers
    }
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
        _input: SkillInput<'_>,
        _ctx: &mut SkillExecutionContext<'_>,
    ) -> Result<SkillOutput, SkillExecuteError> {
        self.execute_count.set(self.execute_count.get() + 1);
        Ok(SkillOutput::NoOp)
    }
    fn render_log_payload(
        &self,
        _outcome: &SkillOutput,
        _session_id: &str,
        _model: Option<&str>,
    ) -> (&'static str, serde_json::Value) {
        ("agent.tier_e2e.completed", serde_json::Value::Null)
    }
    fn tier(&self) -> SkillTrustTier {
        self.tier_value
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

type EventCapture = Rc<RefCell<Vec<(&'static str, serde_json::Value)>>>;

fn make_state<'a>(snapshot: &'a SessionSnapshot, plan_mode: bool) -> RuntimeState<'a> {
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

fn invoke_helper(
    registry: &mut SkillRegistry,
    skill_name: &'static str,
    state: &RuntimeState,
    events: &EventCapture,
) -> bool {
    let mut wm = WorkingMemory::default();
    let root = PathBuf::from("/tmp/skill_tier_e2e");
    let ctx = SkillExecutionContext {
        working_memory: &mut wm,
        workspace_root: &root,
    };
    let events_clone = events.clone();
    let mut emit_event = move |k: &'static str, v: serde_json::Value| {
        events_clone.borrow_mut().push((k, v));
    };
    let get_env = |_k: &str| None;
    let result = registry.invoke(SkillInvocationRequest {
        skill_name,
        trigger: SkillTrigger::PostLoop,
        state,
        input: SkillInput::NoOp,
        ctx,
        get_env: &get_env,
        emit_event: &mut emit_event,
        session_id: "sess-e2e",
        model: Some("test-model"),
    });
    matches!(result.output, Some(SkillOutput::PermissionDenied(_)))
}

// ---------------------------------------------------------------------------
// Tests (8 ケース)
// ---------------------------------------------------------------------------

/// 1. ExternalDisabled 宣言 dummy skill が deny される.
#[test]
fn external_disabled_skill_is_denied_in_act_mode() {
    let mut registry = SkillRegistry::new();
    registry.register(TierE2ESkill::new(
        "ext_e2e",
        SkillTrustTier::ExternalDisabled,
    ));
    let snapshot = SessionSnapshot::default();
    let state = make_state(&snapshot, false);
    let events = Rc::new(RefCell::new(Vec::new()));
    let denied = invoke_helper(&mut registry, "ext_e2e", &state, &events);
    assert!(denied, "ExternalDisabled tier must be denied");
    let evs = events.borrow();
    assert_eq!(evs.len(), 1);
    assert_eq!(evs[0].0, "agent.skill.permission_denied");
    assert_eq!(evs[0].1["reason"], "external_disabled");
    assert_eq!(evs[0].1["requested_capability"], "external");
}

/// 2. Plan mode + BuiltInCanWriteTemp dummy skill が deny される.
#[test]
fn plan_mode_can_write_temp_is_denied() {
    let mut registry = SkillRegistry::new();
    registry.register(TierE2ESkill::new(
        "wt_e2e",
        SkillTrustTier::BuiltInCanWriteTemp,
    ));
    let snapshot = SessionSnapshot::default();
    let state = make_state(&snapshot, true);
    let events = Rc::new(RefCell::new(Vec::new()));
    let denied = invoke_helper(&mut registry, "wt_e2e", &state, &events);
    assert!(denied);
    let evs = events.borrow();
    assert_eq!(evs[0].0, "agent.skill.permission_denied");
    assert_eq!(evs[0].1["reason"], "plan_mode_violation");
    assert_eq!(evs[0].1["requested_capability"], "write_repo");
}

/// 3. Plan mode + BuiltInReadOnly skill は tier_check 通過 (deny されない).
#[test]
fn plan_mode_built_in_read_only_passes_tier_check() {
    let mut registry = SkillRegistry::new();
    registry.register(TierE2ESkill::new("ro_e2e", SkillTrustTier::BuiltInReadOnly));
    let snapshot = SessionSnapshot::default();
    let state = make_state(&snapshot, true);
    let events = Rc::new(RefCell::new(Vec::new()));
    let denied = invoke_helper(&mut registry, "ro_e2e", &state, &events);
    assert!(
        !denied,
        "BuiltInReadOnly should pass tier_check in Plan mode"
    );
    let evs = events.borrow();
    assert_eq!(evs[0].0, "agent.tier_e2e.completed");
}

/// 4. Plan mode + "verifier" 名 (BuiltInCanRequestBash) は tier_check bypass で run() 到達.
#[test]
fn plan_mode_verifier_bypasses_tier_check() {
    let mut registry = SkillRegistry::new();
    registry.register(TierE2ESkill::new(
        "verifier",
        SkillTrustTier::BuiltInCanRequestBash,
    ));
    let snapshot = SessionSnapshot::default();
    let state = make_state(&snapshot, true);
    let events = Rc::new(RefCell::new(Vec::new()));
    let denied = invoke_helper(&mut registry, "verifier", &state, &events);
    assert!(
        !denied,
        "verifier should bypass Plan mode tier_check (DR-466-001)"
    );
}

/// 5. Act mode + ExternalDisabled 以外の tier は allow.
#[test]
fn act_mode_non_external_tiers_allow() {
    let snapshot = SessionSnapshot::default();
    let state = make_state(&snapshot, false);
    for (name, tier) in [
        ("ro_act", SkillTrustTier::BuiltInReadOnly),
        ("wt_act", SkillTrustTier::BuiltInCanWriteTemp),
        ("rb_act", SkillTrustTier::BuiltInCanRequestBash),
    ] {
        let mut registry = SkillRegistry::new();
        registry.register(TierE2ESkill::new(name, tier));
        let events = Rc::new(RefCell::new(Vec::new()));
        let denied = invoke_helper(&mut registry, name, &state, &events);
        assert!(!denied, "Act mode + {tier:?} should not be denied");
    }
}

/// 6. permission_denied 後、per-turn cap が立たないことを 2 連続 invoke で確認.
#[test]
fn permission_denied_does_not_consume_per_turn_cap() {
    let mut registry = SkillRegistry::new();
    registry.register(TierE2ESkill::new(
        "ext_cap",
        SkillTrustTier::ExternalDisabled,
    ));
    let snapshot = SessionSnapshot::default();
    let state = make_state(&snapshot, false);
    let events = Rc::new(RefCell::new(Vec::new()));
    // 1 回目
    let denied1 = invoke_helper(&mut registry, "ext_cap", &state, &events);
    assert!(denied1);
    // 2 回目: per-turn cap が立たないため再度 deny される (event は 2 件記録)
    let denied2 = invoke_helper(&mut registry, "ext_cap", &state, &events);
    assert!(denied2, "second invoke must also deny (cap not consumed)");
    let evs = events.borrow();
    assert_eq!(evs.len(), 2, "event count must be 2 (1 per invoke)");
    assert_eq!(evs[0].0, "agent.skill.permission_denied");
    assert_eq!(evs[1].0, "agent.skill.permission_denied");
}

/// 7. session.json round-trip で SkillPermissionDenied が UnknownFailure に潰れない.
#[test]
fn skill_permission_denied_serde_round_trip() {
    let kind = FeedbackKind::SkillPermissionDenied;
    let json = serde_json::to_string(&kind).unwrap();
    assert_eq!(json, "\"skill_permission_denied\"");
    let decoded: FeedbackKind = serde_json::from_str(&json).unwrap();
    assert_eq!(decoded, FeedbackKind::SkillPermissionDenied);
    assert_ne!(decoded, FeedbackKind::UnknownFailure);
}

/// 8. event payload 全体組み立てで snake_case 部分文字列を含む (DR1-005).
#[test]
fn permission_denied_event_payload_contains_snake_case_tier() {
    let mut registry = SkillRegistry::new();
    registry.register(TierE2ESkill::new(
        "ext_snake",
        SkillTrustTier::ExternalDisabled,
    ));
    let snapshot = SessionSnapshot::default();
    let state = make_state(&snapshot, false);
    let events = Rc::new(RefCell::new(Vec::new()));
    invoke_helper(&mut registry, "ext_snake", &state, &events);
    let evs = events.borrow();
    let payload_str = serde_json::to_string(&evs[0].1).unwrap();
    assert!(
        payload_str.contains("\"tier\":\"external_disabled\""),
        "expected snake_case tier in payload, got: {payload_str}"
    );
    assert!(payload_str.contains("\"skill_name\":\"ext_snake\""));
    assert!(payload_str.contains("\"reason\":\"external_disabled\""));
    assert!(payload_str.contains("\"requested_capability\":\"external\""));
}
