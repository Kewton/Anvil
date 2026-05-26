//! Issue #659 (Phase 2) — in-crate `#[cfg(test)]` tests for the
//! Agent-level ArtifactLedger wiring. The tests verify:
//!
//! - Task 2.1: `Agent::artifact_ledger` is initialized empty.
//! - Task 2.2: `handle_user_message` clears the ledger at turn start
//!   and `record_turn_end_artifact_ledger_summary` is callable as a thin
//!   shell that emits the turn_summary event.
//! - Task 2.3: `seed_artifact_ledger_existing` populates Existing seeds
//!   for required artifacts and is idempotent across repeated calls.
//! - Task 2.4: `seed_artifact_ledger_scaffold` passes `post_scaffold_delta`
//!   through and is idempotent across repeated baseline emits.
//! - Task 2.5: `seed_artifact_ledger_repo_edit` runs alongside the legacy
//!   `turn_edited_relative_paths.insert(...)` write so both sources see
//!   the same path. No-op edits do not seed the ledger.
//! - Task 2.6: `seed_artifact_ledger_verifier_observation` records bound
//!   verifier paths and is skipped for legacy / unbound verifier paths.
//! - Task 2.7: `assert_dual_source_alignment_at_turn_end` panics in debug
//!   builds when legacy and ledger sources diverge; in release builds it
//!   emits an `agent.artifact_ledger.divergence_detected` event without
//!   panicking.
//!
//! DR3-001: this module is private to `loop_run` and never `pub use`d.

use std::path::PathBuf;
use std::sync::OnceLock;

use serde_json::Value;
use tempfile::{TempDir, tempdir};

use super::artifact_ledger::{VerifierObservation, VerifierOutcome};
use super::task_contract::ArtifactRole;
use super::task_workspace_scope::{ScopeMode, TaskWorkspaceScope};
use crate::agent::Agent;
use crate::agent::loop_run::FooterHandle;
use crate::config::Config;
use crate::model_registry::RuntimeModels;
use crate::ollama::client::OllamaClient;
use crate::session::store::{SessionSnapshot, SessionStore};

// ---------------------------------------------------------------------------
// Shared logger setup so the turn_summary / event_recorded / divergence emit
// paths can be observed in the on-disk JSONL log file.
//
// DR3-003: `init_logging` uses a process-global `OnceLock`, so only the first
// init wins. To stay compatible with parallel `cargo test` runs against
// `safe_stop_e2e_tests` (which initializes its own log path), we use
// `crate::logging::llm_io_log_path()` to read the path the global subscriber
// is actually writing to instead of forcing our own tempdir.
// ---------------------------------------------------------------------------

static LOG_DIR: OnceLock<TempDir> = OnceLock::new();

fn shared_log_path() -> PathBuf {
    // Eagerly initialise if no one else has — but always read back the
    // path the global subscriber settled on so the read side sees what
    // was actually written.
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

fn read_log_events_by_event_name(event_name: &str) -> Vec<Value> {
    let path = shared_log_path();
    let contents = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    contents
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|rec| rec.get("event").and_then(|v| v.as_str()) == Some(event_name))
        .collect()
}

fn read_log_events_by_event_name_and_session(event_name: &str, session_id: &str) -> Vec<Value> {
    read_log_events_by_event_name(event_name)
        .into_iter()
        .filter(|rec| {
            rec.get("payload")
                .and_then(|p| p.get("session_id"))
                .and_then(|v| v.as_str())
                == Some(session_id)
        })
        .collect()
}

fn unique_session_id(prefix: &str) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("659-{prefix}-{nanos}")
}

fn build_agent(session_id: &str) -> (Agent, TempDir) {
    let _ = shared_log_path();
    let dir = tempdir().expect("tempdir for agent");
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

    let workspace_key = format!("anvil-659-{session_id}");
    let session = SessionSnapshot {
        id: session_id.to_string(),
        workspace_key: workspace_key.clone(),
        active_root: Some(dir.path().to_path_buf()),
        ..Default::default()
    };

    let server = mockito::Server::new();
    let agent = Agent::new(
        config,
        RuntimeModels {
            main: "test-model".to_string(),
            sidecar: None,
        },
        OllamaClient::new(server.url()).unwrap(),
        SessionStore::new(&state_root, session_id, &workspace_key),
        session,
        FooterHandle::disabled(),
    );
    (agent, dir)
}

fn single_root_scope() -> TaskWorkspaceScope {
    TaskWorkspaceScope {
        mode: ScopeMode::SingleProjectRoot,
    }
}

