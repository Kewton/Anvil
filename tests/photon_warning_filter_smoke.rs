//! Issue #583 — photon warnings-based seed filter smoke suite.
//!
//! End-to-end tests covering the live HTTP path through `invoke_photon_context_pack`:
//!
//!   WF-09  ANVIL_PHOTON_RESPECT_WARNINGS=false → filter disabled, no warning_blocked
//!   WF-10  mockito response with warnings     → matching items skipped from prompt
//!   WF-11  shadow_mode=true                    → 0 warning_blocked events
//!   WF-12  canary=0                            → 0 warning_blocked events
//!   WF-13  photon_enabled=false                → 0 warning_blocked events
//!   SEC-04 warning_filter_enabled=false in completed event payload
//!   SEC-05 photon_url localhost validation still rejects external host
//!
//! The shared on-disk log is the process-global `init_logging` sink. Each test
//! uses a unique `session_id` so log assertions filter by session and remain
//! safe under cargo's parallel test execution (DR3-003).

use std::path::PathBuf;
use std::sync::OnceLock;

use anvil::agent::Agent;
use anvil::agent::loop_run::FooterHandle;
use anvil::cli::CliArgs;
use anvil::config::Config;
use anvil::model_registry::RuntimeModels;
use anvil::ollama::client::OllamaClient;
use anvil::session::store::{SessionSnapshot, SessionStore};
use tempfile::{TempDir, tempdir};

fn minimal_args(cwd: &std::path::Path) -> CliArgs {
    CliArgs {
        cwd: Some(cwd.to_path_buf()),
        prompt: None,
        model: None,
        provider: None,
        planner_model: None,
        planner_provider: None,
        sidecar_model: None,
        ollama_host: None,
        context_budget: None,
        num_predict: None,
        max_iterations: None,
        chat_timeout_secs: None,
        chat_retries: None,
        verbose: false,
        trace: false,
        debug: false,
        stream: false,
        yes: false,
        fresh_session: false,
        oneshot: false,
        auto_plan: false,
        plan_steps: None,
        plan_run: None,
        run_plan: None,
        ultra_plan: None,
        ultra_plan_run: None,
        run_ultra_plan: None,
        ultra_style: None,
        ultra_profile: None,
        offline: false,
        deterministic_fallback: None,
        engine: None,
        experimental_specialized_fallback: None,
        no_footer: false,
        resume: None,
        state_dir: None,
        command: None,
    }
}

// ---------------------------------------------------------------------------
// Shared logger setup (DR3-003)
// ---------------------------------------------------------------------------

static LOG_DIR: OnceLock<TempDir> = OnceLock::new();

/// Initialise the process-global llm-io logger pointed at a shared tempdir.
/// Returns the path to `llm-io.jsonl` for log assertions.
fn shared_log_path() -> PathBuf {
    let dir = LOG_DIR.get_or_init(|| {
        let dir = tempdir().expect("tempdir for shared log");
        let log_path = dir.path().join("llm-io.jsonl");
        // best-effort init; if init_logging has already been called by another
        // test, the OnceLock inside logging.rs will reject it silently.
        let _ = anvil::logging::init_logging(anvil::config::LogLevel::Info, &log_path);
        dir
    });
    dir.path().join("llm-io.jsonl")
}

/// Read all jsonl events for the given session_id.
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

fn count_events(session_id: &str, event_name: &str) -> usize {
    read_session_events(session_id)
        .into_iter()
        .filter(|rec| rec.get("event").and_then(|v| v.as_str()) == Some(event_name))
        .count()
}

fn find_first_event(session_id: &str, event_name: &str) -> Option<serde_json::Value> {
    read_session_events(session_id)
        .into_iter()
        .find(|rec| rec.get("event").and_then(|v| v.as_str()) == Some(event_name))
}

/// Build a mockito body for `/v1/context/pack` with the given items and
/// optional warnings. Items each carry `id=seed_<i>` so we can flag specific
/// entries by ID. Uses the real photon v0.2 sidecar layout where items and
/// warnings are nested under `context_pack` (Issue #587).
fn mock_context_pack_body(num_items: usize, warnings: serde_json::Value) -> String {
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
            "warnings": warnings,
        }
    })
    .to_string()
}

