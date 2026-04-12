//! Tests for the closure-mode reasoning loop guard (Issue #349).
//!
//! Regression: after `agent.fix_slice` fails, the parent agent sometimes
//! enters a natural-language closure loop — producing zero-tool-call
//! reasoning that repeats the same cut points (resolveAutoAnswer /
//! truncated file / late-stage closure) until the runner kills the
//! process at 600s. The existing `LoopDetector` only fires on repeated
//! tool calls and never engages for this shape. `ClosureLoopDetector`
//! fingerprints reasoning-only responses while the guard is armed and
//! escalates through `Warn → StrongWarn → Break`.

use anvil::app::closure_loop_detector::{ClosureLoopDetector, Fingerprint};
use anvil::app::loop_detector::LoopAction;

// The two paraphrases below share the same domain vocabulary (~80%
// significant-token overlap) but swap connector words. This is the exact
// cycle 8/9 shape the phase-bench runner exposed: reworded reasoning with
// the same resolveAutoAnswer / truncated / closure / worker keywords.
const LOOP_MSG_A: &str = "The resolveAutoAnswer helper appears truncated. \
    Because the worker produced no valid proposal, the fix_slice invocation \
    failed. The late-stage closure branch should therefore abort instead \
    of looping. resolveAutoAnswer, truncated, closure, invalid, proposal, \
    worker, failure, fix_slice, target_path, rewrite, iterations, session.";

const LOOP_MSG_A_REWORDED: &str = "Worker did not produce any valid proposal, \
    so the fix_slice invocation has failed. The resolveAutoAnswer helper \
    still appears truncated, therefore the late-stage closure branch \
    should abort instead of looping further, because continuing reasoning \
    is unhelpful. Keywords: closure, invalid, proposal, worker, failure, \
    fix_slice, target_path, rewrite, iterations, session, \
    resolveAutoAnswer, truncated.";

const UNRELATED_MSG: &str = "Different angle now: the ANVIL_PLAN required \
    additional verification reads against configuration modules. Running \
    cargo check would reveal compilation blockers. Consider auditing \
    provider ollama sidecar summarize helpers, exploring retrieval \
    cache, evaluating telemetry snapshot format, inspecting hooks \
    engine wiring, scanning tooling shell policy.";

#[test]
fn short_responses_do_not_arm_the_detector() {
    // A one-line acknowledgement must not be treated as "reasoning" — we
    // only fingerprint content with enough significant tokens.
    assert!(Fingerprint::from_raw("Plan is done.").is_none());
    assert!(Fingerprint::from_raw("ok").is_none());

    let mut det = ClosureLoopDetector::new();
    for _ in 0..5 {
        assert_eq!(det.record_and_check("ok"), LoopAction::Continue);
    }
}

#[test]
fn paraphrased_reasoning_is_near_duplicate() {
    let fp1 = Fingerprint::from_raw(LOOP_MSG_A).expect("fingerprintable");
    let fp2 = Fingerprint::from_raw(LOOP_MSG_A_REWORDED).expect("fingerprintable");
    assert!(
        fp1.jaccard(&fp2) >= 0.7,
        "reworded paraphrase with the same keyword set should pass the Jaccard threshold; got {}",
        fp1.jaccard(&fp2)
    );
}

#[test]
fn first_closure_response_is_always_continue() {
    let mut det = ClosureLoopDetector::new();
    let action = det.record_and_check(LOOP_MSG_A);
    assert_eq!(
        action,
        LoopAction::Continue,
        "a single closure response must not escalate"
    );
}

#[test]
fn repeated_closure_reasoning_walks_the_escalation_ladder() {
    // Reproduces the Issue #349 cycle: the parent agent keeps producing
    // the same verbose reasoning without any tool-advancing output. Each
    // repeat should climb one rung: Warn → StrongWarn → Break.
    let mut det = ClosureLoopDetector::new();
    assert_eq!(det.record_and_check(LOOP_MSG_A), LoopAction::Continue);
    assert!(
        matches!(det.record_and_check(LOOP_MSG_A), LoopAction::Warn(_)),
        "second identical reasoning turn must Warn"
    );
    assert!(
        matches!(
            det.record_and_check(LOOP_MSG_A_REWORDED),
            LoopAction::StrongWarn(_)
        ),
        "third reasoning turn (even reworded) must StrongWarn"
    );
    assert!(
        matches!(det.record_and_check(LOOP_MSG_A), LoopAction::Break(_)),
        "fourth reasoning turn must terminate the loop"
    );
}

#[test]
fn unrelated_reasoning_does_not_match_prior_fingerprint() {
    let mut det = ClosureLoopDetector::new();
    det.record_and_check(LOOP_MSG_A);
    assert_eq!(
        det.record_and_check(UNRELATED_MSG),
        LoopAction::Continue,
        "a genuinely different cut point must not trip the guard"
    );
}

#[test]
fn unrelated_interleaving_does_not_break_escalation() {
    // cycle 6 mixed different reasoning cuts between turns and still
    // completed successfully. The guard must not reset escalation just
    // because one novel response interleaves — only a worker success or
    // an explicit reset should clear state.
    let mut det = ClosureLoopDetector::new();
    det.record_and_check(LOOP_MSG_A);
    assert_eq!(det.record_and_check(UNRELATED_MSG), LoopAction::Continue);
    let action = det.record_and_check(LOOP_MSG_A_REWORDED);
    assert!(
        matches!(action, LoopAction::Warn(_)),
        "LOOP_MSG_A repeating after an interleaved UNRELATED_MSG must still Warn, got {:?}",
        action
    );
}

#[test]
fn reset_clears_history_between_armed_phases() {
    let mut det = ClosureLoopDetector::new();
    det.record_and_check(LOOP_MSG_A);
    det.record_and_check(LOOP_MSG_A);
    det.reset();
    assert_eq!(
        det.record_and_check(LOOP_MSG_A),
        LoopAction::Continue,
        "after reset, a previously-seen fingerprint must look fresh again"
    );
}