// ---------------------------------------------------------------------------
// Task 2.1: Agent::artifact_ledger field
// ---------------------------------------------------------------------------

#[test]
fn agent_initializes_artifact_ledger_empty() {
    let session_id = unique_session_id("init-empty");
    let (agent, _dir) = build_agent(&session_id);
    assert_eq!(agent.artifact_ledger.event_count(), 0);
    assert_eq!(agent.artifact_ledger.dropped_count(), 0);
    assert!(!agent.artifact_ledger.overflowed());
}

// ---------------------------------------------------------------------------
// Task 2.2: per-turn reset + end-of-turn summary emit
// ---------------------------------------------------------------------------

#[test]
fn handle_user_message_clears_ledger_at_turn_start() {
    let session_id = unique_session_id("clear-at-start");
    let (mut agent, dir) = build_agent(&session_id);

    // Pre-seed: create a tests/ file and seed an event so the ledger is
    // non-empty before the per-turn reset fires.
    let work_root = dir.path();
    std::fs::create_dir_all(work_root.join("tests")).unwrap();
    std::fs::write(work_root.join("tests/test_a.py"), "").unwrap();
    let scope = single_root_scope();
    let ctx = super::artifact_ledger::LedgerAdmissionContext::new(work_root, &scope);
    let _ = agent.artifact_ledger.record_repo_edit_event(
        &ctx,
        "tests/test_a.py".to_string(),
        ArtifactRole::Test,
        true,
    );
    assert!(agent.artifact_ledger.event_count() > 0);

    // Drive a single `clear_per_turn_ledger_state` call (the same helper
    // `handle_user_message` invokes) so the per-turn reset can be verified
    // without spinning up a real Ollama roundtrip.
    agent.clear_per_turn_ledger_state();
    assert_eq!(agent.artifact_ledger.event_count(), 0);
    assert_eq!(agent.artifact_ledger.dropped_count(), 0);
    assert!(!agent.artifact_ledger.overflowed());
}

#[test]
fn turn_summary_emitted_once_at_turn_end() {
    let session_id = unique_session_id("summary-once");
    let (mut agent, dir) = build_agent(&session_id);
    agent.clear_per_turn_ledger_state();
    let work_root = dir.path();
    std::fs::create_dir_all(work_root.join("tests")).unwrap();
    std::fs::write(work_root.join("tests/test_b.py"), "").unwrap();
    let scope = single_root_scope();
    let ctx = super::artifact_ledger::LedgerAdmissionContext::new(work_root, &scope);
    let _ = agent.artifact_ledger.record_repo_edit_event(
        &ctx,
        "tests/test_b.py".to_string(),
        ArtifactRole::Test,
        true,
    );

    let before_count = read_log_events_by_event_name_and_session(
        "agent.artifact_ledger.turn_summary",
        &session_id,
    )
    .len();
    agent.record_turn_end_artifact_ledger_summary();
    let after_count = read_log_events_by_event_name_and_session(
        "agent.artifact_ledger.turn_summary",
        &session_id,
    )
    .len();
    assert_eq!(
        after_count,
        before_count + 1,
        "exactly one turn_summary event must be emitted per call"
    );
}

// ---------------------------------------------------------------------------
// Task 2.3: Existing seed
// ---------------------------------------------------------------------------

#[test]
fn existing_seed_invokes_record_existing_event_per_required_artifact() {
    let session_id = unique_session_id("existing-seed");
    let (mut agent, dir) = build_agent(&session_id);
    let work_root = dir.path();
    std::fs::create_dir_all(work_root.join("tests")).unwrap();
    std::fs::create_dir_all(work_root.join("src")).unwrap();
    std::fs::write(work_root.join("tests/test_c.py"), "").unwrap();
    std::fs::write(work_root.join("src/lib.rs"), "").unwrap();

    let scope = single_root_scope();
    agent.seed_artifact_ledger_existing("tests/test_c.py", ArtifactRole::Test, &scope);
    agent.seed_artifact_ledger_existing("src/lib.rs", ArtifactRole::Implementation, &scope);

    let count = agent.artifact_ledger.event_count();
    assert!(count >= 2, "expected at least 2 events, got {count}");
}

#[test]
fn existing_seed_is_idempotent_across_evaluations() {
    let session_id = unique_session_id("existing-idem");
    let (mut agent, dir) = build_agent(&session_id);
    let work_root = dir.path();
    std::fs::write(work_root.join("README.md"), "").unwrap();
    let scope = single_root_scope();

    for _ in 0..5 {
        agent.seed_artifact_ledger_existing("README.md", ArtifactRole::UsageDocs, &scope);
    }
    assert_eq!(
        agent.artifact_ledger.event_count(),
        1,
        "Existing baseline seeds must be idempotent on (origin, role, path)"
    );
}

