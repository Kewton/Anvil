//! E2E integration tests for Issue #458 (Temporary Test Workspace).
//!
//! These cover the 10 acceptance criteria described in
//! `dev-reports/design/issue-458-tmp-tests-design-policy.md` §8 (14 cases
//! after the 4a–4d / 7a–7b sub-scenarios). They prefer the public CLI / Agent
//! integration paths (`ToolRegistry::execute` with a properly-wired
//! `ToolContext`, `run_tmp_tests_*` / `execute_clean_plan` from
//! `sessions_cli`) over the lifecycle helpers' raw API, while still allowing
//! `tmp_tests::*` direct calls when the criterion is purely about on-disk
//! shape.
//!
//! The tests deliberately do NOT spawn the `anvil` binary (no Ollama, no
//! network); they exercise the public Rust API surface only so they stay
//! deterministic and fast.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anvil::modes::plan_act::{ExecutionMode, ModeState, PlanStage, TaskProfile, WorkMode};
use anvil::session::sessions_cli::{
    CleanArgs, execute_clean_plan, plan_clean, run_tmp_tests_discard, run_tmp_tests_list,
    run_tmp_tests_promote, scan_session_meta, validate_explicit_session_id,
};
use anvil::session::store::{ConversationMessage, SessionSnapshot};
use anvil::session::tmp_tests::{
    self, TMP_TEST_SCHEMA_VERSION, TmpTest, TmpTestStatus, create_generated_test, derive_test_id,
    discard_tmp_test, list_tmp_tests, promote_tmp_test, read_metadata_dir,
};
use anvil::tools::registry::{ToolContext, ToolRegistry};
use serde_json::{Value, json};
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// Fixture helpers
// ---------------------------------------------------------------------------

fn v7_id() -> String {
    uuid::Uuid::now_v7().to_string()
}

/// Lay out `state_root/sessions/<id>/session.json` so that
/// `validate_explicit_session_id` and `require_session_in_workspace` accept
/// the resulting session id.
fn write_session_snapshot(state_root: &Path, id: &str, workspace_key: &str) -> PathBuf {
    let session_dir = state_root.join("sessions").join(id);
    fs::create_dir_all(&session_dir).unwrap();
    let snap = SessionSnapshot {
        id: id.to_string(),
        workspace_key: workspace_key.to_string(),
        messages: vec![ConversationMessage::user("hi".into())],
        mode_state: ModeState {
            mode: ExecutionMode::Act,
            active_plan_path: None,
            task_profile: TaskProfile::Generic,
            work_mode: WorkMode::Auto,
            plan_stage: PlanStage::Stage1,
        },
        active_root: None,
        ..Default::default()
    };
    let session_json = session_dir.join("session.json");
    fs::write(&session_json, serde_json::to_string_pretty(&snap).unwrap()).unwrap();
    session_json
}

/// Build a session fixture for `id` and return:
///   * `(state_root, workspace_root, session_dir, tmp_tests_root)`.
struct SessionFixture {
    _state_root_keep: TempDir,
    _workspace_keep: TempDir,
    state_root: PathBuf,
    workspace_root: PathBuf,
    session_id: String,
    workspace_key: String,
    session_dir: PathBuf,
    tmp_tests_root: PathBuf,
}

fn make_session_fixture(workspace_key: &str) -> SessionFixture {
    let state = TempDir::new().unwrap();
    let workspace = TempDir::new().unwrap();
    let id = v7_id();
    write_session_snapshot(state.path(), &id, workspace_key);
    let session_dir = state.path().join("sessions").join(&id);
    let tmp_tests_root = session_dir.join("tmp-tests");
    SessionFixture {
        state_root: state.path().to_path_buf(),
        workspace_root: workspace.path().to_path_buf(),
        session_id: id,
        workspace_key: workspace_key.to_string(),
        session_dir,
        tmp_tests_root,
        _state_root_keep: state,
        _workspace_keep: workspace,
    }
}

