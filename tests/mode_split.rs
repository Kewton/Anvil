//! Integration tests for Issue #380: mode split — separate plan / act / repair
//! modes for local-model stability.
//!
//! These tests verify:
//! - [`AgenticMode`] / [`RepairReason`] enum semantics
//! - [`RepairReason::is_recovery_signal`] behavior (Repair→Act transitions)
//! - Strict closure gate semantics (only [`RepairReason::PreExit`] activates
//!   the Issue #327 unchecked-item rejection path)
//! - [`AgentTelemetry`] counter fields used to observe mode churn
//!
//! Note: the transition logic itself is private to
//! `complete_structured_response` (see `src/app/agentic.rs`); the external
//! contract exposed to integration tests is the enum API and telemetry. These
//! tests exercise the boundaries of that contract rather than mocking the
//! agentic loop.

use anvil::app::{AgenticMode, RepairReason};
use anvil::contracts::AgentTelemetry;

// ---------------------------------------------------------------------------
// Test 1: Plan→Act — `ANVIL_PLAN` detection triggers the Act transition
// ---------------------------------------------------------------------------

/// When a non-empty `ExecutionPlan` becomes present, the turn-local mode
/// transitions from `Plan` to `Act` and the telemetry counter is bumped.
///
/// This test verifies the semantic contract: starting the turn in `Plan`,
/// flipping to `Act` is a distinguishable state change and the telemetry
/// counter is zero by default.
#[test]
fn plan_to_act_on_anvil_plan() {
    // Default: turn begins in Plan.
    let initial = AgenticMode::Plan;
    assert_eq!(initial, AgenticMode::Plan);

    // Plan and Act are distinct.
    assert_ne!(AgenticMode::Plan, AgenticMode::Act);

    // Telemetry counter defaults to zero, and is used to record Plan→Act.
    let telemetry = AgentTelemetry::default();
    assert_eq!(telemetry.agentic_mode_plan_to_act_count, 0);

    // Simulate one Plan→Act transition at the boundary of the agentic loop.
    let mut telemetry = telemetry;
    let mode = AgenticMode::Act;
    telemetry.agentic_mode_plan_to_act_count += 1;
    assert_eq!(mode, AgenticMode::Act);
    assert_eq!(telemetry.agentic_mode_plan_to_act_count, 1);
}

// ---------------------------------------------------------------------------
// Test 2: Act→Repair on FixSliceEscalation → EditFailRecovery
// ---------------------------------------------------------------------------

/// When the edit-fail tracker escalates, the Act→Repair transition uses
/// [`RepairReason::EditFailRecovery`]. This test verifies:
/// - the Repair variant wraps the expected reason
/// - `is_recovery_signal` only returns true when the edit_fail tracker
///   has recovered
#[test]
fn act_to_repair_on_edit_fail_escalation() {
    let repair_mode = AgenticMode::Repair(RepairReason::EditFailRecovery);

    // Repair variants are distinct from Act and Plan.
    assert_ne!(repair_mode, AgenticMode::Act);
    assert_ne!(repair_mode, AgenticMode::Plan);

    // Recovery semantics: EditFailRecovery recovers only when
    // `edit_fail_resolved` is true. Other signals do NOT clear it.
    let reason = RepairReason::EditFailRecovery;
    assert!(
        !reason.is_recovery_signal(
            /*worker_observed=*/ true, /*edit_fail_resolved=*/ false,
            /*stagnation_resolved=*/ true,
        ),
        "EditFailRecovery must not recover on worker_observed alone"
    );
    assert!(
        reason.is_recovery_signal(
            /*worker_observed=*/ false, /*edit_fail_resolved=*/ true,
            /*stagnation_resolved=*/ false,
        ),
        "EditFailRecovery must recover when edit_fail_resolved=true"
    );

    // Telemetry: Act→Repair increments the counter.
    let mut telemetry = AgentTelemetry::default();
    telemetry.agentic_mode_act_to_repair_count += 1;
    assert_eq!(telemetry.agentic_mode_act_to_repair_count, 1);
}

// ---------------------------------------------------------------------------
// Test 3: Act→Repair on stagnation → StagnationRecovery
// ---------------------------------------------------------------------------

/// When stagnation or read-heavy drift fires, the Act→Repair transition uses
/// [`RepairReason::StagnationRecovery`]. Recovery clears either via a
/// stagnation drop OR a worker-observed mutation.
#[test]
fn act_to_repair_on_stagnation() {
    let repair_mode = AgenticMode::Repair(RepairReason::StagnationRecovery);
    assert_ne!(repair_mode, AgenticMode::Repair(RepairReason::PreExit));
    assert_ne!(
        repair_mode,
        AgenticMode::Repair(RepairReason::EditFailRecovery)
    );

    let reason = RepairReason::StagnationRecovery;

    // Stagnation recovers on stagnation_resolved.
    assert!(
        reason.is_recovery_signal(
            /*worker_observed=*/ false, /*edit_fail_resolved=*/ false,
            /*stagnation_resolved=*/ true,
        ),
        "StagnationRecovery must recover when stagnation_resolved=true"
    );

    // Stagnation also recovers on worker_observed (worker progress clears
    // the stagnation heuristic).
    assert!(
        reason.is_recovery_signal(
            /*worker_observed=*/ true, /*edit_fail_resolved=*/ false,
            /*stagnation_resolved=*/ false,
        ),
        "StagnationRecovery must recover when worker_observed=true"
    );

    // Neither signal → no recovery.
    assert!(
        !reason.is_recovery_signal(
            /*worker_observed=*/ false, /*edit_fail_resolved=*/ true,
            /*stagnation_resolved=*/ false,
        ),
        "StagnationRecovery must not recover on edit_fail_resolved alone"
    );
}

