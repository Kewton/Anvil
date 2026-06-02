//! Issue #659 (Phase 3) — in-crate `#[cfg(test)]` tests for the
//! legacy-helper internal-implementation switch. The tests verify:
//!
//! - Task 3.1: `Agent::turn_edited_relative_paths` (legacy authority during
//!   the adapter period) matches the ledger's
//!   `repo_edit_projection_set()` view after write-through seeding.
//! - Task 3.1: the legacy field signature (`HashSet<String>`) and access
//!   pattern are unchanged — compile-time contract test.
//! - Task 3.2: `task_contract_artifact_states` keeps producing the same
//!   `Vec<ArtifactState>` shape and returns the ledger projection after the
//!   legacy derivation seeds baseline evidence.
//! - Task 3.3: `owned_test_artifacts_for_verifier` keeps returning
//!   Issue #651-equivalent results and the caller signature
//!   (`&mut self`, `&TaskContract`, `Vec<String>`) is preserved.
//!
//! DR3-001: this module is private to `loop_run` and never `pub use`d.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::OnceLock;

use tempfile::{TempDir, tempdir};

use super::task_contract::{ArtifactRole, ArtifactState, ArtifactStateKind, TaskContract};
use super::task_workspace_scope::{ScopeMode, TaskWorkspaceScope};
use crate::agent::Agent;
use crate::agent::loop_run::FooterHandle;
use crate::config::Config;
use crate::model_registry::RuntimeModels;
use crate::ollama::client::OllamaClient;
use crate::session::store::{SessionSnapshot, SessionStore};

// ---------------------------------------------------------------------------
// Shared logger / fixture setup. Mirrors the Phase 2 test scaffolding so
// the two suites can run in parallel without racing on the global
// `OnceLock` logger (DR3-003).
// ---------------------------------------------------------------------------

static LOG_DIR: OnceLock<TempDir> = OnceLock::new();