fn tool_context_for(fixture: &SessionFixture) -> ToolContext {
    ToolContext {
        root: fixture.workspace_root.clone(),
        mode: ExecutionMode::Act,
        plan_path: None,
        plan_stage: PlanStage::Stage1,
        auto_approve: true,
        interactive_approval: false,
        offline: false,
        cancel_flag: None,
        tmp_tests_root: Some(fixture.tmp_tests_root.clone()),
        tester_active: false,
    }
}

// ---------------------------------------------------------------------------
// Case 1: tmp-tests/files and metadata are created under the session dir
// ---------------------------------------------------------------------------

#[test]
fn creates_tmp_test_under_session_dir() {
    let fx = make_session_fixture("ws-1");
    let registry = ToolRegistry::default();
    let ctx = tool_context_for(&fx);

    // Drive a Write through the structured tool path so the tmp-tests prefix
    // is interpreted via `resolve_tmp_tests_path` (i.e. via the `ToolContext`
    // wiring that `turn.rs` would set up at runtime).
    registry
        .execute(
            "Write",
            &json!({
                "path": "tmp-tests/src/test_alpha.rs",
                "content": "#[test] fn alpha() { assert_eq!(1, 1); }\n",
            }),
            &ctx,
        )
        .expect("Write tmp-tests/<rel> must succeed via registry");

    // The tool path only writes the body; create the metadata via the
    // lifecycle helper as well to anchor case #1's full session-scope shape.
    let tt = create_generated_test(
        &fx.tmp_tests_root,
        "src/test_beta.rs",
        b"#[test] fn beta() {}",
    )
    .unwrap();

    // tmp-tests/files/<rel> is under the session dir, NOT the workspace.
    let expected_files_dir = fx.session_dir.join("tmp-tests/files");
    let expected_meta_dir = fx.session_dir.join("tmp-tests/metadata");
    assert!(expected_files_dir.is_dir(), "files dir must exist");
    assert!(expected_meta_dir.is_dir(), "metadata dir must exist");
    assert!(expected_files_dir.join("src/test_alpha.rs").is_file());
    assert!(expected_files_dir.join("src/test_beta.rs").is_file());

    // metadata JSON exists for the lifecycle-created entry.
    let meta_path = expected_meta_dir.join(format!("{}.json", tt.id));
    assert!(meta_path.is_file(), "metadata JSON must be written");
}

// ---------------------------------------------------------------------------
// Case 2: cross-session structured-tool reads are rejected
// ---------------------------------------------------------------------------

#[test]
fn cross_session_read_via_structured_tool_is_rejected() {
    // Two independent fixtures (different state_roots) so session A's tool
    // context cannot reach session B's tmp-tests dir via path traversal.
    let a = make_session_fixture("ws-A");
    let b = make_session_fixture("ws-B");
    let registry = ToolRegistry::default();

    // Seed a tmp-test under session B.
    let b_tt = create_generated_test(&b.tmp_tests_root, "secret.rs", b"// b secret").unwrap();

    // Attempt to read it through session A's ToolContext (which is bound to
    // A's tmp_tests_root). The `tmp-tests/<rel>` prefix lands under A's
    // files dir, which never contains session B's files.
    let ctx_a = tool_context_for(&a);
    let read_result = registry.execute("Read", &json!({"path": "tmp-tests/secret.rs"}), &ctx_a);
    // Must fail (file does not exist under A); critically, must NOT succeed
    // by reading B's body.
    assert!(
        read_result.is_err(),
        "session A must not be able to read session B's tmp-test via structured Read"
    );

    // Try a path-traversal variant pointing at the absolute path of B's
    // file. Both `path_guard::resolve_user_path` (via the workspace root) and
    // `resolve_tmp_tests_path` (via A's tmp_tests_root) must reject it.
    let absolute_b = b.tmp_tests_root.join("files/secret.rs");
    let traversal = registry.execute(
        "Read",
        &json!({"path": absolute_b.to_string_lossy().to_string()}),
        &ctx_a,
    );
    assert!(
        traversal.is_err(),
        "absolute path into session B's tmp-tests must be rejected by path_guard"
    );

    // Also confirm B's body is intact (the failed reads must not have
    // mutated anything).
    let body = fs::read_to_string(absolute_b).unwrap();
    assert_eq!(body, "// b secret");
    let _ = b_tt;
}

