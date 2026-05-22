//! Issue #666 — structured job report E2E (in-crate `#[cfg(test)]` suite).
//!
//! Pattern lifted from `safe_stop_e2e_tests.rs` (CB-001 fix precedent):
//! a real `Agent` is constructed against a mockito-loopback Ollama URL
//! and a TempDir-backed state root. `maybe_emit_job_reports` is invoked
//! through a `pub(super)` test seam to exercise the production emit
//! pipeline (`record_job_report` → `build_envelope` → `enforce_bounds` →
//! `log_llm_event` → `mask_payload_inplace`) and persistence pipeline
//! (`persist_job_report_to_session` → `SessionStore::append_job_report`).
//!
//! Tests do NOT touch `tests/` (DR3-001 / CB-001 fix) and never invoke a
//! real LLM — `OllamaClient` is constructed against a loopback URL that
//! is never hit because the emit pipeline runs synchronously.

use std::path::PathBuf;
use std::sync::OnceLock;

use serde_json::Value;
use tempfile::{TempDir, tempdir};

use crate::agent::Agent;
use crate::agent::loop_run::{FooterHandle, maybe_emit_job_reports_for_test};
use crate::config::Config;
use crate::model_registry::RuntimeModels;
use crate::ollama::client::OllamaClient;
use crate::session::store::{SessionSnapshot, SessionStore};

static LOG_DIR: OnceLock<TempDir> = OnceLock::new();

fn shared_log_path() -> PathBuf {
    let _ = LOG_DIR.get_or_init(|| {
        let dir = tempdir().expect("tempdir for shared log");
        let log_path = dir.path().join("llm-io.jsonl");
        let _ = crate::logging::init_logging(crate::config::LogLevel::Info, &log_path);
        dir
    });
    crate::logging::llm_io_log_path()
        .map(|p| p.to_path_buf())
        .expect("logging must be initialised by this point")
}

fn read_session_events(session_id: &str) -> Vec<Value> {
    let path = shared_log_path();
    let contents = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    contents
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|rec| {
            rec.get("payload")
                .and_then(|p| p.get("session_id"))
                .and_then(|v| v.as_str())
                == Some(session_id)
        })
        .collect()
}

fn events_by_name(session_id: &str, event_name: &str) -> Vec<Value> {
    read_session_events(session_id)
        .into_iter()
        .filter(|rec| rec.get("event").and_then(|v| v.as_str()) == Some(event_name))
        .collect()
}

fn build_live_agent(session_id: &str) -> (Agent, TempDir) {
    let dir = tempdir().unwrap();
    let state_root = dir.path().join("state");
    std::fs::create_dir_all(state_root.join("sessions").join(session_id)).unwrap();

    let config = Config {
        cwd: dir.path().to_path_buf(),
        requested_model: Some("test-model".to_string()),
        state_dir_override: Some(state_root.clone()),
        yes_mode: true,
        max_iterations: 1,
        ..Config::default()
    };

    let workspace_key = format!("anvil-666-{session_id}");
    let session = SessionSnapshot {
        id: session_id.to_string(),
        workspace_key: workspace_key.clone(),
        ..Default::default()
    };

    let agent = Agent::new(
        config,
        RuntimeModels {
            main: "test-model".to_string(),
            sidecar: None,
        },
        OllamaClient::new("http://127.0.0.1:1".to_string()).unwrap(),
        SessionStore::new(&state_root, session_id, &workspace_key),
        session,
        FooterHandle::disabled(),
    );
    (agent, dir)
}

fn unique_session_id(prefix: &str) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let pid = std::process::id();
    format!("{prefix}-{pid}-{nanos}-12345678-1234-1234-1234-123456789012")
}

const EVENT_AC: &str = "agent.artifact_completion.report";
const EVENT_VE: &str = "agent.verification.report";
const EVENT_RE: &str = "agent.repair.report";
const EVENT_ME: &str = "agent.memory.report";