fn shared_log_path() -> PathBuf {
    // DR3-003 isolation: `init_logging` uses a global `OnceLock`, so when
    // multiple in-crate `#[cfg(test)]` modules co-init only the first
    // call wins. Under parallel test execution another module's init
    // can be racing with ours, so we spin briefly until the global
    // subscriber has settled and `llm_io_log_path()` returns the path
    // the first init landed on.
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
    format!("659p3-{prefix}-{nanos}")
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

    let workspace_key = format!("anvil-659p3-{session_id}");
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
// Task 3.1: turn_edited_relative_paths set vs ledger RepoEdit projection
// ---------------------------------------------------------------------------

/// Issue #659 (Task 3.1): the write-through seed helper keeps the legacy
/// `turn_edited_relative_paths` HashSet in lockstep with the
/// ledger's `repo_edit_projection_set()` projection. The Phase 3
/// SSOT-uniqueness invariant is that both views observe the same paths;
/// callers may continue to read the legacy field directly until the
/// adapter is removed (post #660/#661/#663 cleanup).
#[test]
fn legacy_set_matches_ledger_projection() {
    let session_id = unique_session_id("legacy-eq-ledger");
    let (mut agent, dir) = build_agent(&session_id);
    let work_root = dir.path();
    std::fs::create_dir_all(work_root.join("tests")).unwrap();
    std::fs::create_dir_all(work_root.join("src")).unwrap();
    std::fs::write(work_root.join("tests/test_a.py"), "").unwrap();
    std::fs::write(work_root.join("tests/test_b.py"), "").unwrap();
    std::fs::write(work_root.join("src/lib.rs"), "").unwrap();

    let scope = single_root_scope();
    super::artifact_ledger_state::seed_artifact_ledger_repo_edit(
        &mut agent,
        "tests/test_a.py",
        ArtifactRole::Test,
        &scope,
    );
    super::artifact_ledger_state::seed_artifact_ledger_repo_edit(
        &mut agent,
        "tests/test_b.py",
        ArtifactRole::Test,
        &scope,
    );
    super::artifact_ledger_state::seed_artifact_ledger_repo_edit(
        &mut agent,
        "src/lib.rs",
        ArtifactRole::Implementation,
        &scope,
    );

    let legacy: std::collections::BTreeSet<String> =
        agent.turn_edited_relative_paths.iter().cloned().collect();
    let ledger = agent.artifact_ledger.repo_edit_projection_set();
    assert_eq!(
        legacy, ledger,
        "Task 3.1 adapter contract: write-through must keep both views aligned"
    );
    assert!(legacy.contains("tests/test_a.py"));
    assert!(legacy.contains("tests/test_b.py"));
    assert!(legacy.contains("src/lib.rs"));
}

/// Issue #659 (Task 3.1): compile-time contract that
/// `Agent::turn_edited_relative_paths` remains a `HashSet<String>`. The
/// Phase 3 internal switch must NOT change the legacy field's type or
/// accessor pattern — every existing caller (`artifact_ownership`'s
/// `edited_this_session_for` predicate, `success.rs`, `verifier_skill.rs`)
/// reads the set via the same `.contains(path)` / `.iter()` API. If a
/// future refactor swaps the field for a method, this assertion's
/// trait-bound expression stops compiling.
#[test]
fn caller_signature_unchanged_for_turn_edited_relative_paths() {
    let session_id = unique_session_id("legacy-sig");
    let (mut agent, dir) = build_agent(&session_id);
    let work_root = dir.path();
    std::fs::create_dir_all(work_root.join("tests")).unwrap();
    std::fs::write(work_root.join("tests/test_sig.py"), "").unwrap();
    let scope = single_root_scope();
    super::artifact_ledger_state::seed_artifact_ledger_repo_edit(
        &mut agent,
        "tests/test_sig.py",
        ArtifactRole::Test,
        &scope,
    );

    // Compile-time witness: the field must keep its HashSet<String>
    // shape. If the type changes (e.g. to `BTreeSet<String>` or behind a
    // method `fn turn_edited(&self) -> &HashSet<...>`), this binding
    // stops compiling.
    let _typed: &HashSet<String> = &agent.turn_edited_relative_paths;

    // Behavioural witness: the .contains(path) API contract.
    assert!(
        agent
            .turn_edited_relative_paths
            .contains("tests/test_sig.py"),
        ".contains(&str-like) API contract must not regress"
    );
}

// ---------------------------------------------------------------------------
// Task 3.2: task_contract_artifact_states preserves shape + uses ledger
// ---------------------------------------------------------------------------

/// Issue #659 (Task 3.2): without any seeded edits / scaffolds, both the
/// legacy and ledger-derived `task_contract_artifact_states` projections
/// collapse to the empty vector. The empty case is the simplest shape
/// regression check: it proves the new path doesn't produce phantom
/// `ArtifactState::exists` rows when the workspace has nothing to anchor
/// them to.
#[test]
fn task_contract_artifact_states_returns_same_shape_as_before_empty() {
    let session_id = unique_session_id("states-empty");
    let (mut agent, _dir) = build_agent(&session_id);
    let contract = TaskContract::from_request("explain what this repo does");
    // Pre-condition: an Explain contract has no required artifacts, so
    // both derivations should be empty. (The Phase 3 implementation
    // delegates to legacy on divergence; the legacy and ledger
    // derivations must align for the empty case.)
    let states = super::artifact_state_projection::task_contract_artifact_states_for_test(
        &mut agent, &contract,
    );
    assert!(
        states.is_empty(),
        "no required artifacts => no states; got {states:?}"
    );
}

/// Issue #659 (Task 3.2): seeded RepoEdit observations into the ledger
/// produce a corresponding `ArtifactState::exists` row in the
/// ledger-projection helper (the internal switch consumer). The legacy
/// derivation also produces the same row because the write-through
/// adapter (Task 2.5) inserts into both sources; the test asserts the
/// caller-facing helper returns the ledger row and that the legacy helper
/// agrees with it in the no-divergence case.
#[test]
fn task_contract_artifact_states_uses_ledger_projection() {
    let session_id = unique_session_id("states-ledger");
    let (mut agent, dir) = build_agent(&session_id);
    let work_root = dir.path();
    std::fs::create_dir_all(work_root.join("tests")).unwrap();
    std::fs::write(work_root.join("tests/test_ledger.py"), "x = 1\n").unwrap();
    let scope = single_root_scope();
    super::artifact_ledger_state::seed_artifact_ledger_repo_edit(
        &mut agent,
        "tests/test_ledger.py",
        ArtifactRole::Test,
        &scope,
    );

    // Use a Build-class contract so `required_artifacts` includes Test.
    let contract =
        TaskContract::from_request("FastAPIでCRUD APIを作成してREADMEとテストも追加してください");
    assert!(
        contract.required_artifacts.contains(&ArtifactRole::Test),
        "fixture contract must require a Test artifact"
    );

    let legacy_states =
        super::artifact_state_projection::task_contract_artifact_states_legacy_for_test(
            &mut agent, &contract,
        );
    let ledger_states =
        super::artifact_state_projection::task_contract_artifact_states_from_ledger_for_test(
            &agent, &contract,
        );

    let edit_test_state = |states: &[ArtifactState]| -> Option<ArtifactState> {
        states
            .iter()
            .find(|s| {
                s.role == ArtifactRole::Test
                    && s.path.as_deref() == Some("tests/test_ledger.py")
                    && s.kind == ArtifactStateKind::ExistsButUnverified
            })
            .cloned()
    };

    assert!(
        edit_test_state(&legacy_states).is_some(),
        "legacy must produce ExistsButUnverified for the seeded RepoEdit path"
    );
    assert!(
        edit_test_state(&ledger_states).is_some(),
        "ledger projection must produce the same ExistsButUnverified row for the seeded RepoEdit path"
    );
}

// ---------------------------------------------------------------------------
// Task 3.3: owned_test_artifacts_for_verifier
// ---------------------------------------------------------------------------

/// Issue #659 (Task 3.3): `owned_test_artifacts_for_verifier` keeps the
/// Issue #651 behavior — seeded RepoEdit test paths surface in the
/// returned slice. The legacy and ledger derivations agree under
/// write-through, so the function returns the ledger slice without emitting
/// the divergence event.
#[test]
fn owned_test_artifacts_for_verifier_matches_issue651_behavior() {
    let session_id = unique_session_id("verifier-match");
    let (mut agent, dir) = build_agent(&session_id);
    let work_root = dir.path();
    std::fs::create_dir_all(work_root.join("tests")).unwrap();
    std::fs::write(
        work_root.join("tests/test_match.py"),
        "def test_fastapi_crud_api_contract():\n    assert 'FastAPI CRUD API README テスト'\n",
    )
    .unwrap();
    let scope = single_root_scope();
    super::artifact_ledger_state::seed_artifact_ledger_repo_edit(
        &mut agent,
        "tests/test_match.py",
        ArtifactRole::Test,
        &scope,
    );

    let contract =
        TaskContract::from_request("FastAPIでCRUD APIを作成してREADMEとテストも追加してください");
    let owned =
        super::owned_test_projection::owned_test_artifacts_for_verifier(&mut agent, &contract);
    assert!(
        owned.iter().any(|p| p == "tests/test_match.py"),
        "Issue #651 contract: seeded Test edit must surface in owned slice; got {owned:?}"
    );

    // Cross-check with the ledger projection directly so the test
    // documents the SSOT path's agreement.
    let ledger_owned = agent
        .artifact_ledger
        .owned_test_artifacts(ArtifactRole::Test);
    assert_eq!(
        owned, ledger_owned,
        "ledger projection must align with the legacy slice in the no-divergence case"
    );
}

/// Issue #659 (Task 3.3): compile-time contract for the public(super)
/// helper signature. Phase 3 must NOT change the function's argument
/// shape (`&mut self`, `&TaskContract`) or its return type (`Vec<String>`).
/// If a future refactor narrows `&mut self` to `&self` (Phase 4
/// cleanup), this test must be updated explicitly — it must not happen
/// silently.
#[test]
fn owned_test_artifacts_for_verifier_signature_unchanged() {
    let session_id = unique_session_id("verifier-sig");
    let (mut agent, _dir) = build_agent(&session_id);
    let contract = TaskContract::from_request("FastAPIでCRUD APIを作成してください");

    // Compile-time witness via a function-pointer cast: the type ascribed
    // here is the Phase-2 signature. If `owned_test_artifacts_for_verifier`
    // changes to a different shape (different receiver mut-ness, different
    // arg/return types), this assignment fails to compile. (Parent #680:
    // the method was promoted to a free fn in `owned_test_projection`; the
    // signature itself is unchanged.)
    let signature_witness: fn(&mut Agent, &TaskContract) -> Vec<String> =
        super::owned_test_projection::owned_test_artifacts_for_verifier;
    let result = signature_witness(&mut agent, &contract);
    // The fixture has no edits or workspace files, so the slice is empty.
    assert!(
        result.is_empty(),
        "no seeded edits => empty owned slice; got {result:?}"
    );
}
