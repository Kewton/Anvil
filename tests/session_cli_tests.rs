//! Integration tests for Issue #409: `--resume` and `sessions` subcommand.
//!
//! These exercise the public helpers that the `run_cli` wiring layers on top
//! of. The tests deliberately avoid spawning the `anvil` binary so they run
//! offline (no Ollama, no network) and stay deterministic.

use anvil::cli::{CliArgs, Command, ResumeRequest, SessionsAction};
use anvil::modes::plan_act::{ExecutionMode, ModeState, PlanStage, TaskProfile};
use anvil::session::compact::{COMPACT_SUMMARY_PREFIX, compact_messages, find_last_user_prompt};
use anvil::session::sessions_cli::{
    CleanArgs, ShowView, compute_list_rows, plan_clean, scan_session_meta,
    validate_explicit_session_id, validate_session_dir, validate_session_id_format,
};
use anvil::session::store::{
    ConversationMessage, SessionSnapshot, SessionStore, reconcile_resume_state,
};
use clap::Parser;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};
use tempfile::TempDir;

// --------------------------------------------------------------------------
// Helpers
// --------------------------------------------------------------------------

fn v7_id() -> String {
    uuid::Uuid::now_v7().to_string()
}

fn write_session(
    state_root: &Path,
    id: &str,
    workspace_key: &str,
    messages: Vec<ConversationMessage>,
    mode: ExecutionMode,
    active_plan_path: Option<PathBuf>,
    active_root: Option<PathBuf>,
) -> PathBuf {
    let session_dir = state_root.join("sessions").join(id);
    fs::create_dir_all(&session_dir).unwrap();
    let snap = SessionSnapshot {
        id: id.to_string(),
        workspace_key: workspace_key.to_string(),
        messages,
        mode_state: ModeState {
            mode,
            active_plan_path,
            task_profile: TaskProfile::Generic,
            plan_stage: PlanStage::Stage1,
        },
        active_root,
        ..Default::default()
    };
    let path = session_dir.join("session.json");
    fs::write(&path, serde_json::to_string_pretty(&snap).unwrap()).unwrap();
    path
}

fn simple_user_only(id: &str, ws: &str, state_root: &Path, text: &str) -> PathBuf {
    write_session(
        state_root,
        id,
        ws,
        vec![ConversationMessage::user(text.into())],
        ExecutionMode::Act,
        None,
        None,
    )
}

// --------------------------------------------------------------------------
// 1 & 2: CLI exclusivity
// --------------------------------------------------------------------------

#[test]
fn resume_and_fresh_session_are_exclusive() {
    let args = CliArgs::try_parse_from(["anvil", "--resume", "--fresh-session"]).expect("parse");
    let err = args.validate().expect_err("must reject");
    assert!(err.contains("--resume"), "got: {err}");
    assert!(err.contains("--fresh-session"), "got: {err}");
}

#[test]
fn resume_and_prompt_are_exclusive() {
    let args = CliArgs::try_parse_from(["anvil", "--resume", "--prompt", "hi"]).expect("parse");
    let err = args.validate().expect_err("must reject");
    assert!(err.contains("--resume"), "got: {err}");
}

#[test]
fn resume_and_oneshot_are_exclusive() {
    let args = CliArgs::try_parse_from(["anvil", "--resume", "--oneshot"]).expect("parse");
    let err = args.validate().expect_err("must reject");
    assert!(err.contains("--resume"), "got: {err}");
}

// --------------------------------------------------------------------------
// Task 2.2 / 15: existing CLI surface remains backward compatible
// --------------------------------------------------------------------------

#[test]
fn existing_prompt_flow_still_parses_after_subcommand_introduction() {
    let args = CliArgs::try_parse_from(["anvil", "--prompt", "hello world"]).expect("parse");
    assert!(args.command.is_none());
    assert_eq!(args.prompt.as_deref(), Some("hello world"));
    assert!(args.validate().is_ok());
}

#[test]
fn sessions_subcommand_parses() {
    let args = CliArgs::try_parse_from(["anvil", "sessions", "list", "--json"]).expect("parse");
    match args.command {
        Some(Command::Sessions {
            action: SessionsAction::List { all, json },
        }) => {
            assert!(!all);
            assert!(json);
        }
        _ => panic!("expected Sessions::List"),
    }
}