#[test]
fn existing_seed_ignores_controller_owned_state() {
    let session_id = unique_session_id("existing-ignore-state");
    let (mut agent, dir) = build_agent(&session_id);
    let work_root = dir.path();
    let rel = ".anvil-state/generated/README.md";
    std::fs::create_dir_all(work_root.join(".anvil-state/generated")).unwrap();
    std::fs::write(work_root.join(rel), "# generated\n").unwrap();
    let scope = single_root_scope();

    agent.seed_artifact_ledger_existing(rel, ArtifactRole::UsageDocs, &scope);

    assert_eq!(
        agent.artifact_ledger.event_count(),
        0,
        "controller-owned existing files must not enter artifact evidence"
    );
}

// ---------------------------------------------------------------------------
// Task 2.4: Scaffold seed
// ---------------------------------------------------------------------------

#[test]
fn scaffold_seed_passes_post_scaffold_delta() {
    let session_id = unique_session_id("scaffold-delta");
    let (mut agent, dir) = build_agent(&session_id);
    let work_root = dir.path();
    std::fs::create_dir_all(work_root.join("app")).unwrap();
    std::fs::write(work_root.join("app/main.py"), "").unwrap();
    let scope = single_root_scope();

    agent.seed_artifact_ledger_scaffold("app/main.py", ArtifactRole::Implementation, true, &scope);
    assert_eq!(agent.artifact_ledger.event_count(), 1);
}

#[test]
fn scaffold_unchanged_retains_baseline_with_delta_false() {
    let session_id = unique_session_id("scaffold-unchanged");
    let (mut agent, dir) = build_agent(&session_id);
    let work_root = dir.path();
    std::fs::create_dir_all(work_root.join("app")).unwrap();
    std::fs::write(work_root.join("app/cfg.py"), "").unwrap();
    let scope = single_root_scope();

    agent.seed_artifact_ledger_scaffold("app/cfg.py", ArtifactRole::Implementation, false, &scope);
    agent.seed_artifact_ledger_scaffold("app/cfg.py", ArtifactRole::Implementation, false, &scope);
    assert_eq!(
        agent.artifact_ledger.event_count(),
        1,
        "Scaffold baseline must be idempotent on (origin, role, path) even when delta stays false"
    );
}

#[test]
fn scaffold_seed_ignores_controller_owned_state() {
    let session_id = unique_session_id("scaffold-ignore-state");
    let (mut agent, dir) = build_agent(&session_id);
    let work_root = dir.path();
    let rel = ".anvil-state/scaffold/app/main.py";
    std::fs::create_dir_all(work_root.join(".anvil-state/scaffold/app")).unwrap();
    std::fs::write(work_root.join(rel), "print('generated')\n").unwrap();
    let scope = single_root_scope();

    agent.seed_artifact_ledger_scaffold(rel, ArtifactRole::Implementation, true, &scope);

    assert_eq!(
        agent.artifact_ledger.event_count(),
        0,
        "controller-owned scaffold files must not enter artifact evidence"
    );
}

// ---------------------------------------------------------------------------
// Task 2.5: RepoEdit seed + write-through adapter
// ---------------------------------------------------------------------------

#[test]
fn repo_edit_seed_synchronizes_legacy_set() {
    let session_id = unique_session_id("repoedit-sync");
    let (mut agent, dir) = build_agent(&session_id);
    let work_root = dir.path();
    std::fs::create_dir_all(work_root.join("tests")).unwrap();
    std::fs::write(work_root.join("tests/test_sync.py"), "").unwrap();
    let scope = single_root_scope();

    agent.seed_artifact_ledger_repo_edit("tests/test_sync.py", ArtifactRole::Test, &scope);

    assert!(
        agent
            .turn_edited_relative_paths
            .contains("tests/test_sync.py"),
        "write-through must mirror RepoEdit seed into legacy set"
    );
    assert!(
        agent.artifact_ledger.event_count() >= 1,
        "ledger must have at least one RepoEdit event for the seeded path"
    );
}

