//! E2E smoke tests for photon rollout policy (Issue #561).
//!
//! Ollama-free: uses tmpdir fixtures to exercise collect_rollout_stats_from_state
//! and evaluate_rollout_conditions directly without any LLM calls.

use anvil::session::rollout_policy::{
    ConditionStatus, RolloutPolicyConfig, collect_rollout_stats_from_state,
    evaluate_rollout_conditions,
};
use std::fs;
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_state_root() -> TempDir {
    TempDir::new().expect("tempdir")
}

/// iter_session_dirs requires UUID directory names and a valid session.json.
const SESSION_JSON_MINIMAL: &str =
    r#"{"mode_state":{"mode":"Act"},"messages":[],"checkpoints":[]}"#;

/// Create `state_root/sessions/<uuid>/logs/eval.jsonl` with given lines.
/// The session directory uses a deterministic UUID so tests are reproducible.
/// A minimal session.json is always written to satisfy iter_session_dirs.
fn write_eval_jsonl(state_root: &std::path::Path, session_uuid: &str, lines: &[&str]) {
    let session_dir = state_root.join("sessions").join(session_uuid);
    let log_dir = session_dir.join("logs");
    fs::create_dir_all(&log_dir).expect("create logs dir");
    fs::write(session_dir.join("session.json"), SESSION_JSON_MINIMAL).expect("write session.json");
    fs::write(log_dir.join("eval.jsonl"), lines.join("\n")).expect("write eval.jsonl");
}

/// Produce a JSON line with photon_eval present (non-null).
fn eval_line_with_photon() -> &'static str {
    r#"{"schema_version":1,"session_id":"test","ts_ms":0,"task":"t","model":"m","mode":"act","tool_protocol":"xml","tool_calls":[],"feedback_frame":null,"active_precautions":[],"anvil_score":null,"changed_file_classes":{"impl_files":0,"test_files":0,"setup_files":0},"verify_commands":[],"case_retrieval_result":null,"photon_eval":{"admission_decision":true,"warnings":[],"prompt_adopted":false,"task_outcome":null,"retry_summary":null},"photon_canary":0,"final_outcome":"ok"}"#
}

/// Produce a JSON line WITHOUT photon_eval (null).
fn eval_line_no_photon() -> &'static str {
    r#"{"schema_version":1,"session_id":"test","ts_ms":0,"task":"t","model":"m","mode":"act","tool_protocol":"xml","tool_calls":[],"feedback_frame":null,"active_precautions":[],"anvil_score":null,"changed_file_classes":{"impl_files":0,"test_files":0,"setup_files":0},"verify_commands":[],"case_retrieval_result":null,"photon_eval":null,"photon_canary":0,"final_outcome":"ok"}"#
}

// ---------------------------------------------------------------------------
// R1: No eval.jsonl → condition 2 Ng
// ---------------------------------------------------------------------------
#[test]
fn r1_no_eval_log_condition2_ng() {
    let tmp = make_state_root();
    // Create a session dir with session.json but NO eval.jsonl (UUID required)
    let sess_dir = tmp
        .path()
        .join("sessions")
        .join("01900001-0000-7000-8000-000000000001");
    fs::create_dir_all(sess_dir.join("logs")).expect("create dir");
    fs::write(sess_dir.join("session.json"), SESSION_JSON_MINIMAL).expect("session.json");

    let stats = collect_rollout_stats_from_state(tmp.path()).expect("collect");
    assert_eq!(stats.photon_eval_turns, 0);

    let status = evaluate_rollout_conditions(
        &stats,
        RolloutPolicyConfig {
            min_eval_turns: 100,
        },
    );
    let cond2 = status.conditions.iter().find(|c| c.id == 2).unwrap();
    assert!(matches!(cond2.status, ConditionStatus::Ng(_)));
    assert!(!status.ready_for_canary);
}

// ---------------------------------------------------------------------------
// R2: 99 photon_eval turns → condition 2 Ng (threshold 100)
// ---------------------------------------------------------------------------
#[test]
fn r2_99_photon_eval_turns_ng() {
    let tmp = make_state_root();
    let lines: Vec<&str> = (0..99).map(|_| eval_line_with_photon()).collect();
    write_eval_jsonl(tmp.path(), "01900001-0000-7000-8000-000000000002", &lines);

    let stats = collect_rollout_stats_from_state(tmp.path()).expect("collect");
    assert_eq!(stats.photon_eval_turns, 99);

    let status = evaluate_rollout_conditions(
        &stats,
        RolloutPolicyConfig {
            min_eval_turns: 100,
        },
    );
    let cond2 = status.conditions.iter().find(|c| c.id == 2).unwrap();
    assert!(matches!(cond2.status, ConditionStatus::Ng(_)));
    assert!(!status.automatic_checks_passed);
}

