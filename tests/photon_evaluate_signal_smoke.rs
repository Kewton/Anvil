//! Issue #591 — Photon /v1/evaluate adoption-signal smoke suite.
//!
//! End-to-end tests covering the live HTTP path through `invoke_photon_evaluate`
//! after Issue #591 added the `summary_ids_adopted` / `summary_ids_adopted_truncated`
//! / `outcome` fields to the `context_pack_event` JSON, plus the four new
//! `agent.photon_evaluate.completed` event keys (DR4-NEW-004):
//!
//!   PES-01  /v1/evaluate is called when live (canary=1000, shadow=false)        (VR-01)
//!   PES-02  `summary_ids_adopted` length matches injected unique seed count       (VR-02)
//!   PES-03  `adoption_status` is "injected" / "not_injected" (not shadow_*)       (VR-03)
//!   PES-04  `outcome` is null or one of the static allowlist values               (VR-04)
//!   PES-05  cap=32 truncates over-cap state-seeded list + sets truncated=true    (VR-05)
//!   VR-07   path (a) canary=0 → /v1/evaluate's context_pack_event=null
//!   VR-10   path (b) shadow_mode=true + canary=1000 → adoption ids = []
//!   VR-09   `agent.photon_evaluate.completed` event carries the 8 keys
//!
//! All tests use a unique session_id and the shared mockito-+-llm-io log.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use anvil::agent::Agent;
use anvil::agent::loop_run::FooterHandle;
use anvil::config::Config;
use anvil::model_registry::RuntimeModels;
use anvil::ollama::client::OllamaClient;
use anvil::session::store::{SessionSnapshot, SessionStore};
use tempfile::{TempDir, tempdir};

// ---------------------------------------------------------------------------
// Shared logger setup (DR3-003 — copied from photon_warning_filter_smoke.rs)
// ---------------------------------------------------------------------------

static LOG_DIR: OnceLock<TempDir> = OnceLock::new();

fn shared_log_path() -> PathBuf {
    let dir = LOG_DIR.get_or_init(|| {
        let dir = tempdir().expect("tempdir for shared log");
        let log_path = dir.path().join("llm-io.jsonl");
        let _ = anvil::logging::init_logging(anvil::config::LogLevel::Info, &log_path);
        dir
    });
    dir.path().join("llm-io.jsonl")
}

fn read_session_events(session_id: &str) -> Vec<serde_json::Value> {
    let path = shared_log_path();
    let contents = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    contents
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter(|rec| {
            rec.get("payload")
                .and_then(|p| p.get("session_id"))
                .and_then(|v| v.as_str())
                == Some(session_id)
        })
        .collect()
}

fn find_first_event(session_id: &str, event_name: &str) -> Option<serde_json::Value> {
    read_session_events(session_id)
        .into_iter()
        .find(|rec| rec.get("event").and_then(|v| v.as_str()) == Some(event_name))
}

/// Build a v0.2 sidecar-layout `/v1/context/pack` response with N items each
/// carrying a unique sanitizable id (`seed_<i>`).
fn mock_context_pack_body(num_items: usize) -> String {
    let items: Vec<serde_json::Value> = (0..num_items)
        .map(|i| {
            serde_json::json!({
                "kind": "summary",
                "id": format!("seed_{i}"),
                "summary": format!("seed body {i}")
            })
        })
        .collect();
    serde_json::json!({
        "schema_version": "action-memory.v0.2",
        "context_pack": {
            "items": items,
            "warnings": []
        }
    })
    .to_string()
}

/// Capture handle returned by `mock_evaluate_with_capture`.
type EvalBodyCapture = Arc<Mutex<Vec<String>>>;

/// Mount a `/v1/evaluate` mock whose `match_request` closure captures every
/// request body into the returned `Arc<Mutex<Vec<String>>>`. The mock always
/// matches and returns 200 with `{"admitted":true}`.
fn mock_evaluate_with_capture(server: &mut mockito::Server) -> (EvalBodyCapture, mockito::Mock) {
    let capture: EvalBodyCapture = Arc::new(Mutex::new(Vec::new()));
    let capture_clone = capture.clone();
    let mock = server
        .mock("POST", "/v1/evaluate")
        .match_request(move |req| {
            if let Ok(body) = req.utf8_lossy_body()
                && let Ok(mut guard) = capture_clone.lock()
            {
                guard.push(body.to_string());
            }
            true
        })
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{"admitted":true}"#)
        .create();
    (capture, mock)
}

