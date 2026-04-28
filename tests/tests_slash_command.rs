//! E2E coverage for the `/tests` slash command (Issue #460).
//!
//! These exercise the public REPL surface (`Agent::process_line`) end-to-end
//! against a real on-disk tmp-tests directory, so the dispatcher, the
//! `tmp_tests::*` lifecycle helpers, and the rendering helper stay wired
//! together. They share the same fixture pattern as `tests/precautions_repl.rs`
//! and `tests/tmp_tests_e2e.rs` (no Ollama, no network).

use std::fs;
use std::path::Path;

use anvil::agent::Agent;
use anvil::agent::loop_run::{AgentEvent, FooterHandle};
use anvil::config::Config;
use anvil::model_registry::RuntimeModels;
use anvil::modes::plan_act::ExecutionMode;
use anvil::ollama::client::OllamaClient;
use anvil::session::store::{SessionSnapshot, SessionStore};
use anvil::session::tmp_tests;
use tempfile::tempdir;

const SESSION_ID: &str = "0199fe00-0000-7000-8000-000000000460";
const WORKSPACE_KEY: &str = "anvil-460-tests";

fn build_agent(cwd: &Path, state_root: &Path, session: SessionSnapshot, yes_mode: bool) -> Agent {
    let mut config = Config::default();
    config.cwd = cwd.to_path_buf();
    config.requested_model = Some("test-model".to_string());
    config.ollama_host = "http://127.0.0.1:11434".to_string();
    config.state_dir_override = Some(state_root.to_path_buf());
    config.yes_mode = yes_mode;

    Agent::new(
        config,
        RuntimeModels {
            main: "test-model".to_string(),
            sidecar: None,
        },
        OllamaClient::new("http://127.0.0.1:11434".to_string()).unwrap(),
        SessionStore::new(state_root, SESSION_ID, WORKSPACE_KEY),
        session,
        FooterHandle::disabled(),
    )
}

fn continue_message(event: AgentEvent) -> String {
    match event {
        AgentEvent::Continue(Some(message)) => message,
        AgentEvent::Continue(None) => panic!("expected Continue(Some), got Continue(None)"),
        AgentEvent::Exit => panic!("expected Continue, got Exit"),
    }
}

/// Lay out `state_root/sessions/<id>/` so `session_store.path()` resolves and
/// `tmp_tests_root` / `session.json` siblings match production.
fn prepare_session_dir(state_root: &Path) -> std::path::PathBuf {
    let session_dir = state_root.join("sessions").join(SESSION_ID);
    fs::create_dir_all(&session_dir).unwrap();
    session_dir
}

fn act_session() -> SessionSnapshot {
    SessionSnapshot {
        id: SESSION_ID.to_string(),
        workspace_key: WORKSPACE_KEY.to_string(),
        ..SessionSnapshot::default()
    }
}

#[test]
fn tests_list_on_fresh_session_reports_empty() {
    let temp = tempdir().unwrap();
    let state_root = temp.path().join(".anvil-state");
    prepare_session_dir(&state_root);

    let mut agent = build_agent(temp.path(), &state_root, act_session(), false);
    let out = continue_message(agent.process_line("/tests", false).unwrap());
    assert_eq!(out, "no tmp-tests found");

    let out = continue_message(agent.process_line("/tests list", false).unwrap());
    assert_eq!(out, "no tmp-tests found");
}

#[test]
fn tests_list_renders_existing_tmp_tests() {
    let temp = tempdir().unwrap();
    let state_root = temp.path().join(".anvil-state");
    let session_dir = prepare_session_dir(&state_root);
    let tmp_tests_root = session_dir.join("tmp-tests");

    let tt = tmp_tests::create_generated_test(
        &tmp_tests_root,
        "tests/smoke_460.rs",
        b"#[test]\nfn smoke() {}\n",
    )
    .unwrap();

    let mut agent = build_agent(temp.path(), &state_root, act_session(), false);
    let out = continue_message(agent.process_line("/tests", false).unwrap());

    assert!(
        out.starts_with("ID                                              STATUS"),
        "expected header, got: {out}"
    );
    assert!(out.contains(&tt.id), "list missing test id: {out}");
    assert!(out.contains("draft"), "list missing draft status: {out}");
    assert!(
        out.contains("tests/smoke_460.rs"),
        "list missing rel path: {out}"
    );
}

#[test]
fn tests_promote_writes_workspace_file_and_marks_promoted() {
    let temp = tempdir().unwrap();
    let cwd = temp.path().join("workspace");
    fs::create_dir_all(&cwd).unwrap();
    let state_root = temp.path().join(".anvil-state");
    let session_dir = prepare_session_dir(&state_root);
    let tmp_tests_root = session_dir.join("tmp-tests");

    let tt = tmp_tests::create_generated_test(
        &tmp_tests_root,
        "tests/promoted_460.rs",
        b"#[test]\nfn ok() {}\n",
    )
    .unwrap();

    let mut agent = build_agent(&cwd, &state_root, act_session(), false);
    let out = continue_message(
        agent
            .process_line(&format!("/tests promote {}", tt.id), false)
            .unwrap(),
    );
    assert!(out.starts_with("promoted "), "{out}");

    let dst = cwd.join("tests/promoted_460.rs");
    assert!(dst.exists(), "promote did not write workspace file");
    let body = fs::read_to_string(&dst).unwrap();
    assert!(body.contains("fn ok()"), "wrong contents: {body}");

    let listed = tmp_tests::list_tmp_tests(&tmp_tests_root).unwrap();
    let found = listed.iter().find(|t| t.id == tt.id).expect("metadata");
    assert!(matches!(found.status, tmp_tests::TmpTestStatus::Promoted));
}