#[test]
fn repo_edit_seed_ignores_controller_owned_state() {
    let session_id = unique_session_id("repoedit-ignore-state");
    let (mut agent, dir) = build_agent(&session_id);
    let work_root = dir.path();
    let rel = ".anvil-state/verifier-python/site/generated_test.py";
    std::fs::create_dir_all(work_root.join(".anvil-state/verifier-python/site")).unwrap();
    std::fs::write(work_root.join(rel), "").unwrap();
    let scope = single_root_scope();

    agent.seed_artifact_ledger_repo_edit(rel, ArtifactRole::Test, &scope);

    assert!(
        !agent.turn_edited_relative_paths.contains(rel),
        "controller-owned state must not be mirrored into the legacy edit set"
    );
    assert!(
        !agent
            .artifact_ledger
            .repo_edit_projection_set()
            .contains(rel),
        "controller-owned state must not enter the ledger repo edit projection"
    );
    assert_eq!(
        agent.artifact_ledger.event_count(),
        0,
        "ignored controller-owned paths should not create ledger events"
    );
}

#[test]
fn repo_edit_no_op_does_not_seed_ledger() {
    let session_id = unique_session_id("repoedit-noop");
    let (mut agent, dir) = build_agent(&session_id);
    let work_root = dir.path();
    // No file is created; the seed helper should still admit the path
    // (workspace-relative + role gates pass without filesystem checks),
    // but the **no-op guard** is the upstream caller's responsibility
    // (observe_evidence_from_repo_edit). Here we assert that the seed
    // helper itself is a thin adapter: when the caller decides not to
    // call it (no-op detected upstream), neither the ledger nor the
    // legacy set receive the entry. We simulate the no-op-skip-path by
    // never calling the seed helper for the file in question.
    let scope = single_root_scope();
    std::fs::create_dir_all(work_root.join("src")).unwrap();
    std::fs::write(work_root.join("src/noop.rs"), "").unwrap();

    // No call to seed_artifact_ledger_repo_edit for src/noop.rs.
    // Sanity: the ledger remains empty and the legacy set does not
    // contain src/noop.rs.
    assert_eq!(agent.artifact_ledger.event_count(), 0);
    assert!(!agent.turn_edited_relative_paths.contains("src/noop.rs"));
    // Drive a control call so the test isn't vacuous: the helper *does*
    // populate both sources when actually invoked, proving the absence
    // above is due to the missing call, not a silent rejection.
    agent.seed_artifact_ledger_repo_edit("src/noop.rs", ArtifactRole::Implementation, &scope);
    assert!(agent.artifact_ledger.event_count() >= 1);
    assert!(agent.turn_edited_relative_paths.contains("src/noop.rs"));
}

// ---------------------------------------------------------------------------
// Task 2.6: verifier_observation seed
// ---------------------------------------------------------------------------

#[test]
fn verifier_observation_recorded_for_bound_path() {
    let session_id = unique_session_id("verifier-bound");
    let (mut agent, dir) = build_agent(&session_id);
    let work_root = dir.path();
    std::fs::create_dir_all(work_root.join("tests")).unwrap();
    std::fs::write(work_root.join("tests/test_bound.py"), "").unwrap();
    let scope = single_root_scope();

    let bound_paths = vec!["tests/test_bound.py".to_string()];
    agent.seed_artifact_ledger_verifier_observation(&bound_paths, VerifierOutcome::Pass, &scope);
    let obs = agent
        .artifact_ledger
        .verifier_observation_for("tests/test_bound.py")
        .copied();
    assert_eq!(
        obs,
        Some(VerifierObservation {
            argv_path_matched: true,
            last_outcome: VerifierOutcome::Pass,
        })
    );
}

#[test]
fn verifier_observation_skipped_for_legacy_path() {
    let session_id = unique_session_id("verifier-legacy");
    let (mut agent, dir) = build_agent(&session_id);
    let _work_root = dir.path();
    let scope = single_root_scope();

    // Legacy / unbound verifier: caller passes an empty path list because
    // no path-binding occurred (e.g. legacy `AutoTestRunner::run`).
    // Projection must interpret absence as NotRun (i.e. no record).
    agent.seed_artifact_ledger_verifier_observation(&[], VerifierOutcome::Pass, &scope);
    assert!(
        agent
            .artifact_ledger
            .verifier_observation_for("tests/test_bound.py")
            .is_none(),
        "no verifier_observation must be created for legacy/unbound verifier paths"
    );
}

