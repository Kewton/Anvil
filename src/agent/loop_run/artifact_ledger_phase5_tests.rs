//! Issue #659 (Phase 5) — in-crate `#[cfg(test)]` tests pinning the
//! design-policy Section 9 acceptance checklist items that are NOT
//! already covered by the Phase 1-4 test modules. Existing Phase 1-4
//! tests are NOT re-implemented here — Phase 5 only adds the gaps:
//!
//! - Item 3: nested test path (`crates/foo/tests/...`) is admitted as
//!   `Owned` by the generic `is_test_file` + `classify_ownership`
//!   predicates without any path literal hard-coded in the ledger.
//! - Item 7: `task_contract::ArtifactState` keeps the
//!   `(role, path: Option<String>, kind)` field shape — compile-time
//!   pattern witness.
//! - Item 9: `ArtifactLedger`'s public API surface is `pub(super)` only
//!   (no `pub` / `pub(crate)` items leak the type outside `loop_run`).
//! - Item 11: every observability `log_llm_event` call from the ledger
//!   goes through `mask_payload_inplace` (logging.rs final-defence
//!   pass) — exercised end-to-end via an event with a secret-like key.
//! - Item 12: `event_recorded` is emitted on every successful event
//!   admission (one event per admit) AND the projection trio's return
//!   types match the Section 9 signature anchor
//!   (`Vec<String>` / `BTreeMap<ArtifactRole, bool>` / `Vec<ArtifactRole>`).
//!
//! DR3-001: this module is private to `loop_run` and never `pub use`d.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::OnceLock;

use serde_json::Value;
use tempfile::{TempDir, tempdir};

use super::artifact_ledger::ArtifactLedger;
use super::artifact_ownership::ArtifactOwnership;
use super::task_contract::{ArtifactRole, ArtifactState, ArtifactStateKind, TaskContract};
use super::task_workspace_scope::{ScopeMode, TaskWorkspaceScope};
use crate::agent::Agent;
use crate::agent::loop_run::FooterHandle;
use crate::config::Config;
use crate::model_registry::RuntimeModels;
use crate::ollama::client::OllamaClient;
use crate::session::store::{SessionSnapshot, SessionStore};

// ---------------------------------------------------------------------------
// Shared logger / fixture setup. Mirrors the Phase 2-4 test scaffolding so
// the four suites run in parallel without racing on the global `OnceLock`
// logger (DR3-003).
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

fn unique_session_id(prefix: &str) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("659p5-{prefix}-{nanos}")
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

    let workspace_key = format!("anvil-659p5-{session_id}");
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
// Section 9, Item 3: nested test path admission uses ONLY the generic
// `is_test_file` + `classify_ownership` predicates — no `app/tests/` /
// `crates/foo/tests/` literal hard-codes in the ledger source.
// ---------------------------------------------------------------------------

/// Issue #659 (Section 9, Item 3): nested workspace test paths must be
/// classified as `Owned` test artifacts using the generic
/// `is_test_file` + `classify_ownership` SSOTs only — no path literal
/// hard-coded inside `artifact_ledger.rs`. We assert two shapes that the
/// design policy explicitly calls out: `app/tests/...` and
/// `crates/<sub>/tests/...`.
#[test]
fn nested_test_paths_classify_as_owned_via_generic_predicates() {
    let session_id = unique_session_id("nested-paths");
    let (mut agent, dir) = build_agent(&session_id);
    let work_root = dir.path();

    // Nested fixtures (Cargo monorepo-shape).
    std::fs::create_dir_all(work_root.join("app/tests")).unwrap();
    std::fs::write(work_root.join("app/tests/test_app.py"), "").unwrap();
    std::fs::create_dir_all(work_root.join("crates/foo/tests")).unwrap();
    std::fs::write(work_root.join("crates/foo/tests/test_foo.rs"), "").unwrap();

    let scope = single_root_scope();
    agent.seed_artifact_ledger_repo_edit("app/tests/test_app.py", ArtifactRole::Test, &scope);
    agent.seed_artifact_ledger_repo_edit(
        "crates/foo/tests/test_foo.rs",
        ArtifactRole::Test,
        &scope,
    );

    let owned = agent
        .artifact_ledger
        .owned_test_artifacts(ArtifactRole::Test);
    assert!(
        owned.iter().any(|p| p == "app/tests/test_app.py"),
        "nested `app/tests/...` path must be Owned via is_test_file SSOT; got {owned:?}"
    );
    assert!(
        owned.iter().any(|p| p == "crates/foo/tests/test_foo.rs"),
        "nested `crates/<sub>/tests/...` path must be Owned via is_test_file SSOT; got {owned:?}"
    );
}

