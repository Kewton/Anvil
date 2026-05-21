//! Issue #659 (Phase 4) — in-crate `#[cfg(test)]` tests for the
//! production-aligned test seam `seed_artifact_ledger_repo_edit_for_test`
//! that replaces the legacy `seed_turn_edited_relative_path_for_test`
//! (introduced by Issue #654 / CB-005). The Phase 4 tests verify:
//!
//! - Task 4.1: the new seam routes through the production RepoEdit
//!   write-through adapter (admission via `classify_ownership`, role
//!   inference via `classify_repo_edit_path`, ledger record + legacy
//!   `turn_edited_relative_paths` insert in the same instruction).
//! - Task 4.1: the seam signature does NOT expose any `pub(super)` types
//!   from the `artifact_ledger` / `task_contract` modules — only `&mut
//!   Agent` and `String` cross the `pub(crate)` boundary, so the
//!   `private_interfaces` lint cannot trigger.
//! - Task 4.1: the legacy `turn_edited_relative_paths` set stays in
//!   lockstep with the ledger `repo_edit_projection_set()` projection
//!   (write-through divergence anchor).
//!
//! DR3-001: this module is private to `loop_run` and never `pub use`d.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::OnceLock;

use tempfile::{TempDir, tempdir};

use super::task_workspace_scope::{ScopeMode, TaskWorkspaceScope};
use crate::agent::Agent;
use crate::agent::loop_run::{FooterHandle, seed_artifact_ledger_repo_edit_for_test};
use crate::config::Config;
use crate::model_registry::RuntimeModels;
use crate::ollama::client::OllamaClient;
use crate::session::store::{SessionSnapshot, SessionStore};

// ---------------------------------------------------------------------------
// Shared logger / fixture setup. Mirrors the Phase 2 / Phase 3 test
// scaffolding so the suites run in parallel without racing on the global
// `OnceLock` logger (DR3-003).
// ---------------------------------------------------------------------------

static LOG_DIR: OnceLock<TempDir> = OnceLock::new();

fn shared_log_path() -> PathBuf {
    let _ = LOG_DIR.get_or_init(|| {
        let dir = tempdir().expect("tempdir for shared log");
        let log_path = dir.path().join("llm-io.jsonl");
        let _ = crate::logging::init_logging(crate::config::LogLevel::Info, &log_path);
        dir
    });
    for _ in 0..50 {
        if let Some(p) = crate::logging::llm_io_log_path() {
            return p.to_path_buf();
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    panic!("logging must be initialised by this point");
}

fn unique_session_id(prefix: &str) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("659p4-{prefix}-{nanos}")
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

    let workspace_key = format!("anvil-659p4-{session_id}");
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

#[allow(dead_code)]
fn single_root_scope() -> TaskWorkspaceScope {
    TaskWorkspaceScope {
        mode: ScopeMode::SingleProjectRoot,
    }
}

// ---------------------------------------------------------------------------
// Task 4.1: test seam goes through the production admission path
// ---------------------------------------------------------------------------

/// Issue #659 (Task 4.1): when the new seam admits a Test-classified path,
/// the production RepoEdit write-through must produce a ledger event with
/// `ArtifactOrigin::RepoEdit` for that path. We verify this by reading
/// the ledger's `repo_edit_projection_set()` projection (the same view
/// the Phase 3 tests use to assert lockstep with `turn_edited_relative_paths`).
#[test]
fn seed_test_seam_goes_through_admission_path() {
    let session_id = unique_session_id("admit");
    let (mut agent, dir) = build_agent(&session_id);
    let work_root = dir.path();
    std::fs::create_dir_all(work_root.join("tests")).unwrap();
    std::fs::write(work_root.join("tests/test_seam.rs"), "").unwrap();

    seed_artifact_ledger_repo_edit_for_test(&mut agent, "tests/test_seam.rs".to_string());

    let projection = agent.artifact_ledger.repo_edit_projection_set();
    assert!(
        projection.contains("tests/test_seam.rs"),
        "test seam must seed the ledger RepoEdit projection (production write-through), got {projection:?}",
    );
}

/// Issue #659 (Task 4.1): the seam's signature must remain
/// `pub(crate) fn(&mut Agent, String)`. This is a compile-time witness
/// — if a future refactor leaks `ArtifactRole` / `ArtifactOrigin` /
/// `LedgerAdmissionContext` etc. into the signature, this binding stops
/// compiling.
#[test]
fn seed_test_seam_does_not_expose_internal_types() {
    let _: fn(&mut Agent, String) = seed_artifact_ledger_repo_edit_for_test;
}

/// Issue #659 (Task 4.1): the seam's write-through must keep the legacy
/// `turn_edited_relative_paths` HashSet in lockstep with the ledger's
/// `repo_edit_projection_set()` projection — exactly the same adapter
/// invariant the Phase 3 tests assert for production callers.
#[test]
fn seed_test_seam_synchronizes_legacy_set() {
    let session_id = unique_session_id("legacy-eq-ledger");
    let (mut agent, dir) = build_agent(&session_id);
    let work_root = dir.path();
    std::fs::create_dir_all(work_root.join("tests")).unwrap();
    std::fs::write(work_root.join("tests/test_a.rs"), "").unwrap();
    std::fs::write(work_root.join("tests/test_b.rs"), "").unwrap();

    seed_artifact_ledger_repo_edit_for_test(&mut agent, "tests/test_a.rs".to_string());
    seed_artifact_ledger_repo_edit_for_test(&mut agent, "tests/test_b.rs".to_string());

    let legacy: BTreeSet<String> = agent.turn_edited_relative_paths.iter().cloned().collect();
    let ledger = agent.artifact_ledger.repo_edit_projection_set();
    assert_eq!(
        legacy, ledger,
        "test seam must write through both views (legacy set + ledger projection)",
    );
    assert!(legacy.contains("tests/test_a.rs"));
    assert!(legacy.contains("tests/test_b.rs"));
}

/// Issue #659 (Task 4.1): the seam must accept non-test paths and still
/// insert into the legacy `turn_edited_relative_paths` set so the e2e
/// suite's "non-test paths are filtered by `is_test_file`" assertion
/// continues to hold. The ledger entry (if any) carries the Implementation
/// role — `collect_owned_test_artifacts` filters by `is_test_file` before
/// consulting ownership, so the row is correctly excluded from the
/// `owned_test_artifacts` projection downstream.
#[test]
fn seed_test_seam_accepts_non_test_path_into_legacy_set() {
    let session_id = unique_session_id("non-test");
    let (mut agent, dir) = build_agent(&session_id);
    let work_root = dir.path();
    std::fs::create_dir_all(work_root.join("src")).unwrap();
    std::fs::write(work_root.join("src/lib.rs"), "").unwrap();

    seed_artifact_ledger_repo_edit_for_test(&mut agent, "src/lib.rs".to_string());

    assert!(
        agent.turn_edited_relative_paths.contains("src/lib.rs"),
        "test seam must always insert into the legacy set (even for non-test paths) so downstream classifier-based filtering applies",
    );
}