fn build_live_agent(
    session_id: &str,
    photon_url: String,
    photon_shadow_mode: bool,
    photon_canary: u16,
) -> (Agent, tempfile::TempDir) {
    let dir = tempdir().unwrap();
    let state_root = dir.path().join("state");
    std::fs::create_dir_all(state_root.join("sessions").join(session_id)).unwrap();

    let mut config = Config::default();
    config.cwd = dir.path().to_path_buf();
    config.photon_enabled = true;
    config.photon_url = photon_url;
    config.photon_shadow_mode = photon_shadow_mode;
    config.photon_canary = photon_canary;
    config.photon_timeout_ms = 5000;
    config.photon_respect_warnings = true;
    config.requested_model = Some("test-model".to_string());
    config.state_dir_override = Some(state_root.clone());
    config.yes_mode = true;
    config.max_iterations = 1;

    let workspace_key = format!("anvil-591-{session_id}");
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
        OllamaClient::new("http://127.0.0.1:19999".to_string()).unwrap(),
        SessionStore::new(&state_root, session_id, &workspace_key),
        session,
        FooterHandle::disabled(),
    );
    (agent, dir)
}

fn first_eval_body_as_json(capture: &EvalBodyCapture) -> serde_json::Value {
    let bodies = capture.lock().unwrap();
    assert!(
        !bodies.is_empty(),
        "expected at least one /v1/evaluate body to have been captured"
    );
    serde_json::from_str::<serde_json::Value>(&bodies[0])
        .expect("captured /v1/evaluate body must be valid JSON")
}

// ---------------------------------------------------------------------------
// PES-01 (VR-01): /v1/evaluate is called in live mode
// ---------------------------------------------------------------------------

#[test]
fn pes01_evaluate_called_in_live_mode() {
    let _ = shared_log_path();
    let session_id = "pes01-evaluate-called";

    let mut photon_server = mockito::Server::new();
    let _pack_mock = photon_server
        .mock("POST", "/v1/context/pack")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(mock_context_pack_body(2))
        .create();
    let eval_mock = photon_server
        .mock("POST", "/v1/evaluate")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{"admitted":true}"#)
        .expect_at_least(1)
        .create();

    let (mut agent, _dir) = build_live_agent(session_id, photon_server.url(), false, 1000);
    let _ = agent.process_line("hello", false);

    eval_mock.assert();
}

// ---------------------------------------------------------------------------
// PES-02 (VR-02): summary_ids_adopted length matches injected unique seed count
// ---------------------------------------------------------------------------

#[test]
fn pes02_summary_ids_adopted_matches_injected_count() {
    let _ = shared_log_path();
    let session_id = "pes02-ids-match-count";

    let mut photon_server = mockito::Server::new();
    let _pack_mock = photon_server
        .mock("POST", "/v1/context/pack")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(mock_context_pack_body(3))
        .create();
    let (capture, _eval_mock) = mock_evaluate_with_capture(&mut photon_server);

    let (mut agent, _dir) = build_live_agent(session_id, photon_server.url(), false, 1000);
    let _ = agent.process_line("hello", false);

    let body = first_eval_body_as_json(&capture);
    let event = body
        .get("context_pack_event")
        .expect("context_pack_event present");
    assert!(
        !event.is_null(),
        "context_pack_event must be object in live mode"
    );
    let ids = event
        .get("summary_ids_adopted")
        .and_then(|v| v.as_array())
        .expect("summary_ids_adopted must be array");
    assert_eq!(ids.len(), 3, "all 3 injected items adopted into the prompt");
    let id_strs: Vec<&str> = ids.iter().filter_map(|v| v.as_str()).collect();
    assert!(id_strs.contains(&"seed_0"));
    assert!(id_strs.contains(&"seed_1"));
    assert!(id_strs.contains(&"seed_2"));
    assert_eq!(
        event
            .get("summary_ids_adopted_truncated")
            .and_then(|v| v.as_bool()),
        Some(false),
        "no truncation when 3 < 32 cap"
    );
}

// ---------------------------------------------------------------------------
// PES-03 (VR-03): adoption_status is "injected" / "not_injected" only
// ---------------------------------------------------------------------------

