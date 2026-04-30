//! Smoke tests for `session::export` — closure DI, no Ollama required.
//!
//! Test cases:
//! C1  success trace → --success-only includes it
//! C2  failed trace  → --failed-only includes it
//! C3  reminder-only (no matching score) → not exported
//! C4  corrupt JSONL line → skip + skipped_corrupt_lines count
//! C5  --output absent: jsonl_writer receives records, summary_writer receives summary
//! C6  --output FILE: file written, summary_writer receives summary (tests via run_export_with_io)
//! C7  secret-looking value is scrubbed
//! C8  absolute path anonymised to <workdir>
//! C9  payload shape {ts_ms, event, payload} correctly parsed via payload.session_id + payload.turn_index
//! C10 missing required field → record skip + skipped_missing_required_count
//! C11 oversize llm-io.jsonl → session skipped + skipped_oversize_session_count
//! C12 --output FILE with symlink target → rejected

use std::fs;
use std::path::{Path, PathBuf};

use anvil::session::export::{
    ExportConfig, ExportFilter, ExportOutput, ExportScope, MAX_EXPORT_JSONL_BYTES,
    run_export_with_io,
};
use anvil::session::store::SessionSnapshot;

// ---------------------------------------------------------------------------
// Fixture helpers
// ---------------------------------------------------------------------------

fn reminder_payload(session_id: &str, turn: u64) -> String {
    serde_json::json!({
        "ts_ms": 1700000000000u64,
        "event": "agent.reminder.completed",
        "payload": {
            "session_id": session_id,
            "turn_index": turn,
            "task_at_call_time": "fix the bug",
            "precautions_at_call_time": [],
            "feedback_kind": "compile_error",
            "feedback_excerpt": "error: unused variable",
            "added_precautions_text": ["watch unused vars"],
            "added_count": 1
        }
    })
    .to_string()
}

fn score_payload(session_id: &str, turn: u64, build: bool, tests: bool) -> String {
    serde_json::json!({
        "ts_ms": 1700000000001u64,
        "event": "agent.anvil_score.computed",
        "payload": {
            "session_id": session_id,
            "turn_index": turn,
            "score": {
                "build_passed": build,
                "tests_passed": tests,
                "user_visible_artifact": false
            },
            "render_chars": 100,
            "compute_ms": 2
        }
    })
    .to_string()
}

fn make_config<'a>(
    state_root: &'a Path,
    filter: ExportFilter,
    output: ExportOutput,
    max_bytes: usize,
) -> ExportConfig<'a> {
    ExportConfig {
        state_root,
        workspace_root: Path::new("/workspace/project"),
        current_ws: "test_ws",
        scope: ExportScope::AllWorkspaces,
        filter,
        output,
        max_bytes,
    }
}

/// Run export with in-memory line injection, bypassing filesystem.
fn export_lines(
    lines: Vec<String>,
    filter: ExportFilter,
) -> (anvil::session::export::ExportResult, Vec<String>, String) {
    let tmp = tempfile::tempdir().unwrap();
    let config = make_config(
        tmp.path(),
        filter,
        ExportOutput::Stdout,
        MAX_EXPORT_JSONL_BYTES,
    );

    let mut jsonl_out: Vec<String> = Vec::new();
    let mut summary_out = String::new();

    // The reader ignores the path and provides lines from memory
    let mut lr =
        |_path: &Path, cb: &mut dyn FnMut(String) -> Result<(), String>| -> Result<(), String> {
            for line in &lines {
                cb(line.clone())?;
            }
            Ok(())
        };
    let mut jw = |line: &str| {
        jsonl_out.push(line.to_string());
        Ok(())
    };
    let mut sw = |s: &str| {
        summary_out.push_str(s);
        Ok(())
    };

    // Bypass collect_log_paths by routing through run_with_explicit_paths
    let result = run_export_via_fake_reader(
        &config,
        &[PathBuf::from("/fake/path")],
        &mut lr,
        &mut jw,
        &mut sw,
    )
    .unwrap();
    (result, jsonl_out, summary_out)
}

