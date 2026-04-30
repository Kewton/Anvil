//! Fine-tuning dataset export (Issue #473).
//!
//! Reads `llm-io.jsonl` from session directories, joins
//! `agent.reminder.completed` and `agent.anvil_score.computed` by
//! `(session_id, turn_index)`, scrubs secrets / absolute paths, and emits
//! training-ready JSONL records.
//!
//! Layer constraint: this module must not import from `crate::agent`.

use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use regex::Regex;
use serde_json::{Value, json};

use crate::session::discovery::iter_session_dirs;
use crate::session::feedback::mask_secrets;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

pub const MAX_EXPORT_JSONL_BYTES: usize = 64 * 1024 * 1024; // 64 MiB

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

pub struct ExportConfig<'a> {
    pub state_root: &'a Path,
    pub workspace_root: &'a Path,
    pub current_ws: &'a str,
    pub scope: ExportScope,
    pub filter: ExportFilter,
    pub output: ExportOutput,
    pub max_bytes: usize,
}

pub enum ExportScope {
    CurrentWorkspace,
    AllWorkspaces,
    Session(String),
}

pub enum ExportFilter {
    All,
    SuccessOnly,
    FailedOnly,
}

pub enum ExportOutput {
    Stdout,
    File(PathBuf),
}

#[derive(Debug, Default)]
pub struct ExportResult {
    pub total_records: usize,
    pub success_count: usize,
    pub failed_count: usize,
    pub skipped_corrupt_lines: usize,
    pub skipped_missing_required_count: usize,
    pub skipped_oversize_session_count: usize,
    pub scrubbed_field_count: usize,
    pub path_anonymized_count: usize,
}

// ---------------------------------------------------------------------------
// Path anonymization (unix only; no-op on other platforms)
// ---------------------------------------------------------------------------

fn home_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"/(?:home|Users)/[^/]+/").expect("valid static home-dir regex"))
}

#[cfg(unix)]
pub(crate) fn anonymize_paths(text: &str, work_root: &Path) -> String {
    let work_root_str = work_root.to_string_lossy();
    let s1 = if !work_root_str.is_empty() {
        text.replace(work_root_str.as_ref(), "<workdir>")
    } else {
        text.to_string()
    };
    home_pattern().replace_all(&s1, "~/").into_owned()
}

#[cfg(not(unix))]
pub(crate) fn anonymize_paths(text: &str, _work_root: &Path) -> String {
    text.to_string()
}

// ---------------------------------------------------------------------------
// Scrub helpers that update counters
// ---------------------------------------------------------------------------

fn scrub_field(
    text: &str,
    work_root: &Path,
    scrubbed_count: &mut usize,
    path_anon_count: &mut usize,
) -> String {
    let anon = anonymize_paths(text, work_root);
    if anon != text {
        *path_anon_count += 1;
    }
    let out = mask_secrets(&anon);
    if out != text {
        *scrubbed_count += 1;
    }
    out
}

fn scrub_string_vec(
    v: &[String],
    work_root: &Path,
    scrubbed_count: &mut usize,
    path_anon_count: &mut usize,
) -> Vec<String> {
    v.iter()
        .map(|s| scrub_field(s, work_root, scrubbed_count, path_anon_count))
        .collect()
}

// ---------------------------------------------------------------------------
// Internal record types for join
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct ReminderRecord {
    session_id: String,
    turn_index: u64,
    task_at_call_time: String,
    precautions_at_call_time: Vec<String>,
    feedback_kind: String,
    feedback_excerpt: String,
    added_precautions_text: Vec<String>,
}

#[derive(Debug)]
struct ScoreRecord {
    session_id: String,
    turn_index: u64,
    score: Value,
}

type JoinKey = (String, u64); // (session_id, turn_index)

// ---------------------------------------------------------------------------
// Parsing helpers
// ---------------------------------------------------------------------------