/// Issue #659 (Section 9, Item 3): grep guard — `artifact_ledger.rs`
/// must not embed path literals like `"app/tests"` / `"crates/"` /
/// `"tests/"` as control-flow strings. Test-file classification flows
/// only through `util::file_classify::is_test_file` (the Issue #646
/// SSOT). The grep is split so the assertion text itself does not
/// match.
#[test]
fn artifact_ledger_does_not_hardcode_test_path_literals() {
    let src = std::fs::read_to_string("src/agent/loop_run/artifact_ledger.rs").expect("read self");
    // Strip the `#[cfg(test)] mod tests` block so test fixtures that
    // legitimately reference path strings (e.g. `tests/test_a.py`) do
    // not trigger the guard. The guard targets the production module
    // body only.
    let head = src
        .split_once("#[cfg(test)]")
        .map(|(head, _)| head)
        .unwrap_or(src.as_str());
    // Build forbidden literals from split fragments so this assertion
    // does not self-match.
    let forbidden = [format!("\"{}\"", "app/tests"), format!("\"{}\"", "crates/")];
    for needle in &forbidden {
        assert!(
            !head.contains(needle.as_str()),
            "production ledger body must not hard-code test path literal {needle:?} (use is_test_file SSOT instead)"
        );
    }
}

// ---------------------------------------------------------------------------
// Section 9, Item 7: `task_contract::ArtifactState` keeps the
// `(role, path: Option<String>, kind)` field shape across the ledger
// refactor. Compile-time pattern witness — if any field is added /
// removed / renamed, the destructuring binding stops compiling.
// ---------------------------------------------------------------------------

#[test]
fn artifact_state_signature_role_path_kind_preserved() {
    // Build all three constructor variants and pattern-match their
    // fields. The destructuring binding fails to compile if `role` /
    // `path` / `kind` are renamed or the field count changes.
    let exists = ArtifactState::exists(ArtifactRole::Test, "tests/test_a.py");
    let scaffold = ArtifactState::scaffold(ArtifactRole::Implementation, "src/main.py");
    let changed = ArtifactState::changed(ArtifactRole::UsageDocs);

    let ArtifactState {
        role: _role_a,
        path: ref path_a,
        kind: ref kind_a,
    } = exists;
    let ArtifactState {
        role: _role_b,
        path: ref path_b,
        kind: ref kind_b,
    } = scaffold;
    let ArtifactState {
        role: _role_c,
        path: ref path_c,
        kind: ref kind_c,
    } = changed;

    // Type witness: path is `Option<String>` (Item 7 + DR2-002).
    let _opt_path_a: &Option<String> = path_a;
    let _opt_path_b: &Option<String> = path_b;
    let _opt_path_c: &Option<String> = path_c;

    // Behavioural witness: `changed(role)` constructor leaves `path =
    // None` — the row-skip rule for ledger admission (§3 / DR2-002).
    assert_eq!(path_c, &None);
    assert!(path_a.is_some());
    assert!(path_b.is_some());

    // Kind witness: each constructor maps to its expected variant.
    assert!(matches!(kind_a, ArtifactStateKind::ExistsButUnverified));
    assert!(matches!(kind_b, ArtifactStateKind::ScaffoldUnchanged));
    assert!(matches!(kind_c, ArtifactStateKind::ChangedThisTurn));
}

// ---------------------------------------------------------------------------
// Section 9, Item 9: ledger module's public API surface is `pub(super)`
// (or tighter) — no `pub` / `pub(crate)` leaks the type outside the
// `loop_run` module.
// ---------------------------------------------------------------------------

