//! Issue #462 — CaseRecord extraction E2E tests, in three groups:
//!
//!   (a) fixture-based pure-function path: build CaseRecordInputs directly
//!   (b) git-fixture path: tempdir + `git init` + `git config remote.origin.url`
//!   (c) auto_test integration is exercised through `verify_commands` in (a),
//!       since the agent-layer adapter (turn.rs) is the integration seam — a
//!       full agent run E2E lives outside this tree (Ollama-dependent).

use std::path::Path;
use std::process::Command;

use anvil::session::anvil_score::AnvilScore;
use anvil::session::case_record::{
    CaseRecord, CaseRecordInputs, MAX_CASE_RECORDS, PersistError, PrecautionSnapshot,
    capture_repo_fingerprint, derive_case_id, extract, hash_language_stack, iter_case_files,
    persist, sanitize_git_remote, validate_case_id,
};
use anvil::session::feedback::FeedbackKind;
use anvil::session::precaution::{Precaution, PrecautionSource, PrecautionStatus, Severity};

fn fake_score_success() -> AnvilScore {
    AnvilScore {
        build_passed: Some(true),
        tests_passed: Some(true),
        compile_errors_delta: Some(0),
        test_failures_delta: Some(0),
        compile_error_count: Some(0),
        test_failure_count: Some(0),
        implementation_files_changed: Some(1),
        test_files_changed: Some(1),
        setup_files_changed: Some(0),
        unsafe_actions_blocked: 0,
        consecutive_no_progress_turns: 0,
        user_visible_artifact: true,
    }
}

fn fake_precaution(id: &str, text: &str, status: PrecautionStatus) -> Precaution {
    Precaution {
        id: id.to_string(),
        source: PrecautionSource::BuildFailure,
        severity: Severity::High,
        text: text.to_string(),
        applies_to: vec![],
        status,
        retired_reason: None,
    }
}

// ---------------------------------------------------------------------------
// Group (a): fixture-based single-path
// ---------------------------------------------------------------------------

#[test]
fn extract_assembles_case_with_active_precautions_only() {
    let tmp = tempfile::tempdir().unwrap();
    let pres = vec![
        fake_precaution("p1", "active rule", PrecautionStatus::Active),
        fake_precaution("p2", "old rule", PrecautionStatus::Retired),
    ];
    let score = fake_score_success();
    let cf: Vec<String> = vec!["src/case_record.rs".into()];
    let v: Vec<String> = vec!["cargo test".into()];
    let lang: Vec<String> = vec!["rust".into()];
    let fb: Vec<FeedbackKind> = vec![];
    let inputs = CaseRecordInputs {
        workspace_key: "ws-key",
        work_root: tmp.path(),
        active_task: Some("fix off-by-one"),
        language_stack: &lang,
        initial_feedback: &fb,
        active_precautions: &pres,
        changed_files: &cf,
        verify_commands: &v,
        anvil_score: &score,
        repo_edit_succeeded_this_turn: true,
        unsafe_blocks_this_turn: 0,
        auto_test_active: true,
    };
    let case = extract(&inputs).expect("Some(CaseRecord)");
    assert!(validate_case_id(&case.case_id));
    assert_eq!(case.successful_precautions.len(), 1);
    assert_eq!(case.successful_precautions[0].id, "p1");
    assert_eq!(case.outcome_score.build_passed, Some(true));
    assert!(
        case.verify_commands
            .iter()
            .any(|c| c.contains("cargo test"))
    );
}

#[test]
fn persist_writes_under_state_root_cases() {
    let tmp = tempfile::tempdir().unwrap();
    let pres = vec![];
    let score = fake_score_success();
    let cf: Vec<String> = vec![];
    let v: Vec<String> = vec![];
    let lang: Vec<String> = vec![];
    let fb: Vec<FeedbackKind> = vec![];
    let inputs = CaseRecordInputs {
        workspace_key: "ws-x",
        work_root: tmp.path(),
        active_task: Some("noop"),
        language_stack: &lang,
        initial_feedback: &fb,
        active_precautions: &pres,
        changed_files: &cf,
        verify_commands: &v,
        anvil_score: &score,
        repo_edit_succeeded_this_turn: true,
        unsafe_blocks_this_turn: 0,
        auto_test_active: true,
    };
    let case = extract(&inputs).unwrap();
    let bytes = persist(tmp.path(), &case).unwrap();
    let path = tmp
        .path()
        .join("cases")
        .join(format!("{}.json", case.case_id));
    assert!(path.exists());
    assert_eq!(bytes, std::fs::metadata(&path).unwrap().len() as usize);
}