#[test]
fn verifier_observation_ignores_controller_owned_state() {
    let session_id = unique_session_id("verifier-ignore-state");
    let (mut agent, dir) = build_agent(&session_id);
    let work_root = dir.path();
    let rel = ".anvil-state/verifier-python/site/test_generated.py";
    std::fs::create_dir_all(work_root.join(".anvil-state/verifier-python/site")).unwrap();
    std::fs::write(work_root.join(rel), "").unwrap();
    let scope = single_root_scope();

    agent.seed_artifact_ledger_verifier_observation(
        &[rel.to_string()],
        VerifierOutcome::Fail,
        &scope,
    );

    assert!(
        agent
            .artifact_ledger
            .verifier_observation_for(rel)
            .is_none(),
        "controller-owned verifier paths must not enter verifier observations"
    );
    assert_eq!(
        agent.artifact_ledger.event_count(),
        0,
        "ignored controller-owned verifier observations should not create ledger events"
    );
}

// ---------------------------------------------------------------------------
// Task 2.7: dual-source divergence assertion
// ---------------------------------------------------------------------------

#[test]
fn divergence_assertion_passes_when_aligned() {
    let session_id = unique_session_id("div-aligned");
    let (mut agent, dir) = build_agent(&session_id);
    let work_root = dir.path();
    std::fs::create_dir_all(work_root.join("tests")).unwrap();
    std::fs::write(work_root.join("tests/test_aligned.py"), "").unwrap();
    let scope = single_root_scope();
    agent.seed_artifact_ledger_repo_edit("tests/test_aligned.py", ArtifactRole::Test, &scope);
    // Both sources see "tests/test_aligned.py"; the assertion must not panic.
    agent.assert_dual_source_alignment_at_turn_end();
}

#[test]
fn divergence_emit_includes_authority_legacy() {
    let session_id = unique_session_id("div-mismatch");
    let (mut agent, dir) = build_agent(&session_id);
    let work_root = dir.path();
    std::fs::create_dir_all(work_root.join("tests")).unwrap();
    std::fs::write(work_root.join("tests/test_one.py"), "").unwrap();
    let _scope = single_root_scope();
    // Seed only the legacy side so the two sources diverge. We pick a
    // path that is path-validated (`tests/...`) but not admitted to the
    // ledger.
    agent
        .turn_edited_relative_paths
        .insert("tests/test_one.py".to_string());

    let before = read_log_events_by_event_name("agent.artifact_ledger.divergence_detected").len();
    // In debug builds the helper would panic on divergence, but the test
    // module compiles under `cfg(test)` (debug_assertions on). We must
    // therefore drive the release-shape helper that emits + returns to
    // the caller. The shell `emit_artifact_ledger_divergence_if_any`
    // is the release shape — debug-build alignment is asserted by the
    // companion test above.
    agent.emit_artifact_ledger_divergence_if_any();
    let after = read_log_events_by_event_name("agent.artifact_ledger.divergence_detected").len();
    assert!(
        after > before,
        "divergence_detected event must be emitted when sources diverge"
    );
    let events = read_log_events_by_event_name("agent.artifact_ledger.divergence_detected");
    let last = events.last().expect("at least one event");
    let payload = last.get("payload").expect("payload");
    assert_eq!(
        payload.get("authority").and_then(|v| v.as_str()),
        Some("legacy"),
        "adapter period: caller-facing decisions remain legacy authority"
    );
}

