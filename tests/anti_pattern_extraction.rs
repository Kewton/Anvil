//! Issue #464 — AntiPattern extraction + retrieval E2E tests.
//!
//! Two groups:
//!   (a) extract_or_increment upsert lifecycle (create → increment).
//!   (b) retrieve_with_iter scoring + threshold + cap behaviour.

use std::path::Path;

use anvil::session::anti_pattern::{
    AntiPatternFileEntry, AntiPatternRecord, AntiPatternRecordInputs, AntiPatternRetrievalInputs,
    AntiPatternStatus, ExtractOutcome, MAX_SELECTED_ANTI_PATTERNS, REPEAT_THRESHOLD,
    RetrievalOutcome, SkipReason, build_record_from_inputs, derive_anti_pattern_id,
    extract_or_increment, format_for_prompt, is_repeat_eligible_kind, iter_anti_pattern_files,
    retire_anti_pattern, retrieve_with_iter, validate_anti_pattern_id,
};
use anvil::session::case_record::RepoFingerprint;
use anvil::session::feedback::FeedbackKind;

fn fake_repo_fp(ws: &str) -> RepoFingerprint {
    RepoFingerprint {
        workspace_key: ws.to_string(),
        git_remote: None,
        git_head_branch: None,
        language_stack_hash: "h0".into(),
    }
}

fn make_inputs<'a>(
    work_root: &'a Path,
    ws: &'a str,
    task: Option<&'a str>,
    lang: &'a [String],
    touched: &'a [String],
    kind: FeedbackKind,
    summary: &'a str,
) -> AntiPatternRecordInputs<'a> {
    AntiPatternRecordInputs {
        workspace_key: ws,
        work_root,
        active_task: task,
        language_stack: lang,
        touched_files: touched,
        feedback_kind: kind,
        failed_action_summary: summary,
    }
}

// ---------------------------------------------------------------------------
// Group (a): extract_or_increment lifecycle
// ---------------------------------------------------------------------------

#[test]
fn first_failure_creates_with_repeat_count_one() {
    let tmp = tempfile::tempdir().unwrap();
    let lang: Vec<String> = vec!["rust".into()];
    let touched: Vec<String> = vec!["src/foo.rs".into()];
    let inputs = make_inputs(
        tmp.path(),
        "ws-A",
        Some("fix off-by-one"),
        &lang,
        &touched,
        FeedbackKind::EditFailure,
        "exact-match Edit failed for src/foo.rs",
    );
    match extract_or_increment(tmp.path(), &inputs).unwrap() {
        ExtractOutcome::Created(r) => {
            assert_eq!(r.repeat_count, 1);
            assert_eq!(r.feedback_kind, FeedbackKind::EditFailure);
            assert!(validate_anti_pattern_id(&r.anti_pattern_id));
        }
        other => panic!("expected Created, got {other:?}"),
    }
}

#[test]
fn second_same_failure_increments_repeat_count() {
    let tmp = tempfile::tempdir().unwrap();
    let lang: Vec<String> = vec!["rust".into()];
    let touched: Vec<String> = vec!["src/foo.rs".into()];
    let inputs = make_inputs(
        tmp.path(),
        "ws-A",
        Some("fix bug"),
        &lang,
        &touched,
        FeedbackKind::EditFailure,
        "edit failed",
    );
    extract_or_increment(tmp.path(), &inputs).unwrap();
    match extract_or_increment(tmp.path(), &inputs).unwrap() {
        ExtractOutcome::Incremented(r) => assert_eq!(r.repeat_count, 2),
        other => panic!("expected Incremented, got {other:?}"),
    }
}

#[test]
fn different_kind_creates_separate_record() {
    let tmp = tempfile::tempdir().unwrap();
    let lang: Vec<String> = vec!["rust".into()];
    let touched: Vec<String> = vec![];
    let edit_inputs = make_inputs(
        tmp.path(),
        "ws-A",
        Some("fix bug"),
        &lang,
        &touched,
        FeedbackKind::EditFailure,
        "x",
    );
    let compile_inputs = make_inputs(
        tmp.path(),
        "ws-A",
        Some("fix bug"),
        &lang,
        &touched,
        FeedbackKind::CompileError,
        "x",
    );
    extract_or_increment(tmp.path(), &edit_inputs).unwrap();
    extract_or_increment(tmp.path(), &compile_inputs).unwrap();
    let entries = iter_anti_pattern_files(tmp.path());
    assert_eq!(entries.len(), 2, "different kinds → distinct records");
}