#[test]
fn emits_four_reports_in_one_turn() {
    let session_id = unique_session_id("ac4");
    let _ = shared_log_path(); // ensure global logger initialized before emit
    let (mut agent, _td) = build_live_agent(&session_id);

    maybe_emit_job_reports_for_test(&mut agent);

    let ac = events_by_name(&session_id, EVENT_AC);
    let ve = events_by_name(&session_id, EVENT_VE);
    let re = events_by_name(&session_id, EVENT_RE);
    let me = events_by_name(&session_id, EVENT_ME);

    assert_eq!(ac.len(), 1, "artifact_completion exactly once");
    assert_eq!(ve.len(), 1, "verification exactly once");
    assert_eq!(re.len(), 1, "repair exactly once");
    assert_eq!(me.len(), 1, "memory exactly once");
}

#[test]
fn payload_schema_version_is_v1_per_report() {
    let session_id = unique_session_id("sv1");
    let _ = shared_log_path(); // ensure global logger initialized before emit
    let (mut agent, _td) = build_live_agent(&session_id);

    maybe_emit_job_reports_for_test(&mut agent);

    for event in [EVENT_AC, EVENT_VE, EVENT_RE, EVENT_ME] {
        let recs = events_by_name(&session_id, event);
        let payload = recs[0].get("payload").unwrap();
        let sv = payload
            .get("schema_version")
            .and_then(|v| v.as_u64())
            .unwrap();
        assert_eq!(sv, 1, "{event} schema_version must be 1");
        let kind = payload.get("kind").and_then(|v| v.as_str()).unwrap();
        assert_eq!(kind, event);
    }
}

#[test]
fn per_turn_dedup_suppresses_second_emit_in_same_turn() {
    let session_id = unique_session_id("dedup");
    let _ = shared_log_path(); // ensure global logger initialized before emit
    let (mut agent, _td) = build_live_agent(&session_id);

    maybe_emit_job_reports_for_test(&mut agent);
    // Second call in the same turn — dedup must suppress all 4 reports.
    maybe_emit_job_reports_for_test(&mut agent);

    for event in [EVENT_AC, EVENT_VE, EVENT_RE, EVENT_ME] {
        let recs = events_by_name(&session_id, event);
        assert_eq!(
            recs.len(),
            1,
            "{event} must fire exactly once per turn even with double call"
        );
    }
}

#[test]
fn persisted_jsonl_records_schema_version_one() {
    let session_id = unique_session_id("persist");
    let (mut agent, td) = build_live_agent(&session_id);

    maybe_emit_job_reports_for_test(&mut agent);

    let jsonl_path = td
        .path()
        .join("state")
        .join("sessions")
        .join(&session_id)
        .join("job-reports.jsonl");
    let contents = std::fs::read_to_string(&jsonl_path).expect("job-reports.jsonl exists");
    let records: Vec<Value> = contents
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .collect();
    assert_eq!(records.len(), 4, "exactly 4 records persisted");
    for r in &records {
        assert_eq!(
            r.get("schema_version").and_then(|v| v.as_u64()),
            Some(1u64),
            "every record carries envelope schema_version=1"
        );
        assert!(r.get("recorded_at_unix_ms").is_some());
        assert!(r.get("turn_index").is_some());
        assert!(r.get("event_name").is_some());
        assert!(r.get("envelope").is_some());
    }
}

#[test]
fn safe_stop_emit_order_is_job_reports_then_safe_stop() {
    // CB-001 regression: when SafeStopReport is emitted, the 4 job
    // reports must appear in the llm-io stream BEFORE the
    // `agent.safe_stop.report` event (design Section 8-2).
    use crate::agent::loop_run::emit_safe_stop_report_verifier_weak_for_test;
    let session_id = unique_session_id("order");
    let _ = shared_log_path();
    let (mut agent, _td) = build_live_agent(&session_id);
    // Drive the production safe_stop emit path; this should internally
    // emit the 4 job reports BEFORE the safe_stop event.
    emit_safe_stop_report_verifier_weak_for_test(&mut agent);

    let events = read_session_events(&session_id);
    // Find indices of the 4 job reports and the safe_stop event.
    let mut safe_stop_idx: Option<usize> = None;
    let mut last_job_report_idx: Option<usize> = None;
    for (i, rec) in events.iter().enumerate() {
        let name = rec.get("event").and_then(|v| v.as_str()).unwrap_or("");
        if name == "agent.safe_stop.report" {
            safe_stop_idx = Some(i);
        }
        if matches!(
            name,
            "agent.artifact_completion.report"
                | "agent.verification.report"
                | "agent.repair.report"
                | "agent.memory.report"
        ) {
            last_job_report_idx = Some(i);
        }
    }
    let s = safe_stop_idx.expect("safe_stop.report must be emitted");
    let j = last_job_report_idx.expect("at least one job report must be emitted");
    assert!(
        j < s,
        "emit order must be `agent.*.report` (idx {j}) → `agent.safe_stop.report` (idx {s})"
    );
}