#[test]
fn resume_with_id_parses() {
    let id = v7_id();
    let argv = ["anvil", "--resume", id.as_str()];
    let args = CliArgs::try_parse_from(argv).expect("parse");
    assert_eq!(
        ResumeRequest::from_flag(args.resume.clone()),
        ResumeRequest::WithId(id)
    );
    assert!(args.validate().is_ok());
}

#[test]
fn resume_without_id_parses_as_latest() {
    let args = CliArgs::try_parse_from(["anvil", "--resume"]).expect("parse");
    assert_eq!(
        ResumeRequest::from_flag(args.resume.clone()),
        ResumeRequest::Latest
    );
}

// --------------------------------------------------------------------------
// 3 & 10 & 4: find_last_user_prompt + compaction behavior
// --------------------------------------------------------------------------

#[test]
fn find_last_user_with_tail_assistant() {
    let msgs = vec![
        ConversationMessage::user("hello".into()),
        ConversationMessage::assistant("hi".into(), vec![]),
    ];
    assert_eq!(find_last_user_prompt(&msgs).as_deref(), Some("hello"));
}

#[test]
fn find_last_user_with_tail_tool() {
    let msgs = vec![
        ConversationMessage::user("do something".into()),
        ConversationMessage::assistant("".into(), vec![]),
        ConversationMessage::tool("read".into(), "contents".into()),
    ];
    assert_eq!(
        find_last_user_prompt(&msgs).as_deref(),
        Some("do something")
    );
}

#[test]
fn find_last_user_returns_none_when_compaction_removed_all_users() {
    let mut messages = (0..30)
        .map(|i| ConversationMessage::assistant(format!("reply-{i}"), vec![]))
        .collect::<Vec<_>>();
    messages.push(ConversationMessage::system(format!(
        "{COMPACT_SUMMARY_PREFIX}\nsummary"
    )));
    assert!(find_last_user_prompt(&messages).is_none());
}

#[test]
fn find_last_user_skips_compaction_summary_prefix() {
    let msgs = vec![
        ConversationMessage::user("original".into()),
        ConversationMessage::system(format!("{COMPACT_SUMMARY_PREFIX}\nuser: old prompt")),
    ];
    assert_eq!(
        find_last_user_prompt(&msgs).as_deref(),
        Some("original"),
        "compaction summary should be skipped"
    );
}

#[test]
fn compaction_then_find_last_user_still_works_when_tail_keeps_user() {
    let mut messages: Vec<ConversationMessage> = (0..40)
        .map(|i| {
            if i % 3 == 0 {
                ConversationMessage::user(format!("prompt-{i}"))
            } else {
                ConversationMessage::assistant(format!("reply-{i}"), vec![])
            }
        })
        .collect();
    compact_messages(&mut messages, 10);
    let got = find_last_user_prompt(&messages);
    assert!(got.is_some(), "tail should still contain a user message");
}

// --------------------------------------------------------------------------
// 5 / 6 / 7: list filtering and JSON-ish scope tagging
// --------------------------------------------------------------------------

#[test]
fn list_filters_current_workspace_only_by_default() {
    let tmp = TempDir::new().unwrap();
    simple_user_only(&v7_id(), "ws-current", tmp.path(), "A");
    simple_user_only(&v7_id(), "ws-other", tmp.path(), "B");
    let metas = scan_session_meta(tmp.path());
    let rows = compute_list_rows(&metas, "ws-current", false);
    assert_eq!(rows.len(), 1);
}

#[test]
fn list_all_includes_unassigned_and_foreign() {
    let tmp = TempDir::new().unwrap();
    simple_user_only(&v7_id(), "ws-current", tmp.path(), "A");
    simple_user_only(&v7_id(), "", tmp.path(), "B"); // unassigned
    simple_user_only(&v7_id(), "ws-other", tmp.path(), "C");
    let metas = scan_session_meta(tmp.path());
    let rows = compute_list_rows(&metas, "ws-current", true);
    assert_eq!(rows.len(), 3);
    assert!(rows.iter().any(|r| matches!(
        r.scope,
        anvil::session::sessions_cli::WorkspaceScope::Unassigned
    )));
}

