//! Structured job report persistence (Issue #666).
//!
//! Session-layer DTO + JSONL append/read for `agent.{artifact_completion,
//! verification,repair,memory}.report` events. Designed to be a bounded
//! summary stream — detailed turn-by-turn observability stays in the
//! transient `llm-io.jsonl`; `job-reports.jsonl` is the persisted
//! bounded summary that UAT, regression tests, and future Issues
//! (#659/#661/#662/#663/#667) can consume.
//!
//! ## Invariants (Issue #666 design)
//!
//! - **DR3-002 (unidirectional dependency)**: This module does NOT import
//!   `crate::agent::*` or `crate::photon::*`. The wire DTO is owned by
//!   the session layer; agent-layer reports are converted to
//!   `serde_json::Value` envelopes before being passed here.
//! - **DR1-001 (no `chrono`)**: time is recorded as Unix-epoch
//!   milliseconds via `std::time::SystemTime`, mirroring the convention at
//!   `src/session/tmp_tests.rs:21` and `src/session/case_photon_bridge.rs`.
//! - **DR4-001 (final redaction defence)**: callers MUST pass an envelope
//!   that has already traversed `logging::mask_payload_inplace`. This
//!   module does NOT redact — it persists a frozen wire value.
//! - **DR4-002 (read-side DoS defence)**: `read_job_reports` enforces
//!   `MAX_JOB_REPORT_LINE_BYTES` per line and `MAX_JOB_REPORT_FILE_BYTES`
//!   total to prevent allocation DoS from a hostile / corrupted file.
//! - **DR4-003 (path traversal defence)**: the `job-reports.jsonl` path
//!   resolves through `SessionStore::session_id()`, which is constrained
//!   to a UUID by the resume path; `read_job_reports`/`append_job_report`
//!   refuse session_ids containing `/`, `\\`, `..`, or NUL.

use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::session::store::SessionStore;

/// Envelope-level schema version for `StructuredJobReportRecord`.
///
/// Distinct from per-Report `PAYLOAD_SCHEMA_VERSION` defined on the
/// agent-layer `JobReport` trait — that one is per-payload, this one is
/// per-record (top-level fields of `StructuredJobReportRecord`). Bump only
/// when the top-level fields change (e.g. removing/typing
/// `recorded_at_unix_ms`).
pub(crate) const JOB_REPORT_SCHEMA_VERSION: u32 = 1;

/// Hard line cap for a single JSONL record on read.
///
/// 16 KiB is comfortably above the 8 KiB envelope budget (Section 11 of
/// the design) and the 4 KiB safe-stop envelope, leaving headroom for
/// transport-layer escaping. Lines exceeding this are skipped with a
/// WARN; the agent loop continues (best-effort read, DR1-009 / DR4-002).
pub(crate) const MAX_JOB_REPORT_LINE_BYTES: usize = 16 * 1024;

/// Hard file cap for `read_job_reports`.
///
/// 16 MiB sits an order of magnitude below
/// `session::export::MAX_EXPORT_JSONL_BYTES` (64 MiB), giving the typical
/// 32 KiB/turn × 200 turn projection (~6.4 MiB) ~2.5× headroom while
/// blocking pathological growth at read time.
#[allow(dead_code)] // read API for future Issues (#659/#661/#662/#663/#667 consumers).
pub(crate) const MAX_JOB_REPORT_FILE_BYTES: u64 = 16 * 1024 * 1024;

/// Hard cap on number of records returned by `read_job_reports`.
///
/// CB-003 fix: prevents unbounded Vec growth from a file packed with
/// small valid records. 8 KiB × N >= 16 MiB cap puts the natural upper
/// bound at ~2048 records; 4096 leaves headroom for short records while
/// still bounding allocation.
#[allow(dead_code)]
pub(crate) const MAX_JOB_REPORT_RECORDS_READ: usize = 4096;

/// One persisted snapshot of a single `agent.*.report` event.
///
/// `envelope` carries the already-redacted payload — see the module-level
/// DR4-001 invariant. `schema_version` is `JOB_REPORT_SCHEMA_VERSION` for
/// records produced by the current binary; `#[serde(default)]` lets us
/// tolerate records emitted by an older binary that did not yet have the
/// field (defaults to `0` → skipped by the reader, DR1-005 mismatch
/// policy).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StructuredJobReportRecord {
    #[serde(default)]
    pub schema_version: u32,
    pub recorded_at_unix_ms: u64,
    pub turn_index: u64,
    pub event_name: String,
    pub envelope: serde_json::Value,
}

impl StructuredJobReportRecord {
    /// Build with the current binary's schema version.
    pub fn new(
        recorded_at_unix_ms: u64,
        turn_index: u64,
        event_name: String,
        envelope: serde_json::Value,
    ) -> Self {
        Self {
            schema_version: JOB_REPORT_SCHEMA_VERSION,
            recorded_at_unix_ms,
            turn_index,
            event_name,
            envelope,
        }
    }
}