#[test]
fn memory_report_has_no_safe_stop_linkage() {
    let session_id = unique_session_id("noss");
    let _ = shared_log_path(); // ensure global logger initialized before emit
    let (mut agent, _td) = build_live_agent(&session_id);

    maybe_emit_job_reports_for_test(&mut agent);

    let recs = events_by_name(&session_id, EVENT_ME);
    let payload = recs[0].get("payload").unwrap();
    assert!(
        payload.get("safe_stop").is_none(),
        "MemoryReport must NOT carry safe_stop linkage (DR1-004 asymmetry)"
    );
}

/// Issue #662: when the production safe-stop emit fires with `RepairExhausted`,
/// the 4 job reports must observe the linkage (`safe_stop.reason =
/// "repair_exhausted"`) on the 3 non-Memory reports and leave the Memory
/// report untouched (S7-003 DR1-004 asymmetry preserved).
#[test]
fn repair_exhausted_linkage_propagates_to_three_reports() {
    use crate::agent::loop_run::emit_safe_stop_report_repair_exhausted_for_test;

    let session_id = unique_session_id("re-link");
    let _ = shared_log_path();
    let (mut agent, _td) = build_live_agent(&session_id);

    // Production safe-stop emit emits the 4 job reports first (CB-001 order)
    // with the linkage built in, then the safe_stop.report event.
    emit_safe_stop_report_repair_exhausted_for_test(&mut agent);

    // ArtifactCompletionReport / VerificationReport / RepairReport carry the
    // linkage; MemoryReport does not. The llm-io record nests as:
    //   record.payload (envelope) → envelope.payload (Report) → Report.safe_stop.
    for event in [EVENT_AC, EVENT_VE, EVENT_RE] {
        let recs = events_by_name(&session_id, event);
        assert!(
            !recs.is_empty(),
            "{event} must be emitted at least once via the production path"
        );
        let envelope = recs[0].get("payload").unwrap();
        let report = envelope
            .get("payload")
            .unwrap_or_else(|| panic!("{event} envelope must carry an inner payload"));
        let safe_stop = report
            .get("safe_stop")
            .unwrap_or_else(|| panic!("{event} must carry safe_stop linkage"));
        let reason = safe_stop.get("reason").and_then(|v| v.as_str());
        assert_eq!(
            reason,
            Some("repair_exhausted"),
            "{event} safe_stop.reason must equal the 6th documented label"
        );
        let report_emitted = safe_stop
            .get("report_emitted")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        assert!(
            report_emitted,
            "{event} safe_stop.report_emitted must be true when emitted with linkage override"
        );
    }

    // MemoryReport asymmetry (S7-003 / DR1-004): even if Memory is emitted
    // for any reason on this turn, the report payload must not carry a
    // `safe_stop` field. When the safe-stop emit path is the only trigger,
    // Memory does NOT get emitted (it requires PAM-observable state), so
    // the absence-of-records case is also acceptable.
    let me_recs = events_by_name(&session_id, EVENT_ME);
    for rec in &me_recs {
        let me_envelope = rec.get("payload").unwrap();
        let me_report = me_envelope
            .get("payload")
            .expect("memory envelope must carry an inner payload");
        assert!(
            me_report.get("safe_stop").is_none(),
            "MemoryReport must NOT carry safe_stop linkage even for repair_exhausted (S7-003)"
        );
    }
}