// ---------------------------------------------------------------------------
// Case 3: `sessions clean` removes the session dir together with tmp-tests
// ---------------------------------------------------------------------------

#[test]
fn sessions_clean_removes_tmp_tests_with_session_dir() {
    let fx = make_session_fixture("ws-clean");

    // Materialize a tmp-test so the session dir actually contains
    // `tmp-tests/{files,metadata}/...`.
    create_generated_test(&fx.tmp_tests_root, "src/keep.rs", b"// keep").unwrap();
    assert!(fx.tmp_tests_root.join("files/src/keep.rs").is_file());

    // Drive `execute_clean_plan` directly — `run_clean` is the same plumbing
    // but pulls from the resolver. `plan_clean` lets us bypass that.
    let metas = scan_session_meta(&fx.state_root);
    let args = CleanArgs {
        id: Some(fx.session_id.clone()),
        ..Default::default()
    };
    let plan = plan_clean(
        &metas,
        &fx.workspace_key,
        &args,
        None,
        std::time::SystemTime::now(),
    )
    .unwrap();
    assert_eq!(plan.to_delete.len(), 1, "single-id plan must delete one");
    execute_clean_plan(&fx.state_root, &plan, true).unwrap();

    // Whole session dir gone => tmp-tests gone.
    assert!(
        !fx.session_dir.exists(),
        "session dir must be removed by clean --force"
    );
    assert!(!fx.tmp_tests_root.exists(), "tmp-tests must be gone too");
}

// ---------------------------------------------------------------------------
// Case 4a: tmp-tests/<rel> Write does not leak into the workspace
// ---------------------------------------------------------------------------

#[test]
fn tmp_tests_prefix_does_not_leak_to_workspace() {
    let fx = make_session_fixture("ws-4a");
    let registry = ToolRegistry::default();
    let ctx = tool_context_for(&fx);

    registry
        .execute(
            "Write",
            &json!({
                "path": "tmp-tests/foo.rs",
                "content": "fn main() {}",
            }),
            &ctx,
        )
        .unwrap();

    // The workspace must remain clean — no `tmp-tests/` directory and no
    // `foo.rs` should appear under the workspace root.
    assert!(
        !fx.workspace_root.join("tmp-tests").exists(),
        "tmp-tests/ must not appear under workspace root"
    );
    assert!(
        !fx.workspace_root.join("foo.rs").exists(),
        "foo.rs must not appear at workspace root"
    );

    // The body did land under the session-scoped tmp-tests files dir.
    assert!(fx.tmp_tests_root.join("files/foo.rs").is_file());
}

// ---------------------------------------------------------------------------
// Case 4b: tmp-tests/<rel> with `tmp_tests_root = None` is rejected
// ---------------------------------------------------------------------------

#[test]
fn tmp_tests_prefix_with_none_root_is_rejected() {
    let workspace = TempDir::new().unwrap();
    let registry = ToolRegistry::default();
    let ctx = ToolContext {
        root: workspace.path().to_path_buf(),
        mode: ExecutionMode::Act,
        plan_path: None,
        plan_stage: PlanStage::Stage1,
        auto_approve: true,
        interactive_approval: false,
        offline: false,
        cancel_flag: None,
        // Critical: simulate the early-startup / unit-test path.
        tmp_tests_root: None,
        tester_active: false,
    };

    let err = registry
        .execute(
            "Write",
            &json!({"path": "tmp-tests/x.rs", "content": "x"}),
            &ctx,
        )
        .unwrap_err();
    assert!(
        err.contains("tmp-tests root is unavailable"),
        "expected unavailable-root error, got: {err}"
    );
    // Workspace must remain clean.
    assert!(!workspace.path().join("tmp-tests").exists());
    assert!(!workspace.path().join("x.rs").exists());
}

// ---------------------------------------------------------------------------
// Case 4c: promote without approval is rejected
// ---------------------------------------------------------------------------