/// Thin wrapper that routes explicit paths through `run_export_with_io`
/// by using the reader to bypass filesystem collection.  We inject a
/// reader that is keyed to a single fake path.
fn run_export_via_fake_reader<FR, FW, FS>(
    config: &ExportConfig<'_>,
    expected_paths: &[PathBuf],
    line_reader: &mut FR,
    jsonl_writer: &mut FW,
    summary_writer: &mut FS,
) -> Result<anvil::session::export::ExportResult, String>
where
    FR: FnMut(&Path, &mut dyn FnMut(String) -> Result<(), String>) -> Result<(), String>,
    FW: FnMut(&str) -> Result<(), String>,
    FS: FnMut(&str) -> Result<(), String>,
{
    // Create a real session dir so collect_log_paths picks up a path.
    // The reader will be called with that path but will ignore it.
    let state_root = config.state_root;
    let session_id = uuid::Uuid::now_v7().to_string();
    let session_dir = state_root.join("sessions").join(&session_id);
    let logs_dir = session_dir.join("logs");
    fs::create_dir_all(&logs_dir).unwrap();

    let snap = SessionSnapshot {
        id: session_id.clone(),
        workspace_key: "test_ws".to_string(),
        ..Default::default()
    };
    fs::write(
        session_dir.join("session.json"),
        serde_json::to_string(&snap).unwrap(),
    )
    .unwrap();

    // Create an empty log file so the oversize check passes (len == 0 ≤ max_bytes)
    let log_path = logs_dir.join("llm-io.jsonl");
    fs::write(&log_path, b"").unwrap();

    // The reader is called for each log path found by collect_log_paths.
    // We wrap it to ignore what path is actually passed (it will be the one
    // we just created), delegating to the injected `line_reader` regardless.
    let mut wrapped_reader =
        |_path: &Path, cb: &mut dyn FnMut(String) -> Result<(), String>| -> Result<(), String> {
            // Redirect to the injected reader — path is irrelevant in tests
            line_reader(
                expected_paths.first().unwrap_or(&PathBuf::from("/fake")),
                cb,
            )
        };

    run_export_with_io(config, &mut wrapped_reader, jsonl_writer, summary_writer)
}

// ---------------------------------------------------------------------------
// C1: success trace → --success-only includes it
// ---------------------------------------------------------------------------

#[test]
fn c1_success_trace_included_in_success_only() {
    let sid = "sess-c1";
    let lines = vec![reminder_payload(sid, 1), score_payload(sid, 1, true, true)];
    let (res, jsonl, _) = export_lines(lines, ExportFilter::SuccessOnly);
    assert_eq!(res.total_records, 1, "expected 1 record");
    assert_eq!(res.success_count, 1);
    let rec: serde_json::Value = serde_json::from_str(&jsonl[0]).unwrap();
    assert_eq!(rec["label"]["success"], true);
    assert_eq!(rec["input"]["task"], "fix the bug");
}

// ---------------------------------------------------------------------------
// C2: failed trace → --failed-only includes it
// ---------------------------------------------------------------------------

#[test]
fn c2_failed_trace_included_in_failed_only() {
    let sid = "sess-c2";
    let lines = vec![
        reminder_payload(sid, 2),
        score_payload(sid, 2, false, false),
    ];
    let (res, jsonl, _) = export_lines(lines, ExportFilter::FailedOnly);
    assert_eq!(res.total_records, 1);
    assert_eq!(res.failed_count, 1);
    let rec: serde_json::Value = serde_json::from_str(&jsonl[0]).unwrap();
    assert_eq!(rec["label"]["success"], false);
}

// ---------------------------------------------------------------------------
// C3: reminder only (no matching score) → not exported
// ---------------------------------------------------------------------------

#[test]
fn c3_reminder_only_no_score_not_exported() {
    let sid = "sess-c3";
    let lines = vec![reminder_payload(sid, 3)];
    let (res, jsonl, _) = export_lines(lines, ExportFilter::All);
    assert_eq!(res.total_records, 0);
    assert!(jsonl.is_empty());
}

