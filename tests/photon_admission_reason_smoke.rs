//! Issue #589 — photon admission_reason-based softer block smoke suite.
//!
//! End-to-end tests covering the live HTTP path through
//! `invoke_photon_context_pack`. The photon sidecar may emit a
//! `summary_quality_gate` warning for an item *and* simultaneously mitigate the
//! premature-termination risk via `admission_reason: "...next_hints suppressed: ..."`.
//! In that case the item must NOT be removed from the rendered prompt; instead
//! the block set must subtract the photon-handled IDs and the
//! `warning_blocked` event must record the bookkeeping for audit.
//!
//!   AR-01  normal partial admit → block from being skipped, event payload
//!   AR-02  marker substring match (text around the marker)
//!   AR-03  no admission_reason field → block kept (backward compatible)
//!   AR-04  control / bidi chars in reason → block kept (fail-closed)
//!   AR-05  1024 byte reason → block kept (>512 byte cap, fail-closed)
//!   AR-06  reason contains `token` → block kept (secret-like)
//!   AR-07  `context_pack.items[].admission_reason` (v0.2 nested layout)
//!
//! Each test uses a unique `session_id` so the shared `init_logging` log can be
//! filtered safely under cargo's parallel test execution (DR3-003).

use std::path::PathBuf;
use std::sync::OnceLock;

use anvil::agent::Agent;
use anvil::agent::loop_run::FooterHandle;
use anvil::config::Config;
use anvil::model_registry::RuntimeModels;
use anvil::ollama::client::OllamaClient;
use anvil::session::store::{SessionSnapshot, SessionStore};
use tempfile::{TempDir, tempdir};

// ---------------------------------------------------------------------------
// Shared logger setup (DR3-003)
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

/// Build a v0.2 nested context_pack body. `items` and `warnings` are placed
/// under the `context_pack` key, matching the real photon sidecar layout.
fn mock_body_nested(items: serde_json::Value, warnings: serde_json::Value) -> String {
    serde_json::json!({
        "schema_version": "action-memory.v0.2",
        "context_pack": {
            "items": items,
            "warnings": warnings,
        }
    })
    .to_string()
}