// --------------------------------------------------------------------------
// 8 & 19: show fields + explicit-id rejection
// --------------------------------------------------------------------------

#[test]
fn show_view_contains_acceptance_fields() {
    let tmp = TempDir::new().unwrap();
    let id = v7_id();
    let plan_path = tmp.path().join("plan.md");
    fs::write(&plan_path, "plan").unwrap();
    write_session(
        tmp.path(),
        &id,
        "ws-a",
        vec![
            ConversationMessage::user("first user".into()),
            ConversationMessage::assistant("ok".into(), vec![]),
        ],
        ExecutionMode::Plan,
        Some(plan_path),
        Some(tmp.path().to_path_buf()),
    );
    let session_json = tmp.path().join("sessions").join(&id).join("session.json");
    let data = fs::read_to_string(&session_json).unwrap();
    let snap: SessionSnapshot = serde_json::from_str(&data).unwrap();

    let view = ShowView::from_snapshot(&snap);
    assert_eq!(view.id, id);
    assert_eq!(view.workspace_key, "ws-a");
    assert_eq!(view.message_count, 2);
    assert_eq!(view.first_user_preview.as_deref(), Some("first user"));
    assert!(view.last_assistant_or_tool_preview.is_some());
    assert_eq!(view.mode, ExecutionMode::Plan);
}

#[test]
fn validate_explicit_rejects_non_v7_ids() {
    assert!(validate_session_id_format("not-a-uuid").is_err());
    assert!(validate_session_id_format("0190f3c0-1111-4a00-8000-000000000000").is_err());
}

#[test]
fn validate_explicit_rejects_missing_directory() {
    let tmp = TempDir::new().unwrap();
    let err = validate_explicit_session_id(tmp.path(), &v7_id()).unwrap_err();
    assert!(err.contains("session"), "unexpected: {err}");
}

#[cfg(unix)]
#[test]
fn validate_explicit_rejects_symlink() {
    let tmp = TempDir::new().unwrap();
    let sessions = tmp.path().join("sessions");
    fs::create_dir_all(&sessions).unwrap();

    let real_id = v7_id();
    let real_dir = sessions.join(&real_id);
    fs::create_dir_all(&real_dir).unwrap();
    fs::write(real_dir.join("session.json"), "{}").unwrap();

    let link_id = v7_id();
    std::os::unix::fs::symlink(&real_dir, sessions.join(&link_id)).unwrap();

    let err = validate_explicit_session_id(tmp.path(), &link_id).unwrap_err();
    assert!(err.contains("symlink"), "unexpected: {err}");
}

#[test]
fn validate_session_dir_rejects_path_traversal_attempt() {
    let tmp = TempDir::new().unwrap();
    // An id that happens to parse as UUID v7 but references something that
    // does not exist in state_root/sessions/ — format check is upstream.
    let err = validate_session_dir(tmp.path(), "0199fe00-0000-7000-8000-aaaaaaaaaaaa").unwrap_err();
    assert!(err.contains("session"), "unexpected: {err}");
}

// --------------------------------------------------------------------------
// 9 / 10 / 12: clean dry-run, protection, sort stability
// --------------------------------------------------------------------------

#[test]
fn plan_clean_older_than_matches_list_order_desc() {
    let tmp = TempDir::new().unwrap();
    let now = SystemTime::now();

    let fresh_id = v7_id();
    simple_user_only(&fresh_id, "ws", tmp.path(), "fresh");

    let old_id = v7_id();
    let old_path = simple_user_only(&old_id, "ws", tmp.path(), "old");
    // Drive session.json mtime 40 days into the past so --older-than 30 trips.
    let old_mtime = now - Duration::from_secs(60 * 60 * 24 * 40);
    let f = fs::File::options().write(true).open(&old_path).unwrap();
    f.set_modified(old_mtime).unwrap();
    drop(f);

    let metas = scan_session_meta(tmp.path());
    // list view comes out newest-first
    let list_rows = compute_list_rows(&metas, "ws", false);
    assert_eq!(
        list_rows.first().map(|r| r.id.as_str()),
        Some(fresh_id.as_str())
    );

    let args = CleanArgs {
        older_than_days: Some(30),
        ..Default::default()
    };
    let plan = plan_clean(&metas, "ws", &args, Some(&fresh_id), now).unwrap();
    assert_eq!(plan.to_delete.len(), 1);
    assert_eq!(plan.to_delete[0].id, old_id);
}