#[test]
fn ineligible_kind_is_skipped() {
    let tmp = tempfile::tempdir().unwrap();
    let lang: Vec<String> = vec!["rust".into()];
    let touched: Vec<String> = vec![];
    let inputs = make_inputs(
        tmp.path(),
        "ws-A",
        Some("fix bug"),
        &lang,
        &touched,
        FeedbackKind::TestPass,
        "passed",
    );
    assert!(matches!(
        extract_or_increment(tmp.path(), &inputs).unwrap(),
        ExtractOutcome::SkippedIneligibleKind
    ));
    assert!(iter_anti_pattern_files(tmp.path()).is_empty());
}

#[test]
fn missing_active_task_is_skipped() {
    let tmp = tempfile::tempdir().unwrap();
    let lang: Vec<String> = vec!["rust".into()];
    let touched: Vec<String> = vec![];
    let inputs = make_inputs(
        tmp.path(),
        "ws-A",
        None,
        &lang,
        &touched,
        FeedbackKind::EditFailure,
        "x",
    );
    assert!(matches!(
        extract_or_increment(tmp.path(), &inputs).unwrap(),
        ExtractOutcome::SkippedNoActiveTask
    ));
}

#[test]
fn retire_then_repeat_reactivates() {
    let tmp = tempfile::tempdir().unwrap();
    let lang: Vec<String> = vec!["rust".into()];
    let touched: Vec<String> = vec![];
    let inputs = make_inputs(
        tmp.path(),
        "ws-A",
        Some("fix bug"),
        &lang,
        &touched,
        FeedbackKind::EditFailure,
        "edit failed",
    );
    let r = match extract_or_increment(tmp.path(), &inputs).unwrap() {
        ExtractOutcome::Created(r) => r,
        other => panic!("expected Created, got {other:?}"),
    };
    assert!(retire_anti_pattern(tmp.path(), &r.anti_pattern_id));

    // Next failure of the same shape should re-Activate.
    match extract_or_increment(tmp.path(), &inputs).unwrap() {
        ExtractOutcome::Incremented(updated) => {
            assert_eq!(updated.status, AntiPatternStatus::Active);
            assert_eq!(updated.repeat_count, 2);
        }
        other => panic!("expected Incremented re-active, got {other:?}"),
    }
}

#[test]
fn build_record_avoid_text_matches_kind() {
    let tmp = tempfile::tempdir().unwrap();
    let lang: Vec<String> = vec!["rust".into()];
    let touched: Vec<String> = vec![];
    let inputs = make_inputs(
        tmp.path(),
        "ws-A",
        Some("fix bug"),
        &lang,
        &touched,
        FeedbackKind::UnsafeCommandBlocked,
        "rm -rf /",
    );
    let r = build_record_from_inputs(&inputs).expect("Some");
    assert!(r.avoid_precaution.starts_with("Avoid the unsafe command:"));
}

#[test]
fn derive_id_separates_workspaces() {
    let a = derive_anti_pattern_id("ws-A", "fix bug", &FeedbackKind::EditFailure);
    let b = derive_anti_pattern_id("ws-B", "fix bug", &FeedbackKind::EditFailure);
    assert_ne!(a, b);
    assert!(validate_anti_pattern_id(&a));
}

#[test]
fn is_repeat_eligible_kind_covers_all_listed_in_issue() {
    // Issue #464 generation targets:
    // repeated edit failure / repeated bash failure / no-progress loop /
    // repeated unsafe command attempt / repeated malformed tool call.
    assert!(is_repeat_eligible_kind(&FeedbackKind::EditFailure));
    assert!(is_repeat_eligible_kind(&FeedbackKind::CompileError));
    assert!(is_repeat_eligible_kind(&FeedbackKind::TestFailure));
    assert!(is_repeat_eligible_kind(&FeedbackKind::NoRepoProgress));
    assert!(is_repeat_eligible_kind(&FeedbackKind::UnsafeCommandBlocked));
    assert!(is_repeat_eligible_kind(&FeedbackKind::ToolProtocolFailure));
    assert!(is_repeat_eligible_kind(&FeedbackKind::NoToolCall));
}

