//! Tests for Issue #303: pre-mutation plan barrier and plan conflict fix.
//!
//! Covers:
//! - MutationBarrier blocks mutation tools when no plan is registered
//! - MutationBarrier passes through when plan is registered
//! - Non-mutation tools always pass through
//! - Barrier max count (auto-release after 2 blocks)
//! - Auto-release after plan registration
//! - Telemetry recording
//! - Barrier result `[plan_barrier]` prefix
//! - ANVIL_PLAN_UPDATE on empty plan registers new plan
//! - PROMPT_TOOL_RULES contains ANVIL_PLAN guidance

mod common;

use anvil::app::mutation_barrier::{
    MUTATION_BARRIER_MAX, MUTATION_BARRIER_MESSAGE, MutationBarrier,
};
use anvil::contracts::{AgentTelemetry, ExecutionPlan};
use anvil::tooling::{
    ExecutionClass, ExecutionMode, PermissionClass, PlanModePolicy, RollbackPolicy,
    ToolExecutionRequest, ToolExecutionStatus, ToolInput, ToolKind, ToolSpec,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_request(name: &str, kind: ToolKind, input: ToolInput) -> (usize, ToolExecutionRequest) {
    (
        0,
        ToolExecutionRequest {
            tool_call_id: format!("call_{name}"),
            spec: ToolSpec {
                version: 1,
                name: name.to_string(),
                kind,
                execution_class: ExecutionClass::Mutating,
                permission_class: PermissionClass::Confirm,
                execution_mode: ExecutionMode::SequentialOnly,
                plan_mode: PlanModePolicy::AllowedWithScope,
                rollback_policy: RollbackPolicy::None,
            },
            input,
            extra_field_warnings: Vec::new(),
        },
    )
}

fn make_edit_request() -> (usize, ToolExecutionRequest) {
    make_request(
        "file.edit",
        ToolKind::FileEdit,
        ToolInput::FileEdit {
            path: "src/foo.rs".into(),
            old_string: "old".into(),
            new_string: "new".into(),
        },
    )
}

fn make_write_request() -> (usize, ToolExecutionRequest) {
    make_request(
        "file.write",
        ToolKind::FileWrite,
        ToolInput::FileWrite {
            path: "src/foo.rs".into(),
            content: "content".into(),
        },
    )
}

fn make_read_request() -> (usize, ToolExecutionRequest) {
    make_request(
        "file.read",
        ToolKind::FileRead,
        ToolInput::FileRead {
            path: "src/foo.rs".into(),
        },
    )
}

// ---------------------------------------------------------------------------
// Test 1: barrier blocks edit without plan
// ---------------------------------------------------------------------------

#[test]
fn test_mutation_barrier_blocks_edit_without_plan() {
    let mut barrier = MutationBarrier::new();
    let requests = vec![make_edit_request()];

    let result = barrier.check_and_filter(requests, true);

    assert_eq!(result.blocked_count, 1);
    assert!(result.passed_requests.is_empty());
    assert_eq!(result.blocked_results.len(), 1);
    let (_, blocked) = &result.blocked_results[0];
    assert_eq!(blocked.status, ToolExecutionStatus::Blocked);
    assert_eq!(blocked.tool_name, "file.edit");
}

// ---------------------------------------------------------------------------
// Test 2: barrier allows edit with plan
// ---------------------------------------------------------------------------

#[test]
fn test_mutation_barrier_allows_edit_with_plan() {
    let mut barrier = MutationBarrier::new();
    let requests = vec![make_edit_request()];

    // execution_plan_empty = false (plan registered)
    let result = barrier.check_and_filter(requests, false);

    assert_eq!(result.blocked_count, 0);
    assert_eq!(result.passed_requests.len(), 1);
    assert!(result.blocked_results.is_empty());
}

// ---------------------------------------------------------------------------
// Test 3: barrier allows non-mutation tools
// ---------------------------------------------------------------------------

#[test]
fn test_mutation_barrier_allows_non_mutation_tools() {
    let mut barrier = MutationBarrier::new();
    let requests = vec![make_read_request()];

    // Even without a plan, non-mutation tools pass through
    let result = barrier.check_and_filter(requests, true);

    assert_eq!(result.blocked_count, 0);
    assert_eq!(result.passed_requests.len(), 1);
    assert!(result.blocked_results.is_empty());
}

// ---------------------------------------------------------------------------
// Test 4: barrier max count
// ---------------------------------------------------------------------------

#[test]
fn test_mutation_barrier_max_count() {
    let mut barrier = MutationBarrier::new();

    // First block
    let result1 = barrier.check_and_filter(vec![make_edit_request()], true);
    assert_eq!(result1.blocked_count, 1);

    // Second block
    let result2 = barrier.check_and_filter(vec![make_edit_request()], true);
    assert_eq!(result2.blocked_count, 1);

    // Third call: should pass through (max reached)
    let result3 = barrier.check_and_filter(vec![make_edit_request()], true);
    assert_eq!(result3.blocked_count, 0);
    assert_eq!(result3.passed_requests.len(), 1);

    // Verify max is 2
    assert_eq!(MUTATION_BARRIER_MAX, 2);
}

// ---------------------------------------------------------------------------
// Test 5: barrier auto-release after plan registration
// ---------------------------------------------------------------------------

#[test]
fn test_mutation_barrier_auto_release_after_plan() {
    let mut barrier = MutationBarrier::new();

    // Block once
    let result1 = barrier.check_and_filter(vec![make_edit_request()], true);
    assert_eq!(result1.blocked_count, 1);

    // Plan is now registered (execution_plan_empty = false)
    let result2 = barrier.check_and_filter(vec![make_edit_request()], false);
    assert_eq!(result2.blocked_count, 0);
    assert_eq!(result2.passed_requests.len(), 1);
}

// ---------------------------------------------------------------------------
// Test 6: telemetry mutation barrier count
// ---------------------------------------------------------------------------

#[test]
fn test_telemetry_mutation_barrier_count() {
    let mut telemetry = AgentTelemetry::new();
    assert_eq!(telemetry.mutation_barrier_block_count, 0);

    telemetry.record_mutation_barrier_block();
    assert_eq!(telemetry.mutation_barrier_block_count, 1);

    telemetry.record_mutation_barrier_block();
    assert_eq!(telemetry.mutation_barrier_block_count, 2);
}

// ---------------------------------------------------------------------------
// Test 7: barrier result has [plan_barrier] prefix
// ---------------------------------------------------------------------------

#[test]
fn test_barrier_result_plan_barrier_prefix() {
    let mut barrier = MutationBarrier::new();
    let requests = vec![make_edit_request()];

    let result = barrier.check_and_filter(requests, true);
    let (_, blocked) = &result.blocked_results[0];

    assert!(
        blocked.summary.starts_with("[plan_barrier]"),
        "blocked result summary should start with [plan_barrier], got: {}",
        blocked.summary
    );
    assert!(
        blocked.summary.contains(MUTATION_BARRIER_MESSAGE),
        "blocked result should contain the barrier message"
    );
}

// ---------------------------------------------------------------------------
// Test 8: ANVIL_PLAN_UPDATE on empty plan — verify parsing path
// ---------------------------------------------------------------------------

#[test]
fn test_plan_update_on_empty_plan() {
    // Test that ANVIL_PLAN_UPDATE content can be parsed and used to create
    // a new ExecutionPlan. The actual integration (try_update_plan calling
    // register_plan_from_items when plan is empty) is pub(crate) and tested
    // indirectly via the agentic loop.
    let content =
        "```ANVIL_PLAN_UPDATE\n- [ ] src/foo.rs: add feature\n- [ ] src/bar.rs: update module\n```";
    let block = anvil::agent::extract_plan_update_block(content);
    assert!(block.is_some(), "should extract ANVIL_PLAN_UPDATE block");

    let items = anvil::agent::parse_plan_items(&block.unwrap());
    assert_eq!(items.len(), 2);

    // Simulate what register_plan_from_items does
    let plan = ExecutionPlan::new(items);
    assert!(!plan.is_empty());
    assert_eq!(plan.items.len(), 2);
    assert_eq!(plan.items[0].description, "src/foo.rs: add feature");
}

// ---------------------------------------------------------------------------
// Test 9: PROMPT_TOOL_RULES contains ANVIL_PLAN guidance
// ---------------------------------------------------------------------------

#[test]
fn test_prompt_rules_anvil_plan_guidance() {
    let prompt = anvil::agent::tool_protocol_system_prompt_all_tools(&[], None);
    assert!(
        prompt.contains("Implementation plans MUST use the ANVIL_PLAN block format"),
        "system prompt should contain ANVIL_PLAN guidance"
    );
    assert!(
        prompt.contains("agent.plan is for read-only exploration/investigation only"),
        "system prompt should contain agent.plan exploration-only guidance"
    );
}

// ---------------------------------------------------------------------------
// Test 10: mixed batch — mutation blocked, non-mutation passes
// ---------------------------------------------------------------------------

#[test]
fn test_mutation_barrier_mixed_batch() {
    let mut barrier = MutationBarrier::new();
    let requests = vec![
        make_read_request(),
        make_edit_request(),
        make_write_request(),
    ];

    let result = barrier.check_and_filter(requests, true);

    // read should pass, edit and write should be blocked
    assert_eq!(result.passed_requests.len(), 1);
    assert_eq!(result.passed_requests[0].1.spec.name, "file.read");
    assert_eq!(result.blocked_count, 2);
    assert_eq!(result.blocked_results.len(), 2);
}

// ---------------------------------------------------------------------------
// Test 11: tag-based prompt contains exploration-only note for agent.plan
// ---------------------------------------------------------------------------

#[test]
fn test_tag_spec_agent_plan_exploration_note() {
    let spec = anvil::agent::tag_spec::find_spec("agent.plan").expect("agent.plan should exist");
    assert!(
        spec.example.contains("for exploration only"),
        "agent.plan example should contain exploration-only note, got: {}",
        spec.example
    );
}

// ---------------------------------------------------------------------------
// Test 12: telemetry serde default for new field
// ---------------------------------------------------------------------------

#[test]
fn test_telemetry_serde_default_mutation_barrier() {
    // Deserializing old telemetry without the new field should default to 0
    let json = r#"{"premature_final_count":0,"total_final_requests":0,"plan_registration_count":0,"plan_update_count":0,"sync_from_touched_files_count":0}"#;
    let telemetry: AgentTelemetry = serde_json::from_str(json).unwrap();
    assert_eq!(telemetry.mutation_barrier_block_count, 0);
}