// ---------------------------------------------------------------------------
// C4: corrupt JSONL line → skip + count
// ---------------------------------------------------------------------------

#[test]
fn c4_corrupt_line_skipped_and_counted() {
    let sid = "sess-c4";
    let lines = vec![
        "{this is not json".to_string(),
        reminder_payload(sid, 4),
        score_payload(sid, 4, true, true),
    ];
    let (res, _, _) = export_lines(lines, ExportFilter::All);
    assert_eq!(res.skipped_corrupt_lines, 1);
    assert_eq!(res.total_records, 1);
}

// ---------------------------------------------------------------------------
// C5: --output absent: jsonl_writer receives records, summary_writer receives summary
// ---------------------------------------------------------------------------

#[test]
fn c5_no_output_flag_jsonl_to_writer_summary_to_summary_writer() {
    let sid = "sess-c5";
    let lines = vec![reminder_payload(sid, 5), score_payload(sid, 5, true, true)];
    let (res, jsonl, summary) = export_lines(lines, ExportFilter::All);
    assert_eq!(res.total_records, 1);
    assert_eq!(jsonl.len(), 1, "jsonl_writer must have received 1 record");
    assert!(
        !summary.is_empty(),
        "summary_writer must have received summary text"
    );
    // Verify the jsonl record is valid JSON (no summary mixed in)
    let parsed: serde_json::Value =
        serde_json::from_str(&jsonl[0]).expect("jsonl writer output must be pure JSONL");
    assert!(parsed.get("input").is_some());
    assert!(parsed.get("label").is_some());
    assert!(!summary.contains("{"), "summary should not be JSON");
}

// ---------------------------------------------------------------------------
// C6: jsonl_writer receives records + summary_writer receives summary (DI seam)
// ---------------------------------------------------------------------------

#[test]
fn c6_jsonl_writer_and_summary_writer_separate() {
    let tmp = tempfile::tempdir().unwrap();
    let out_file = tmp.path().join("out.jsonl");
    let out_file_ref = out_file.clone();

    let config = make_config(
        tmp.path(),
        ExportFilter::All,
        ExportOutput::Stdout,
        MAX_EXPORT_JSONL_BYTES,
    );

    let session_id = uuid::Uuid::now_v7().to_string();
    let session_dir = tmp.path().join("sessions").join(&session_id);
    let logs_dir = session_dir.join("logs");
    fs::create_dir_all(&logs_dir).unwrap();
    let snap = SessionSnapshot {
        id: session_id.clone(),
        workspace_key: "test_ws".to_string(),
        ..Default::default()
    };
    fs::write(
        session_dir.join("session.json"),
        serde_json::to_string(&snap).unwrap(),
    )
    .unwrap();
    fs::write(logs_dir.join("llm-io.jsonl"), b"").unwrap();

    let sid = "sess-c6";
    let lines = vec![reminder_payload(sid, 6), score_payload(sid, 6, true, true)];
    let lines_ref = &lines;

    // jsonl_writer writes to a file; summary_writer collects to a String.
    // This verifies the two streams stay separate.
    let mut file_out = fs::File::create(&out_file).unwrap();
    let mut summary_out = String::new();

    let mut lr =
        |_path: &Path, cb: &mut dyn FnMut(String) -> Result<(), String>| -> Result<(), String> {
            for line in lines_ref {
                cb(line.clone())?;
            }
            Ok(())
        };
    let mut jw = |line: &str| {
        use std::io::Write;
        writeln!(file_out, "{line}").map_err(|e| format!("write: {e}"))
    };
    let mut sw = |s: &str| {
        summary_out.push_str(s);
        Ok(())
    };

    let res = run_export_with_io(&config, &mut lr, &mut jw, &mut sw).unwrap();
    assert_eq!(res.total_records, 1);

    // File must contain valid JSONL, summary must be text
    let file_content = fs::read_to_string(&out_file_ref).unwrap();
    assert!(!file_content.is_empty(), "file must have JSONL content");
    let parsed: serde_json::Value = serde_json::from_str(file_content.trim()).unwrap();
    assert!(parsed.get("input").is_some());

    assert!(
        !summary_out.is_empty(),
        "summary_writer must receive summary"
    );
    assert!(
        !summary_out.contains("{\"input"),
        "summary must not contain raw JSONL"
    );
}