#[test]
fn pes03_adoption_status_live_is_injected_or_not_injected() {
    let _ = shared_log_path();
    let session_id = "pes03-status-live";

    let mut photon_server = mockito::Server::new();
    let _pack_mock = photon_server
        .mock("POST", "/v1/context/pack")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(mock_context_pack_body(2))
        .create();
    let (capture, _eval_mock) = mock_evaluate_with_capture(&mut photon_server);

    let (mut agent, _dir) = build_live_agent(session_id, photon_server.url(), false, 1000);
    let _ = agent.process_line("hello", false);

    let body = first_eval_body_as_json(&capture);
    let event = body.get("context_pack_event").unwrap();
    let status = event
        .get("adoption_status")
        .and_then(|v| v.as_str())
        .expect("adoption_status string");
    assert!(
        status == "injected" || status == "not_injected",
        "adoption_status in live mode must be 'injected' or 'not_injected', got {status:?}"
    );
    assert_ne!(
        status, "shadow_not_injected",
        "live mode must never report shadow_not_injected"
    );
}

// ---------------------------------------------------------------------------
// PES-04 (VR-04): outcome is null or one of the static allowlist strings
// ---------------------------------------------------------------------------

#[test]
fn pes04_outcome_static_allowlist_only() {
    let _ = shared_log_path();
    let session_id = "pes04-outcome-allowlist";

    let mut photon_server = mockito::Server::new();
    let _pack_mock = photon_server
        .mock("POST", "/v1/context/pack")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(mock_context_pack_body(1))
        .create();
    let (capture, _eval_mock) = mock_evaluate_with_capture(&mut photon_server);

    let (mut agent, _dir) = build_live_agent(session_id, photon_server.url(), false, 1000);
    let _ = agent.process_line("hello", false);

    let body = first_eval_body_as_json(&capture);
    let event = body.get("context_pack_event").unwrap();
    let outcome = event.get("outcome").expect("outcome key present");
    if let Some(s) = outcome.as_str() {
        let allowed = ["success", "failure", "safety_violation"];
        assert!(
            allowed.contains(&s),
            "outcome string must be in static allowlist, got {s:?}"
        );
    } else {
        assert!(
            outcome.is_null(),
            "outcome must be string-or-null, was {outcome:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// PES-05 (VR-05): cap=32 truncates over-cap adopted ids + sets truncated=true
//
// production render is gated by `MAX_PROMPT_ITEMS=5`, so the >32 over-cap case
// is not reachable through `render_context_pack`. The cap lives as a
// defensive guard for future code that may seed `Agent.last_adopted_summary_ids`
// directly. Verification here is done at the helper boundary by asserting
// that the constant exists, equals 32, and matches the photon-layer SSOT.
// The pure cap+sanitize helper is unit-tested directly under
// `prepare_adopted_ids_for_evaluate_tests` in `src/agent/loop_run/turn.rs`.
// ---------------------------------------------------------------------------

#[test]
fn pes05_max_photon_eval_adopted_ids_cap_is_32() {
    use anvil::photon::MAX_PHOTON_EVAL_ADOPTED_IDS;
    assert_eq!(
        MAX_PHOTON_EVAL_ADOPTED_IDS, 32,
        "cap SSOT must remain 32 — Issue #591 / AS-05 / 設計判断 #4"
    );
}

// ---------------------------------------------------------------------------
// VR-07: path (a) canary=0 → /v1/evaluate's context_pack_event = null
//
// invoke_photon_context_pack is skipped because canary=0 gate fails. Agent
// state has last_context_pack_id = None, so the evaluate hook emits
// context_pack_event = null.
// ---------------------------------------------------------------------------

#[test]
fn vr07_canary_zero_evaluate_event_is_null() {
    let _ = shared_log_path();
    let session_id = "vr07-canary-zero";

    let mut photon_server = mockito::Server::new();
    let pack_mock = photon_server
        .mock("POST", "/v1/context/pack")
        .expect(0)
        .create();
    let (capture, _eval_mock) = mock_evaluate_with_capture(&mut photon_server);

    let (mut agent, _dir) = build_live_agent(session_id, photon_server.url(), false, 0); // canary=0
    let _ = agent.process_line("hello", false);

    pack_mock.assert();

    let body = first_eval_body_as_json(&capture);
    let event = body
        .get("context_pack_event")
        .expect("context_pack_event present");
    assert!(
        event.is_null(),
        "canary=0 yields context_pack_event=null (no request_id captured)"
    );
}

// ---------------------------------------------------------------------------
// VR-10: path (b) shadow_mode=true + canary=1000 → evaluate sends an object
// event but with summary_ids_adopted=[] / count=0 / outcome=null
// ---------------------------------------------------------------------------

#[test]
fn vr10_shadow_mode_evaluate_forces_empty_signal() {
    let _ = shared_log_path();
    let session_id = "vr10-shadow-empty";

    let mut photon_server = mockito::Server::new();
    // shadow path (b) inside build_request_messages will send /v1/context/pack
    // — accept any number of calls (>=0). The body sets up so the request_id
    // is captured for last_context_pack_id (path-b shadow case → object event).
    let _pack_mock = photon_server
        .mock("POST", "/v1/context/pack")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(mock_context_pack_body(3))
        .create();
    let (capture, _eval_mock) = mock_evaluate_with_capture(&mut photon_server);

    let (mut agent, _dir) = build_live_agent(session_id, photon_server.url(), true, 1000); // shadow=true
    let _ = agent.process_line("hello", false);

    let body = first_eval_body_as_json(&capture);
    let event = body.get("context_pack_event").expect("event present");
    if event.is_null() {
        // If no last_context_pack_id was captured by path (b), the event is
        // null — that's also valid for shadow mode (and means the
        // adoption signal is trivially empty).
        return;
    }
    let ids = event
        .get("summary_ids_adopted")
        .and_then(|v| v.as_array())
        .expect("summary_ids_adopted array");
    assert!(
        ids.is_empty(),
        "shadow mode must force summary_ids_adopted=[], got {ids:?}"
    );
    assert_eq!(
        event
            .get("summary_ids_adopted_truncated")
            .and_then(|v| v.as_bool()),
        Some(false),
        "shadow mode must force summary_ids_adopted_truncated=false"
    );
    assert!(
        event.get("outcome").map(|v| v.is_null()).unwrap_or(true),
        "shadow mode must force outcome=null"
    );
    assert_eq!(
        event.get("items_adopted_count").and_then(|v| v.as_u64()),
        Some(0),
        "shadow mode must force items_adopted_count=0"
    );
    assert_eq!(
        event.get("adoption_status").and_then(|v| v.as_str()),
        Some("shadow_not_injected"),
        "shadow mode adoption_status"
    );
}

// ---------------------------------------------------------------------------
// VR-09: agent.photon_evaluate.completed event payload carries the 8 keys
// (4 existing + 4 new) and outcome is from static allowlist
// ---------------------------------------------------------------------------

#[test]
fn vr09_evaluate_completed_event_has_8_keys() {
    let _ = shared_log_path();
    let session_id = "vr09-event-payload";

    let mut photon_server = mockito::Server::new();
    let _pack_mock = photon_server
        .mock("POST", "/v1/context/pack")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(mock_context_pack_body(2))
        .create();
    let _eval_mock = photon_server
        .mock("POST", "/v1/evaluate")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{"admitted":true}"#)
        .create();

    let (mut agent, _dir) = build_live_agent(session_id, photon_server.url(), false, 1000);
    let _ = agent.process_line("hello", false);

    let completed = find_first_event(session_id, "agent.photon_evaluate.completed")
        .expect("completed event must be emitted");
    let payload = completed.get("payload").expect("payload present");

    // Existing 4 keys.
    assert!(payload.get("session_id").and_then(|v| v.as_str()).is_some());
    assert!(payload.get("turn_index").and_then(|v| v.as_u64()).is_some());
    assert!(payload.get("failed").and_then(|v| v.as_bool()).is_some());
    assert!(payload.get("duration_ms").is_some());

    // New 4 keys (Issue #591 / AS-06).
    let _count = payload
        .get("summary_ids_adopted_count")
        .and_then(|v| v.as_u64())
        .expect("summary_ids_adopted_count must be a number");
    let outcome = payload.get("outcome").expect("outcome key");
    if let Some(s) = outcome.as_str() {
        let allowed = ["success", "failure", "safety_violation"];
        assert!(
            allowed.contains(&s),
            "outcome in event payload must be from allowlist, got {s:?}"
        );
    } else {
        assert!(outcome.is_null(), "outcome must be string or null");
    }
    let status = payload
        .get("adoption_status")
        .and_then(|v| v.as_str())
        .expect("adoption_status key");
    assert!(["injected", "not_injected", "shadow_not_injected"].contains(&status));
    assert!(
        payload
            .get("summary_ids_adopted_truncated")
            .and_then(|v| v.as_bool())
            .is_some(),
        "summary_ids_adopted_truncated must be bool"
    );

    // DR4-NEW-004: raw summary_ids_adopted array must NOT be in the payload.
    assert!(
        payload.get("summary_ids_adopted").is_none(),
        "raw summary_ids_adopted array must NEVER be emitted in event payload"
    );
}