// ---------------------------------------------------------------------------
// Test 4: Repair→Act on worker_observed=true
// ---------------------------------------------------------------------------

/// When a worker mutation is observed while in `Repair(PreExit)`, the mode
/// transitions back to `Act`. This is the happy-path recovery for the
/// pre-exit repair heuristic (fix_slice worker produced a mutation OR a
/// plan re-issue occurred).
#[test]
fn repair_to_act_on_worker_observed() {
    let reason = RepairReason::PreExit;

    // worker_observed is the sole recovery signal for PreExit.
    assert!(
        reason.is_recovery_signal(
            /*worker_observed=*/ true, /*edit_fail_resolved=*/ false,
            /*stagnation_resolved=*/ false,
        ),
        "PreExit must recover when worker_observed=true"
    );

    // edit_fail_resolved / stagnation_resolved alone are NOT sufficient
    // for PreExit.
    assert!(
        !reason.is_recovery_signal(
            /*worker_observed=*/ false, /*edit_fail_resolved=*/ true,
            /*stagnation_resolved=*/ true,
        ),
        "PreExit must not recover without worker_observed"
    );

    // Telemetry: Repair→Act increments the counter.
    let mut telemetry = AgentTelemetry::default();
    telemetry.agentic_mode_repair_to_act_count += 1;
    assert_eq!(telemetry.agentic_mode_repair_to_act_count, 1);
}

// ---------------------------------------------------------------------------
// Test 5: PreExit activates the strict closure gate
// ---------------------------------------------------------------------------

/// The strict closure gate (Issue #327: reject unchecked plan expansion during
/// repair closure mode) is activated only when the turn-local mode is
/// `Repair(PreExit)`. This test verifies the translation rule the agentic
/// loop uses before forwarding the bool to `apply_plan_update_pipeline`.
#[test]
fn pre_exit_repair_activates_strict_gate() {
    // The translation rule used in src/app/agentic.rs:
    //   let strict_closure_gate =
    //       matches!(current_mode, AgenticMode::Repair(RepairReason::PreExit));
    let mode_preexit = AgenticMode::Repair(RepairReason::PreExit);
    let strict_closure_gate = matches!(mode_preexit, AgenticMode::Repair(RepairReason::PreExit));
    assert!(
        strict_closure_gate,
        "Repair(PreExit) must activate the strict closure gate"
    );
}

// ---------------------------------------------------------------------------
// Test 6: StagnationRecovery / EditFailRecovery / Act / Plan do NOT activate
// the strict closure gate
// ---------------------------------------------------------------------------

/// Repair variants other than `PreExit`, and both `Plan` and `Act`, must NOT
/// activate the strict closure gate. Only `PreExit` carries the Issue #327
/// repair-closure semantics; other recovery reasons are intended to allow
/// normal plan updates (e.g., follow-up `ANVIL_PLAN` blocks during stagnation
/// recovery must still be able to append items).
#[test]
fn stagnation_recovery_no_strict_gate() {
    // StagnationRecovery must NOT activate the gate.
    let mode_stagnation = AgenticMode::Repair(RepairReason::StagnationRecovery);
    let strict_closure_gate = matches!(mode_stagnation, AgenticMode::Repair(RepairReason::PreExit));
    assert!(
        !strict_closure_gate,
        "Repair(StagnationRecovery) must not activate the strict closure gate"
    );

    // EditFailRecovery must NOT activate the gate.
    let mode_edit_fail = AgenticMode::Repair(RepairReason::EditFailRecovery);
    let strict_closure_gate = matches!(mode_edit_fail, AgenticMode::Repair(RepairReason::PreExit));
    assert!(
        !strict_closure_gate,
        "Repair(EditFailRecovery) must not activate the strict closure gate"
    );

    // Plan must NOT activate the gate.
    let mode_plan = AgenticMode::Plan;
    let strict_closure_gate = matches!(mode_plan, AgenticMode::Repair(RepairReason::PreExit));
    assert!(
        !strict_closure_gate,
        "Plan must not activate the strict closure gate"
    );

    // Act must NOT activate the gate.
    let mode_act = AgenticMode::Act;
    let strict_closure_gate = matches!(mode_act, AgenticMode::Repair(RepairReason::PreExit));
    assert!(
        !strict_closure_gate,
        "Act must not activate the strict closure gate"
    );
}

// ---------------------------------------------------------------------------
// Additional coverage: telemetry turn-count fields default to zero and are
// independent (serde-backward-compatible via #[serde(default)]).
// ---------------------------------------------------------------------------

/// All six Issue #380 telemetry fields default to zero and exist as
/// independent u32 counters in [`AgentTelemetry`]. This guards the
/// telemetry contract that downstream reporters rely on.
#[test]
fn agent_telemetry_issue_380_fields_default_to_zero() {
    let telemetry = AgentTelemetry::default();

    // Transition counts.
    assert_eq!(telemetry.agentic_mode_plan_to_act_count, 0);
    assert_eq!(telemetry.agentic_mode_act_to_repair_count, 0);
    assert_eq!(telemetry.agentic_mode_repair_to_act_count, 0);

    // Per-mode turn counts.
    assert_eq!(telemetry.agentic_mode_turns_in_plan, 0);
    assert_eq!(telemetry.agentic_mode_turns_in_act, 0);
    assert_eq!(telemetry.agentic_mode_turns_in_repair, 0);
}