/// Reject session_ids that could escape the sessions directory.
///
/// UUIDs from the resume path will always pass; hostile inputs that
/// contain a separator, `..`, or NUL fail closed. Returning `false` is
/// always a hard refusal — callers do not get partial state.
fn is_safe_session_id(session_id: &str) -> bool {
    if session_id.is_empty() {
        return false;
    }
    !session_id
        .chars()
        .any(|c| c == '/' || c == '\\' || c == '\0')
        && !session_id.contains("..")
}

impl SessionStore {
    /// Path to the per-session `job-reports.jsonl` file.
    ///
    /// `None` if the session id would escape the sessions directory.
    pub(crate) fn job_reports_path(&self) -> Option<PathBuf> {
        if !is_safe_session_id(self.session_id()) {
            return None;
        }
        Some(
            self.state_root()
                .join("sessions")
                .join(self.session_id())
                .join("job-reports.jsonl"),
        )
    }

    /// Append a record as a single JSONL line.
    ///
    /// Best-effort from the caller's perspective: callers (turn.rs)
    /// downgrade an `Err` here to `tracing::error!` and continue.
    ///
    /// CB-002 fix (storage-boundary final defence): the record envelope
    /// is cloned and re-passed through `mask_payload_inplace` before
    /// serialization so any future crate-internal caller that hands us
    /// an un-redacted envelope still produces a redacted JSONL line.
    /// This is defence-in-depth — the production `turn.rs` caller
    /// already masks at `record_job_report` time.
    pub(crate) fn append_job_report(
        &self,
        record: &StructuredJobReportRecord,
    ) -> Result<(), String> {
        let path = self
            .job_reports_path()
            .ok_or_else(|| "session id unsafe for job-reports path".to_string())?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("failed to create {}: {e}", parent.display()))?;
        }
        let mut redacted = record.clone();
        crate::logging::mask_payload_inplace(&mut redacted.envelope);
        let mut line = serde_json::to_string(&redacted)
            .map_err(|e| format!("failed to serialize job report: {e}"))?;
        // Hard cap on write-side. Per design Section 11-1 we never silently
        // emit raw oversized envelopes — `enforce_bounds` upstream is the
        // primary defence, but this is the last line of defence at the
        // persistence boundary (defence in depth).
        if line.len() > MAX_JOB_REPORT_LINE_BYTES {
            return Err(format!(
                "job report line exceeds {} bytes",
                MAX_JOB_REPORT_LINE_BYTES
            ));
        }
        line.push('\n');
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|e| format!("failed to open {}: {e}", path.display()))?;
        file.write_all(line.as_bytes())
            .map_err(|e| format!("failed to write {}: {e}", path.display()))?;
        Ok(())
    }

    /// Read all valid records from `job-reports.jsonl` in append order.
    ///
    /// Skips records with `schema_version != JOB_REPORT_SCHEMA_VERSION`,
    /// records that fail JSON parse, oversized lines, and stops early if
    /// the file exceeds `MAX_JOB_REPORT_FILE_BYTES`. Always returns
    /// successfully (best-effort read, DR4-002) so the agent loop can
    /// resume even with a poisoned file.
    ///
    /// Public read surface for downstream Issues (#659/#661/#662/#663/
    /// #667) and E2E test harnesses. No production caller in Issue #666.
    #[allow(dead_code)]
    pub(crate) fn read_job_reports(&self) -> Vec<StructuredJobReportRecord> {
        let Some(path) = self.job_reports_path() else {
            return Vec::new();
        };
        let Ok(meta) = std::fs::metadata(&path) else {
            return Vec::new();
        };
        if meta.len() > MAX_JOB_REPORT_FILE_BYTES {
            tracing::warn!(
                path = %path.display(),
                size = meta.len(),
                cap = MAX_JOB_REPORT_FILE_BYTES,
                "job-reports.jsonl exceeds cap; refusing to read"
            );
            return Vec::new();
        }
        let Ok(file) = File::open(&path) else {
            return Vec::new();
        };
        let mut reader = BufReader::new(file);
        let mut out: Vec<StructuredJobReportRecord> = Vec::new();
        let mut buf: Vec<u8> = Vec::with_capacity(MAX_JOB_REPORT_LINE_BYTES);
        loop {
            buf.clear();
            // CB-003 fix: use `read_until('\n')` so we control the
            // per-line allocation, instead of `BufRead::lines()` which
            // would slurp an entire newline-free file into one String.
            let bytes_read = match reader.read_until(b'\n', &mut buf) {
                Ok(0) => break, // EOF
                Ok(n) => n,
                Err(_) => break,
            };
            if buf.last().copied() == Some(b'\n') {
                buf.pop();
                if buf.last().copied() == Some(b'\r') {
                    buf.pop();
                }
            }
            if buf.is_empty() {
                continue;
            }
            if bytes_read > MAX_JOB_REPORT_LINE_BYTES {
                tracing::warn!(
                    cap = MAX_JOB_REPORT_LINE_BYTES,
                    actual = bytes_read,
                    "job report line exceeds cap; skipping"
                );
                continue;
            }
            // CB-003 fix: cap the returned record count to prevent
            // unbounded Vec growth from a file packed with many small
            // valid records.
            if out.len() >= MAX_JOB_REPORT_RECORDS_READ {
                tracing::warn!(
                    cap = MAX_JOB_REPORT_RECORDS_READ,
                    "job-reports.jsonl record count cap reached; stopping read"
                );
                break;
            }
            match serde_json::from_slice::<StructuredJobReportRecord>(&buf) {
                Ok(record) if record.schema_version == JOB_REPORT_SCHEMA_VERSION => {
                    out.push(record);
                }
                Ok(record) => {
                    tracing::warn!(
                        schema_version = record.schema_version,
                        expected = JOB_REPORT_SCHEMA_VERSION,
                        "job report schema_version mismatch; skipping"
                    );
                }
                Err(e) => {
                    tracing::warn!(error = %e, "job report parse failed; skipping line");
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::time::{SystemTime, UNIX_EPOCH};
    use tempfile::TempDir;

    fn now_ms() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }

    fn make_store(tmp: &TempDir, session_id: &str) -> SessionStore {
        SessionStore::new(tmp.path(), session_id, "workspace-key")
    }

    fn sample_record(event: &str) -> StructuredJobReportRecord {
        StructuredJobReportRecord::new(
            now_ms(),
            1,
            event.to_string(),
            json!({"schema_version": 1, "kind": event, "payload": {"job_present": true}}),
        )
    }

    #[test]
    fn round_trip_append_and_read_returns_same_record() {
        let tmp = TempDir::new().unwrap();
        let store = make_store(&tmp, "11111111-1111-1111-1111-111111111111");
        let rec = sample_record("agent.artifact_completion.report");
        store.append_job_report(&rec).unwrap();
        let read = store.read_job_reports();
        assert_eq!(read.len(), 1);
        assert_eq!(read[0].event_name, rec.event_name);
        assert_eq!(read[0].turn_index, rec.turn_index);
        assert_eq!(read[0].schema_version, JOB_REPORT_SCHEMA_VERSION);
    }

    #[test]
    fn schema_version_mismatch_is_skipped_on_read() {
        let tmp = TempDir::new().unwrap();
        let store = make_store(&tmp, "22222222-2222-2222-2222-222222222222");
        // Write valid record + two mismatch records by hand.
        let valid = sample_record("agent.repair.report");
        store.append_job_report(&valid).unwrap();
        let path = store.job_reports_path().unwrap();
        let mut older = serde_json::to_value(&valid).unwrap();
        older["schema_version"] = json!(0u32);
        let mut newer = serde_json::to_value(&valid).unwrap();
        newer["schema_version"] = json!(99u32);
        let mut file = OpenOptions::new().append(true).open(&path).unwrap();
        writeln!(file, "{}", older).unwrap();
        writeln!(file, "{}", newer).unwrap();
        let read = store.read_job_reports();
        // Only the v1 line is returned.
        assert_eq!(read.len(), 1);
        assert_eq!(read[0].schema_version, JOB_REPORT_SCHEMA_VERSION);
    }

    #[test]
    fn parse_failure_does_not_break_resume() {
        let tmp = TempDir::new().unwrap();
        let store = make_store(&tmp, "33333333-3333-3333-3333-333333333333");
        let rec = sample_record("agent.verification.report");
        store.append_job_report(&rec).unwrap();
        let path = store.job_reports_path().unwrap();
        let mut file = OpenOptions::new().append(true).open(&path).unwrap();
        writeln!(file, "{{not valid json").unwrap();
        store.append_job_report(&rec).unwrap();
        let read = store.read_job_reports();
        // Two valid records survive the bad middle line.
        assert_eq!(read.len(), 2);
    }

    #[test]
    fn oversized_line_is_skipped_on_read() {
        let tmp = TempDir::new().unwrap();
        let store = make_store(&tmp, "44444444-4444-4444-4444-444444444444");
        let rec = sample_record("agent.memory.report");
        store.append_job_report(&rec).unwrap();
        let path = store.job_reports_path().unwrap();
        let mut file = OpenOptions::new().append(true).open(&path).unwrap();
        let oversize = "x".repeat(MAX_JOB_REPORT_LINE_BYTES + 1);
        writeln!(file, "{}", oversize).unwrap();
        let read = store.read_job_reports();
        assert_eq!(read.len(), 1);
    }

    #[test]
    fn append_refuses_unsafe_session_id() {
        let tmp = TempDir::new().unwrap();
        let store = make_store(&tmp, "../escape");
        assert!(store.job_reports_path().is_none());
        let rec = sample_record("agent.artifact_completion.report");
        let err = store.append_job_report(&rec).unwrap_err();
        assert!(err.contains("unsafe"));
    }

    #[test]
    fn oversized_file_short_circuits_read() {
        let tmp = TempDir::new().unwrap();
        let store = make_store(&tmp, "55555555-5555-5555-5555-555555555555");
        let rec = sample_record("agent.repair.report");
        // Create a file that exceeds the cap by writing 1 byte beyond it.
        store.append_job_report(&rec).unwrap();
        let path = store.job_reports_path().unwrap();
        let mut file = OpenOptions::new().append(true).open(&path).unwrap();
        let big = "y".repeat((MAX_JOB_REPORT_FILE_BYTES + 1) as usize);
        // Bypass the line cap by writing raw bytes (no newline at end of cap-breaker).
        file.write_all(big.as_bytes()).unwrap();
        drop(file);
        let read = store.read_job_reports();
        assert!(read.is_empty(), "oversized file should produce empty read");
    }

    #[test]
    fn append_refuses_when_serialized_line_exceeds_cap() {
        let tmp = TempDir::new().unwrap();
        let store = make_store(&tmp, "66666666-6666-6666-6666-666666666666");
        let mut huge_payload = serde_json::Map::new();
        huge_payload.insert(
            "blob".to_string(),
            json!("z".repeat(MAX_JOB_REPORT_LINE_BYTES)),
        );
        let rec = StructuredJobReportRecord::new(
            now_ms(),
            1,
            "agent.artifact_completion.report".to_string(),
            serde_json::Value::Object(huge_payload),
        );
        let err = store.append_job_report(&rec).unwrap_err();
        assert!(err.contains("exceeds"));
        // No file created.
        assert!(!store.job_reports_path().unwrap().exists());
    }

    #[test]
    fn read_returns_empty_when_no_file() {
        let tmp = TempDir::new().unwrap();
        let store = make_store(&tmp, "77777777-7777-7777-7777-777777777777");
        let read = store.read_job_reports();
        assert!(read.is_empty());
    }

    #[test]
    fn append_redacts_secret_keys_at_storage_boundary() {
        // CB-002 regression: even if a hypothetical caller passes a
        // record whose envelope carries a raw `api_key`, the storage
        // boundary must re-apply `mask_payload_inplace`.
        let tmp = TempDir::new().unwrap();
        let store = make_store(&tmp, "88888888-8888-8888-8888-888888888888");
        let rec = StructuredJobReportRecord::new(
            now_ms(),
            1,
            "agent.artifact_completion.report".to_string(),
            json!({
                "kind": "agent.artifact_completion.report",
                "schema_version": 1,
                "payload": {
                    "api_key": "sk-LIVE-KEY-MUST-NOT-PERSIST",
                    "harmless_field": "hello"
                }
            }),
        );
        store.append_job_report(&rec).unwrap();
        let contents = std::fs::read_to_string(store.job_reports_path().unwrap()).unwrap();
        assert!(
            !contents.contains("sk-LIVE-KEY-MUST-NOT-PERSIST"),
            "raw api_key value must NOT appear in persisted JSONL"
        );
        assert!(
            contents.contains("***"),
            "mask_payload_inplace marker must be present in persisted JSONL"
        );
    }

    #[test]
    fn read_record_count_is_capped() {
        // CB-003 regression: file packed with valid small records must
        // not produce an unbounded Vec — record count capped at
        // MAX_JOB_REPORT_RECORDS_READ.
        let tmp = TempDir::new().unwrap();
        let store = make_store(&tmp, "99999999-9999-9999-9999-999999999999");
        let rec = sample_record("agent.memory.report");
        // Write MAX + 100 records.
        let target = MAX_JOB_REPORT_RECORDS_READ + 100;
        for _ in 0..target {
            store.append_job_report(&rec).unwrap();
        }
        let read = store.read_job_reports();
        assert_eq!(read.len(), MAX_JOB_REPORT_RECORDS_READ);
    }

    #[test]
    fn safe_session_id_classification() {
        assert!(is_safe_session_id("11111111-1111-1111-1111-111111111111"));
        assert!(!is_safe_session_id("../escape"));
        assert!(!is_safe_session_id("a/b"));
        assert!(!is_safe_session_id("a\\b"));
        assert!(!is_safe_session_id(""));
        assert!(!is_safe_session_id("with\0nul"));
    }
}