fn build_live_agent(session_id: &str, photon_url: String) -> (Agent, tempfile::TempDir) {
    let dir = tempdir().unwrap();
    let state_root = dir.path().join("state");
    std::fs::create_dir_all(state_root.join("sessions").join(session_id)).unwrap();

    let mut config = Config::default();
    config.cwd = dir.path().to_path_buf();
    config.photon_enabled = true;
    config.photon_url = photon_url;
    config.photon_shadow_mode = false;
    config.photon_canary = 1000;
    config.photon_timeout_ms = 5000;
    config.photon_respect_warnings = true;
    config.requested_model = Some("test-model".to_string());
    config.state_dir_override = Some(state_root.clone());
    config.yes_mode = true;
    config.max_iterations = 1;

    let workspace_key = format!("anvil-589-{session_id}");
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

// ---------------------------------------------------------------------------
// AR-01: normal partial admit
// ---------------------------------------------------------------------------

#[test]
fn ar01_admission_reason_removes_id_from_block_set() {
    let _ = shared_log_path();
    let session_id = "ar01-partial-admit";

    let mut photon_server = mockito::Server::new();
    // seed_1 is flagged by warnings AND carries an admission_reason saying the
    // sidecar already handled the risk → the block must be respected.
    let items = serde_json::json!([
        { "kind": "summary", "id": "seed_0", "summary": "alpha body" },
        {
            "kind": "summary",
            "id": "seed_1",
            "summary": "beta body",
            "admission_reason": "next_hints suppressed: tail truncated"
        },
        { "kind": "summary", "id": "seed_2", "summary": "gamma body" },
    ]);
    let warnings = serde_json::json!([
        { "kind": "summary_quality_gate", "message": "seed_1: premature_termination_risk" }
    ]);
    let body = mock_body_nested(items, warnings);
    let pack_mock = photon_server
        .mock("POST", "/v1/context/pack")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(body)
        .expect(1)
        .create();

    let (mut agent, _dir) = build_live_agent(session_id, photon_server.url());
    let _ = agent.process_line("hello", false);

    pack_mock.assert();

    // warning_blocked event must fire (admission_reason removed 1 id),
    // blocked_summary_ids array must be empty (no IDs left blocked).
    let evt = find_first_event(session_id, "agent.photon_context_pack.warning_blocked")
        .expect("warning_blocked event must fire when admission_reason takes effect");
    let payload = evt.get("payload").expect("payload");
    assert_eq!(
        payload
            .get("respected_by_admission_reason")
            .and_then(|v| v.as_u64()),
        Some(1),
        "1 id must be removed by admission_reason"
    );
    assert_eq!(
        payload.get("still_blocked").and_then(|v| v.as_u64()),
        Some(0),
        "no ids must remain in the block set"
    );
    let ids = payload
        .get("blocked_summary_ids")
        .and_then(|v| v.as_array())
        .expect("blocked_summary_ids array");
    assert!(
        ids.is_empty(),
        "blocked_summary_ids must be empty after admission_reason subtraction"
    );

    // completed event reports items_blocked=0 (no render-side skip).
    let completed = find_first_event(session_id, "agent.photon_context_pack.completed")
        .expect("completed event");
    let cp = completed.get("payload").unwrap();
    assert_eq!(
        cp.get("items_blocked").and_then(|v| v.as_u64()),
        Some(0),
        "items_blocked must be 0 — beta body survives via admission_reason"
    );
    // All three items should be adopted.
    assert_eq!(
        cp.get("items_adopted").and_then(|v| v.as_u64()),
        Some(3),
        "all 3 items must be adopted into the prompt"
    );
}

// ---------------------------------------------------------------------------
// AR-02: marker substring match (text before / after the marker)
// ---------------------------------------------------------------------------

#[test]
fn ar02_admission_reason_marker_substring_match() {
    let _ = shared_log_path();
    let session_id = "ar02-marker-substring";

    let mut photon_server = mockito::Server::new();
    let items = serde_json::json!([
        {
            "kind": "summary",
            "id": "seed_a",
            "summary": "alpha body",
            "admission_reason": "tail clipped; next_hints suppressed by sidecar policy."
        },
    ]);
    let warnings = serde_json::json!([
        { "kind": "summary_quality_gate", "message": "seed_a: premature_termination_risk" }
    ]);
    let body = mock_body_nested(items, warnings);
    let _pack = photon_server
        .mock("POST", "/v1/context/pack")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(body)
        .expect(1)
        .create();

    let (mut agent, _dir) = build_live_agent(session_id, photon_server.url());
    let _ = agent.process_line("hello", false);

    let evt = find_first_event(session_id, "agent.photon_context_pack.warning_blocked")
        .expect("warning_blocked event must fire with substring match");
    let payload = evt.get("payload").unwrap();
    assert_eq!(
        payload
            .get("respected_by_admission_reason")
            .and_then(|v| v.as_u64()),
        Some(1)
    );
    assert_eq!(
        payload.get("still_blocked").and_then(|v| v.as_u64()),
        Some(0)
    );
}

// ---------------------------------------------------------------------------
// AR-03: no admission_reason → block stays in effect (backward compatible)
// ---------------------------------------------------------------------------

#[test]
fn ar03_no_admission_reason_keeps_block() {
    let _ = shared_log_path();
    let session_id = "ar03-no-admission-reason";

    let mut photon_server = mockito::Server::new();
    let items = serde_json::json!([
        { "kind": "summary", "id": "seed_x", "summary": "to block" },
    ]);
    let warnings = serde_json::json!([
        { "kind": "summary_quality_gate", "message": "seed_x: premature_termination_risk" }
    ]);
    let body = mock_body_nested(items, warnings);
    let _pack = photon_server
        .mock("POST", "/v1/context/pack")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(body)
        .expect(1)
        .create();

    let (mut agent, _dir) = build_live_agent(session_id, photon_server.url());
    let _ = agent.process_line("hello", false);

    let evt = find_first_event(session_id, "agent.photon_context_pack.warning_blocked")
        .expect("warning_blocked event must fire for backward-compat path");
    let payload = evt.get("payload").unwrap();
    assert_eq!(
        payload
            .get("respected_by_admission_reason")
            .and_then(|v| v.as_u64()),
        Some(0),
        "no admission_reason → 0 ids respected"
    );
    assert_eq!(
        payload.get("still_blocked").and_then(|v| v.as_u64()),
        Some(1),
        "1 id remains blocked (Issue #583 behaviour preserved)"
    );
    let ids = payload
        .get("blocked_summary_ids")
        .and_then(|v| v.as_array())
        .unwrap();
    assert_eq!(ids.len(), 1);
    assert_eq!(ids[0].as_str(), Some("seed_x"));
}

// ---------------------------------------------------------------------------
// AR-04: bidi / control characters in admission_reason → block kept (fail-closed)
// ---------------------------------------------------------------------------

#[test]
fn ar04_bidi_control_in_admission_reason_keeps_block() {
    let _ = shared_log_path();
    let session_id = "ar04-bidi-control";

    let mut photon_server = mockito::Server::new();
    // Marker surrounded by control / bidi chars — the fail-closed sanitiser
    // must reject the whole reason and keep the item blocked.
    let items = serde_json::json!([
        {
            "kind": "summary",
            "id": "seed_b",
            "summary": "body b",
            "admission_reason": "next_hints suppressed\u{202e}rev"
        },
    ]);
    let warnings = serde_json::json!([
        { "kind": "summary_quality_gate", "message": "seed_b: premature_termination_risk" }
    ]);
    let body = mock_body_nested(items, warnings);
    let _pack = photon_server
        .mock("POST", "/v1/context/pack")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(body)
        .expect(1)
        .create();

    let (mut agent, _dir) = build_live_agent(session_id, photon_server.url());
    let _ = agent.process_line("hello", false);

    let evt = find_first_event(session_id, "agent.photon_context_pack.warning_blocked")
        .expect("warning_blocked event must fire when bidi/control bypass attempted");
    let payload = evt.get("payload").unwrap();
    assert_eq!(
        payload
            .get("respected_by_admission_reason")
            .and_then(|v| v.as_u64()),
        Some(0),
        "fail-closed: bidi/control chars must not bypass the block"
    );
    assert_eq!(
        payload.get("still_blocked").and_then(|v| v.as_u64()),
        Some(1)
    );
}

// ---------------------------------------------------------------------------
// AR-05: > 512 byte admission_reason → block kept (fail-closed)
// ---------------------------------------------------------------------------

#[test]
fn ar05_oversize_admission_reason_keeps_block() {
    let _ = shared_log_path();
    let session_id = "ar05-oversize-reason";

    // 1024 bytes total, includes the marker but exceeds MAX_PHOTON_ADMISSION_REASON_BYTES=512.
    let reason = format!("{} next_hints suppressed", "x".repeat(1024));
    let mut photon_server = mockito::Server::new();
    let items = serde_json::json!([
        {
            "kind": "summary",
            "id": "seed_o",
            "summary": "oversize body",
            "admission_reason": reason,
        },
    ]);
    let warnings = serde_json::json!([
        { "kind": "summary_quality_gate", "message": "seed_o: premature_termination_risk" }
    ]);
    let body = mock_body_nested(items, warnings);
    let _pack = photon_server
        .mock("POST", "/v1/context/pack")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(body)
        .expect(1)
        .create();

    let (mut agent, _dir) = build_live_agent(session_id, photon_server.url());
    let _ = agent.process_line("hello", false);

    let evt = find_first_event(session_id, "agent.photon_context_pack.warning_blocked")
        .expect("warning_blocked event must fire when oversize bypass attempted");
    let payload = evt.get("payload").unwrap();
    assert_eq!(
        payload
            .get("respected_by_admission_reason")
            .and_then(|v| v.as_u64()),
        Some(0),
        "fail-closed: oversize reason must not bypass the block"
    );
    assert_eq!(
        payload.get("still_blocked").and_then(|v| v.as_u64()),
        Some(1)
    );
}

// ---------------------------------------------------------------------------
// AR-06: secret-like word in admission_reason → block kept (fail-closed)
// ---------------------------------------------------------------------------

#[test]
fn ar06_secret_like_admission_reason_keeps_block() {
    let _ = shared_log_path();
    let session_id = "ar06-secret-like";

    let mut photon_server = mockito::Server::new();
    let items = serde_json::json!([
        {
            "kind": "summary",
            "id": "seed_s",
            "summary": "body s",
            // marker present but reason also contains a secret-like word → reject.
            "admission_reason": "next_hints suppressed (token leaked, scrub)"
        },
    ]);
    let warnings = serde_json::json!([
        { "kind": "summary_quality_gate", "message": "seed_s: premature_termination_risk" }
    ]);
    let body = mock_body_nested(items, warnings);
    let _pack = photon_server
        .mock("POST", "/v1/context/pack")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(body)
        .expect(1)
        .create();

    let (mut agent, _dir) = build_live_agent(session_id, photon_server.url());
    let _ = agent.process_line("hello", false);

    let evt = find_first_event(session_id, "agent.photon_context_pack.warning_blocked")
        .expect("warning_blocked event must fire for secret-like reason");
    let payload = evt.get("payload").unwrap();
    assert_eq!(
        payload
            .get("respected_by_admission_reason")
            .and_then(|v| v.as_u64()),
        Some(0),
        "fail-closed: secret-like reason must not bypass the block"
    );
    assert_eq!(
        payload.get("still_blocked").and_then(|v| v.as_u64()),
        Some(1)
    );
}

// ---------------------------------------------------------------------------
// AR-07: nested context_pack.items[].admission_reason layout (v0.2 sidecar)
// ---------------------------------------------------------------------------

#[test]
fn ar07_nested_layout_admission_reason_detected() {
    let _ = shared_log_path();
    let session_id = "ar07-nested-layout";

    let mut photon_server = mockito::Server::new();
    // The nested layout is already exercised by mock_body_nested; this test
    // pins the assertion explicitly so a regression that loses
    // `context_pack.items[].admission_reason` lookup is caught.
    let items = serde_json::json!([
        {
            "kind": "summary",
            "id": "seed_n",
            "summary": "nested body",
            "admission_reason": "next_hints suppressed"
        },
    ]);
    let warnings = serde_json::json!([
        { "kind": "summary_quality_gate", "message": "seed_n: premature_termination_risk" }
    ]);
    let body = mock_body_nested(items, warnings);
    let _pack = photon_server
        .mock("POST", "/v1/context/pack")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(body)
        .expect(1)
        .create();

    let (mut agent, _dir) = build_live_agent(session_id, photon_server.url());
    let _ = agent.process_line("hello", false);

    let evt = find_first_event(session_id, "agent.photon_context_pack.warning_blocked")
        .expect("warning_blocked event must fire on nested layout");
    let payload = evt.get("payload").unwrap();
    assert_eq!(
        payload
            .get("respected_by_admission_reason")
            .and_then(|v| v.as_u64()),
        Some(1),
        "nested layout must be detected by extract_photon_handled_ids"
    );
    assert_eq!(
        payload.get("still_blocked").and_then(|v| v.as_u64()),
        Some(0)
    );

    let completed = find_first_event(session_id, "agent.photon_context_pack.completed")
        .expect("completed event");
    let cp = completed.get("payload").unwrap();
    assert_eq!(
        cp.get("items_blocked").and_then(|v| v.as_u64()),
        Some(0),
        "nested item survives the filter"
    );
    assert_eq!(
        cp.get("items_adopted").and_then(|v| v.as_u64()),
        Some(1),
        "the only item must be adopted"
    );
}

// ---------------------------------------------------------------------------
// AR-08 (CB-001 regression): non-summary item with the same id must NOT
// release the block on the corresponding summary item.
// ---------------------------------------------------------------------------

#[test]
fn ar08_non_summary_kind_with_same_id_keeps_block() {
    let _ = shared_log_path();
    let session_id = "ar08-non-summary-same-id";

    let mut photon_server = mockito::Server::new();
    // seed_x has TWO items sharing the same id:
    //   (1) kind="summary"  with NO admission_reason  → would normally be blocked
    //   (2) kind="log"      with admission_reason marker → must NOT release block
    let items = serde_json::json!([
        {
            "kind": "summary",
            "id": "seed_x",
            "summary": "summary body"
        },
        {
            "kind": "log",
            "id": "seed_x",
            "summary": "log body",
            "admission_reason": "next_hints suppressed: premature_termination_risk"
        },
    ]);
    let warnings = serde_json::json!([
        { "kind": "summary_quality_gate", "message": "seed_x: premature_termination_risk" }
    ]);
    let body = mock_body_nested(items, warnings);
    let _pack = photon_server
        .mock("POST", "/v1/context/pack")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(body)
        .expect(1)
        .create();

    let (mut agent, _dir) = build_live_agent(session_id, photon_server.url());
    let _ = agent.process_line("hello", false);

    let evt = find_first_event(session_id, "agent.photon_context_pack.warning_blocked")
        .expect("warning_blocked event must fire — block must not be released by non-summary item");
    let payload = evt.get("payload").unwrap();
    assert_eq!(
        payload
            .get("respected_by_admission_reason")
            .and_then(|v| v.as_u64()),
        Some(0),
        "CB-001: non-summary item must not unblock the summary"
    );
    assert_eq!(
        payload.get("still_blocked").and_then(|v| v.as_u64()),
        Some(1),
        "summary remains blocked because only summary-kind admission_reason counts"
    );
}