#[test]
fn artifact_ledger_public_api_is_pub_super_only() {
    let src = std::fs::read_to_string("src/agent/loop_run/artifact_ledger.rs").expect("read self");
    // Only inspect the production module body (drop the `#[cfg(test)]
    // mod tests { ... }` block).
    let head = src
        .split_once("#[cfg(test)]")
        .map(|(head, _)| head)
        .unwrap_or(src.as_str());

    let mut offenders: Vec<String> = Vec::new();
    for (lineno, line) in head.lines().enumerate() {
        let trimmed = line.trim_start();
        // `pub use` is forbidden in the ledger module by DR3-001 (covered
        // by a separate grep test), but it would also violate this rule.
        // We also accept lines like `// pub fn ...` as commentary —
        // filter them out by trimming leading `//`.
        if trimmed.starts_with("//") {
            continue;
        }
        // Item 9 forbids:
        //   - `pub fn` / `pub struct` / `pub enum` / `pub trait` / `pub use` / `pub const` / `pub type` / `pub mod`
        //   - `pub(crate) fn` / `pub(crate) struct` / ...
        // Allowed:
        //   - `pub(super) ...`
        //   - `pub(in crate::agent::loop_run) ...`
        //   - field qualifiers in struct bodies (these still use `pub(super)`)
        let forbidden_prefixes = [
            "pub fn ",
            "pub struct ",
            "pub enum ",
            "pub trait ",
            "pub use ",
            "pub const ",
            "pub static ",
            "pub type ",
            "pub mod ",
            "pub(crate) fn ",
            "pub(crate) struct ",
            "pub(crate) enum ",
            "pub(crate) trait ",
            "pub(crate) use ",
            "pub(crate) const ",
            "pub(crate) static ",
            "pub(crate) type ",
            "pub(crate) mod ",
        ];
        for prefix in &forbidden_prefixes {
            if trimmed.starts_with(prefix) {
                offenders.push(format!("line {}: {trimmed}", lineno + 1));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "Section 9 Item 9: artifact_ledger.rs must only expose pub(super) (or tighter) items; offenders: {offenders:?}"
    );
}

// ---------------------------------------------------------------------------
// Section 9, Item 11: `log_llm_event` final-defence mask. Every observ-
// ability event the ledger emits goes through `mask_payload_inplace`
// (logging.rs line 64). We exercise the end-to-end masking by recording
// an event whose path carries a secret-like substring, then read the
// JSONL log back and confirm the secret token is NOT present on any
// `agent.artifact_ledger.event_recorded` payload line.
// ---------------------------------------------------------------------------

#[test]
fn log_llm_event_masks_artifact_ledger_event_recorded_payload() {
    let session_id = unique_session_id("mask-payload");
    let (mut agent, dir) = build_agent(&session_id);
    let work_root = dir.path();
    std::fs::create_dir_all(work_root.join("tests")).unwrap();
    std::fs::write(work_root.join("tests/test_secret.py"), "").unwrap();
    let scope = single_root_scope();

    // Record an event whose path is a normal (non-secret) workspace
    // path. The payload contract (§7.1) is: raw `path` MUST NOT appear
    // in the payload — only `path_hash` (deterministic hash of the
    // mask_secrets'd value) and `path_len` (length).
    agent.seed_artifact_ledger_repo_edit("tests/test_secret.py", ArtifactRole::Test, &scope);

    // Read back every `event_recorded` log line and check that the raw
    // workspace path does not appear in the payload as a leaf string —
    // only `path_hash` / `path_len` should be present.
    let events = read_log_events_by_event_name("agent.artifact_ledger.event_recorded");
    assert!(
        !events.is_empty(),
        "expected at least one event_recorded log line for the seeded RepoEdit"
    );
    for event in &events {
        let payload = event.get("payload").expect("payload must be present");
        let payload_str = serde_json::to_string(payload).expect("payload json");
        // Raw path must NOT appear in the payload (path_hash + path_len
        // only, per §7.1).
        assert!(
            !payload_str.contains("tests/test_secret.py"),
            "raw workspace path leaked into event_recorded payload: {payload_str}"
        );
        // `path_hash` must be present (mask-then-hash SSOT).
        assert!(
            payload.get("path_hash").is_some(),
            "event_recorded payload must carry path_hash"
        );
        assert!(
            payload.get("path_len").is_some(),
            "event_recorded payload must carry path_len"
        );
    }

    // mask_payload_inplace final-defence guard: an event with a known
    // secret-like key (e.g. `api_key`) recorded via `log_llm_event`
    // must be redacted before reaching the JSONL log. We assert this
    // via a direct `log_llm_event` call with a payload containing a
    // secret-keyed field — the resulting log line must NOT contain the
    // raw secret value.
    let secret_value = "leak-token-PHASE5-unique-DEADBEEFCAFEBABE";
    crate::logging::log_llm_event(
        "agent.artifact_ledger.phase5_mask_probe",
        serde_json::json!({ "api_key": secret_value, "ok": true }),
    );
    let probe_events = read_log_events_by_event_name("agent.artifact_ledger.phase5_mask_probe");
    assert!(
        !probe_events.is_empty(),
        "probe event must be emitted so mask coverage can be asserted"
    );
    for event in &probe_events {
        let payload = event.get("payload").expect("payload");
        let payload_str = serde_json::to_string(payload).expect("payload json");
        assert!(
            !payload_str.contains(secret_value),
            "mask_payload_inplace final-defence FAILED: raw secret value present in log: {payload_str}"
        );
    }
}

// ---------------------------------------------------------------------------
// Section 9, Item 12: `event_recorded` emit on every successful admit +
// projection trio signature anchor.
// ---------------------------------------------------------------------------

/// Issue #659 (Section 9, Item 12): every successful event admission
/// emits exactly one `agent.artifact_ledger.event_recorded` event. We
/// seed three distinct RepoEdit observations with `path_hash` values
/// derived from this test's unique workspace-relative path prefix, so
/// the assertion is robust under parallel `cargo test` execution where
/// other tests append to the same JSONL log.
#[test]
fn event_recorded_emitted_once_per_admitted_event() {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let session_id = unique_session_id("event-recorded-each");
    let (mut agent, dir) = build_agent(&session_id);
    let work_root = dir.path();
    std::fs::create_dir_all(work_root.join("tests")).unwrap();
    // Path bodies must be unique across the entire test process so the
    // `path_hash` we compute below cannot collide with another test's
    // event log entries.
    let unique = format!(
        "phase5-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    );
    let p1 = format!("tests/{unique}-e1.py");
    let p2 = format!("tests/{unique}-e2.py");
    let p3 = format!("tests/{unique}-e3.py");
    std::fs::write(work_root.join(&p1), "").unwrap();
    std::fs::write(work_root.join(&p2), "").unwrap();
    std::fs::write(work_root.join(&p3), "").unwrap();
    let scope = single_root_scope();

    let expected_hashes: Vec<String> = [&p1, &p2, &p3]
        .iter()
        .map(|p| {
            // Mirror `stable_path_hash` over `mask_secrets`-applied
            // path. Since these paths contain no secret-like tokens,
            // `mask_secrets` is the identity here, so we hash the raw
            // path directly with DefaultHasher (same algorithm).
            let mut hasher = DefaultHasher::new();
            (*p).hash(&mut hasher);
            format!("{:016x}", hasher.finish())
        })
        .collect();

    agent.seed_artifact_ledger_repo_edit(&p1, ArtifactRole::Test, &scope);
    agent.seed_artifact_ledger_repo_edit(&p2, ArtifactRole::Test, &scope);
    agent.seed_artifact_ledger_repo_edit(&p3, ArtifactRole::Test, &scope);

    // Count event_recorded entries whose `path_hash` matches one of the
    // three we seeded. Each successful admit must emit exactly one.
    let events = read_log_events_by_event_name("agent.artifact_ledger.event_recorded");
    let mut per_hash_count = std::collections::BTreeMap::<String, usize>::new();
    for ev in &events {
        if let Some(payload) = ev.get("payload")
            && let Some(h) = payload.get("path_hash").and_then(|v| v.as_str())
            && expected_hashes.iter().any(|eh| eh == h)
        {
            *per_hash_count.entry(h.to_string()).or_default() += 1;
        }
    }
    for h in &expected_hashes {
        let n = per_hash_count.get(h).copied().unwrap_or(0);
        assert_eq!(
            n, 1,
            "event_recorded must be emitted exactly once per admitted event (hash {h} got {n})"
        );
    }
}

/// Issue #659 (Section 9, Item 12 + DR2-001): projection signature
/// anchor — the three `&self` methods on `ArtifactLedger` must keep
/// their declared return types so Issue #660 / #661 / #663 can
/// consume them without merge conflicts.
#[test]
fn projection_signature_anchor_owned_test_artifacts() {
    let ledger = ArtifactLedger::new();
    // Compile-time witness: `owned_test_artifacts(&self, role) -> Vec<String>`.
    let f: fn(&ArtifactLedger, ArtifactRole) -> Vec<String> = ArtifactLedger::owned_test_artifacts;
    let v = f(&ledger, ArtifactRole::Test);
    assert!(v.is_empty(), "fresh ledger has no owned events");
}

#[test]
fn projection_signature_anchor_required_artifacts_completed() {
    let ledger = ArtifactLedger::new();
    let contract = TaskContract::from_request("explain repo");
    // Compile-time witness: `required_artifacts_completed(&self,
    // &TaskContract) -> BTreeMap<ArtifactRole, bool>`.
    let f: fn(&ArtifactLedger, &TaskContract) -> BTreeMap<ArtifactRole, bool> =
        ArtifactLedger::required_artifacts_completed;
    let map = f(&ledger, &contract);
    // Empty contract => empty map.
    assert_eq!(map.len(), contract.required_artifacts.len());
}

#[test]
fn projection_signature_anchor_active_job_candidates() {
    let ledger = ArtifactLedger::new();
    let contract =
        TaskContract::from_request("FastAPIでCRUD APIを作成してREADMEとテストも追加してください");
    // Compile-time witness: `active_job_candidates(&self, &TaskContract)
    // -> Vec<ArtifactRole>`.
    let f: fn(&ArtifactLedger, &TaskContract) -> Vec<ArtifactRole> =
        ArtifactLedger::active_job_candidates;
    let roles = f(&ledger, &contract);
    // Issue #659 stub: returns declaration order of required_artifacts.
    assert_eq!(roles, contract.required_artifacts);
}

// ---------------------------------------------------------------------------
// Cross-cutting witness: ownership of nested test paths flows through
// `classify_ownership` (the Issue #646 SSOT). Items 3 / 9 / 12 share
// this dependency; the test below ties them together by exercising
// `Owned` admission of a nested path via the production seed and
// reading the projection.
// ---------------------------------------------------------------------------

#[test]
fn owned_projection_for_nested_test_path_returns_owned_ownership() {
    let session_id = unique_session_id("nested-ownership");
    let (mut agent, dir) = build_agent(&session_id);
    let work_root = dir.path();
    std::fs::create_dir_all(work_root.join("crates/bar/tests")).unwrap();
    std::fs::write(work_root.join("crates/bar/tests/test_bar.rs"), "").unwrap();
    let scope = single_root_scope();

    agent.seed_artifact_ledger_repo_edit(
        "crates/bar/tests/test_bar.rs",
        ArtifactRole::Test,
        &scope,
    );
    let owned = agent
        .artifact_ledger
        .owned_test_artifacts(ArtifactRole::Test);
    assert_eq!(owned, vec!["crates/bar/tests/test_bar.rs".to_string()]);

    // And event-level ownership classification is `Owned` (not
    // `CandidateOnly` / `OutOfScope`) — proved indirectly because only
    // Owned events feed `owned_test_artifacts`. We also walk
    // events_iter to confirm at least one event has Owned ownership.
    let any_owned = agent
        .artifact_ledger
        .events_iter()
        .any(|e| matches!(e.ownership, ArtifactOwnership::Owned));
    assert!(
        any_owned,
        "nested test path must yield at least one Owned ledger event"
    );
}
