//! Issue #463 — Case retrieval smoke tests.
//!
//! Exercises the session-layer public API end-to-end: `persist` real
//! `CaseRecord`s under a tempdir `state_root`, then call
//! `retrieve_relevant_cases` (the on-disk wrapper) and `retrieve_with_iter`
//! (the closure-DI seam used by R6 fixture). Adapter-level acceptance
//! conditions (R5/R7/R8/R9) that require a full `Agent` are exercised by
//! the unit tests inside `src/session/case_retrieval.rs` plus the agent
//! layer's compile-time integration via `try_inject_case_retrieval_message`.

use std::path::Path;

use anvil::session::anvil_score::AnvilScore;
use anvil::session::case_record::{CaseFileEntry, CaseRecord, RepoFingerprint, persist};
use anvil::session::case_retrieval::{
    CaseRetrievalInputs, MAX_CASE_RENDERED_CHARS_TOTAL, MAX_SELECTED_CASES, RetrievalOutcome,
    SkipReason, format_for_prompt, retrieve_relevant_cases, retrieve_with_iter,
};
use anvil::session::feedback::FeedbackKind;

fn fake_score() -> AnvilScore {
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

fn fake_record(case_id: &str, task: &str, ws: &str, stack_hash: &str) -> CaseRecord {
    CaseRecord {
        case_id: case_id.to_string(),
        created_at: 1000,
        repo_fingerprint: RepoFingerprint {
            workspace_key: ws.to_string(),
            git_remote: None,
            git_head_branch: None,
            language_stack_hash: stack_hash.to_string(),
        },
        task_signature: task.to_string(),
        language_stack: vec!["rust".to_string()],
        initial_feedback: vec![FeedbackKind::CompileError],
        successful_precautions: vec![],
        changed_files_summary: vec!["src/foo.rs (impl)".to_string()],
        verify_commands: vec!["cargo test".to_string()],
        outcome_score: fake_score(),
    }
}

fn matched_inputs<'a>(
    task: &'a str,
    stack: &'a [String],
    fp: &'a RepoFingerprint,
) -> CaseRetrievalInputs<'a> {
    CaseRetrievalInputs {
        current_task_signature: task,
        current_language_stack: stack,
        current_repo_fingerprint: fp,
        current_touched_files: &[],
        current_feedback_kind: Some(FeedbackKind::CompileError),
        current_active_precautions: &[],
    }
}

fn fp_match() -> RepoFingerprint {
    RepoFingerprint {
        workspace_key: "ws-A".to_string(),
        git_remote: None,
        git_head_branch: None,
        language_stack_hash: "h".to_string(),
    }
}

// --- R1 -----------------------------------------------------------------

#[test]
fn relevant_case_is_injected_when_above_threshold() {
    let tmp = tempfile::tempdir().unwrap();
    let rec = fake_record("case_aaaaaaaaaaaaaaaaaaaa", "fix offset bug", "ws-A", "h");
    persist(tmp.path(), &rec).unwrap();

    let stack = vec!["rust".to_string()];
    let fp = fp_match();
    let inputs = matched_inputs("fix offset bug", &stack, &fp);
    let out = retrieve_relevant_cases(tmp.path(), &inputs, false).unwrap();
    match out {
        RetrievalOutcome::Completed {
            selected,
            candidate_count,
            ..
        } => {
            assert_eq!(candidate_count, 1);
            assert_eq!(selected.len(), 1);
            assert!(selected[0].breakdown.total >= 0.40);
            let section = format_for_prompt(&selected).unwrap();
            assert!(section.starts_with("Relevant Local Cases:\n"));
            assert!(section.contains("fix offset bug"));
        }
        other => panic!("expected Completed, got {other:?}"),
    }
}

// --- R2 -----------------------------------------------------------------

#[test]
fn unrelated_case_is_not_injected() {
    let tmp = tempfile::tempdir().unwrap();
    // Same workspace but completely different task signature, different stack hash.
    let rec = fake_record(
        "case_bbbbbbbbbbbbbbbbbbbb",
        "completely unrelated task xyz",
        "ws-DIFFERENT",
        "h-other",
    );
    persist(tmp.path(), &rec).unwrap();

    let stack = vec!["rust".to_string()];
    let fp = fp_match();
    let inputs = CaseRetrievalInputs {
        current_task_signature: "alpha bravo charlie",
        current_language_stack: &stack,
        current_repo_fingerprint: &fp,
        current_touched_files: &[],
        current_feedback_kind: None,
        current_active_precautions: &[],
    };
    let out = retrieve_relevant_cases(tmp.path(), &inputs, false).unwrap();
    match out {
        RetrievalOutcome::Skipped { reason, .. } => {
            assert_eq!(reason, SkipReason::BelowThreshold);
        }
        other => panic!("expected Skipped(BelowThreshold), got {other:?}"),
    }
}