#[test]
fn persist_skips_when_oversized() {
    let tmp = tempfile::tempdir().unwrap();
    let mut case = CaseRecord {
        case_id: "case_aaaaaaaaaaaaaaaaaaaaaaaa".into(),
        created_at: 1,
        repo_fingerprint: anvil::session::case_record::RepoFingerprint {
            workspace_key: "ws".into(),
            git_remote: None,
            git_head_branch: None,
            language_stack_hash: "0000000000000000".into(),
        },
        task_signature: "x".into(),
        language_stack: vec![],
        initial_feedback: vec![],
        successful_precautions: vec![],
        changed_files_summary: vec![],
        verify_commands: vec![],
        outcome_score: fake_score_success(),
    };
    case.changed_files_summary = (0..2000).map(|i| format!("file_{i}.rs")).collect();
    let err = persist(tmp.path(), &case).unwrap_err();
    assert!(matches!(err, PersistError::TooLarge { .. }));
}

#[test]
fn extract_does_not_include_raw_assistant_text() {
    // Issue #462 invariant: case must not carry raw LLM conversation /
    // assistant reply. Our shape only exposes scrubbed task_signature; this
    // test asserts the absence of any field that holds free-form LLM text.
    let tmp = tempfile::tempdir().unwrap();
    let pres = vec![];
    let score = fake_score_success();
    let cf: Vec<String> = vec![];
    let v: Vec<String> = vec![];
    let lang: Vec<String> = vec![];
    let fb: Vec<FeedbackKind> = vec![];
    let inputs = CaseRecordInputs {
        workspace_key: "ws",
        work_root: tmp.path(),
        active_task: Some("api_key=sk_live_super_secret"),
        language_stack: &lang,
        initial_feedback: &fb,
        active_precautions: &pres,
        changed_files: &cf,
        verify_commands: &v,
        anvil_score: &score,
        repo_edit_succeeded_this_turn: true,
        unsafe_blocks_this_turn: 0,
        auto_test_active: true,
    };
    let case = extract(&inputs).unwrap();
    let json = serde_json::to_string(&case).unwrap();
    assert!(
        !json.contains("sk_live_super_secret"),
        "task_signature must mask secrets"
    );
}

#[test]
fn lru_evicts_oldest_when_over_cap_via_persist() {
    let tmp = tempfile::tempdir().unwrap();

    for i in 0..MAX_CASE_RECORDS {
        let id = format!("case_{:0>24}", format!("{i:x}"));
        let case = CaseRecord {
            case_id: id.clone(),
            created_at: i as u64,
            repo_fingerprint: anvil::session::case_record::RepoFingerprint {
                workspace_key: "ws".into(),
                git_remote: None,
                git_head_branch: None,
                language_stack_hash: "0000000000000000".into(),
            },
            task_signature: "t".into(),
            language_stack: vec![],
            initial_feedback: vec![],
            successful_precautions: vec![],
            changed_files_summary: vec![],
            verify_commands: vec![],
            outcome_score: fake_score_success(),
        };
        persist(tmp.path(), &case).unwrap();
        // Sub-second mtime resolution: stagger so iter_case_files sort is
        // deterministic regardless of filesystem (APFS / ext4) behavior.
        std::thread::sleep(std::time::Duration::from_millis(5));
    }

    // Capture the actual oldest by mtime (the "victim" lazy_evict will pick).
    let entries_before = iter_case_files(tmp.path());
    assert_eq!(entries_before.len(), MAX_CASE_RECORDS);
    let oldest = entries_before[0].case_id.clone();

    // One more persist triggers eviction.
    let extra = CaseRecord {
        case_id: "case_fffffffffffffffffffffffe".into(),
        created_at: 9_999,
        repo_fingerprint: anvil::session::case_record::RepoFingerprint {
            workspace_key: "ws".into(),
            git_remote: None,
            git_head_branch: None,
            language_stack_hash: "0000000000000000".into(),
        },
        task_signature: "t".into(),
        language_stack: vec![],
        initial_feedback: vec![],
        successful_precautions: vec![],
        changed_files_summary: vec![],
        verify_commands: vec![],
        outcome_score: fake_score_success(),
    };
    persist(tmp.path(), &extra).unwrap();

    let entries = iter_case_files(tmp.path());
    assert_eq!(entries.len(), MAX_CASE_RECORDS);
    assert!(
        !entries.iter().any(|e| e.case_id == oldest),
        "oldest (by mtime) must be evicted: {oldest}"
    );
    assert!(
        entries
            .iter()
            .any(|e| e.case_id == "case_fffffffffffffffffffffffe"),
        "newly persisted case must be present"
    );
}