fn parse_reminder_record(payload: &Value) -> Option<ReminderRecord> {
    let session_id = payload.get("session_id")?.as_str()?.to_string();
    let turn_index = payload.get("turn_index")?.as_u64()?;
    let task_at_call_time = payload.get("task_at_call_time")?.as_str()?.to_string();
    let precautions_at_call_time = payload
        .get("precautions_at_call_time")?
        .as_array()?
        .iter()
        .map(|v| Some(v.as_str()?.to_string()))
        .collect::<Option<Vec<String>>>()?;
    let feedback_kind = payload.get("feedback_kind")?.as_str()?.to_string();
    let feedback_excerpt = payload.get("feedback_excerpt")?.as_str()?.to_string();
    let added_precautions_text = payload
        .get("added_precautions_text")?
        .as_array()?
        .iter()
        .map(|v| Some(v.as_str()?.to_string()))
        .collect::<Option<Vec<String>>>()?;
    Some(ReminderRecord {
        session_id,
        turn_index,
        task_at_call_time,
        precautions_at_call_time,
        feedback_kind,
        feedback_excerpt,
        added_precautions_text,
    })
}

fn parse_score_record(payload: &Value) -> Option<ScoreRecord> {
    let session_id = payload.get("session_id")?.as_str()?.to_string();
    let turn_index = payload.get("turn_index")?.as_u64()?;
    let score = payload.get("score")?.clone();
    if !score.is_object() {
        return None;
    }
    Some(ScoreRecord {
        session_id,
        turn_index,
        score,
    })
}

fn is_success(score: &Value) -> bool {
    score
        .get("build_passed")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
        && score
            .get("tests_passed")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
}

// ---------------------------------------------------------------------------
// Build output record from a joined (reminder, score) pair
// ---------------------------------------------------------------------------

fn build_output_record(
    reminder: &ReminderRecord,
    score: &ScoreRecord,
    work_root: &Path,
    result: &mut ExportResult,
) -> Value {
    let task = scrub_field(
        &reminder.task_at_call_time,
        work_root,
        &mut result.scrubbed_field_count,
        &mut result.path_anonymized_count,
    );
    let active_precautions: Vec<Value> = scrub_string_vec(
        &reminder.precautions_at_call_time,
        work_root,
        &mut result.scrubbed_field_count,
        &mut result.path_anonymized_count,
    )
    .into_iter()
    .map(Value::String)
    .collect();
    let feedback_excerpt = scrub_field(
        &reminder.feedback_excerpt,
        work_root,
        &mut result.scrubbed_field_count,
        &mut result.path_anonymized_count,
    );
    let added_precautions: Vec<Value> = scrub_string_vec(
        &reminder.added_precautions_text,
        work_root,
        &mut result.scrubbed_field_count,
        &mut result.path_anonymized_count,
    )
    .into_iter()
    .map(Value::String)
    .collect();

    let success = is_success(&score.score);
    if success {
        result.success_count += 1;
    } else {
        result.failed_count += 1;
    }
    result.total_records += 1;

    json!({
        "input": {
            "task": task,
            "active_precautions": active_precautions,
            "feedback_kind": reminder.feedback_kind,
            "feedback_excerpt": feedback_excerpt,
        },
        "output": {
            "added_precautions": added_precautions,
        },
        "label": {
            "next_anvil_score": score.score,
            "success": success,
        }
    })
}

// ---------------------------------------------------------------------------
// JSONL parsing — consumes lines from the reader DI seam
// ---------------------------------------------------------------------------

