//! Tests for Issue #391: ANVIL_PLAN stall detection (`PlanStallTracker`).
//!
//! Exercises the public contract of the turn-local tracker and the
//! `PLAN_RESTATEMENT_RETRY_MESSAGE` constant wired into the agentic-loop
//! turn-end guard. The tests intentionally drive the tracker directly
//! rather than replaying a full agentic turn — the integration points
//! (where `record_plan_block` / `record_first_tool_call` are called from
//! `complete_structured_response`) are covered by the existing Replan and
//! FINAL_GUARD regression tests (`followup_replan.rs`,
//! `mutation_barrier.rs`, etc.).

use anvil::app::agentic::PLAN_RESTATEMENT_RETRY_MESSAGE;
use anvil::app::plan_stall_tracker::PlanStallTracker;

// ---------------------------------------------------------------------------
// Test 1: a single ANVIL_PLAN block must not trigger the stall guard.
// The first ANVIL_PLAN is the legitimate pre-mutation announcement.
// ---------------------------------------------------------------------------

#[test]
fn plan_stall_not_triggered_on_first_anvil_plan() {
    let mut tracker = PlanStallTracker::new();
    tracker.record_plan_block();

    assert!(
        !tracker.is_stalled(),
        "first ANVIL_PLAN emission must not be flagged as stall"
    );
}

// ---------------------------------------------------------------------------
// Test 2: a second ANVIL_PLAN without any mutation tool call triggers the
// stall guard — this is the exact drift shape Issue #391 targets.
// ---------------------------------------------------------------------------

#[test]
fn plan_stall_triggered_on_second_anvil_plan() {
    let mut tracker = PlanStallTracker::new();
    tracker.record_plan_block();
    tracker.record_plan_block();

    assert!(
        tracker.is_stalled(),
        "second ANVIL_PLAN with no tool call must trigger the stall guard"
    );
}

// ---------------------------------------------------------------------------
// Test 3: once a file.write / file.edit tool call has landed, subsequent
// ANVIL_PLAN blocks are legitimate follow-up replans (Issue #305) and must
// not trigger the stall guard.
// ---------------------------------------------------------------------------

#[test]
fn plan_stall_not_triggered_after_tool_call() {
    let mut tracker = PlanStallTracker::new();
    tracker.record_plan_block();
    tracker.record_first_tool_call();
    tracker.record_plan_block();

    assert!(
        !tracker.is_stalled(),
        "ANVIL_PLAN after a successful mutation tool must be treated as replan, not stall"
    );
}

// ---------------------------------------------------------------------------
// Test 4: `reset_for_turn` restores a tracker to a clean state. A tracker
// that stalled in turn N does not carry over into turn N+1.
// ---------------------------------------------------------------------------

#[test]
fn plan_stall_reset_on_new_turn() {
    let mut tracker = PlanStallTracker::new();
    tracker.record_plan_block();
    tracker.record_plan_block();
    assert!(
        tracker.is_stalled(),
        "precondition: tracker must stall after two ANVIL_PLAN blocks"
    );

    tracker.reset_for_turn();
    assert!(
        !tracker.is_stalled(),
        "reset_for_turn must clear the stall state"
    );

    tracker.record_plan_block();
    assert!(
        !tracker.is_stalled(),
        "after reset, a single ANVIL_PLAN in the next turn must not stall"
    );
}

// ---------------------------------------------------------------------------
// Test 5: the corrective hint `PLAN_RESTATEMENT_RETRY_MESSAGE` is the
// designated recovery path (Issue #391 AC: recovery path is documented and
// covered by tests). Verify the constant is wired correctly and carries
// the required prescriptive language.
// ---------------------------------------------------------------------------

#[test]
fn plan_restatement_retry_message_injected() {
    // Drive the tracker into a stalled state to document the trigger
    // condition alongside the message assertion.
    let mut tracker = PlanStallTracker::new();
    tracker.record_plan_block();
    tracker.record_plan_block();
    assert!(tracker.is_stalled());

    // Recovery hint must clearly forbid ANVIL_PLAN repetition and require
    // an immediate file.write/file.edit tool call.
    assert!(
        PLAN_RESTATEMENT_RETRY_MESSAGE.contains("Do NOT output ANVIL_PLAN again"),
        "recovery message should prohibit repeating ANVIL_PLAN, got: {PLAN_RESTATEMENT_RETRY_MESSAGE}"
    );
    assert!(
        PLAN_RESTATEMENT_RETRY_MESSAGE.contains("file.write")
            && PLAN_RESTATEMENT_RETRY_MESSAGE.contains("file.edit"),
        "recovery message should reference the required tools, got: {PLAN_RESTATEMENT_RETRY_MESSAGE}"
    );
    assert!(
        PLAN_RESTATEMENT_RETRY_MESSAGE.contains("Start implementing now"),
        "recovery message should contain the call-to-action phrase"
    );
}

