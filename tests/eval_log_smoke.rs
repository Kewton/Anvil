//! E2E smoke tests for the structured evaluation log (Issue #471).
//!
//! Ollama-free: exercises `build_eval_record` and `write_eval_record` directly
//! without any LLM calls.

use anvil::session::case_retrieval::CaseScoreBreakdown;
use anvil::session::eval_log::{
    AnvilScoreSummary, CaseRetrievalSummary, ChangedFileClasses, EvalPrecautionSnapshot,
    EvalRecord, EvaluationTaxonomySummary, FeedbackFrameSummary, MAX_EVAL_LOG_RECORD_BYTES,
    MAX_EVAL_PRECAUTIONS, MAX_EVAL_TASK_BYTES, PamEvalSummary, ToolCallSummary, build_eval_record,
    build_terminal_diagnostics, scrub_absolute_paths, write_eval_record_to,
};
use serde_json::Value;
use std::fs::OpenOptions;
use std::io::{BufRead, BufReader};
use std::sync::Mutex;
use tempfile::NamedTempFile;

fn make_tool_calls() -> Vec<ToolCallSummary> {
    vec![
        ToolCallSummary {
            name: "Bash".to_string(),
            args_summary: r#"{"command":"cargo build"}"#.to_string(),
        },
        ToolCallSummary {
            name: "Read".to_string(),
            args_summary: r#"{"file_path":"src/main.rs"}"#.to_string(),
        },
    ]
}