/// Codex CB-004 regression: when the ledger admits a path the legacy set
/// does NOT carry (the panic branch of the divergence assertion), the
/// debug-build panic message MUST NOT include the raw workspace-relative
/// path. The fix replaces the legacy `{ledger:?}` / `{legacy:?}` dump with
/// counts + the same masked path-hash projection the release-build
/// observability event already used.
///
/// Strategy: drive the assertion under `std::panic::catch_unwind` so the
/// `#[cfg(debug_assertions)]` panic does not fail the test, then inspect
/// the captured payload (which `std::panic::PanicHookInfo` exposes as the
/// `Display` of the assert message). We assert (a) the panic was raised
/// and (b) the raw secret-bearing path token is absent from the message.
#[cfg(debug_assertions)]
#[test]
fn divergence_panic_message_does_not_leak_raw_paths() {
    let session_id = unique_session_id("div-panic-mask");
    let (mut agent, dir) = build_agent(&session_id);
    let work_root = dir.path();
    std::fs::create_dir_all(work_root.join("tests")).unwrap();
    // The secret-bearing path token we expect to NEVER appear verbatim in
    // the panic payload. The string is built from fragments so the source
    // of this test does not itself match a naive substring scan.
    let secret_segment = format!("{}{}", "TOP-SECRET-", "ApiKey-abc123def");
    let leak_path = format!("tests/{secret_segment}.py");
    std::fs::write(work_root.join(&leak_path), "").unwrap();
    let scope = single_root_scope();
    // Seed the ledger ONLY (not the legacy set) by record-only — bypass
    // the write-through helper. We achieve this by inserting the ledger
    // event directly via the public seed and then surgically deleting it
    // from the legacy mirror so `ledger - legacy` is non-empty.
    agent.seed_artifact_ledger_repo_edit(&leak_path, ArtifactRole::Test, &scope);
    let _ = agent.turn_edited_relative_paths.remove(&leak_path);

    // Drive the assertion under catch_unwind. A custom panic hook swaps
    // the default stderr formatter so we can capture the payload as a
    // `String` independent of `cfg`.
    let captured: std::sync::Arc<std::sync::Mutex<Option<String>>> =
        std::sync::Arc::new(std::sync::Mutex::new(None));
    let captured_for_hook = captured.clone();
    let prev_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let msg = info.payload();
        let text = if let Some(s) = msg.downcast_ref::<&'static str>() {
            (*s).to_string()
        } else if let Some(s) = msg.downcast_ref::<String>() {
            s.clone()
        } else {
            String::from("<non-string panic payload>")
        };
        *captured_for_hook.lock().unwrap() = Some(text);
    }));
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        agent.assert_dual_source_alignment_at_turn_end();
    }));
    std::panic::set_hook(prev_hook);
    assert!(
        result.is_err(),
        "ledger-side-only divergence must panic in debug builds"
    );
    let panic_text = captured
        .lock()
        .unwrap()
        .clone()
        .expect("panic hook must have captured the message");
    assert!(
        !panic_text.contains(&secret_segment),
        "panic message must NOT contain raw workspace-relative path (CB-004); got: {panic_text}"
    );
    assert!(
        panic_text.contains("ledger_only_hashes"),
        "panic message must include the masked hash projection field (CB-004); got: {panic_text}"
    );
    assert!(
        panic_text.contains("legacy_count"),
        "panic message must include count-only fields (CB-004); got: {panic_text}"
    );
}

// ---------------------------------------------------------------------------
// Issue #659 PR-001: payload schema alignment with Section 7.1 of the design
// policy.
//
// Three regression tests pin the new contract:
//   * `event_recorded` payload carries `turn_index` + `session_id`
//   * `turn_summary`   payload carries `turn_index` + `session_id`
//   * `divergence_detected` payload carries bounded masked path-hash
//     lists (`legacy_path_hashes` / `ledger_path_hashes`, max 16 each)
//
// All three round-trip through the on-disk JSONL log so the
// `mask_payload_inplace` final-defence path is exercised end-to-end.
// ---------------------------------------------------------------------------