// --- R3 -----------------------------------------------------------------

#[test]
fn selected_count_is_capped_at_max() {
    let tmp = tempfile::tempdir().unwrap();
    for i in 0..(MAX_SELECTED_CASES + 2) {
        // case_id pattern: case_<20 hex-ish chars>
        let id = format!("case_{:a<20}", i);
        let rec = fake_record(&id, "fix offset bug", "ws-A", "h");
        persist(tmp.path(), &rec).unwrap();
    }
    let stack = vec!["rust".to_string()];
    let fp = fp_match();
    let inputs = matched_inputs("fix offset bug", &stack, &fp);
    let out = retrieve_relevant_cases(tmp.path(), &inputs, false).unwrap();
    match out {
        RetrievalOutcome::Completed { selected, .. } => {
            assert_eq!(selected.len(), MAX_SELECTED_CASES);
        }
        other => panic!("expected Completed, got {other:?}"),
    }
}

// --- R4 -----------------------------------------------------------------

#[test]
fn total_render_chars_does_not_exceed_cap() {
    let tmp = tempfile::tempdir().unwrap();
    for i in 0..MAX_SELECTED_CASES {
        let id = format!("case_{:a<20}", i);
        let mut rec = fake_record(&id, "fix offset bug", "ws-A", "h");
        // Stretch task_signature to push render against the cap.
        rec.task_signature = "x".repeat(300);
        persist(tmp.path(), &rec).unwrap();
    }
    let stack = vec!["rust".to_string()];
    let fp = fp_match();
    let inputs = CaseRetrievalInputs {
        current_task_signature: "x x x x x x",
        current_language_stack: &stack,
        current_repo_fingerprint: &fp,
        current_touched_files: &[],
        current_feedback_kind: Some(FeedbackKind::CompileError),
        current_active_precautions: &[],
    };
    let out = retrieve_relevant_cases(tmp.path(), &inputs, false).unwrap();
    if let RetrievalOutcome::Completed { selected, .. } = out {
        let section = format_for_prompt(&selected).unwrap();
        assert!(
            section.chars().count() <= MAX_CASE_RENDERED_CHARS_TOTAL,
            "section length {} exceeds cap {}",
            section.chars().count(),
            MAX_CASE_RENDERED_CHARS_TOTAL
        );
    }
}

// --- R5 -----------------------------------------------------------------

#[test]
fn completed_payload_contains_top_score_and_reasons() {
    // Surface what the adapter's `agent.case_retrieval.completed` payload
    // assembles: completed.selected[*].breakdown is what gets serialised as
    // `selected_reasons`. Verify the breakdown contains all score dimensions
    // plus a total in [0, 1].
    let tmp = tempfile::tempdir().unwrap();
    let rec = fake_record("case_aaaaaaaaaaaaaaaaaaaa", "fix offset bug", "ws-A", "h");
    persist(tmp.path(), &rec).unwrap();
    let stack = vec!["rust".to_string()];
    let fp = fp_match();
    let inputs = matched_inputs("fix offset bug", &stack, &fp);
    let out = retrieve_relevant_cases(tmp.path(), &inputs, false).unwrap();
    if let RetrievalOutcome::Completed {
        selected,
        compute_ms,
        skipped_corrupt_count,
        ..
    } = out
    {
        let b = &selected[0].breakdown;
        assert!(b.total >= 0.0 && b.total <= 1.0);
        // serde_json round-trip mirrors the adapter's payload assembly.
        let v = serde_json::to_value(&selected[0].breakdown).unwrap();
        for k in [
            "case_id",
            "task",
            "semantic",
            "stack",
            "repo",
            "files",
            "kind",
            "precautions",
            "total",
        ] {
            assert!(v.get(k).is_some(), "selected_reasons missing key {k}");
        }
        assert!(compute_ms >= 0.0);
        assert_eq!(skipped_corrupt_count, 0);
    } else {
        panic!("expected Completed");
    }
}