// ---------------------------------------------------------------------------
// C7: secret-looking value scrubbed
// ---------------------------------------------------------------------------

#[test]
fn c7_secret_scrubbed() {
    let sid = "sess-c7";
    let secret_task = "use TOKEN=ghp_abc123def456 to authenticate";
    let lines = vec![
        serde_json::json!({
            "ts_ms": 0u64, "event": "agent.reminder.completed",
            "payload": {
                "session_id": sid, "turn_index": 7u64,
                "task_at_call_time": secret_task,
                "precautions_at_call_time": [],
                "feedback_kind": "compile_error",
                "feedback_excerpt": "err",
                "added_precautions_text": [],
            }
        })
        .to_string(),
        score_payload(sid, 7, true, true),
    ];
    let (res, jsonl, _) = export_lines(lines, ExportFilter::All);
    assert_eq!(res.total_records, 1);
    let rec: serde_json::Value = serde_json::from_str(&jsonl[0]).unwrap();
    let task = rec["input"]["task"].as_str().unwrap();
    assert!(
        !task.contains("ghp_abc123def456"),
        "token not scrubbed: {task}"
    );
}

// ---------------------------------------------------------------------------
// C8: absolute path anonymised
// ---------------------------------------------------------------------------

#[cfg(unix)]
#[test]
fn c8_absolute_path_anonymized() {
    let sid = "sess-c8";
    let path_task = "error in /workspace/project/src/main.rs";
    let lines = vec![
        serde_json::json!({
            "ts_ms": 0u64, "event": "agent.reminder.completed",
            "payload": {
                "session_id": sid, "turn_index": 8u64,
                "task_at_call_time": path_task,
                "precautions_at_call_time": [],
                "feedback_kind": "compile_error",
                "feedback_excerpt": "err",
                "added_precautions_text": [],
            }
        })
        .to_string(),
        score_payload(sid, 8, true, true),
    ];
    let (res, jsonl, _) = export_lines(lines, ExportFilter::All);
    assert_eq!(res.total_records, 1);
    let rec: serde_json::Value = serde_json::from_str(&jsonl[0]).unwrap();
    let task = rec["input"]["task"].as_str().unwrap();
    assert!(task.contains("<workdir>"), "path not anonymized: {task}");
    assert!(
        !task.contains("/workspace/project"),
        "original path still present: {task}"
    );
}

// ---------------------------------------------------------------------------
// C9: fixture uses {ts_ms, event, payload} shape and payload.session_id / payload.turn_index join
// ---------------------------------------------------------------------------

#[test]
fn c9_payload_shape_and_join_key_correct() {
    // Two sessions with same turn_index — join must be per (session_id, turn_index)
    let lines = vec![
        reminder_payload("sess-A", 1),
        reminder_payload("sess-B", 1),
        score_payload("sess-A", 1, true, true),
        score_payload("sess-B", 1, false, false),
    ];
    let (res, jsonl, _) = export_lines(lines, ExportFilter::All);
    assert_eq!(res.total_records, 2, "both sessions should export");
    assert_eq!(jsonl.len(), 2);
    let successes: Vec<bool> = jsonl
        .iter()
        .map(|l| {
            let v: serde_json::Value = serde_json::from_str(l).unwrap();
            v["label"]["success"].as_bool().unwrap()
        })
        .collect();
    assert!(successes.contains(&true));
    assert!(successes.contains(&false));
}

// ---------------------------------------------------------------------------
// C10: missing required field → skip + skipped_missing_required_count
// ---------------------------------------------------------------------------