/// Parse lines from `line_reader` into `reminder_map` / `score_map`.
///
/// The oversize check and file-existence check are **not** done here; the
/// caller must guard those before calling this function. The reader DI seam
/// is responsible for opening the actual file (or providing fixture lines in
/// tests).
fn parse_log_lines<FR>(
    log_path: &Path,
    line_reader: &mut FR,
    reminder_map: &mut HashMap<JoinKey, ReminderRecord>,
    score_map: &mut HashMap<JoinKey, ScoreRecord>,
    corrupt_count: &mut usize,
    missing_count: &mut usize,
) -> Result<(), String>
where
    FR: FnMut(&Path, &mut dyn FnMut(String) -> Result<(), String>) -> Result<(), String>,
{
    // Collect parsed events first to avoid borrow-checker conflicts between
    // the line-by-line callback and the two output maps.
    let mut reminder_recs: Vec<ReminderRecord> = Vec::new();
    let mut score_recs: Vec<ScoreRecord> = Vec::new();
    let mut local_corrupt: usize = 0;
    let mut local_missing: usize = 0;

    line_reader(log_path, &mut |line: String| {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return Ok(());
        }
        let Ok(value): Result<Value, _> = serde_json::from_str(trimmed) else {
            local_corrupt += 1;
            return Ok(());
        };
        let Some(event) = value.get("event").and_then(|v| v.as_str()) else {
            local_corrupt += 1;
            return Ok(());
        };
        let Some(payload) = value.get("payload") else {
            local_corrupt += 1;
            return Ok(());
        };
        match event {
            "agent.reminder.completed" => {
                if let Some(rec) = parse_reminder_record(payload) {
                    reminder_recs.push(rec);
                } else {
                    local_missing += 1;
                }
            }
            "agent.anvil_score.computed" => {
                if let Some(rec) = parse_score_record(payload) {
                    score_recs.push(rec);
                } else {
                    local_missing += 1;
                }
            }
            _ => {}
        }
        Ok(())
    })?;

    *corrupt_count += local_corrupt;
    *missing_count += local_missing;

    for rec in reminder_recs {
        let key = (rec.session_id.clone(), rec.turn_index);
        reminder_map.insert(key, rec);
    }
    for rec in score_recs {
        let key = (rec.session_id.clone(), rec.turn_index);
        score_map.insert(key, rec);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Output file helpers
// ---------------------------------------------------------------------------

fn open_output_file(path: &Path) -> Result<File, String> {
    match path.symlink_metadata() {
        Ok(meta) if meta.file_type().is_symlink() => {
            return Err(format!("output path is a symlink: {}", path.display()));
        }
        Ok(meta) if !meta.is_file() => {
            return Err(format!(
                "output path exists and is not a regular file: {}",
                path.display()
            ));
        }
        _ => {}
    }

    let mut opts = OpenOptions::new();
    opts.write(true).create(true).truncate(true);

    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }

    opts.open(path)
        .map_err(|e| format!("failed to open output file {}: {e}", path.display()))
}

// ---------------------------------------------------------------------------
// Collect log paths from config scope
// ---------------------------------------------------------------------------

fn collect_log_paths(config: &ExportConfig<'_>) -> Vec<PathBuf> {
    match &config.scope {
        ExportScope::Session(session_id) => {
            vec![
                config
                    .state_root
                    .join("sessions")
                    .join(session_id)
                    .join("logs")
                    .join("llm-io.jsonl"),
            ]
        }
        ExportScope::CurrentWorkspace => iter_session_dirs(config.state_root)
            .into_iter()
            .filter(|e| e.snapshot.workspace_key == config.current_ws)
            .map(|e| e.dir.join("logs").join("llm-io.jsonl"))
            .collect(),
        ExportScope::AllWorkspaces => iter_session_dirs(config.state_root)
            .into_iter()
            .map(|e| e.dir.join("logs").join("llm-io.jsonl"))
            .collect(),
    }
}

// ---------------------------------------------------------------------------
// Summary formatting
// ---------------------------------------------------------------------------

fn format_summary(result: &ExportResult) -> String {
    format!(
        "Exported {} records ({} success, {} failed). \
        Skipped: {} corrupt lines, {} missing-field records, {} oversize sessions. \
        Scrubbed {} fields ({} path-anonymized).\n",
        result.total_records,
        result.success_count,
        result.failed_count,
        result.skipped_corrupt_lines,
        result.skipped_missing_required_count,
        result.skipped_oversize_session_count,
        result.scrubbed_field_count,
        result.path_anonymized_count,
    )
}

// ---------------------------------------------------------------------------
// Core join + emit logic shared between production and tests
// ---------------------------------------------------------------------------