#[test]
fn tests_promote_collision_without_force_surfaces_lifecycle_error() {
    let temp = tempdir().unwrap();
    let cwd = temp.path().join("workspace");
    fs::create_dir_all(cwd.join("tests")).unwrap();
    fs::write(cwd.join("tests/collide.rs"), b"existing").unwrap();
    let state_root = temp.path().join(".anvil-state");
    let session_dir = prepare_session_dir(&state_root);
    let tmp_tests_root = session_dir.join("tmp-tests");

    let tt =
        tmp_tests::create_generated_test(&tmp_tests_root, "tests/collide.rs", b"new body").unwrap();

    let mut agent = build_agent(&cwd, &state_root, act_session(), false);
    let out = continue_message(
        agent
            .process_line(&format!("/tests promote {}", tt.id), false)
            .unwrap(),
    );
    assert!(
        out.contains("failed to create promote target") && out.contains("use --force to overwrite"),
        "expected lifecycle collision error, got: {out}"
    );

    // Original workspace file must remain untouched (TOCTOU guarantee).
    assert_eq!(
        fs::read_to_string(cwd.join("tests/collide.rs")).unwrap(),
        "existing"
    );
}

#[test]
fn tests_promote_with_force_overwrites_regular_file() {
    let temp = tempdir().unwrap();
    let cwd = temp.path().join("workspace");
    fs::create_dir_all(cwd.join("tests")).unwrap();
    fs::write(cwd.join("tests/over.rs"), b"old").unwrap();
    let state_root = temp.path().join(".anvil-state");
    let session_dir = prepare_session_dir(&state_root);
    let tmp_tests_root = session_dir.join("tmp-tests");

    let tt =
        tmp_tests::create_generated_test(&tmp_tests_root, "tests/over.rs", b"replacement").unwrap();

    let mut agent = build_agent(&cwd, &state_root, act_session(), false);
    let out = continue_message(
        agent
            .process_line(&format!("/tests promote {} --force", tt.id), false)
            .unwrap(),
    );
    assert!(out.starts_with("promoted "), "{out}");
    assert_eq!(
        fs::read_to_string(cwd.join("tests/over.rs")).unwrap(),
        "replacement"
    );
}

#[test]
fn tests_promote_in_plan_mode_is_rejected_without_touching_workspace() {
    let temp = tempdir().unwrap();
    let cwd = temp.path().join("workspace");
    fs::create_dir_all(&cwd).unwrap();
    let state_root = temp.path().join(".anvil-state");
    let session_dir = prepare_session_dir(&state_root);
    let tmp_tests_root = session_dir.join("tmp-tests");

    let tt = tmp_tests::create_generated_test(&tmp_tests_root, "tests/plan.rs", b"x").unwrap();

    let mut session = act_session();
    session.mode_state.mode = ExecutionMode::Plan;
    let mut agent = build_agent(&cwd, &state_root, session, false);
    let out = continue_message(
        agent
            .process_line(&format!("/tests promote {}", tt.id), false)
            .unwrap(),
    );
    assert!(
        out.starts_with("Plan mode disallows /tests promote"),
        "expected Plan-mode rejection, got: {out}"
    );
    assert!(
        !cwd.join("tests/plan.rs").exists(),
        "promote in Plan mode must not write to workspace"
    );

    // List and discard remain available in Plan mode.
    let list = continue_message(agent.process_line("/tests", false).unwrap());
    assert!(
        list.contains(&tt.id),
        "list should work in Plan mode: {list}"
    );
    let discarded = continue_message(
        agent
            .process_line(&format!("/tests discard {}", tt.id), false)
            .unwrap(),
    );
    assert_eq!(discarded, format!("discarded {}", tt.id));
}

#[test]
fn tests_discard_removes_body_and_metadata() {
    let temp = tempdir().unwrap();
    let state_root = temp.path().join(".anvil-state");
    let session_dir = prepare_session_dir(&state_root);
    let tmp_tests_root = session_dir.join("tmp-tests");
    let tt = tmp_tests::create_generated_test(&tmp_tests_root, "tests/d.rs", b"x").unwrap();

    let mut agent = build_agent(temp.path(), &state_root, act_session(), false);
    let out = continue_message(
        agent
            .process_line(&format!("/tests discard {}", tt.id), false)
            .unwrap(),
    );
    assert_eq!(out, format!("discarded {}", tt.id));

    let after = tmp_tests::list_tmp_tests(&tmp_tests_root).unwrap();
    assert!(after.iter().all(|t| t.id != tt.id));
    assert!(!tmp_tests_root.join("files/tests/d.rs").exists());
}

#[test]
fn tests_invalid_id_does_not_panic() {
    let temp = tempdir().unwrap();
    let state_root = temp.path().join(".anvil-state");
    prepare_session_dir(&state_root);

    let mut agent = build_agent(temp.path(), &state_root, act_session(), false);

    let promote_out = continue_message(
        agent
            .process_line("/tests promote not-an-id", false)
            .unwrap(),
    );
    assert!(promote_out.starts_with("error: "), "{promote_out}");

    let discard_out = continue_message(
        agent
            .process_line("/tests discard not-an-id", false)
            .unwrap(),
    );
    assert!(discard_out.starts_with("error: "), "{discard_out}");
}

#[test]
fn tests_unknown_subcommand_prints_usage() {
    let temp = tempdir().unwrap();
    let state_root = temp.path().join(".anvil-state");
    prepare_session_dir(&state_root);

    let mut agent = build_agent(temp.path(), &state_root, act_session(), false);
    let out = continue_message(agent.process_line("/tests bogus", false).unwrap());
    assert!(out.starts_with("usage: /tests"), "{out}");
}