#[test]
fn c10_missing_required_field_skipped() {
    let sid = "sess-c10";
    // Reminder record missing `task_at_call_time`
    let bad_reminder = serde_json::json!({
        "ts_ms": 0u64, "event": "agent.reminder.completed",
        "payload": {
            "session_id": sid, "turn_index": 10u64,
            // task_at_call_time missing
            "precautions_at_call_time": [],
            "feedback_kind": "compile_error",
            "feedback_excerpt": "err",
            "added_precautions_text": [],
        }
    })
    .to_string();
    let lines = vec![bad_reminder, score_payload(sid, 10, true, true)];
    let (res, jsonl, _) = export_lines(lines, ExportFilter::All);
    assert_eq!(
        res.total_records, 0,
        "record with missing field must be skipped"
    );
    assert!(jsonl.is_empty());
    assert_eq!(res.skipped_missing_required_count, 1);
}

// ---------------------------------------------------------------------------
// C11: oversize llm-io.jsonl → session skipped + skipped_oversize_session_count
// ---------------------------------------------------------------------------

#[test]
fn c11_oversize_session_skipped() {
    let tmp = tempfile::tempdir().unwrap();

    // Create a real session dir with a log file that exceeds max_bytes
    let session_id = uuid::Uuid::now_v7().to_string();
    let session_dir = tmp.path().join("sessions").join(&session_id);
    let logs_dir = session_dir.join("logs");
    fs::create_dir_all(&logs_dir).unwrap();
    let snap = SessionSnapshot {
        id: session_id.clone(),
        workspace_key: "test_ws".to_string(),
        ..Default::default()
    };
    fs::write(
        session_dir.join("session.json"),
        serde_json::to_string(&snap).unwrap(),
    )
    .unwrap();
    let log_path = logs_dir.join("llm-io.jsonl");
    fs::write(&log_path, b"x").unwrap(); // 1 byte

    let config = make_config(
        tmp.path(),
        ExportFilter::All,
        ExportOutput::Stdout,
        0, /* max_bytes=0 → any non-empty file is oversize */
    );

    let mut reader_called = false;
    let mut lr = |_: &Path, _: &mut dyn FnMut(String) -> Result<(), String>| {
        reader_called = true;
        Ok(())
    };
    let mut jw = |_: &str| Ok(());
    let mut sw = |_: &str| Ok(());

    let res = run_export_with_io(&config, &mut lr, &mut jw, &mut sw).unwrap();
    assert_eq!(res.skipped_oversize_session_count, 1);
    assert!(
        !reader_called,
        "reader must not be called for oversize session"
    );
}

// ---------------------------------------------------------------------------
// C12: --output FILE with symlink → rejected
// ---------------------------------------------------------------------------

#[cfg(unix)]
#[test]
fn c12_output_file_symlink_rejected() {
    use anvil::session::export::run_export;

    let tmp = tempfile::tempdir().unwrap();
    let target = tmp.path().join("real.jsonl");
    let link = tmp.path().join("link.jsonl");
    fs::write(&target, b"").unwrap();
    std::os::unix::fs::symlink(&target, &link).unwrap();

    // Build a minimal state_root so dispatch doesn't fail before hitting the file check
    let session_id = uuid::Uuid::now_v7().to_string();
    let session_dir = tmp.path().join("sessions").join(&session_id);
    fs::create_dir_all(session_dir.join("logs")).unwrap();
    let snap = SessionSnapshot {
        id: session_id,
        workspace_key: "test_ws".to_string(),
        ..Default::default()
    };
    fs::write(
        session_dir.join("session.json"),
        serde_json::to_string(&snap).unwrap(),
    )
    .unwrap();

    let config = ExportConfig {
        state_root: tmp.path(),
        workspace_root: tmp.path(),
        current_ws: "test_ws",
        scope: ExportScope::AllWorkspaces,
        filter: ExportFilter::All,
        output: ExportOutput::File(link),
        max_bytes: MAX_EXPORT_JSONL_BYTES,
    };
    let result = run_export(&config);
    assert!(result.is_err(), "symlink output must be rejected");
    let err = result.unwrap_err();
    assert!(err.contains("symlink"), "error must mention symlink: {err}");
}