fn run_join_and_emit<FW, FS>(
    reminder_map: HashMap<JoinKey, ReminderRecord>,
    score_map: HashMap<JoinKey, ScoreRecord>,
    config: &ExportConfig<'_>,
    jsonl_writer: &mut FW,
    summary_writer: &mut FS,
    result: &mut ExportResult,
) -> Result<(), String>
where
    FW: FnMut(&str) -> Result<(), String>,
    FS: FnMut(&str) -> Result<(), String>,
{
    let mut keys: Vec<JoinKey> = reminder_map
        .keys()
        .filter(|k| score_map.contains_key(k))
        .cloned()
        .collect();
    keys.sort();

    for key in &keys {
        let reminder = &reminder_map[key];
        let score = &score_map[key];

        let success = is_success(&score.score);
        let include = match &config.filter {
            ExportFilter::All => true,
            ExportFilter::SuccessOnly => success,
            ExportFilter::FailedOnly => !success,
        };
        if !include {
            continue;
        }

        let record = build_output_record(reminder, score, config.workspace_root, result);
        let line = serde_json::to_string(&record).map_err(|e| format!("serialize error: {e}"))?;
        jsonl_writer(&line)?;
    }

    let summary = format_summary(result);
    summary_writer(&summary)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Production export: opens `llm-io.jsonl` files and writes to stdout or a
/// file depending on `config.output`.
pub fn run_export(config: &ExportConfig<'_>) -> Result<ExportResult, String> {
    let mut line_reader = |path: &Path,
                           cb: &mut dyn FnMut(String) -> Result<(), String>|
     -> Result<(), String> {
        let file = match File::open(path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(format!("failed to open {}: {e}", path.display())),
        };
        let reader = BufReader::new(file);
        for line in reader.lines() {
            let line = line.map_err(|e| format!("I/O error reading {}: {e}", path.display()))?;
            cb(line)?;
        }
        Ok(())
    };

    match &config.output {
        ExportOutput::File(path) => {
            let mut file = open_output_file(path)?;
            let mut jsonl_writer =
                |line: &str| writeln!(file, "{line}").map_err(|e| format!("write error: {e}"));
            let mut summary_writer = |s: &str| {
                print!("{s}");
                Ok(())
            };
            run_export_with_io(
                config,
                &mut line_reader,
                &mut jsonl_writer,
                &mut summary_writer,
            )
        }
        ExportOutput::Stdout => {
            let mut jsonl_writer = |line: &str| {
                println!("{line}");
                Ok(())
            };
            let mut summary_writer = |s: &str| {
                eprint!("{s}");
                Ok(())
            };
            run_export_with_io(
                config,
                &mut line_reader,
                &mut jsonl_writer,
                &mut summary_writer,
            )
        }
    }
}

/// DI-seam variant for testing. Caller injects:
/// - `line_reader`: called for each session log path; calls `cb` once per
///   line (may be a fixture provider in tests, or a real file reader)
/// - `jsonl_writer`: receives each serialized output record
/// - `summary_writer`: receives the final summary text
pub fn run_export_with_io<FR, FW, FS>(
    config: &ExportConfig<'_>,
    line_reader: &mut FR,
    jsonl_writer: &mut FW,
    summary_writer: &mut FS,
) -> Result<ExportResult, String>
where
    FR: FnMut(&Path, &mut dyn FnMut(String) -> Result<(), String>) -> Result<(), String>,
    FW: FnMut(&str) -> Result<(), String>,
    FS: FnMut(&str) -> Result<(), String>,
{
    let mut result = ExportResult::default();
    let mut reminder_map: HashMap<JoinKey, ReminderRecord> = HashMap::new();
    let mut score_map: HashMap<JoinKey, ScoreRecord> = HashMap::new();

    let log_paths = collect_log_paths(config);

    for log_path in &log_paths {
        // Oversize guard: only applies when the file actually exists.
        // Missing files are handled gracefully by the reader itself.
        if fs::metadata(log_path).is_ok_and(|m| m.len() > config.max_bytes as u64) {
            result.skipped_oversize_session_count += 1;
            continue;
        }

        parse_log_lines(
            log_path,
            line_reader,
            &mut reminder_map,
            &mut score_map,
            &mut result.skipped_corrupt_lines,
            &mut result.skipped_missing_required_count,
        )?;
    }

    run_join_and_emit(
        reminder_map,
        score_map,
        config,
        jsonl_writer,
        summary_writer,
        &mut result,
    )
    .map(|()| result)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_config<'a>(
        state_root: &'a Path,
        scope: ExportScope,
        filter: ExportFilter,
    ) -> ExportConfig<'a> {
        ExportConfig {
            state_root,
            workspace_root: Path::new("/workspace/proj"),
            current_ws: "test_ws",
            scope,
            filter,
            output: ExportOutput::Stdout,
            max_bytes: MAX_EXPORT_JSONL_BYTES,
        }
    }

    fn reminder_line(session_id: &str, turn: u64) -> String {
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
                "added_count": 1,
                "model": "qwen",
            }
        })
        .to_string()
    }

    fn score_line(session_id: &str, turn: u64, build: bool, tests: bool) -> String {
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

    /// Inject `lines` directly via the DI seam, bypassing filesystem.
    fn run_with_lines(
        lines: Vec<String>,
        scope: ExportScope,
        filter: ExportFilter,
    ) -> (ExportResult, Vec<String>, String) {
        let tmp = tempfile::tempdir().unwrap();
        let config = make_config(tmp.path(), scope, filter);

        let mut jsonl_out: Vec<String> = Vec::new();
        let mut summary_out = String::new();

        let fake_path = PathBuf::from("/nonexistent/fake.jsonl");

        let mut lr = |_path: &Path,
                      cb: &mut dyn FnMut(String) -> Result<(), String>|
         -> Result<(), String> {
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

        let result =
            run_with_explicit_paths(&config, &[fake_path], &mut lr, &mut jw, &mut sw).unwrap();
        (result, jsonl_out, summary_out)
    }

    /// Like `run_export_with_io` but uses explicit log paths instead of
    /// `collect_log_paths`. Used by unit tests to bypass the filesystem.
    fn run_with_explicit_paths<FR, FW, FS>(
        config: &ExportConfig<'_>,
        log_paths: &[PathBuf],
        line_reader: &mut FR,
        jsonl_writer: &mut FW,
        summary_writer: &mut FS,
    ) -> Result<ExportResult, String>
    where
        FR: FnMut(&Path, &mut dyn FnMut(String) -> Result<(), String>) -> Result<(), String>,
        FW: FnMut(&str) -> Result<(), String>,
        FS: FnMut(&str) -> Result<(), String>,
    {
        let mut result = ExportResult::default();
        let mut reminder_map: HashMap<JoinKey, ReminderRecord> = HashMap::new();
        let mut score_map: HashMap<JoinKey, ScoreRecord> = HashMap::new();

        for log_path in log_paths {
            parse_log_lines(
                log_path,
                line_reader,
                &mut reminder_map,
                &mut score_map,
                &mut result.skipped_corrupt_lines,
                &mut result.skipped_missing_required_count,
            )?;
        }

        run_join_and_emit(
            reminder_map,
            score_map,
            config,
            jsonl_writer,
            summary_writer,
            &mut result,
        )
        .map(|()| result)
    }

    #[test]
    fn success_trace_included_in_success_only() {
        let sid = "sess-a";
        let lines = vec![reminder_line(sid, 1), score_line(sid, 1, true, true)];
        let (res, jsonl, _) =
            run_with_lines(lines, ExportScope::AllWorkspaces, ExportFilter::SuccessOnly);
        assert_eq!(res.total_records, 1);
        assert_eq!(res.success_count, 1);
        assert_eq!(jsonl.len(), 1);
        let rec: Value = serde_json::from_str(&jsonl[0]).unwrap();
        assert_eq!(rec["label"]["success"], true);
    }

    #[test]
    fn failed_trace_included_in_failed_only() {
        let sid = "sess-b";
        let lines = vec![reminder_line(sid, 2), score_line(sid, 2, false, false)];
        let (res, jsonl, _) =
            run_with_lines(lines, ExportScope::AllWorkspaces, ExportFilter::FailedOnly);
        assert_eq!(res.total_records, 1);
        assert_eq!(res.failed_count, 1);
        assert_eq!(jsonl.len(), 1);
        let rec: Value = serde_json::from_str(&jsonl[0]).unwrap();
        assert_eq!(rec["label"]["success"], false);
    }

    #[test]
    fn score_only_no_reminder_skipped() {
        let sid = "sess-c";
        let lines = vec![score_line(sid, 3, true, true)];
        let (res, jsonl, _) = run_with_lines(lines, ExportScope::AllWorkspaces, ExportFilter::All);
        assert_eq!(res.total_records, 0);
        assert_eq!(jsonl.len(), 0);
    }

    #[test]
    fn corrupt_line_skipped_and_counted() {
        let sid = "sess-d";
        let lines = vec![
            "not valid json!!!".to_string(),
            reminder_line(sid, 4),
            score_line(sid, 4, true, true),
        ];
        let (res, _, _) = run_with_lines(lines, ExportScope::AllWorkspaces, ExportFilter::All);
        assert_eq!(res.skipped_corrupt_lines, 1);
        assert_eq!(res.total_records, 1);
    }

    #[test]
    fn summary_writer_receives_summary() {
        let sid = "sess-e";
        let lines = vec![reminder_line(sid, 5), score_line(sid, 5, true, true)];
        let (_, _, summary) = run_with_lines(lines, ExportScope::AllWorkspaces, ExportFilter::All);
        assert!(summary.contains("Exported"), "summary: {summary}");
    }

    #[test]
    fn secret_scrubbed_in_output() {
        let sid = "sess-secret";
        let secret_task = "fix with ANTHROPIC_API_KEY=sk-ant-secret123 present";
        let lines = vec![
            serde_json::json!({
                "ts_ms": 0u64, "event": "agent.reminder.completed",
                "payload": {
                    "session_id": sid, "turn_index": 1u64,
                    "task_at_call_time": secret_task,
                    "precautions_at_call_time": [],
                    "feedback_kind": "compile_error",
                    "feedback_excerpt": "err",
                    "added_precautions_text": [],
                }
            })
            .to_string(),
            score_line(sid, 1, true, true),
        ];
        let (res, jsonl, _) = run_with_lines(lines, ExportScope::AllWorkspaces, ExportFilter::All);
        assert_eq!(res.total_records, 1);
        let rec: Value = serde_json::from_str(&jsonl[0]).unwrap();
        let task = rec["input"]["task"].as_str().unwrap();
        assert!(
            !task.contains("sk-ant-secret123"),
            "secret not scrubbed: {task}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn path_anonymized_in_output() {
        let work_root = Path::new("/workspace/proj");
        let text = "error in /workspace/proj/src/main.rs";
        let out = anonymize_paths(text, work_root);
        assert!(out.contains("<workdir>"), "got: {out}");
        assert!(
            !out.contains("/workspace/proj"),
            "path still present: {out}"
        );
    }

    #[test]
    fn correct_join_by_session_and_turn_index() {
        let lines = vec![
            reminder_line("s1", 1),
            reminder_line("s2", 1),
            score_line("s1", 1, true, true),
            score_line("s2", 1, false, false),
        ];
        let (res, jsonl, _) = run_with_lines(lines, ExportScope::AllWorkspaces, ExportFilter::All);
        assert_eq!(res.total_records, 2);
        assert_eq!(jsonl.len(), 2);
    }

    #[test]
    fn oversize_session_skipped() {
        use crate::session::store::SessionSnapshot;

        let tmp = tempfile::tempdir().unwrap();
        let state_root = tmp.path();

        // Create a fake session directory with an oversized log file
        let session_id = uuid::Uuid::now_v7().to_string();
        let session_dir = state_root.join("sessions").join(&session_id);
        let logs_dir = session_dir.join("logs");
        fs::create_dir_all(&logs_dir).unwrap();

        let snap = SessionSnapshot {
            id: session_id.clone(),
            workspace_key: "ws".to_string(),
            ..Default::default()
        };
        fs::write(
            session_dir.join("session.json"),
            serde_json::to_string(&snap).unwrap(),
        )
        .unwrap();

        let log_path = logs_dir.join("llm-io.jsonl");
        fs::write(&log_path, b"x").unwrap();

        let config = ExportConfig {
            state_root,
            workspace_root: Path::new("/workspace"),
            current_ws: "ws",
            scope: ExportScope::AllWorkspaces,
            filter: ExportFilter::All,
            output: ExportOutput::Stdout,
            max_bytes: 0, // force oversize for any non-empty file
        };

        let mut called = false;
        let mut lr = |_: &Path, _: &mut dyn FnMut(String) -> Result<(), String>| {
            called = true;
            Ok(())
        };
        let mut jw = |_: &str| Ok(());
        let mut sw = |_: &str| Ok(());
        let res = run_export_with_io(&config, &mut lr, &mut jw, &mut sw).unwrap();
        assert_eq!(res.skipped_oversize_session_count, 1);
        assert!(!called, "reader should not be called for oversize files");
    }

    #[cfg(unix)]
    #[test]
    fn output_file_rejects_symlink() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("real.jsonl");
        let link = tmp.path().join("link.jsonl");
        fs::write(&target, b"").unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(open_output_file(&link).is_err());
    }
}