#[test]
fn plan_clean_protects_resolver_winner() {
    let tmp = TempDir::new().unwrap();
    let winner = v7_id();
    simple_user_only(&winner, "ws", tmp.path(), "winner");
    simple_user_only(&v7_id(), "ws", tmp.path(), "loser");

    let metas = scan_session_meta(tmp.path());
    let args = CleanArgs {
        keep: Some(0),
        ..Default::default()
    };
    let plan = plan_clean(&metas, "ws", &args, Some(&winner), SystemTime::now()).unwrap();
    assert!(plan.to_delete.iter().all(|m| m.id != winner));
}

#[cfg(unix)]
#[test]
fn plan_clean_ignores_symlinked_session_dirs() {
    // iter_session_dirs (used by scan_session_meta) already skips symlinks,
    // so clean never even sees them as candidates.
    let tmp = TempDir::new().unwrap();
    let sessions = tmp.path().join("sessions");
    fs::create_dir_all(&sessions).unwrap();

    let real_id = v7_id();
    let real_dir = sessions.join(&real_id);
    fs::create_dir_all(&real_dir).unwrap();
    fs::write(
        real_dir.join("session.json"),
        r#"{"mode_state":{"mode":"Act","active_plan_path":null},"messages":[],"checkpoints":[]}"#,
    )
    .unwrap();

    let link_id = v7_id();
    std::os::unix::fs::symlink(&real_dir, sessions.join(&link_id)).unwrap();

    let metas = scan_session_meta(tmp.path());
    assert_eq!(metas.len(), 1);
    assert_eq!(metas[0].id, real_id);
}

// --------------------------------------------------------------------------
// 13 / 14: reconcile_resume_state
// --------------------------------------------------------------------------

#[test]
fn reconcile_clears_missing_active_root() {
    let tmp = TempDir::new().unwrap();
    let id = v7_id();
    let store = SessionStore::new(tmp.path(), &id, "ws");
    let mut snap = SessionSnapshot {
        id: id.clone(),
        workspace_key: "ws".into(),
        active_root: Some(PathBuf::from("/definitely-not-a-dir-xyz")),
        ..Default::default()
    };
    reconcile_resume_state(&mut snap, tmp.path());
    assert!(snap.active_root.is_none());
    // store is unused but we built it to ensure the helper compiles
    // against the public API.
    let _ = store;
}

#[test]
fn reconcile_downgrades_plan_when_plan_file_missing() {
    let tmp = TempDir::new().unwrap();
    let mut snap = SessionSnapshot {
        mode_state: ModeState {
            mode: ExecutionMode::Plan,
            active_plan_path: Some(PathBuf::from("/definitely-not-a-file-xyz")),
            task_profile: TaskProfile::Generic,
            plan_stage: PlanStage::Stage1,
        },
        ..Default::default()
    };
    reconcile_resume_state(&mut snap, tmp.path());
    assert_eq!(snap.mode_state.mode, ExecutionMode::Act);
    assert!(snap.mode_state.active_plan_path.is_none());
}

#[test]
fn reconcile_is_noop_when_everything_is_valid() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().to_path_buf();
    let plan = tmp.path().join("plan.md");
    fs::write(&plan, "plan").unwrap();
    let mut snap = SessionSnapshot {
        active_root: Some(root.clone()),
        mode_state: ModeState {
            mode: ExecutionMode::Plan,
            active_plan_path: Some(plan.clone()),
            task_profile: TaskProfile::Generic,
            plan_stage: PlanStage::Stage1,
        },
        ..Default::default()
    };
    reconcile_resume_state(&mut snap, tmp.path());
    assert_eq!(snap.active_root.as_deref(), Some(root.as_path()));
    assert_eq!(snap.mode_state.mode, ExecutionMode::Plan);
    assert_eq!(snap.mode_state.active_plan_path.as_ref(), Some(&plan));
}