#[test]
fn promote_without_approval_is_rejected() {
    let fx = make_session_fixture("ws-4c");
    let tt = create_generated_test(&fx.tmp_tests_root, "src/x.rs", b"// x").unwrap();

    let err = promote_tmp_test(
        &fx.workspace_root,
        &fx.tmp_tests_root,
        &tt.id,
        false, // force
        false, // auto_approve
        false, // interactive_approval (no TTY)
    )
    .unwrap_err();
    assert!(
        err.contains("approval") || err.contains("--yes"),
        "expected approval-related error, got: {err}"
    );
    assert!(
        !fx.workspace_root.join("src/x.rs").exists(),
        "workspace must remain clean when promote is denied"
    );
}

// ---------------------------------------------------------------------------
// Case 4d: creating a tmp-test does not modify the workspace
// ---------------------------------------------------------------------------

#[test]
fn tmp_test_create_keeps_workspace_clean() {
    let fx = make_session_fixture("ws-4d");

    // Snapshot the workspace contents pre-create.
    let pre: Vec<PathBuf> = walk(&fx.workspace_root);

    // Create both via lifecycle and via the structured tool.
    create_generated_test(&fx.tmp_tests_root, "src/probe.rs", b"// probe").unwrap();
    let registry = ToolRegistry::default();
    let ctx = tool_context_for(&fx);
    registry
        .execute(
            "Write",
            &json!({
                "path": "tmp-tests/src/probe2.rs",
                "content": "// probe2",
            }),
            &ctx,
        )
        .unwrap();

    let post: Vec<PathBuf> = walk(&fx.workspace_root);
    assert_eq!(
        pre, post,
        "workspace contents must not change after tmp-test creation"
    );
}