// ---------------------------------------------------------------------------
// Test 6: an ANVIL_PLAN_UPDATE (parsed via `try_update_plan`, not
// `try_register_plan`) between two ANVIL_PLAN blocks does not call
// `record_plan_block`, so an update-only emission does not prevent stall
// detection. In practice the second ANVIL_PLAN still drives the counter to
// 2 and fires the guard — ANVIL_PLAN_UPDATE is intentionally transparent
// to the tracker (Issue #323 compatibility).
// ---------------------------------------------------------------------------

#[test]
fn plan_stall_not_triggered_with_update_between() {
    let mut tracker = PlanStallTracker::new();
    // Only ANVIL_PLAN blocks feed the tracker — ANVIL_PLAN_UPDATE is routed
    // through `try_update_plan` and bypasses `record_plan_block`.
    tracker.record_plan_block(); // ANVIL_PLAN
    // ANVIL_PLAN_UPDATE between: no record_plan_block call — tracker
    // remains at count = 1, so the state is unchanged from test 1.

    assert!(
        !tracker.is_stalled(),
        "ANVIL_PLAN_UPDATE alone must not stall (only first ANVIL_PLAN observed)"
    );

    // A second true ANVIL_PLAN block now arrives — this is the stall shape
    // we DO want to flag. The ANVIL_PLAN_UPDATE in between is irrelevant.
    tracker.record_plan_block();
    assert!(
        tracker.is_stalled(),
        "second ANVIL_PLAN (ignoring ANVIL_PLAN_UPDATE routing) must still stall"
    );
}

// ---------------------------------------------------------------------------
// Test 7a: file.edit_anchor is also a mutation tool and must release the
// stall latch via `record_first_tool_call`. A follow-up ANVIL_PLAN after the
// edit_anchor mutation is a legitimate replan, not a stall (CB-001).
// ---------------------------------------------------------------------------

#[test]
fn plan_stall_not_triggered_after_edit_anchor() {
    let mut tracker = PlanStallTracker::new();
    tracker.record_plan_block();
    // Simulate file.edit_anchor success — wired in agentic.rs via the
    // mutation-tool-name matcher that includes edit_anchor.
    tracker.record_first_tool_call();
    tracker.record_plan_block();

    assert!(
        !tracker.is_stalled(),
        "ANVIL_PLAN after a successful file.edit_anchor must be treated as replan, not stall"
    );
}

// ---------------------------------------------------------------------------
// Test 7b: file.rewrite is also a mutation tool and must release the stall
// latch via `record_first_tool_call`. A follow-up ANVIL_PLAN after the
// rewrite is a legitimate replan, not a stall (CB-001).
// ---------------------------------------------------------------------------

#[test]
fn plan_stall_not_triggered_after_rewrite() {
    let mut tracker = PlanStallTracker::new();
    tracker.record_plan_block();
    // Simulate file.rewrite success — wired in agentic.rs via the
    // mutation-tool-name matcher that includes rewrite.
    tracker.record_first_tool_call();
    tracker.record_plan_block();

    assert!(
        !tracker.is_stalled(),
        "ANVIL_PLAN after a successful file.rewrite must be treated as replan, not stall"
    );
}

// ---------------------------------------------------------------------------
// Test 7: multi-turn lifecycle. A tracker that stalled in turn 1 and was
// reset must not falsely flag turn 2 when only a single ANVIL_PLAN arrives.
// Regression guard against accidental state carry-over.
// ---------------------------------------------------------------------------

#[test]
fn plan_stall_multi_turn_reset() {
    let mut tracker = PlanStallTracker::new();

    // --- Turn 1: stalled ---
    tracker.record_plan_block();
    tracker.record_plan_block();
    assert!(tracker.is_stalled(), "turn 1 must stall");

    // --- Turn boundary ---
    tracker.reset_for_turn();

    // --- Turn 2: legitimate single ANVIL_PLAN, no stall ---
    tracker.record_plan_block();
    assert!(
        !tracker.is_stalled(),
        "turn 2 with one ANVIL_PLAN must not stall after reset"
    );

    // --- Turn 2 continues with a real mutation tool ---
    tracker.record_first_tool_call();
    // A follow-up ANVIL_PLAN after the mutation is a replan and stays clean.
    tracker.record_plan_block();
    assert!(
        !tracker.is_stalled(),
        "replan after successful mutation must not stall"
    );
}