// ---------------------------------------------------------------------------
// Group (b): retrieve_with_iter
// ---------------------------------------------------------------------------

fn write_record(dir: &Path, r: &AntiPatternRecord) -> AntiPatternFileEntry {
    std::fs::create_dir_all(dir).unwrap();
    let path = dir.join(format!("{}.json", r.anti_pattern_id));
    std::fs::write(&path, serde_json::to_vec_pretty(r).unwrap()).unwrap();
    AntiPatternFileEntry {
        anti_pattern_id: r.anti_pattern_id.clone(),
        path,
        last_seen_at: r.last_seen_at,
        size: 0,
    }
}

fn record_at(id: &str, task: &str, kind: FeedbackKind, repeat: usize) -> AntiPatternRecord {
    AntiPatternRecord {
        anti_pattern_id: id.to_string(),
        created_at: 1000,
        last_seen_at: 1000,
        repo_fingerprint: fake_repo_fp("ws-A"),
        task_signature: task.to_string(),
        language_stack: vec!["rust".into()],
        failed_action_summary: "edit failed".into(),
        feedback_kind: kind,
        avoid_precaution: "Avoid retrying same edit".into(),
        repeat_count: repeat,
        touched_files_summary: vec!["foo.rs".into()],
        status: AntiPatternStatus::Active,
    }
}

#[test]
fn retrieve_returns_completed_when_repeat_threshold_met() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("anti_patterns");
    let r = record_at(
        "anti_aaaaaaaaaaaaaaaaaaaaaaaa",
        "fix the same bug now",
        FeedbackKind::EditFailure,
        REPEAT_THRESHOLD,
    );
    let entry = write_record(&dir, &r);
    let stack = vec!["rust".to_string()];
    let fp = fake_repo_fp("ws-A");
    let inputs = AntiPatternRetrievalInputs {
        current_task_signature: "fix the same bug now",
        current_language_stack: &stack,
        current_repo_fingerprint: &fp,
        current_touched_files: &[],
        current_feedback_kind: Some(FeedbackKind::EditFailure),
    };
    let out = retrieve_with_iter(&inputs, false, || Ok(vec![entry])).unwrap();
    match out {
        RetrievalOutcome::Completed { selected, .. } => {
            assert_eq!(selected.len(), 1);
            assert_eq!(
                selected[0].record.anti_pattern_id,
                "anti_aaaaaaaaaaaaaaaaaaaaaaaa"
            );
        }
        other => panic!("expected Completed, got {other:?}"),
    }
}

#[test]
fn retrieve_filters_below_repeat_threshold() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("anti_patterns");
    let r = record_at(
        "anti_aaaaaaaaaaaaaaaaaaaaaaaa",
        "fix the same bug now",
        FeedbackKind::EditFailure,
        REPEAT_THRESHOLD - 1,
    );
    let entry = write_record(&dir, &r);
    let stack = vec!["rust".to_string()];
    let fp = fake_repo_fp("ws-A");
    let inputs = AntiPatternRetrievalInputs {
        current_task_signature: "fix the same bug now",
        current_language_stack: &stack,
        current_repo_fingerprint: &fp,
        current_touched_files: &[],
        current_feedback_kind: Some(FeedbackKind::EditFailure),
    };
    let out = retrieve_with_iter(&inputs, false, || Ok(vec![entry])).unwrap();
    match out {
        RetrievalOutcome::Skipped { reason, .. } => {
            assert_eq!(reason, SkipReason::BelowRepeatThreshold);
        }
        other => panic!("expected BelowRepeatThreshold, got {other:?}"),
    }
}