// ---------------------------------------------------------------------------
// R3: 100 photon_eval turns → condition 2 Ok
// ---------------------------------------------------------------------------
#[test]
fn r3_100_photon_eval_turns_ok() {
    let tmp = make_state_root();
    let lines: Vec<&str> = (0..100).map(|_| eval_line_with_photon()).collect();
    write_eval_jsonl(tmp.path(), "01900001-0000-7000-8000-000000000003", &lines);

    let stats = collect_rollout_stats_from_state(tmp.path()).expect("collect");
    assert_eq!(stats.photon_eval_turns, 100);

    let status = evaluate_rollout_conditions(
        &stats,
        RolloutPolicyConfig {
            min_eval_turns: 100,
        },
    );
    let cond2 = status.conditions.iter().find(|c| c.id == 2).unwrap();
    assert_eq!(cond2.status, ConditionStatus::Ok);
    assert!(status.automatic_checks_passed);
}

// ---------------------------------------------------------------------------
// R4: Multiple sessions sum to 100 → condition 2 Ok
// ---------------------------------------------------------------------------
#[test]
fn r4_multiple_sessions_aggregate_100() {
    let tmp = make_state_root();
    // 60 turns in session A, 40 turns in session B = 100 total
    let lines_a: Vec<&str> = (0..60).map(|_| eval_line_with_photon()).collect();
    let lines_b: Vec<&str> = (0..40).map(|_| eval_line_with_photon()).collect();
    write_eval_jsonl(tmp.path(), "01900001-0000-7000-8000-0000000000a1", &lines_a);
    write_eval_jsonl(tmp.path(), "01900001-0000-7000-8000-0000000000b1", &lines_b);

    let stats = collect_rollout_stats_from_state(tmp.path()).expect("collect");
    assert_eq!(stats.photon_eval_turns, 100);

    let status = evaluate_rollout_conditions(
        &stats,
        RolloutPolicyConfig {
            min_eval_turns: 100,
        },
    );
    let cond2 = status.conditions.iter().find(|c| c.id == 2).unwrap();
    assert_eq!(cond2.status, ConditionStatus::Ok);
}

// ---------------------------------------------------------------------------
// R5: Corrupt JSON lines are skipped, aggregation continues
// ---------------------------------------------------------------------------
#[test]
fn r5_corrupt_lines_skipped_aggregation_continues() {
    let tmp = make_state_root();
    let lines = vec![
        eval_line_with_photon(),
        "NOT_VALID_JSON{{{",
        eval_line_with_photon(),
        "",
        eval_line_with_photon(),
    ];
    write_eval_jsonl(tmp.path(), "01900001-0000-7000-8000-000000000005", &lines);

    let stats = collect_rollout_stats_from_state(tmp.path()).expect("collect");
    assert_eq!(stats.photon_eval_turns, 3);
    assert_eq!(stats.skipped_corrupt_records, 1);
}

// ---------------------------------------------------------------------------
// R6: Oversize eval.jsonl is skipped, aggregation continues from other sessions
// ---------------------------------------------------------------------------
#[test]
fn r6_oversize_session_skipped_others_aggregated() {
    use anvil::session::rollout_policy::MAX_ROLLOUT_EVAL_BYTES;

    let tmp = make_state_root();

    // Session A: oversize — UUID session with a file larger than MAX_ROLLOUT_EVAL_BYTES
    let oversize_uuid = "01900001-0000-7000-8000-000000000006";
    let oversize_sess = tmp.path().join("sessions").join(oversize_uuid);
    let oversize_dir = oversize_sess.join("logs");
    fs::create_dir_all(&oversize_dir).expect("create dir");
    fs::write(oversize_sess.join("session.json"), SESSION_JSON_MINIMAL).expect("session.json");
    let oversize_path = oversize_dir.join("eval.jsonl");
    let chunk = vec![b'x'; 1024 * 1024]; // 1 MiB chunks
    let mut content = Vec::new();
    for _ in 0..=(MAX_ROLLOUT_EVAL_BYTES / (1024 * 1024) + 1) {
        content.extend_from_slice(&chunk);
    }
    fs::write(&oversize_path, &content).expect("write oversize");

    // Session B: normal with 10 photon_eval turns
    let lines_b: Vec<&str> = (0..10).map(|_| eval_line_with_photon()).collect();
    write_eval_jsonl(tmp.path(), "01900001-0000-7000-8000-000000000007", &lines_b);

    let stats = collect_rollout_stats_from_state(tmp.path()).expect("collect");
    assert_eq!(stats.photon_eval_turns, 10);
    assert_eq!(stats.skipped_oversize_sessions, 1);
}