// --- R6 -----------------------------------------------------------------

#[test]
fn iter_failure_emits_failed_event_and_continues() {
    let stack = vec!["rust".to_string()];
    let fp = fp_match();
    let inputs = matched_inputs("fix offset bug", &stack, &fp);
    let out = retrieve_with_iter(&inputs, false, || Err("injected i/o failure".to_string()));
    assert!(out.is_err(), "iter_failure must surface as Err");
    assert!(
        out.unwrap_err().contains("injected i/o failure"),
        "error message must propagate"
    );
    // The adapter logs `agent.case_retrieval.failed` and returns None;
    // confirm that retrieve_with_iter itself does not panic so the actor
    // loop can continue.
}

// --- R10 ----------------------------------------------------------------

#[test]
fn dry_run_skips_injection_but_logs() {
    let tmp = tempfile::tempdir().unwrap();
    let rec = fake_record("case_aaaaaaaaaaaaaaaaaaaa", "fix offset bug", "ws-A", "h");
    persist(tmp.path(), &rec).unwrap();
    let stack = vec!["rust".to_string()];
    let fp = fp_match();
    let inputs = matched_inputs("fix offset bug", &stack, &fp);
    let out = retrieve_relevant_cases(tmp.path(), &inputs, /*dry_run=*/ true).unwrap();
    match out {
        RetrievalOutcome::Skipped { reason, .. } => {
            assert_eq!(reason, SkipReason::DryRun);
        }
        other => panic!("expected Skipped(DryRun), got {other:?}"),
    }
}

// --- R11 ----------------------------------------------------------------

#[test]
fn corrupt_file_is_skipped_per_file() {
    let tmp = tempfile::tempdir().unwrap();
    // Persist a valid record.
    let rec = fake_record("case_aaaaaaaaaaaaaaaaaaaa", "fix offset bug", "ws-A", "h");
    persist(tmp.path(), &rec).unwrap();
    // Drop a corrupt sibling. case_id pattern must match the allowlist so
    // iter_case_files picks it up.
    let bad_path = tmp
        .path()
        .join("cases")
        .join("case_bbbbbbbbbbbbbbbbbbbb.json");
    std::fs::write(&bad_path, b"{ not json").unwrap();

    let stack = vec!["rust".to_string()];
    let fp = fp_match();
    let inputs = matched_inputs("fix offset bug", &stack, &fp);
    let out = retrieve_relevant_cases(tmp.path(), &inputs, false).unwrap();
    match out {
        RetrievalOutcome::Completed {
            skipped_corrupt_count,
            selected,
            ..
        } => {
            assert!(
                skipped_corrupt_count >= 1,
                "skipped_corrupt_count should account for the bad file"
            );
            assert_eq!(selected.len(), 1);
        }
        other => panic!("expected Completed with 1 corrupt, got {other:?}"),
    }
}

// --- DR2-006 補助 -------------------------------------------------------

#[test]
fn renderer_masks_unmasked_secrets_in_record() {
    let tmp = tempfile::tempdir().unwrap();
    let mut rec = fake_record("case_aaaaaaaaaaaaaaaaaaaa", "fix bug", "ws-A", "h");
    // Inject something that mask_secrets would catch (AWS access key shape).
    rec.task_signature = "fix bug AKIAIOSFODNN7EXAMPLE leaked".to_string();
    persist(tmp.path(), &rec).unwrap();
    let stack = vec!["rust".to_string()];
    let fp = fp_match();
    let inputs = matched_inputs("fix bug", &stack, &fp);
    let out = retrieve_relevant_cases(tmp.path(), &inputs, false).unwrap();
    if let RetrievalOutcome::Completed { selected, .. } = out {
        let section = format_for_prompt(&selected).unwrap();
        assert!(
            !section.contains("AKIAIOSFODNN7EXAMPLE"),
            "renderer must apply mask_secrets; got: {section}"
        );
    } else {
        panic!("expected Completed");
    }
}

// --- bridge: agent-layer adapter signature compile-check ---------------

#[test]
fn case_file_entry_smoke_compile_check() {
    // Compile-time: `CaseFileEntry` must remain accessible from this file
    // (matches signature used by `retrieve_with_iter`'s iter_provider).
    let _ = |_p: &Path| -> Vec<CaseFileEntry> { Vec::new() };
    let _ = std::any::type_name::<CaseFileEntry>();
}