fn walk(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if !root.exists() {
        return out;
    }
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let read = match fs::read_dir(&dir) {
            Ok(r) => r,
            Err(_) => continue,
        };
        for entry in read.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

// ---------------------------------------------------------------------------
// Case 5: path traversal / NUL / absolute paths are rejected
// ---------------------------------------------------------------------------

#[test]
fn path_traversal_is_rejected() {
    let fx = make_session_fixture("ws-5");
    let registry = ToolRegistry::default();
    let ctx = tool_context_for(&fx);

    // (a) `..` traversal from inside tmp-tests
    let err = registry
        .execute(
            "Write",
            &json!({"path": "tmp-tests/../escape.rs", "content": "x"}),
            &ctx,
        )
        .unwrap_err();
    assert!(
        err.contains("escape") || err.contains("traversal") || err.contains("path"),
        "expected path-escape rejection, got: {err}"
    );

    // (b) Lifecycle helper: relative_path with `..`.
    let err = create_generated_test(&fx.tmp_tests_root, "../etc/passwd", b"x").unwrap_err();
    assert!(err.contains(".."), "expected `..` rejection, got: {err}");

    // (c) NUL byte.
    let err = create_generated_test(&fx.tmp_tests_root, "a\0b.rs", b"x").unwrap_err();
    assert!(
        err.contains("NUL"),
        "expected NUL byte rejection, got: {err}"
    );

    // (d) Absolute path.
    let err = create_generated_test(&fx.tmp_tests_root, "/etc/passwd", b"x").unwrap_err();
    assert!(
        err.contains("absolute"),
        "expected absolute path rejection, got: {err}"
    );

    // No file created above the tmp-tests files root.
    assert!(!fx.session_dir.join("escape.rs").exists());
}

// ---------------------------------------------------------------------------
// Case 6: metadata is persisted with snake_case fields
// ---------------------------------------------------------------------------

#[test]
fn metadata_is_persisted_with_snake_case() {
    let fx = make_session_fixture("ws-6");
    let tt = create_generated_test(&fx.tmp_tests_root, "src/m.rs", b"body").unwrap();

    let meta_path = fx
        .tmp_tests_root
        .join("metadata")
        .join(format!("{}.json", tt.id));
    let raw = fs::read_to_string(&meta_path).unwrap();
    let json: Value = serde_json::from_str(&raw).unwrap();

    // snake_case top-level fields per design §3-2.
    let obj = json.as_object().expect("metadata is a JSON object");
    for required in &[
        "schema_version",
        "id",
        "created_at",
        "relative_path",
        "status",
    ] {
        assert!(
            obj.contains_key(*required),
            "metadata must contain field `{required}`: {raw}"
        );
    }
    // status is snake_case ("draft" / "promoted" / "discarded"), not
    // PascalCase.
    let status = obj.get("status").and_then(Value::as_str).unwrap();
    assert!(
        ["draft", "promoted", "discarded"].contains(&status),
        "status must be snake_case: {status}"
    );
    assert_eq!(status, "draft");

    // schema_version is the initial value (1).
    let v = obj.get("schema_version").and_then(Value::as_u64).unwrap();
    assert_eq!(v, u64::from(TMP_TEST_SCHEMA_VERSION));

    // last_run_result is reserved for future hydration; field structure must
    // remain exposed (either absent — `skip_serializing_if` — or present and
    // structured), per DR1-006.
    if let Some(run) = obj.get("last_run_result") {
        let run_obj = run.as_object().expect("last_run_result is structured");
        for f in &["exit_code", "finished_at", "excerpt"] {
            assert!(
                run_obj.contains_key(*f),
                "last_run_result must contain `{f}`"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Case 7a: promote copies only the test body; metadata stays under tmp-tests
// ---------------------------------------------------------------------------

#[test]
fn promote_copies_only_test_body() {
    let fx = make_session_fixture("ws-7a");
    let tt = create_generated_test(&fx.tmp_tests_root, "src/promote_me.rs", b"body!").unwrap();

    promote_tmp_test(
        &fx.workspace_root,
        &fx.tmp_tests_root,
        &tt.id,
        false, // force
        true,  // auto_approve = --yes
        false, // interactive_approval
    )
    .unwrap();

    // Body landed in the workspace.
    let dst = fx.workspace_root.join("src/promote_me.rs");
    assert!(dst.is_file(), "promoted body must be in the workspace");
    assert_eq!(fs::read(&dst).unwrap(), b"body!");

    // metadata JSON is NOT copied to the workspace.
    let possibly_leaked_meta = fx
        .workspace_root
        .join("metadata")
        .join(format!("{}.json", tt.id));
    assert!(
        !possibly_leaked_meta.exists(),
        "metadata JSON must not leak into the workspace"
    );
    // metadata still under tmp-tests with status flipped to promoted.
    let listed = list_tmp_tests(&fx.tmp_tests_root).unwrap();
    let promoted = listed.iter().find(|t| t.id == tt.id).unwrap();
    assert_eq!(promoted.status, TmpTestStatus::Promoted);
    // body retained under tmp-tests/files for history.
    assert!(fx.tmp_tests_root.join("files/src/promote_me.rs").is_file());
}

// ---------------------------------------------------------------------------
// Case 7b: promote conflict without --force is rejected
// ---------------------------------------------------------------------------

#[test]
fn promote_conflict_without_force_is_rejected() {
    let fx = make_session_fixture("ws-7b");
    fs::create_dir_all(fx.workspace_root.join("src")).unwrap();
    fs::write(fx.workspace_root.join("src/already.rs"), b"existing!").unwrap();

    let tt = create_generated_test(&fx.tmp_tests_root, "src/already.rs", b"new body!").unwrap();

    let err = promote_tmp_test(
        &fx.workspace_root,
        &fx.tmp_tests_root,
        &tt.id,
        false, // force = false
        true,
        false,
    )
    .unwrap_err();
    assert!(
        err.contains("--force") || err.contains("exists") || err.contains("Already"),
        "expected --force collision error, got: {err}"
    );

    // Existing file must not have been overwritten.
    assert_eq!(
        fs::read(fx.workspace_root.join("src/already.rs")).unwrap(),
        b"existing!"
    );
}

// ---------------------------------------------------------------------------
// Case 8: discard removes only the target tmp-test
// ---------------------------------------------------------------------------

#[test]
fn discard_removes_only_target_tmp_test() {
    let fx = make_session_fixture("ws-8");
    // Sibling `logs/` and `plans/` dirs to confirm they're untouched.
    let logs = fx.session_dir.join("logs");
    let plans = fx.session_dir.join("plans");
    fs::create_dir_all(&logs).unwrap();
    fs::create_dir_all(&plans).unwrap();
    fs::write(logs.join("L.log"), b"log line").unwrap();
    fs::write(plans.join("P.md"), b"# plan").unwrap();

    let a = create_generated_test(&fx.tmp_tests_root, "a.rs", b"a").unwrap();
    let b = create_generated_test(&fx.tmp_tests_root, "b.rs", b"b").unwrap();

    // Drive via the CLI handler so we exercise the workspace-confinement
    // wrapper too.
    run_tmp_tests_discard(&fx.state_root, &fx.workspace_key, &fx.session_id, &a.id).unwrap();

    // Target gone.
    assert!(!fx.tmp_tests_root.join("files/a.rs").exists());
    assert!(
        !fx.tmp_tests_root
            .join("metadata")
            .join(format!("{}.json", a.id))
            .exists()
    );
    // Sibling tmp-test untouched.
    assert!(fx.tmp_tests_root.join("files/b.rs").is_file());
    assert!(
        fx.tmp_tests_root
            .join("metadata")
            .join(format!("{}.json", b.id))
            .is_file()
    );
    // logs/ and plans/ untouched.
    assert!(logs.join("L.log").is_file());
    assert!(plans.join("P.md").is_file());
    assert!(fx.session_dir.join("session.json").is_file());
}

// ---------------------------------------------------------------------------
// Case 9: session resume preserves draft tmp-tests
// ---------------------------------------------------------------------------

#[test]
fn session_resume_preserves_draft_tmp_tests() {
    let fx = make_session_fixture("ws-9");
    let tt = create_generated_test(&fx.tmp_tests_root, "src/keep.rs", b"// keep").unwrap();

    // Simulate a `--resume <id>` cycle: re-validate the session id and
    // re-read the metadata directory from disk. The draft must still be
    // listed.
    let validated = validate_explicit_session_id(&fx.state_root, &fx.session_id).unwrap();
    assert_eq!(validated, fx.session_dir);

    let listed_via_helper = list_tmp_tests(&fx.tmp_tests_root).unwrap();
    assert_eq!(listed_via_helper.len(), 1);
    assert_eq!(listed_via_helper[0].id, tt.id);
    assert_eq!(listed_via_helper[0].status, TmpTestStatus::Draft);

    // Same answer through the I/O-layer `read_metadata_dir`.
    let listed_via_dir = read_metadata_dir(&fx.tmp_tests_root.join("metadata")).unwrap();
    assert_eq!(listed_via_dir.len(), 1);
    assert_eq!(listed_via_dir[0].id, tt.id);

    // And through the CLI handler (path-confinement + workspace check).
    run_tmp_tests_list(&fx.state_root, &fx.workspace_key, &fx.session_id).unwrap();
}

// ---------------------------------------------------------------------------
// Case 10: workspace cargo test must not run tmp-tests (physical isolation)
// ---------------------------------------------------------------------------

#[test]
fn workspace_cargo_test_does_not_run_tmp_tests() {
    // Per design §6-2 / §8 case #10: instead of spawning a heavy
    // `cargo test`, we assert the **physical isolation** invariant — the
    // tmp-tests live entirely under `state_root/sessions/<id>/tmp-tests/`
    // and are never written into the workspace tree, so the workspace's
    // own `cargo test` cannot pick them up.
    let fx = make_session_fixture("ws-10");

    // Drive the full structured-tool path so tmp-tests/<rel> is the actual
    // way the test name reaches disk (this is what the LLM would do via
    // Write).
    let registry = ToolRegistry::default();
    let ctx = tool_context_for(&fx);
    registry
        .execute(
            "Write",
            &json!({
                "path": "tmp-tests/src/very_unique_anvil_tmp_test_marker_xyz.rs",
                "content": "#[test] fn very_unique_anvil_tmp_test_marker_xyz_fn() { assert!(true); }\n",
            }),
            &ctx,
        )
        .unwrap();

    // (1) The workspace working tree contains no path with the marker.
    let entries = walk(&fx.workspace_root);
    let marker = "very_unique_anvil_tmp_test_marker_xyz";
    let leaked: Vec<&PathBuf> = entries
        .iter()
        .filter(|p| p.to_string_lossy().contains(marker))
        .collect();
    assert!(
        leaked.is_empty(),
        "tmp-test marker leaked into workspace: {leaked:?}"
    );

    // (2) `git status --porcelain` (when run in a git workspace) reports no
    //     change. We initialise a tiny empty git repo at the workspace and
    //     commit an empty initial state to make the test deterministic; if
    //     git is unavailable on the host, we fall back to (1)'s structural
    //     check.
    if !is_git_available() {
        return;
    }
    init_empty_git_repo(&fx.workspace_root);
    let porcelain = run_git(&fx.workspace_root, &["status", "--porcelain"]);
    assert!(
        porcelain.trim().is_empty(),
        "git status --porcelain must be empty after tmp-test creation: {porcelain:?}"
    );

    // (3) `tmp-tests/` dir does not exist anywhere under the workspace.
    assert!(!fx.workspace_root.join("tmp-tests").exists());

    // (4) The body actually exists in the session-scoped tmp-tests dir
    //     (sanity for the test fixture).
    assert!(
        fx.tmp_tests_root
            .join("files/src/very_unique_anvil_tmp_test_marker_xyz.rs")
            .is_file()
    );
}

fn is_git_available() -> bool {
    Command::new("git")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn init_empty_git_repo(dir: &Path) {
    run_git(dir, &["init", "--quiet"]);
    // Configure user so `commit` works in CI; values don't matter.
    run_git(
        dir,
        &["config", "user.email", "anvil-tmp-tests@example.invalid"],
    );
    run_git(dir, &["config", "user.name", "anvil-tmp-tests"]);
    run_git(dir, &["config", "commit.gpgsign", "false"]);
}

fn run_git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .expect("git invocation");
    if !out.status.success() {
        // For `init`/`config` we don't fail the test — they're best-effort.
        // For `status` the caller will see empty stdout and assert.
        return String::from_utf8_lossy(&out.stderr).into_owned();
    }
    String::from_utf8_lossy(&out.stdout).into_owned()
}

// ---------------------------------------------------------------------------
// Cross-case sanity: full lifecycle exercises CLI promote handler too
// ---------------------------------------------------------------------------

#[test]
fn cli_handler_promote_writes_to_workspace_under_yes() {
    // Belt-and-suspenders: case 4c covers the negative path; here we drive
    // the *positive* path through the CLI handler (`run_tmp_tests_promote`)
    // to keep coverage of the handler's wiring (workspace-key check +
    // `tmp_tests::promote_tmp_test`) without a separate acceptance row.
    let fx = make_session_fixture("ws-cli-promote");
    let tt = create_generated_test(&fx.tmp_tests_root, "src/promoted.rs", b"// p").unwrap();
    run_tmp_tests_promote(
        &fx.state_root,
        &fx.workspace_key,
        &fx.workspace_root,
        &fx.session_id,
        &tt.id,
        false, // force
        true,  // --yes
    )
    .unwrap();
    assert!(fx.workspace_root.join("src/promoted.rs").is_file());
    assert_eq!(
        fs::read(fx.workspace_root.join("src/promoted.rs")).unwrap(),
        b"// p"
    );
}

// ---------------------------------------------------------------------------
// Sanity: helper imports stay alive even when cases shrink (compile gate)
// ---------------------------------------------------------------------------

#[test]
fn fixture_helpers_compile() {
    let _ = derive_test_id;
    let _ = discard_tmp_test;
    let _: fn(&Path) -> Result<Vec<TmpTest>, String> = read_metadata_dir;
    let _: fn(&Path) -> Result<Vec<TmpTest>, String> = list_tmp_tests;
    let _ = TmpTestStatus::Draft;
    let _ = tmp_tests::TMP_TEST_SCHEMA_VERSION;
}