#[test]
fn retrieve_caps_selected_at_max() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("anti_patterns");
    let mut entries = Vec::new();
    for i in 0..(MAX_SELECTED_ANTI_PATTERNS + 2) {
        let id = format!("anti_{:a<24}", i);
        let mut r = record_at(
            &id,
            "fix bug now",
            FeedbackKind::EditFailure,
            REPEAT_THRESHOLD,
        );
        r.last_seen_at = 1000 + i as u64;
        let mut e = write_record(&dir, &r);
        e.last_seen_at = r.last_seen_at;
        entries.push(e);
    }
    let stack = vec!["rust".to_string()];
    let fp = fake_repo_fp("ws-A");
    let inputs = AntiPatternRetrievalInputs {
        current_task_signature: "fix bug now",
        current_language_stack: &stack,
        current_repo_fingerprint: &fp,
        current_touched_files: &[],
        current_feedback_kind: Some(FeedbackKind::EditFailure),
    };
    let out = retrieve_with_iter(&inputs, false, || Ok(entries)).unwrap();
    match out {
        RetrievalOutcome::Completed { selected, .. } => {
            assert_eq!(selected.len(), MAX_SELECTED_ANTI_PATTERNS);
        }
        other => panic!("expected Completed, got {other:?}"),
    }
}

#[test]
fn format_for_prompt_section_starts_with_avoid_header_and_repeat_count() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("anti_patterns");
    let r = record_at(
        "anti_aaaaaaaaaaaaaaaaaaaaaaaa",
        "fix the same bug",
        FeedbackKind::EditFailure,
        3,
    );
    let entry = write_record(&dir, &r);
    let stack = vec!["rust".to_string()];
    let fp = fake_repo_fp("ws-A");
    let inputs = AntiPatternRetrievalInputs {
        current_task_signature: "fix the same bug",
        current_language_stack: &stack,
        current_repo_fingerprint: &fp,
        current_touched_files: &[],
        current_feedback_kind: Some(FeedbackKind::EditFailure),
    };
    let out = retrieve_with_iter(&inputs, false, || Ok(vec![entry])).unwrap();
    match out {
        RetrievalOutcome::Completed { selected, .. } => {
            let rendered = format_for_prompt(&selected).expect("section");
            assert!(rendered.starts_with("Avoid Patterns (from prior failures):\n"));
            assert!(rendered.contains("seen x3"));
        }
        _ => panic!("expected Completed"),
    }
}

#[test]
fn retrieve_dry_run_skips() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("anti_patterns");
    let r = record_at(
        "anti_aaaaaaaaaaaaaaaaaaaaaaaa",
        "fix bug now",
        FeedbackKind::EditFailure,
        REPEAT_THRESHOLD,
    );
    let entry = write_record(&dir, &r);
    let stack = vec!["rust".to_string()];
    let fp = fake_repo_fp("ws-A");
    let inputs = AntiPatternRetrievalInputs {
        current_task_signature: "fix bug now",
        current_language_stack: &stack,
        current_repo_fingerprint: &fp,
        current_touched_files: &[],
        current_feedback_kind: Some(FeedbackKind::EditFailure),
    };
    let out = retrieve_with_iter(&inputs, true, || Ok(vec![entry])).unwrap();
    assert!(matches!(
        out,
        RetrievalOutcome::Skipped {
            reason: SkipReason::DryRun,
            ..
        }
    ));
}

#[test]
fn retire_then_retrieve_drops_record() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("anti_patterns");
    let mut r = record_at(
        "anti_aaaaaaaaaaaaaaaaaaaaaaaa",
        "fix bug now",
        FeedbackKind::EditFailure,
        REPEAT_THRESHOLD,
    );
    r.status = AntiPatternStatus::Retired;
    let entry = write_record(&dir, &r);
    let stack = vec!["rust".to_string()];
    let fp = fake_repo_fp("ws-A");
    let inputs = AntiPatternRetrievalInputs {
        current_task_signature: "fix bug now",
        current_language_stack: &stack,
        current_repo_fingerprint: &fp,
        current_touched_files: &[],
        current_feedback_kind: Some(FeedbackKind::EditFailure),
    };
    let out = retrieve_with_iter(&inputs, false, || Ok(vec![entry])).unwrap();
    // Retired-only candidates are filtered → BelowRepeatThreshold.
    match out {
        RetrievalOutcome::Skipped { .. } => {}
        other => panic!("expected Skipped for retired record, got {other:?}"),
    }
}
