//! Issue #654 — Bounded Safe Stop Report E2E suite (real emit path),
//! in-crate `#[cfg(test)]` edition.
//!
//! CB-001 (Codex review): the previous cross-crate
//! `tests/bounded_safe_stop_report_e2e.rs` reached the emit shells via
//! `#[doc(hidden)] pub fn` seams. `#[doc(hidden)]` is a documentation hint,
//! not an access control: `pub fn` would still be callable from any external
//! crate in a release build. The seams are now `#[cfg(test)]`-only and the
//! suite lives here so it can still exercise the production emit pipeline
//! without widening the release surface.
//!
//! These tests drive the production emit pipeline
//! (`record_safe_stop_report` -> `build_safe_stop_payload` -> `log_llm_event`
//! -> `mask_payload_inplace`) through the public `Agent` surface for each of
//! the 5 stop reasons defined by the design policy:
//!
//! - `mod diagnostic_target_missing`
//! - `mod verifier_failed_safe_stop`
//! - `mod verifier_missing`
//! - `mod artifact_completion_failed`
//! - `mod verifier_weak`
//!
//! Each test uses a unique `session_id` so the shared `init_logging` log can
//! be filtered safely under cargo's parallel test execution (DR3-003).

use std::path::PathBuf;
use std::sync::OnceLock;

use serde_json::Value;
use tempfile::{TempDir, tempdir};

use crate::agent::Agent;
use crate::agent::loop_run::{
    FooterHandle, clear_safe_stop_report_dedup_for_test,
    emit_safe_stop_report_artifact_completion_failed_for_test,
    emit_safe_stop_report_diagnostic_target_missing_for_test,
    emit_safe_stop_report_verifier_failed_safe_stop_for_test,
    emit_safe_stop_report_verifier_missing_for_test, emit_safe_stop_report_verifier_weak_for_test,
    seed_artifact_ledger_repo_edit_for_test,
};
use crate::config::Config;
use crate::model_registry::RuntimeModels;
use crate::ollama::client::OllamaClient;
use crate::session::store::{SessionSnapshot, SessionStore};

// ---------------------------------------------------------------------------
// Shared logger setup (DR3-003) — one TempDir for all tests so OnceLock-backed
// `init_logging` only registers a single subscriber across the parallel run.
// ---------------------------------------------------------------------------

static LOG_DIR: OnceLock<TempDir> = OnceLock::new();