/// Build an Agent wired to the given photon mock URL.
fn build_live_agent(
    session_id: &str,
    photon_url: String,
    photon_shadow_mode: bool,
    photon_canary: u16,
    photon_enabled: bool,
    photon_respect_warnings: bool,
) -> (Agent, tempfile::TempDir) {
    let dir = tempdir().unwrap();
    let state_root = dir.path().join("state");
    std::fs::create_dir_all(state_root.join("sessions").join(session_id)).unwrap();

    let mut config = Config::default();
    config.cwd = dir.path().to_path_buf();
    config.photon_enabled = photon_enabled;
    config.photon_url = photon_url;
    config.photon_shadow_mode = photon_shadow_mode;
    config.photon_canary = photon_canary;
    config.photon_timeout_ms = 5000;
    config.photon_respect_warnings = photon_respect_warnings;
    config.requested_model = Some("test-model".to_string());
    config.state_dir_override = Some(state_root.clone());
    config.yes_mode = true;
    config.max_iterations = 1;

    let workspace_key = format!("anvil-583-{session_id}");
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
// WF-09 / SEC-04: filter disabled by config → no warning_blocked event
// ---------------------------------------------------------------------------

#[test]
fn wf09_filter_disabled_no_warning_blocked_event() {
    let _ = shared_log_path();
    let session_id = "wf09-filter-disabled";

    let mut photon_server = mockito::Server::new();
    let body = mock_context_pack_body(
        3,
        serde_json::json!([
            { "kind": "summary_quality_gate", "message": "seed_0: premature_termination_risk" }
        ]),
    );
    let _pack_mock = photon_server
        .mock("POST", "/v1/context/pack")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(body)
        .create();

    let (mut agent, _dir) = build_live_agent(
        session_id,
        photon_server.url(),
        false, // shadow_mode
        1000,  // canary always-on
        true,  // photon_enabled
        false, // photon_respect_warnings = OFF
    );
    let _ = agent.process_line("hello", false);

    assert_eq!(
        count_events(session_id, "agent.photon_context_pack.warning_blocked"),
        0,
        "warning_blocked must NOT fire when photon_respect_warnings=false"
    );

    // SEC-04: completed event still emits warning_filter_enabled=false
    let completed = find_first_event(session_id, "agent.photon_context_pack.completed")
        .expect("completed event must be emitted on live path");
    assert_eq!(
        completed
            .get("payload")
            .and_then(|p| p.get("warning_filter_enabled"))
            .and_then(|v| v.as_bool()),
        Some(false),
        "completed payload must record warning_filter_enabled=false for audit"
    );
}

// ---------------------------------------------------------------------------
// WF-10: live mockito round-trip → blocked item is skipped
// ---------------------------------------------------------------------------

#[test]
fn wf10_live_warning_blocks_matching_item() {
    let _ = shared_log_path();
    let session_id = "wf10-live-blocks";

    let mut photon_server = mockito::Server::new();
    let body = mock_context_pack_body(
        3,
        serde_json::json!([
            { "kind": "summary_quality_gate", "message": "seed_1: premature_termination_risk" }
        ]),
    );
    let pack_mock = photon_server
        .mock("POST", "/v1/context/pack")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(body)
        .expect(1)
        .create();

    let (mut agent, _dir) = build_live_agent(
        session_id,
        photon_server.url(),
        false, // shadow_mode off → live injection
        1000,  // canary always-on
        true,  // photon_enabled
        true,  // photon_respect_warnings ON
    );
    let _ = agent.process_line("hello", false);

    pack_mock.assert();

    // warning_blocked must fire exactly once and reference seed_1 only.
    let blocked_events: Vec<_> = read_session_events(session_id)
        .into_iter()
        .filter(|rec| {
            rec.get("event").and_then(|v| v.as_str())
                == Some("agent.photon_context_pack.warning_blocked")
        })
        .collect();
    assert_eq!(blocked_events.len(), 1, "exactly one warning_blocked event");
    let payload = blocked_events[0].get("payload").expect("payload present");
    let ids = payload
        .get("blocked_summary_ids")
        .and_then(|v| v.as_array())
        .expect("blocked_summary_ids array");
    let ids: Vec<&str> = ids.iter().filter_map(|v| v.as_str()).collect();
    assert_eq!(ids, vec!["seed_1"]);
    assert_eq!(
        payload.get("total_warnings").and_then(|v| v.as_u64()),
        Some(1)
    );
    assert_eq!(
        payload.get("total_blocked").and_then(|v| v.as_u64()),
        Some(1)
    );

    // completed event records items_blocked=1.
    let completed = find_first_event(session_id, "agent.photon_context_pack.completed")
        .expect("completed event");
    let cp = completed.get("payload").unwrap();
    assert_eq!(cp.get("items_blocked").and_then(|v| v.as_u64()), Some(1));
    assert_eq!(
        cp.get("warning_filter_enabled").and_then(|v| v.as_bool()),
        Some(true)
    );
    // Two items survived → items_adopted == 2.
    assert_eq!(cp.get("items_adopted").and_then(|v| v.as_u64()), Some(2));
}

// ---------------------------------------------------------------------------
// WF-11: shadow_mode=true → 0 warning_blocked events
// ---------------------------------------------------------------------------

#[test]
fn wf11_shadow_mode_no_warning_blocked_event() {
    let _ = shared_log_path();
    let session_id = "wf11-shadow-mode";

    let mut photon_server = mockito::Server::new();
    let body = mock_context_pack_body(
        2,
        serde_json::json!([
            { "kind": "summary_quality_gate", "message": "seed_0: premature_termination_risk" }
        ]),
    );
    // shadow mode short-circuits before fetch — the mock must NOT be hit.
    let _pack_mock = photon_server
        .mock("POST", "/v1/context/pack")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(body)
        .expect(0)
        .create();

    let (mut agent, _dir) = build_live_agent(
        session_id,
        photon_server.url(),
        true, // shadow_mode ON → skip
        1000,
        true,
        true,
    );
    let _ = agent.process_line("hello", false);

    assert_eq!(
        count_events(session_id, "agent.photon_context_pack.warning_blocked"),
        0,
        "warning_blocked must not fire in shadow_mode"
    );
}

// ---------------------------------------------------------------------------
// WF-12: canary=0 → 0 warning_blocked events
// ---------------------------------------------------------------------------

#[test]
fn wf12_canary_zero_no_warning_blocked_event() {
    let _ = shared_log_path();
    let session_id = "wf12-canary-zero";

    let mut photon_server = mockito::Server::new();
    let _pack_mock = photon_server
        .mock("POST", "/v1/context/pack")
        .expect(0)
        .create();

    let (mut agent, _dir) = build_live_agent(
        session_id,
        photon_server.url(),
        false, // shadow_mode off
        0,     // canary 0 → gate fires before HTTP
        true,
        true,
    );
    let _ = agent.process_line("hello", false);

    assert_eq!(
        count_events(session_id, "agent.photon_context_pack.warning_blocked"),
        0,
        "warning_blocked must not fire when canary=0"
    );
}

// ---------------------------------------------------------------------------
// WF-13: photon_enabled=false → 0 warning_blocked events
// ---------------------------------------------------------------------------

#[test]
fn wf13_photon_disabled_no_warning_blocked_event() {
    let _ = shared_log_path();
    let session_id = "wf13-photon-disabled";

    let mut photon_server = mockito::Server::new();
    let _pack_mock = photon_server
        .mock("POST", "/v1/context/pack")
        .expect(0)
        .create();

    let (mut agent, _dir) = build_live_agent(
        session_id,
        photon_server.url(),
        false,
        1000,
        false, // photon_enabled=false → no PhotonClient constructed
        true,
    );
    let _ = agent.process_line("hello", false);

    assert_eq!(
        count_events(session_id, "agent.photon_context_pack.warning_blocked"),
        0,
        "warning_blocked must not fire when photon is disabled"
    );
}

// ---------------------------------------------------------------------------
// SEC-05: external photon_url is still rejected by validate_localhost_url.
// ---------------------------------------------------------------------------

#[test]
fn sec05_external_photon_url_rejected_by_config_load() {
    // SAFETY: we only set/unset env vars within this test; cargo runs tests
    // in parallel so use a unique value to avoid clashes.
    let tmp = tempdir().unwrap();

    // Save and restore the env var around the test body.
    let prev = std::env::var("ANVIL_PHOTON_URL").ok();
    // SAFETY: env::set_var / remove_var are unsafe in the 2024 edition.
    unsafe {
        std::env::set_var("ANVIL_PHOTON_URL", "http://example.com:3030");
    }
    let result = Config::load(minimal_args(tmp.path()));
    unsafe {
        match prev {
            Some(v) => std::env::set_var("ANVIL_PHOTON_URL", v),
            None => std::env::remove_var("ANVIL_PHOTON_URL"),
        }
    }
    assert!(
        result.is_err(),
        "external photon_url must still be rejected"
    );
    let err = result.unwrap_err();
    assert!(
        err.contains("photon_url"),
        "expected 'photon_url' in error: {err}"
    );
}