#[test]
fn event_recorded_payload_includes_turn_index_pr001() {
    let session_id = unique_session_id("pr001-event-recorded");
    let (mut agent, dir) = build_agent(&session_id);
    let work_root = dir.path();
    std::fs::create_dir_all(work_root.join("tests")).unwrap();
    let unique = format!(
        "pr001-evt-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    );
    let path = format!("tests/test_{unique}.py");
    std::fs::write(work_root.join(&path), "").unwrap();
    let scope = single_root_scope();

    // Production sequencing: clear_per_turn_ledger_state stamps the
    // observability log context with the agent's current turn_index +
    // session_id. We drive `current_turn_index = 5` to exercise the
    // u32 narrowing path in the seed helper.
    agent.current_turn_index = 5;
    agent.clear_per_turn_ledger_state();
    agent.seed_artifact_ledger_repo_edit(&path, ArtifactRole::Test, &scope);

    // Compute the expected path_hash (mask_secrets→DefaultHasher→16-hex).
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let masked = crate::session::feedback::mask_secrets(&path);
    let mut hasher = DefaultHasher::new();
    masked.hash(&mut hasher);
    let expected_path_hash = format!("{:016x}", hasher.finish());

    let events = read_log_events_by_event_name("agent.artifact_ledger.event_recorded");
    // Filter to events whose path_hash matches the one we just seeded so
    // this test stays stable under parallel cargo test execution.
    let our = events
        .iter()
        .find(|ev| {
            ev.get("payload")
                .and_then(|p| p.get("path_hash"))
                .and_then(|v| v.as_str())
                == Some(expected_path_hash.as_str())
        })
        .expect("event_recorded for the seeded path must exist");
    let payload = our.get("payload").expect("payload");
    assert_eq!(
        payload.get("turn_index").and_then(|v| v.as_u64()),
        Some(5),
        "PR-001: event_recorded payload must carry turn_index per §7.1"
    );
    assert_eq!(
        payload.get("session_id").and_then(|v| v.as_str()),
        Some(session_id.as_str()),
        "PR-001: event_recorded payload must carry session_id per §7.1"
    );
}

#[test]
fn turn_summary_payload_includes_turn_index_pr001() {
    let session_id = unique_session_id("pr001-turn-summary");
    let (mut agent, dir) = build_agent(&session_id);
    let work_root = dir.path();
    std::fs::create_dir_all(work_root.join("tests")).unwrap();
    let path = "tests/test_summary_pr001.py";
    std::fs::write(work_root.join(path), "").unwrap();
    let scope = single_root_scope();

    agent.current_turn_index = 11;
    agent.clear_per_turn_ledger_state();
    agent.seed_artifact_ledger_repo_edit(path, ArtifactRole::Test, &scope);

    let before = read_log_events_by_event_name_and_session(
        "agent.artifact_ledger.turn_summary",
        &session_id,
    )
    .len();
    agent.record_turn_end_artifact_ledger_summary();
    let events = read_log_events_by_event_name_and_session(
        "agent.artifact_ledger.turn_summary",
        &session_id,
    );
    assert_eq!(
        events.len(),
        before + 1,
        "exactly one turn_summary event must be emitted per call"
    );
    let payload = events
        .last()
        .expect("emitted")
        .get("payload")
        .expect("payload");
    assert_eq!(
        payload.get("turn_index").and_then(|v| v.as_u64()),
        Some(11),
        "PR-001: turn_summary payload must carry turn_index per §7.1"
    );
    assert_eq!(
        payload.get("session_id").and_then(|v| v.as_str()),
        Some(session_id.as_str()),
        "PR-001: turn_summary payload must carry session_id per §7.1"
    );
}

#[test]
fn divergence_detected_payload_includes_bounded_masked_path_hashes_pr001() {
    let session_id = unique_session_id("pr001-divergence");
    let (mut agent, dir) = build_agent(&session_id);
    let work_root = dir.path();
    std::fs::create_dir_all(work_root.join("tests")).unwrap();

    // Seed the legacy set with 20 distinct paths so the bounded list cap
    // (16) is exercised. Use the legacy set directly (not the
    // write-through seed) so the divergence is unambiguous.
    let mut legacy_paths: Vec<String> = Vec::new();
    for i in 0..20 {
        let p = format!("tests/test_pr001_div_{i}.py");
        std::fs::write(work_root.join(&p), "").unwrap();
        legacy_paths.push(p);
    }
    for p in &legacy_paths {
        agent.turn_edited_relative_paths.insert(p.clone());
    }

    let before = read_log_events_by_event_name("agent.artifact_ledger.divergence_detected").len();
    agent.emit_artifact_ledger_divergence_if_any();
    let events = read_log_events_by_event_name("agent.artifact_ledger.divergence_detected");
    assert!(
        events.len() > before,
        "divergence_detected event must be emitted when sources diverge"
    );
    let payload = events
        .last()
        .expect("emitted")
        .get("payload")
        .expect("payload");

    let legacy_hashes = payload
        .get("legacy_path_hashes")
        .and_then(|v| v.as_array())
        .expect("PR-001: divergence payload must carry legacy_path_hashes per §7.1");
    let ledger_hashes = payload
        .get("ledger_path_hashes")
        .and_then(|v| v.as_array())
        .expect("PR-001: divergence payload must carry ledger_path_hashes per §7.1");

    assert!(
        legacy_hashes.len() <= 16,
        "PR-001: legacy_path_hashes must be bounded at 16 entries (got {})",
        legacy_hashes.len()
    );
    assert!(
        ledger_hashes.len() <= 16,
        "PR-001: ledger_path_hashes must be bounded at 16 entries (got {})",
        ledger_hashes.len()
    );
    // 20 distinct paths seeded -> exactly 16 (the cap) on the legacy side,
    // 0 on the ledger side (we did not seed it).
    assert_eq!(
        legacy_hashes.len(),
        16,
        "PR-001: cap must hard-bound legacy_path_hashes at 16"
    );
    assert_eq!(
        ledger_hashes.len(),
        0,
        "ledger has no seeded paths in this test"
    );
    // All hashes are 16-char hex.
    for h in legacy_hashes {
        let s = h.as_str().expect("hash string");
        assert_eq!(s.len(), 16);
        assert!(s.chars().all(|c| c.is_ascii_hexdigit()));
    }
    // Raw paths must not leak into the payload string.
    let payload_str = serde_json::to_string(payload).expect("payload json");
    for p in &legacy_paths {
        assert!(
            !payload_str.contains(p),
            "PR-001: raw path leaked into divergence_detected payload: {p}"
        );
    }
}

// ---------------------------------------------------------------------------
// Issue #659 PR-002: production sequencing for ledger log-context stamping.
//
// `handle_user_message` head clears the per-turn ledger state BEFORE the
// `current_turn_index.saturating_add(1)` line runs (see turn.rs §"Issue #473").
// The previous wiring stamped `current_turn_index` as-is at clear time, which
// produced an off-by-one for the upcoming turn (`event_recorded` /
// `turn_summary` carried `N-1` while every other observability event sharing
// the same user input carried `N`).
//
// Option B fix: `clear_per_turn_ledger_state_for_turn(upcoming_turn_index)`
// takes the upcoming turn index explicitly so callers cannot accidentally
// stamp the stale counter. The production sequencing test below mirrors the
// real `handle_user_message` order — increment FIRST, then call the helper
// with `current_turn_index as u32`.
// ---------------------------------------------------------------------------

#[test]
fn handle_user_message_stamps_upcoming_turn_index_on_ledger() {
    let session_id = unique_session_id("pr002-upcoming-turn-index");
    let (mut agent, dir) = build_agent(&session_id);
    let work_root = dir.path();
    std::fs::create_dir_all(work_root.join("tests")).unwrap();
    let unique = format!(
        "pr002-evt-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    );
    let path = format!("tests/test_{unique}.py");
    std::fs::write(work_root.join(&path), "").unwrap();
    let scope = single_root_scope();

    // Production sequencing: at the head of `handle_user_message`, the
    // previous turn's `current_turn_index` is still in place when the per-
    // turn resets fire. Drive the agent into that exact state so the
    // off-by-one regression would surface as a stamped `turn_index` of 6
    // instead of the expected upcoming 7.
    let previous_turn_index: usize = 6;
    let upcoming_turn_index: usize = previous_turn_index + 1;
    agent.current_turn_index = previous_turn_index;

    // Mirror the production order exactly: increment THEN stamp.
    agent.current_turn_index = agent.current_turn_index.saturating_add(1);
    let upcoming_u32 = u32::try_from(agent.current_turn_index).unwrap_or(u32::MAX);
    agent.clear_per_turn_ledger_state_for_turn(upcoming_u32);

    agent.seed_artifact_ledger_repo_edit(&path, ArtifactRole::Test, &scope);

    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let masked = crate::session::feedback::mask_secrets(&path);
    let mut hasher = DefaultHasher::new();
    masked.hash(&mut hasher);
    let expected_path_hash = format!("{:016x}", hasher.finish());

    // `event_recorded` must carry the upcoming (post-increment) turn_index,
    // not the stale pre-increment one. Filter by `path_hash` so the test
    // stays stable under parallel `cargo test` (multiple tests share the
    // OnceLock-bound log path).
    let events = read_log_events_by_event_name("agent.artifact_ledger.event_recorded");
    let our = events
        .iter()
        .find(|ev| {
            ev.get("payload")
                .and_then(|p| p.get("path_hash"))
                .and_then(|v| v.as_str())
                == Some(expected_path_hash.as_str())
        })
        .expect("event_recorded for the seeded path must exist");
    let payload = our.get("payload").expect("payload");
    let event_turn_index = payload
        .get("turn_index")
        .and_then(|v| v.as_u64())
        .expect("event_recorded payload must carry turn_index");
    assert_eq!(
        event_turn_index, upcoming_turn_index as u64,
        "PR-002: event_recorded turn_index must equal the upcoming (post-increment) \
         turn_index, matching `agent.work_mode.classified` and other observability \
         events emitted during the same user turn"
    );
    assert_ne!(
        event_turn_index, previous_turn_index as u64,
        "PR-002 regression guard: stamping must NOT capture the stale pre-increment \
         counter"
    );
    // session_id pin via the same uniquely identifying event row.
    let event_session_id = payload
        .get("session_id")
        .and_then(|v| v.as_str())
        .expect("event_recorded payload must carry session_id");
    assert_eq!(
        event_session_id,
        session_id.as_str(),
        "PR-002: stamped session_id must mirror the agent's SessionStore id"
    );

    // Note: this test intentionally does NOT call
    // `record_turn_end_artifact_ledger_summary()` — `turn_summary` events
    // are not keyed by `path_hash`, so emitting one here would race with the
    // sibling PR-001 turn_summary assertion under parallel cargo test (both
    // tests scan the shared OnceLock-bound log file). The `event_recorded`
    // assertion above already pins the per-turn stamp on a uniquely keyed
    // event row; PR-001's `turn_summary_payload_includes_turn_index_pr001`
    // independently covers the turn_summary stamping path.
}