// ---------------------------------------------------------------------------
// R7: condition 5 is ManualRequired → ready_for_canary=false even when cond2 Ok
// ---------------------------------------------------------------------------
#[test]
fn r7_manual_required_blocks_ready_for_canary() {
    let tmp = make_state_root();
    let lines: Vec<&str> = (0..100).map(|_| eval_line_with_photon()).collect();
    write_eval_jsonl(tmp.path(), "01900001-0000-7000-8000-000000000008", &lines);

    let stats = collect_rollout_stats_from_state(tmp.path()).expect("collect");
    let status = evaluate_rollout_conditions(
        &stats,
        RolloutPolicyConfig {
            min_eval_turns: 100,
        },
    );

    assert!(status.automatic_checks_passed);
    assert!(status.manual_required);
    assert!(
        !status.ready_for_canary,
        "condition 5 ManualRequired must block READY"
    );

    let cond5 = status.conditions.iter().find(|c| c.id == 5).unwrap();
    assert!(matches!(cond5.status, ConditionStatus::ManualRequired(_)));

    assert_eq!(
        status.eval_turns_found, 100,
        "eval_turns_found reflects actual count"
    );
}

// ---------------------------------------------------------------------------
// R8: eval.jsonl without photon_canary field is still parseable
// ---------------------------------------------------------------------------
#[test]
fn r8_missing_photon_canary_field_backward_compat() {
    let tmp = make_state_root();
    // Older record without photon_canary field — serde(default) should handle it
    let old_record = r#"{"schema_version":1,"session_id":"old","ts_ms":0,"task":"t","model":"m","mode":"act","tool_protocol":"xml","tool_calls":[],"feedback_frame":null,"active_precautions":[],"anvil_score":null,"changed_file_classes":{"impl_files":0,"test_files":0,"setup_files":0},"verify_commands":[],"case_retrieval_result":null,"photon_eval":{"admission_decision":true,"warnings":[],"prompt_adopted":false,"task_outcome":null,"retry_summary":null},"final_outcome":"ok"}"#;
    write_eval_jsonl(
        tmp.path(),
        "01900001-0000-7000-8000-000000000009",
        &[old_record],
    );

    let stats = collect_rollout_stats_from_state(tmp.path()).expect("collect");
    assert_eq!(
        stats.photon_eval_turns, 1,
        "old records without photon_canary should be counted"
    );
    assert_eq!(stats.skipped_corrupt_records, 0);
}

// ---------------------------------------------------------------------------
// R9 + R10: eval_turns_found in RolloutStatus reflects stats
// ---------------------------------------------------------------------------
#[test]
fn r9_eval_turns_found_reflects_stats() {
    let stats = anvil::session::rollout_policy::RolloutStats {
        photon_eval_turns: 42,
        skipped_corrupt_records: 0,
        skipped_oversize_sessions: 0,
    };
    let status = evaluate_rollout_conditions(
        &stats,
        RolloutPolicyConfig {
            min_eval_turns: 100,
        },
    );
    assert_eq!(status.eval_turns_found, 42);
}

// ---------------------------------------------------------------------------
// R10: All conditions 1, 3, 4 are Ok (pre-implemented)
// ---------------------------------------------------------------------------
#[test]
fn r10_conditions_1_3_4_always_ok() {
    let stats = anvil::session::rollout_policy::RolloutStats::default();
    let status = evaluate_rollout_conditions(&stats, RolloutPolicyConfig { min_eval_turns: 1 });

    for id in [1u8, 3, 4] {
        let cond = status.conditions.iter().find(|c| c.id == id).unwrap();
        assert_eq!(
            cond.status,
            ConditionStatus::Ok,
            "Condition {id} should always be Ok (already implemented)"
        );
    }
}

// ---------------------------------------------------------------------------
// R11: No photon_eval (null) turns are not counted
// ---------------------------------------------------------------------------
#[test]
fn r11_null_photon_eval_not_counted() {
    let tmp = make_state_root();
    let lines: Vec<&str> = (0..50).map(|_| eval_line_no_photon()).collect();
    write_eval_jsonl(tmp.path(), "01900001-0000-7000-8000-00000000000b", &lines);

    let stats = collect_rollout_stats_from_state(tmp.path()).expect("collect");
    assert_eq!(
        stats.photon_eval_turns, 0,
        "null photon_eval should not be counted"
    );
}

// ---------------------------------------------------------------------------
// R12: automatic_checks_passed false when cond2 Ng despite others Ok
// ---------------------------------------------------------------------------
#[test]
fn r12_automatic_checks_passed_false_when_cond2_ng() {
    let stats = anvil::session::rollout_policy::RolloutStats {
        photon_eval_turns: 50,
        skipped_corrupt_records: 0,
        skipped_oversize_sessions: 0,
    };
    let status = evaluate_rollout_conditions(
        &stats,
        RolloutPolicyConfig {
            min_eval_turns: 100,
        },
    );

    let cond2 = status.conditions.iter().find(|c| c.id == 2).unwrap();
    assert!(matches!(cond2.status, ConditionStatus::Ng(_)));
    assert!(!status.automatic_checks_passed);
    assert!(!status.ready_for_canary);
}