// ---------------------------------------------------------------------------
// Group (b): git fixture path
// ---------------------------------------------------------------------------

fn git_available() -> bool {
    Command::new("git")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn init_git_repo(work_root: &Path, remote_url: &str) {
    Command::new("git")
        .args(["init", "--quiet"])
        .current_dir(work_root)
        .status()
        .expect("git init");
    // Set local user/email so tests do not depend on global git config.
    Command::new("git")
        .args(["config", "user.email", "test@example.com"])
        .current_dir(work_root)
        .status()
        .expect("git config user.email");
    Command::new("git")
        .args(["config", "user.name", "test"])
        .current_dir(work_root)
        .status()
        .expect("git config user.name");
    Command::new("git")
        .args(["remote", "add", "origin", remote_url])
        .current_dir(work_root)
        .status()
        .expect("git remote add");
}

#[test]
fn capture_repo_fingerprint_returns_none_remote_outside_git_dir() {
    let tmp = tempfile::tempdir().unwrap();
    // Not a git repo.
    let fp = capture_repo_fingerprint("ws", tmp.path(), &["rust".into()]);
    assert_eq!(fp.workspace_key, "ws");
    assert_eq!(fp.git_remote, None);
    assert_eq!(fp.git_head_branch, None);
    assert!(!fp.language_stack_hash.is_empty());
}

#[test]
fn capture_repo_fingerprint_reads_git_remote_when_available() {
    if !git_available() {
        eprintln!(
            "git not available; skipping capture_repo_fingerprint_reads_git_remote_when_available"
        );
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    init_git_repo(tmp.path(), "https://github.com/example/repo.git");
    let fp = capture_repo_fingerprint("ws-y", tmp.path(), &["rust".into()]);
    assert_eq!(
        fp.git_remote.as_deref(),
        Some("https://github.com/example/repo.git")
    );
}

#[test]
fn capture_repo_fingerprint_redacts_credentials_in_remote() {
    if !git_available() {
        eprintln!(
            "git not available; skipping capture_repo_fingerprint_redacts_credentials_in_remote"
        );
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    init_git_repo(
        tmp.path(),
        "https://user:ghp_supersecrettoken@github.com/x/y.git",
    );
    let fp = capture_repo_fingerprint("ws-z", tmp.path(), &["rust".into()]);
    let remote = fp.git_remote.expect("captured a remote");
    assert!(
        !remote.contains("ghp_supersecrettoken"),
        "credentials must not survive in CaseRecord: got `{remote}`"
    );
    assert!(remote.contains("***@github.com"));
}

// ---------------------------------------------------------------------------
// Group (c): standalone helper checks for the public API surface
// ---------------------------------------------------------------------------

#[test]
fn validate_case_id_round_trip_with_derive() {
    let id = derive_case_id("ws", "sig", 1234);
    assert!(validate_case_id(&id));
}

#[test]
fn sanitize_git_remote_full_matrix() {
    assert_eq!(
        sanitize_git_remote("https://user:tok@github.com/x.git").unwrap(),
        "https://***@github.com/x.git"
    );
    assert_eq!(
        sanitize_git_remote("https://a@b:tok@github.com/x.git").unwrap(),
        "https://***@github.com/x.git"
    );
    assert_eq!(
        sanitize_git_remote("https://u:t@[::1]:8080/r.git").unwrap(),
        "https://***@[::1]:8080/r.git"
    );
    assert_eq!(
        sanitize_git_remote("git@github.com:owner/repo.git").unwrap(),
        "***@github.com:owner/repo.git"
    );
    assert_eq!(
        sanitize_git_remote("https://github.com/x.git").unwrap(),
        "https://github.com/x.git"
    );
    assert!(sanitize_git_remote("").is_none());
}

#[test]
fn hash_language_stack_is_stable_across_orderings() {
    let a = hash_language_stack(&["Rust".into(), "Node".into(), "rust".into()]);
    let b = hash_language_stack(&["node".into(), "RUST".into()]);
    assert_eq!(a, b);
    assert_eq!(a.len(), 16);
}

#[test]
fn precaution_snapshot_carries_severity_and_source() {
    let p = fake_precaution("k1", "txt", PrecautionStatus::Active);
    let snap: PrecautionSnapshot = (&p).into();
    assert_eq!(snap.text, "txt");
    assert_eq!(snap.severity, Severity::High);
    assert_eq!(snap.source, PrecautionSource::BuildFailure);
}