fn make_classes() -> ChangedFileClasses {
    ChangedFileClasses {
        test: 1,
        impl_files: 2,
        setup: 0,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// R1: build_eval_record round-trips all fields
// ─────────────────────────────────────────────────────────────────────────────
#[test]
fn r1_build_eval_record_round_trip() {
    let tool_calls = make_tool_calls();
    let rec = build_eval_record(
        "sess-r1",
        1_700_000_000_000,
        "fix the compilation error",
        "qwen3:14b",
        "Act",
        "native",
        &tool_calls,
        Some(FeedbackFrameSummary {
            kind: "CompileError".to_string(),
            excerpt: "error[E0308]: mismatched types".to_string(),
        }),
        &[EvalPrecautionSnapshot {
            id: "p001".to_string(),
            source: "build_failure".to_string(),
            severity: "high".to_string(),
            status: "active".to_string(),
        }],
        Some(AnvilScoreSummary {
            build_passed: Some(true),
            tests_passed: None,
            compile_errors_delta: Some(-1),
            test_failures_delta: None,
            compile_error_count: Some(0),
            test_failure_count: None,
            implementation_files_changed: Some(1),
            test_files_changed: None,
            setup_files_changed: None,
            unsafe_actions_blocked: 0,
            consecutive_no_progress_turns: 0,
            user_visible_artifact: true,
        }),
        make_classes(),
        &["cargo build".to_string()],
        None,
        None,
        None,
        "done",
    );
    assert_eq!(rec.schema_version, 1);
    assert_eq!(rec.session_id, "sess-r1");
    assert_eq!(rec.ts_ms, 1_700_000_000_000);
    assert_eq!(rec.task, "fix the compilation error");
    assert_eq!(rec.model, "qwen3:14b");
    assert_eq!(rec.mode, "Act");
    assert_eq!(rec.tool_protocol, "native");
    assert_eq!(rec.tool_calls.len(), 2);
    assert_eq!(rec.tool_calls[0].name, "Bash");
    assert!(rec.feedback_frame.is_some());
    assert_eq!(rec.active_precautions.len(), 1);
    assert!(rec.anvil_score.is_some());
    assert_eq!(rec.changed_file_classes.test, 1);
    assert_eq!(rec.verify_commands, vec!["cargo build"]);
    assert!(rec.case_retrieval_result.is_none());
    assert_eq!(rec.completion_reason, "verifier_evidence_satisfied");
    assert_eq!(rec.final_outcome, "done");
}

// ─────────────────────────────────────────────────────────────────────────────
// R1b: PAM eval summary is additive and advisory-only
// ─────────────────────────────────────────────────────────────────────────────
#[test]
fn r1b_pam_eval_summary_serializes_advisory_impact() {
    let mut rec = build_eval_record(
        "sess-r1b",
        1_700_000_000_001,
        "fix the compilation error",
        "qwen3:14b",
        "Act",
        "native",
        &[],
        None,
        &[],
        None,
        make_classes(),
        &[],
        None,
        None,
        None,
        "done",
    );
    rec.pam_eval = Some(PamEvalSummary {
        mode: "live".to_string(),
        decision_type: "prompt_context_injection".to_string(),
        decision_types: vec!["prompt_context_injection".to_string()],
        affected_targets: vec![anvil::session::eval_log::PamEvalTarget {
            target_type: "prompt_context".to_string(),
            target: "context_pack_prompt".to_string(),
            decision_type: "prompt_context_injection".to_string(),
            summary_id: Some("seed-a".to_string()),
        }],
        actual_injected_count: 2,
        suppressed_count: 1,
        would_inject_in_live_count: 0,
        advisory_only: true,
        completion_judgement_override: false,
        unused_reason: None,
    });

    let json = serde_json::to_value(&rec).unwrap();
    assert_eq!(
        json["pam_eval"]["decision_type"],
        "prompt_context_injection"
    );
    assert_eq!(json["pam_eval"]["actual_injected_count"], 2);
    assert_eq!(json["pam_eval"]["advisory_only"], true);
    assert_eq!(json["pam_eval"]["completion_judgement_override"], false);
    assert_eq!(
        json["pam_eval"]["affected_targets"][0]["target_type"],
        "prompt_context"
    );
    assert!(json["pam_eval"].get("unused_reason").is_none());
}

#[test]
fn r1c_pam_eval_summary_serializes_unused_reason() {
    let mut rec = build_eval_record(
        "sess-r1c",
        1_700_000_000_002,
        "fix the compilation error",
        "qwen3:14b",
        "Act",
        "native",
        &[],
        None,
        &[],
        None,
        make_classes(),
        &[],
        None,
        None,
        None,
        "safe_stop",
    );
    rec.pam_eval = Some(PamEvalSummary::skipped("canary_gate"));

    let json = serde_json::to_value(&rec).unwrap();
    assert_eq!(json["pam_eval"]["decision_type"], "not_used");
    assert_eq!(json["pam_eval"]["unused_reason"], "canary_gate");
    assert_eq!(json["pam_eval"]["completion_judgement_override"], false);
}

// ─────────────────────────────────────────────────────────────────────────────
// R2: task secret is masked
// ─────────────────────────────────────────────────────────────────────────────
#[test]
fn r2_task_secret_is_masked() {
    let rec = build_eval_record(
        "sess-r2",
        0,
        "authenticate with api_key=sk-proj-secret999 please",
        "model",
        "Act",
        "native",
        &[],
        None,
        &[],
        None,
        ChangedFileClasses {
            test: 0,
            impl_files: 0,
            setup: 0,
        },
        &[],
        None,
        None,
        None,
        "done",
    );
    assert!(
        !rec.task.contains("sk-proj-secret999"),
        "task leaked: {}",
        rec.task
    );
    assert!(
        rec.task.contains("***"),
        "mask marker missing: {}",
        rec.task
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// R3: long task is truncated within byte cap
// ─────────────────────────────────────────────────────────────────────────────
#[test]
fn r3_task_truncated_within_cap() {
    let long_task = "a".repeat(MAX_EVAL_TASK_BYTES + 500);
    let rec = build_eval_record(
        "sess-r3",
        0,
        &long_task,
        "model",
        "Act",
        "native",
        &[],
        None,
        &[],
        None,
        ChangedFileClasses {
            test: 0,
            impl_files: 0,
            setup: 0,
        },
        &[],
        None,
        None,
        None,
        "done",
    );
    // truncated string ≤ MAX + truncation marker overhead
    assert!(rec.task.len() <= MAX_EVAL_TASK_BYTES + 10);
}

// ─────────────────────────────────────────────────────────────────────────────
// R4: active_precautions capped at MAX_EVAL_PRECAUTIONS
// ─────────────────────────────────────────────────────────────────────────────
#[test]
fn r4_precautions_capped() {
    let precs: Vec<EvalPrecautionSnapshot> = (0..20)
        .map(|i| EvalPrecautionSnapshot {
            id: format!("p{i:02}"),
            source: "manual".to_string(),
            severity: "medium".to_string(),
            status: "active".to_string(),
        })
        .collect();
    let rec = build_eval_record(
        "sess-r4",
        0,
        "task",
        "model",
        "Act",
        "native",
        &[],
        None,
        &precs,
        None,
        ChangedFileClasses {
            test: 0,
            impl_files: 0,
            setup: 0,
        },
        &[],
        None,
        None,
        None,
        "done",
    );
    assert!(
        rec.active_precautions.len() <= MAX_EVAL_PRECAUTIONS,
        "got {} precautions",
        rec.active_precautions.len()
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// R5: write_eval_record_to writes valid JSONL and record is parseable
// ─────────────────────────────────────────────────────────────────────────────
#[test]
fn r5_write_eval_record_valid_jsonl() {
    let tmpfile = NamedTempFile::new().expect("tempfile");
    let path = tmpfile.path().to_path_buf();
    let file = Mutex::new(
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .expect("open tmpfile"),
    );

    let rec = build_eval_record(
        "sess-r5",
        9999,
        "write smoke test",
        "qwen3:14b",
        "Act",
        "xml",
        &make_tool_calls(),
        None,
        &[],
        None,
        make_classes(),
        &[],
        None,
        None,
        None,
        "done",
    );
    write_eval_record_to(&rec, &file);

    // read back and parse
    let f = std::fs::File::open(&path).expect("open tmpfile");
    let mut lines = BufReader::new(f).lines();
    let line = lines.next().expect("at least one line").expect("utf8");
    let parsed: Value = serde_json::from_str(&line).expect("valid JSON line");
    assert_eq!(parsed["schema_version"], 1);
    assert_eq!(parsed["session_id"], "sess-r5");
    assert_eq!(parsed["final_outcome"], "done");
    assert_eq!(
        parsed["terminal_diagnostics"]["classification"], "success",
        "eval log should carry issue-848 terminal diagnostics"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// R6: build_eval_record masks secrets in tool args_summary via mask_secrets
// ─────────────────────────────────────────────────────────────────────────────
#[test]
fn r6_build_record_masks_secret_in_verify_command() {
    // verify_commands is processed with mask_secrets in build_eval_record
    let rec = build_eval_record(
        "sess-r6",
        0,
        "task",
        "model",
        "Act",
        "native",
        &[],
        None,
        &[],
        None,
        ChangedFileClasses {
            test: 0,
            impl_files: 0,
            setup: 0,
        },
        &["api_key=supersecret-hunter2 cargo build".to_string()],
        None,
        None,
        None,
        "done",
    );
    // build_eval_record applies mask_secrets to verify_commands
    let cmd = &rec.verify_commands[0];
    assert!(
        !cmd.contains("supersecret-hunter2"),
        "secret leaked in verify_cmd: {cmd}"
    );
    assert!(cmd.contains("***"), "mask marker missing: {cmd}");
}

// ─────────────────────────────────────────────────────────────────────────────
// R7: scrub_absolute_paths removes unix absolute paths
// ─────────────────────────────────────────────────────────────────────────────
#[test]
fn r7_scrub_replaces_unix_absolute_paths() {
    let json = r#"{"task":"edit /home/alice/projects/src/main.rs","ok":true}"#;
    let scrubbed = scrub_absolute_paths(json);
    assert!(!scrubbed.contains("/home/alice"), "scrubbed: {scrubbed}");
    assert!(scrubbed.contains("<path>"), "scrubbed: {scrubbed}");
    serde_json::from_str::<Value>(&scrubbed).expect("scrubbed is valid JSON");
}

// ─────────────────────────────────────────────────────────────────────────────
// R8: scrub_absolute_paths leaves relative paths intact
// ─────────────────────────────────────────────────────────────────────────────
#[test]
fn r8_scrub_preserves_relative_paths() {
    let json = r#"{"file":"src/session/eval_log.rs","count":3}"#;
    let scrubbed = scrub_absolute_paths(json);
    assert_eq!(scrubbed, json, "relative path was wrongly scrubbed");
}

// ─────────────────────────────────────────────────────────────────────────────
// R9: oversized record is dropped (not written)
// ─────────────────────────────────────────────────────────────────────────────
#[test]
fn r9_oversized_record_is_dropped() {
    let tmpfile = NamedTempFile::new().expect("tempfile");
    let path = tmpfile.path().to_path_buf();
    let file = Mutex::new(
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .expect("open tmpfile"),
    );

    // Build a record whose task is huge (>> MAX_EVAL_LOG_RECORD_BYTES)
    // Note: build_eval_record truncates task to MAX_EVAL_TASK_BYTES, so
    // the record itself won't be huge from task alone. We instead build a
    // record with many large verify commands.
    let giant_cmds: Vec<String> = (0..2000)
        .map(|i| "x".repeat(200) + &format!(" cmd{i}"))
        .collect();
    let rec = EvalRecord {
        schema_version: 1,
        session_id: "sess-r9".to_string(),
        ts_ms: 0,
        task: "task".to_string(),
        model: "m".to_string(),
        mode: "Act".to_string(),
        tool_protocol: "native".to_string(),
        tool_calls: vec![],
        feedback_frame: None,
        active_precautions: vec![],
        anvil_score: None,
        changed_file_classes: ChangedFileClasses {
            test: 0,
            impl_files: 0,
            setup: 0,
        },
        verify_commands: giant_cmds,
        case_retrieval_result: None,
        photon_eval: None,
        pam_eval: None,
        photon_canary: 0,
        auto_promote: None,
        terminal_diagnostics: Some(build_terminal_diagnostics(
            "done",
            &ChangedFileClasses {
                test: 0,
                impl_files: 0,
                setup: 0,
            },
            0,
        )),
        evaluation_taxonomy: EvaluationTaxonomySummary {
            pam_variant: "unknown".to_string(),
            task_kind: "coding".to_string(),
            anvil_terminal_class: "success".to_string(),
            outcome_agreement: "external_postcheck_unavailable".to_string(),
            failure_authority: "success".to_string(),
        },
        completion_reason: "answer_or_plan_completion".to_string(),
        final_outcome: "done".to_string(),
    };

    // Confirm the record would exceed the byte cap
    let json = serde_json::to_string(&rec).unwrap();
    if json.len() <= MAX_EVAL_LOG_RECORD_BYTES {
        // If somehow it fits, skip drop assertion (different platform/encoding)
        return;
    }

    write_eval_record_to(&rec, &file);

    // The file should be empty (record dropped)
    let metadata = std::fs::metadata(&path).unwrap();
    assert_eq!(metadata.len(), 0, "oversized record was not dropped");
}

// ─────────────────────────────────────────────────────────────────────────────
// R10: EvalRecord serializes impl field with correct name
// ─────────────────────────────────────────────────────────────────────────────
#[test]
fn r10_changed_file_classes_impl_field_name() {
    let classes = ChangedFileClasses {
        test: 3,
        impl_files: 5,
        setup: 1,
    };
    let json = serde_json::to_value(&classes).unwrap();
    assert_eq!(json["impl"], 5, "impl_files should serialize as 'impl'");
    assert_eq!(json["test"], 3);
    assert_eq!(json["setup"], 1);
}

// ─────────────────────────────────────────────────────────────────────────────
// R11: case_retrieval_result is serialized in the record
// ─────────────────────────────────────────────────────────────────────────────
#[test]
fn r11_case_retrieval_result_serialized() {
    let summary = CaseRetrievalSummary {
        selected: 2,
        scores: vec![CaseScoreBreakdown {
            case_id: "case_abc123".to_string(),
            task: 0.6,
            semantic: 0.0,
            stack: 0.1,
            repo: 0.1,
            files: 0.1,
            kind: 0.05,
            precautions: 0.05,
            total: 0.6,
        }],
    };
    let rec = build_eval_record(
        "sess-r11",
        0,
        "task",
        "model",
        "Act",
        "native",
        &[],
        None,
        &[],
        None,
        ChangedFileClasses {
            test: 0,
            impl_files: 0,
            setup: 0,
        },
        &[],
        Some(summary),
        None,
        None,
        "done",
    );
    assert!(rec.case_retrieval_result.is_some());
    let json = serde_json::to_value(&rec).unwrap();
    assert_eq!(json["case_retrieval_result"]["selected"], 2);
    assert_eq!(
        json["case_retrieval_result"]["scores"][0]["case_id"],
        "case_abc123"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// R12: anvil_score summary faithfully maps all 12 fields
// ─────────────────────────────────────────────────────────────────────────────
#[test]
fn r12_anvil_score_summary_all_fields() {
    use anvil::session::anvil_score::AnvilScore;
    let score = AnvilScore {
        build_passed: Some(false),
        tests_passed: Some(true),
        compile_errors_delta: Some(2),
        test_failures_delta: Some(-3),
        compile_error_count: Some(5),
        test_failure_count: Some(0),
        implementation_files_changed: Some(2),
        test_files_changed: Some(1),
        setup_files_changed: Some(0),
        unsafe_actions_blocked: 1,
        consecutive_no_progress_turns: 3,
        user_visible_artifact: false,
    };
    let summary = AnvilScoreSummary::from(&score);
    assert_eq!(summary.build_passed, Some(false));
    assert_eq!(summary.tests_passed, Some(true));
    assert_eq!(summary.compile_errors_delta, Some(2));
    assert_eq!(summary.test_failures_delta, Some(-3));
    assert_eq!(summary.compile_error_count, Some(5));
    assert_eq!(summary.test_failure_count, Some(0));
    assert_eq!(summary.implementation_files_changed, Some(2));
    assert_eq!(summary.test_files_changed, Some(1));
    assert_eq!(summary.setup_files_changed, Some(0));
    assert_eq!(summary.unsafe_actions_blocked, 1);
    assert_eq!(summary.consecutive_no_progress_turns, 3);
    assert!(!summary.user_visible_artifact);
}