fn shared_log_path() -> PathBuf {
    // Issue #659 Phase 2 (DR3-003 follow-up): `init_logging` uses a global
    // `OnceLock`, so when multiple in-crate `#[cfg(test)]` modules co-init
    // only the first call wins. To stay robust under parallel test
    // execution we eagerly attempt init here but always read back the
    // path the global subscriber actually settled on via
    // `crate::logging::llm_io_log_path()`.
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

/// Return all log records (one JSON value per line) whose `payload.session_id`
/// matches the supplied id. Filtering before assertion / panic message keeps
/// other tests' synthetic secrets out of the failure output (DR4-006).
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

fn safe_stop_events_for(session_id: &str) -> Vec<Value> {
    read_session_events(session_id)
        .into_iter()
        .filter(|rec| rec.get("event").and_then(|v| v.as_str()) == Some("agent.safe_stop.report"))
        .collect()
}

// ---------------------------------------------------------------------------
// Documented schema (DR3-001 / DR3-005) — the closed-fixed label sets the
// downstream `/bug-fix` and semantic repair consumers depend on.
// ---------------------------------------------------------------------------

const SAFE_STOP_PAYLOAD_MAX_BYTES: usize = 4096;

const DOCUMENTED_STOP_REASONS: &[&str] = &[
    "artifact_completion_failed",
    "verifier_failed_safe_stop",
    "verifier_weak",
    "verifier_missing",
    "diagnostic_target_missing",
];

const DOCUMENTED_FAILURE_TYPES: &[&str] = &[
    "compile_or_syntax",
    "import_or_dependency",
    "runtime_error",
    "assertion_failure",
    "missing_verifier_or_config",
    "unknown",
    "diagnostic_target_missing",
];

const DOCUMENTED_DIAGNOSTIC_REASONS: &[&str] = &[
    "scope_excluded",
    "all_candidates_unreadable",
    "read_history_empty",
    "assessment_missing",
];

fn assert_payload_schema(payload: &Value) {
    let stop_reason = payload
        .get("stop_reason")
        .and_then(|v| v.as_str())
        .expect("stop_reason must be a string");
    assert!(
        DOCUMENTED_STOP_REASONS.contains(&stop_reason),
        "stop_reason {stop_reason} not in documented set"
    );

    let failure_type = payload
        .get("failure_type")
        .and_then(|v| v.as_str())
        .expect("failure_type must be a string");
    assert!(
        DOCUMENTED_FAILURE_TYPES.contains(&failure_type),
        "failure_type {failure_type} not in documented set"
    );

    if let Some(reason) = payload
        .get("diagnostic_target_missing_reason")
        .and_then(|v| v.as_str())
    {
        assert!(
            DOCUMENTED_DIAGNOSTIC_REASONS.contains(&reason),
            "diagnostic_target_missing_reason {reason} not in documented set"
        );
    }

    if let Some(target) = payload.get("expected_target").and_then(|v| v.as_str()) {
        assert!(
            target.chars().count() <= 243,
            "expected_target len {} > 243",
            target.chars().count()
        );
    }

    let serialized = serde_json::to_vec(payload).unwrap();
    assert!(
        serialized.len() <= SAFE_STOP_PAYLOAD_MAX_BYTES,
        "payload size {} > {SAFE_STOP_PAYLOAD_MAX_BYTES}",
        serialized.len(),
    );

    assert!(
        payload.get("truncated").and_then(|v| v.as_bool()).is_some(),
        "truncated flag must be a bool"
    );

    for key in [
        "session_id",
        "turn_index",
        "failure_signature",
        "command",
        "output_excerpt",
        "current_role",
        "expected_target",
        "actual_actions",
        "owned_test_artifacts",
        "exhausted_attempts_summary",
        "diagnostic_target_missing_reason",
    ] {
        assert!(
            payload.get(key).is_some(),
            "payload missing required key: {key}"
        );
    }
}

// ---------------------------------------------------------------------------
// Agent fixture — a minimal `Agent` driven by a per-test mockito Ollama
// (which is NEVER hit, since the seams emit synchronously without an LLM
// roundtrip). Each test gets its own `mockito::Server` so port allocation is
// random and the log subscriber is shared via `LOG_DIR`.
//
// The mockito URL is asserted to be a localhost loopback (DR4-004); no
// credentials are placed in it, and no env var is mutated (DR4-005).
// ---------------------------------------------------------------------------

fn build_live_agent(session_id: &str, ollama_url: &str) -> (Agent, TempDir) {
    assert!(
        ollama_url.starts_with("http://127.0.0.1:")
            || ollama_url.starts_with("http://localhost:")
            || ollama_url.starts_with("http://[::1]:"),
        "mockito URL must be a localhost loopback (DR4-004): {ollama_url}"
    );
    assert!(
        !ollama_url.contains('@'),
        "mockito URL must not embed credentials (DR4-004): {ollama_url}"
    );

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

    let workspace_key = format!("anvil-654-{session_id}");
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
        OllamaClient::new(ollama_url.to_string()).unwrap(),
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
    format!("654-{prefix}-{nanos}")
}

/// CB-005 helper: create an `<work_root>/tests/<name>` test file so the
/// classifier's `is_test_file` + `classify_ownership` gates both fire, then
/// seed the `turn_edited_relative_paths` so `collect_owned_test_artifacts`
/// reports it. Returns the workspace-relative path that was seeded.
fn seed_owned_test_artifact(agent: &mut Agent, dir: &TempDir, name: &str) -> String {
    let work_root = dir.path();
    let tests_dir = work_root.join("tests");
    std::fs::create_dir_all(&tests_dir).expect("mkdir tests/");
    let file_path = tests_dir.join(name);
    std::fs::write(&file_path, "// seeded test artifact\n").expect("write test file");
    let rel = format!("tests/{name}");
    seed_artifact_ledger_repo_edit_for_test(agent, rel.clone());
    rel
}

// ---------------------------------------------------------------------------
// mod diagnostic_target_missing — wired production path:
// `record_verifier_diagnostic_unavailable` -> emit shell.
// ---------------------------------------------------------------------------

mod diagnostic_target_missing {
    use super::*;

    #[test]
    fn real_emit_path_produces_event_with_expected_schema() {
        let _ = shared_log_path();
        let session_id = unique_session_id("dtm-basic");
        let server = mockito::Server::new();
        let (mut agent, _dir) = build_live_agent(&session_id, &server.url());

        emit_safe_stop_report_diagnostic_target_missing_for_test(&mut agent);

        let events = safe_stop_events_for(&session_id);
        assert_eq!(
            events.len(),
            1,
            "exactly one agent.safe_stop.report event must be emitted"
        );
        let payload = events[0].get("payload").expect("payload");
        assert_payload_schema(payload);
        assert_eq!(
            payload.get("stop_reason").and_then(|v| v.as_str()),
            Some("diagnostic_target_missing")
        );
        // §11 receipt: diagnostic_target_missing sets failure_type identically.
        assert_eq!(
            payload.get("failure_type").and_then(|v| v.as_str()),
            Some("diagnostic_target_missing")
        );
        // §5 / 6.2: 4-region reason selector returns one of the documented labels.
        let reason = payload
            .get("diagnostic_target_missing_reason")
            .and_then(|v| v.as_str())
            .expect("diagnostic_target_missing payload must populate the reason field");
        assert!(
            DOCUMENTED_DIAGNOSTIC_REASONS.contains(&reason),
            "reason {reason} not in documented set"
        );
    }

    #[test]
    fn dedup_blocks_second_emit_in_same_turn_and_reset_re_emits() {
        let _ = shared_log_path();
        let session_id = unique_session_id("dtm-dedup");
        let server = mockito::Server::new();
        let (mut agent, _dir) = build_live_agent(&session_id, &server.url());

        // First emit: should land in the log.
        emit_safe_stop_report_diagnostic_target_missing_for_test(&mut agent);
        // Second emit in the same turn: dedup must drop it.
        emit_safe_stop_report_diagnostic_target_missing_for_test(&mut agent);

        let after_two = safe_stop_events_for(&session_id);
        assert_eq!(
            after_two.len(),
            1,
            "per-StopReason dedup must block the second emit (S7-001)"
        );

        // Reset emulates `handle_user_message` per-turn clear.
        clear_safe_stop_report_dedup_for_test(&mut agent);
        emit_safe_stop_report_diagnostic_target_missing_for_test(&mut agent);

        let after_reset = safe_stop_events_for(&session_id);
        assert_eq!(
            after_reset.len(),
            2,
            "after dedup reset, the same StopReason must re-emit (DR2-005)"
        );
    }
}

// ---------------------------------------------------------------------------
// mod verifier_failed_safe_stop — wired production path:
// `drive_task_contract_verifier`'s attempt-limit branch.
// ---------------------------------------------------------------------------

mod verifier_failed_safe_stop {
    use super::*;

    #[test]
    fn real_emit_path_produces_event_with_expected_stop_reason() {
        let _ = shared_log_path();
        let session_id = unique_session_id("vfss-basic");
        let server = mockito::Server::new();
        let (mut agent, _dir) = build_live_agent(&session_id, &server.url());

        emit_safe_stop_report_verifier_failed_safe_stop_for_test(&mut agent);

        let events = safe_stop_events_for(&session_id);
        assert_eq!(events.len(), 1, "expected exactly one safe_stop event");
        let payload = events[0].get("payload").expect("payload");
        assert_payload_schema(payload);
        assert_eq!(
            payload.get("stop_reason").and_then(|v| v.as_str()),
            Some("verifier_failed_safe_stop")
        );
        // ExhaustedAttemptsSummary must exist on the FromRepair path (empty
        // `total` is still a structured null-safe object, not a missing key).
        assert!(payload.get("exhausted_attempts_summary").is_some());
    }
}

// ---------------------------------------------------------------------------
// mod verifier_missing — wired production path: `NoVerifier` MissingVerifierJob
// budget exhaustion. The `FromMissingVerifier` builder must default
// `failure_signature` / `command` / `failure_type` to known explicit values
// (R8) so downstream `/bug-fix` does not misfire on empty-string detection.
// ---------------------------------------------------------------------------

mod verifier_missing {
    use super::*;

    #[test]
    fn from_missing_verifier_builder_populates_explicit_defaults() {
        let _ = shared_log_path();
        let session_id = unique_session_id("vm-basic");
        let server = mockito::Server::new();
        let (mut agent, _dir) = build_live_agent(&session_id, &server.url());

        emit_safe_stop_report_verifier_missing_for_test(&mut agent);

        let events = safe_stop_events_for(&session_id);
        assert_eq!(events.len(), 1, "expected exactly one safe_stop event");
        let payload = events[0].get("payload").expect("payload");
        assert_payload_schema(payload);
        assert_eq!(
            payload.get("stop_reason").and_then(|v| v.as_str()),
            Some("verifier_missing")
        );
        assert_eq!(
            payload.get("failure_signature").and_then(|v| v.as_str()),
            Some("missing_verifier_or_config"),
            "FromMissingVerifier builder must default failure_signature (R8)"
        );
        assert_eq!(
            payload.get("command").and_then(|v| v.as_str()),
            Some(""),
            "FromMissingVerifier path has no command — must be empty string"
        );
        assert_eq!(
            payload.get("failure_type").and_then(|v| v.as_str()),
            Some("missing_verifier_or_config")
        );
        // `owned_test_artifacts` must always be an array (may be empty when the
        // turn has no Owned-validated test files).
        assert!(
            payload
                .get("owned_test_artifacts")
                .and_then(|v| v.as_array())
                .is_some()
        );
    }

    #[test]
    fn from_missing_verifier_propagates_seeded_owned_test_artifact() {
        // CB-005 (Codex review): the original E2E only asserted that
        // `owned_test_artifacts` was *an array*. Pin the non-empty case so a
        // regression in `collect_owned_test_artifacts` (e.g. ownership
        // classifier bypass) is caught.
        let _ = shared_log_path();
        let session_id = unique_session_id("vm-owned");
        let server = mockito::Server::new();
        let (mut agent, dir) = build_live_agent(&session_id, &server.url());

        let owned_rel = seed_owned_test_artifact(&mut agent, &dir, "foo_smoke.rs");

        emit_safe_stop_report_verifier_missing_for_test(&mut agent);

        let events = safe_stop_events_for(&session_id);
        assert_eq!(events.len(), 1, "expected exactly one safe_stop event");
        let payload = events[0].get("payload").expect("payload");
        let owned = payload
            .get("owned_test_artifacts")
            .and_then(|v| v.as_array())
            .expect("owned_test_artifacts must be an array");
        assert!(
            !owned.is_empty(),
            "verifier_missing must propagate Owned-validated test artifacts; got {owned:?}",
        );
        assert!(
            owned.iter().any(|v| v.as_str() == Some(owned_rel.as_str())),
            "expected seeded path {owned_rel:?} in {owned:?}",
        );
    }

    #[test]
    fn from_missing_verifier_excludes_candidate_only_and_out_of_scope_paths() {
        // CB-005 follow-up: seed one Owned-validated path **and** one path
        // that fails the ownership classifier (the in-test seam only flips
        // `turn_edited_relative_paths`; non-test file paths fall out at the
        // `is_test_file` gate, and `..`-laden paths are syntactically
        // rejected). Neither must appear in `owned_test_artifacts`.
        let _ = shared_log_path();
        let session_id = unique_session_id("vm-excluded");
        let server = mockito::Server::new();
        let (mut agent, dir) = build_live_agent(&session_id, &server.url());

        let owned_rel = seed_owned_test_artifact(&mut agent, &dir, "kept_smoke.rs");

        // A non-test path is filtered by `is_test_file` -> dropped.
        seed_artifact_ledger_repo_edit_for_test(&mut agent, "src/lib.rs".to_string());

        emit_safe_stop_report_verifier_missing_for_test(&mut agent);

        let events = safe_stop_events_for(&session_id);
        assert_eq!(events.len(), 1, "expected exactly one safe_stop event");
        let payload = events[0].get("payload").expect("payload");
        let owned: Vec<String> = payload
            .get("owned_test_artifacts")
            .and_then(|v| v.as_array())
            .expect("owned_test_artifacts must be an array")
            .iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect();
        assert!(
            owned.iter().any(|p| p == &owned_rel),
            "expected {owned_rel:?} in {owned:?}",
        );
        assert!(
            !owned.iter().any(|p| p == "src/lib.rs"),
            "non-test paths must be excluded by classify_ownership / is_test_file gate; got {owned:?}",
        );
    }
}

// ---------------------------------------------------------------------------
// mod artifact_completion_failed — wired at the 4 role-specific retry budget
// exhaustion points (rejected artifact edit, missing artifact role, missing
// artifact(s), RepairArtifact loop exhausted).
// ---------------------------------------------------------------------------

mod artifact_completion_failed {
    use super::*;

    #[test]
    fn real_emit_carries_role_and_expected_target() {
        let _ = shared_log_path();
        let session_id = unique_session_id("acf-basic");
        let server = mockito::Server::new();
        let (mut agent, _dir) = build_live_agent(&session_id, &server.url());

        emit_safe_stop_report_artifact_completion_failed_for_test(
            &mut agent,
            "implementation",
            Some("src/lib.rs".to_string()),
        );

        let events = safe_stop_events_for(&session_id);
        assert_eq!(events.len(), 1, "expected exactly one safe_stop event");
        let payload = events[0].get("payload").expect("payload");
        assert_payload_schema(payload);
        assert_eq!(
            payload.get("stop_reason").and_then(|v| v.as_str()),
            Some("artifact_completion_failed")
        );
        assert_eq!(
            payload.get("current_role").and_then(|v| v.as_str()),
            Some("implementation")
        );
        assert_eq!(
            payload.get("expected_target").and_then(|v| v.as_str()),
            Some("src/lib.rs")
        );
        // Even with no real RepairJob, actual_actions must be a (possibly empty) array.
        assert!(
            payload
                .get("actual_actions")
                .and_then(|v| v.as_array())
                .is_some()
        );
    }

    #[test]
    fn unsafe_absolute_expected_target_is_dropped_by_builder() {
        let _ = shared_log_path();
        let session_id = unique_session_id("acf-unsafe");
        let server = mockito::Server::new();
        let (mut agent, _dir) = build_live_agent(&session_id, &server.url());

        // Absolute path must be rejected by `safe_relative_path_string` and
        // surfaced as `null` in the payload (DR4-003 / R7).
        emit_safe_stop_report_artifact_completion_failed_for_test(
            &mut agent,
            "test",
            Some("/etc/passwd".to_string()),
        );

        let events = safe_stop_events_for(&session_id);
        assert_eq!(events.len(), 1);
        let payload = events[0].get("payload").expect("payload");
        assert!(
            payload.get("expected_target").is_some()
                && payload
                    .get("expected_target")
                    .map(|v| v.is_null())
                    .unwrap_or(false),
            "absolute path must be projected to null by safe_relative_path_string"
        );
        assert_eq!(
            payload.get("current_role").and_then(|v| v.as_str()),
            Some("test")
        );
    }
}

// ---------------------------------------------------------------------------
// mod verifier_weak — wired at the `VerifierRepairPassOutcome::Invalid` exit
// when controller-applied repair proposals exhaust the retry budget after
// being rejected by validation (weakening detection / parse failure).
// ---------------------------------------------------------------------------

mod verifier_weak {
    use super::*;

    #[test]
    fn real_emit_carries_owned_test_artifacts_array_and_stop_reason() {
        let _ = shared_log_path();
        let session_id = unique_session_id("vw-basic");
        let server = mockito::Server::new();
        let (mut agent, _dir) = build_live_agent(&session_id, &server.url());

        emit_safe_stop_report_verifier_weak_for_test(&mut agent);

        let events = safe_stop_events_for(&session_id);
        assert_eq!(events.len(), 1, "expected exactly one safe_stop event");
        let payload = events[0].get("payload").expect("payload");
        assert_payload_schema(payload);
        assert_eq!(
            payload.get("stop_reason").and_then(|v| v.as_str()),
            Some("verifier_weak")
        );
        // DR2-002: owned_test_artifacts is always an array, may be empty when
        // no test files were Owned-edited this turn.
        assert!(
            payload
                .get("owned_test_artifacts")
                .and_then(|v| v.as_array())
                .is_some()
        );
    }

    #[test]
    fn real_emit_propagates_seeded_owned_test_artifact() {
        // CB-005 (Codex review): pin the non-empty owned_test_artifacts case
        // for the verifier_weak path so `collect_owned_test_artifacts`
        // regressions are detected.
        let _ = shared_log_path();
        let session_id = unique_session_id("vw-owned");
        let server = mockito::Server::new();
        let (mut agent, dir) = build_live_agent(&session_id, &server.url());

        let owned_rel = seed_owned_test_artifact(&mut agent, &dir, "weak_smoke.rs");

        emit_safe_stop_report_verifier_weak_for_test(&mut agent);

        let events = safe_stop_events_for(&session_id);
        assert_eq!(events.len(), 1, "expected exactly one safe_stop event");
        let payload = events[0].get("payload").expect("payload");
        let owned = payload
            .get("owned_test_artifacts")
            .and_then(|v| v.as_array())
            .expect("owned_test_artifacts must be an array");
        assert!(
            !owned.is_empty(),
            "verifier_weak must propagate Owned-validated test artifacts; got {owned:?}",
        );
        assert!(
            owned.iter().any(|v| v.as_str() == Some(owned_rel.as_str())),
            "expected seeded path {owned_rel:?} in {owned:?}",
        );
    }
}
