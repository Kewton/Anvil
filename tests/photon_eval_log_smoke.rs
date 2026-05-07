//! Issue #558: Photon eval log integration smoke tests.
//!
//! Ollama-free: tests `parse_evaluate_response` → `build_eval_record` →
//! `write_eval_record_to` pipeline entirely without LLM calls.
//!
//! Test matrix (T1-T6 per work plan):
//!   T1  normal_admitted    admitted=true  → admission_decision="accepted"
//!   T2  normal_rejected    admitted=false → admission_decision="rejected"
//!   T3  shadow_mode_cpid   context_pack_id fallback used when evaluate response is None
//!   T4  fail_open          EvaluateResponse=None → photon_eval=null in EvalRecord
//!   T5  warnings_masked    secrets in warnings are masked by mask_secrets
//!   T6  cpid_fallback      EvaluateResponse lacks context_pack_id → last_context_pack_id used

use anvil::photon::schema::EvaluateResponse;
use anvil::session::eval_log::{
    ChangedFileClasses, EvalRecord, PhotonEvalSummary, ToolCallSummary, build_eval_record,
    write_eval_record_to,
};
use serde_json::Value;
use std::fs::OpenOptions;
use std::io::{BufRead, BufReader};
use std::sync::Mutex;
use tempfile::NamedTempFile;

fn make_resp(json: serde_json::Value) -> EvaluateResponse {
    EvaluateResponse(json)
}

fn empty_classes() -> ChangedFileClasses {
    ChangedFileClasses {
        test: 0,
        impl_files: 0,
        setup: 0,
    }
}

fn write_and_parse(rec: &EvalRecord) -> Value {
    let tmpfile = NamedTempFile::new().unwrap();
    let file = Mutex::new(
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(tmpfile.path())
            .unwrap(),
    );
    write_eval_record_to(rec, &file);
    let f = std::fs::File::open(tmpfile.path()).unwrap();
    let line = BufReader::new(f).lines().next().unwrap().unwrap();
    serde_json::from_str(&line).unwrap()
}

// T1 ─────────────────────────────────────────────────────────────────────────

/// admitted=true → admission_decision="accepted", prompt_adopted=true in EvalRecord.
#[test]
fn t1_normal_admitted_recorded_in_eval_log() {
    use anvil::photon::eval::parse_evaluate_response;

    let resp = make_resp(serde_json::json!({
        "admitted": true,
        "photon_request_id": "req-001",
        "context_pack_id": "cpid-abc",
        "task_outcome": "success",
    }));
    let summary = parse_evaluate_response(&resp);

    assert_eq!(summary.admission_decision.as_deref(), Some("accepted"));
    assert_eq!(summary.prompt_adopted, Some(true));
    assert_eq!(summary.photon_request_id.as_deref(), Some("req-001"));
    assert_eq!(summary.context_pack_id.as_deref(), Some("cpid-abc"));

    let rec = build_eval_record(
        "sess-t1",
        1_000,
        "task",
        "model",
        "Act",
        "native",
        &[],
        None,
        &[],
        None,
        empty_classes(),
        &[],
        None,
        Some(summary),
        "done",
    );

    let json = write_and_parse(&rec);
    assert_eq!(json["photon_eval"]["admission_decision"], "accepted");
    assert_eq!(json["photon_eval"]["prompt_adopted"], true);
    assert_eq!(json["photon_eval"]["context_pack_id"], "cpid-abc");
}

// T2 ─────────────────────────────────────────────────────────────────────────

/// admitted=false → admission_decision="rejected", prompt_adopted=false.
#[test]
fn t2_normal_rejected_recorded_in_eval_log() {
    use anvil::photon::eval::parse_evaluate_response;

    let resp = make_resp(serde_json::json!({
        "admitted": false,
        "context_pack_id": "cpid-xyz",
    }));
    let summary = parse_evaluate_response(&resp);

    assert_eq!(summary.admission_decision.as_deref(), Some("rejected"));
    assert_eq!(summary.prompt_adopted, Some(false));

    let rec = build_eval_record(
        "sess-t2",
        2_000,
        "task",
        "model",
        "Act",
        "native",
        &[],
        None,
        &[],
        None,
        empty_classes(),
        &[],
        None,
        Some(summary),
        "done",
    );

    let json = write_and_parse(&rec);
    assert_eq!(json["photon_eval"]["admission_decision"], "rejected");
    assert_eq!(json["photon_eval"]["prompt_adopted"], false);
}

// T3 ─────────────────────────────────────────────────────────────────────────

/// Shadow mode: even when photon_context_pack_response is None (shadow=true),
/// context_pack_id extracted from the context_pack response is captured
/// and used as fallback in the eval log when EvaluateResponse lacks it.
///
/// This test simulates the fallback by constructing a PhotonEvalSummary with
/// context_pack_id=None and a pre-captured last_context_pack_id, then applying
/// the same fallback logic that invoke_photon_evaluate uses.
#[test]
fn t3_shadow_mode_context_pack_id_in_eval_log() {
    // Simulate: context_pack response provided cpid, shadow mode prevented injection,
    // evaluate response did NOT include context_pack_id.
    let mut summary = PhotonEvalSummary {
        photon_request_id: None,
        context_pack_id: None, // evaluate response did not carry it
        admission_decision: Some("accepted".to_string()),
        warnings: vec![],
        prompt_adopted: Some(true),
        task_outcome: None,
        retry_summary: None,
    };

    let last_context_pack_id = Some("cpid-from-shadow".to_string());
    // Apply the same fallback logic as invoke_photon_evaluate.
    if summary.context_pack_id.is_none() {
        summary.context_pack_id = last_context_pack_id.clone();
    }

    assert_eq!(summary.context_pack_id.as_deref(), Some("cpid-from-shadow"));

    let rec = build_eval_record(
        "sess-t3",
        3_000,
        "task",
        "model",
        "Act",
        "native",
        &[],
        None,
        &[],
        None,
        empty_classes(),
        &[],
        None,
        Some(summary),
        "done",
    );

    let json = write_and_parse(&rec);
    assert_eq!(json["photon_eval"]["context_pack_id"], "cpid-from-shadow");
    assert_eq!(json["photon_eval"]["admission_decision"], "accepted");
}

// T4 ─────────────────────────────────────────────────────────────────────────

/// Fail-open: when EvaluateResponse is None (network error),
/// photon_eval is null in the EvalRecord — no crash.
#[test]
fn t4_fail_open_evaluate_none_records_null() {
    let rec = build_eval_record(
        "sess-t4",
        4_000,
        "task",
        "model",
        "Act",
        "native",
        &[ToolCallSummary {
            name: "Bash".to_string(),
            args_summary: "cargo build".to_string(),
        }],
        None,
        &[],
        None,
        empty_classes(),
        &[],
        None,
        None, // photon_eval = None (fail-open)
        "done",
    );

    assert!(rec.photon_eval.is_none());

    let json = write_and_parse(&rec);
    assert!(json["photon_eval"].is_null(), "photon_eval should be null");
    assert_eq!(json["final_outcome"], "done");
}

// T5 ─────────────────────────────────────────────────────────────────────────

/// Secrets in warnings are masked by mask_secrets inside parse_evaluate_response.
#[test]
fn t5_warnings_secrets_are_masked() {
    use anvil::photon::eval::parse_evaluate_response;

    let resp = make_resp(serde_json::json!({
        "admitted": true,
        "warnings": [
            "api_key=sk-proj-supersecret123 detected in prompt",
            "normal warning without secrets",
        ],
    }));
    let summary = parse_evaluate_response(&resp);

    assert_eq!(summary.warnings.len(), 2);
    let w0 = &summary.warnings[0];
    assert!(
        !w0.contains("sk-proj-supersecret123"),
        "secret leaked in warning: {w0}"
    );
    assert!(w0.contains("***"), "mask marker missing: {w0}");
    assert_eq!(summary.warnings[1], "normal warning without secrets");
}

// T6 ─────────────────────────────────────────────────────────────────────────

/// context_pack_id fallback: when EvaluateResponse has no context_pack_id,
/// last_context_pack_id from invoke_photon_context_pack is substituted.
#[test]
fn t6_context_pack_id_fallback_from_last() {
    use anvil::photon::eval::parse_evaluate_response;

    // EvaluateResponse does NOT include context_pack_id.
    let resp = make_resp(serde_json::json!({
        "admitted": true,
        "task_outcome": "success",
    }));
    let mut summary = parse_evaluate_response(&resp);

    assert!(
        summary.context_pack_id.is_none(),
        "expected no cpid in response"
    );

    // Simulate the fallback from invoke_photon_evaluate.
    let last_context_pack_id = Some("cpid-fallback-from-context-pack".to_string());
    if summary.context_pack_id.is_none() {
        summary.context_pack_id = last_context_pack_id;
    }

    let rec = build_eval_record(
        "sess-t6",
        6_000,
        "task",
        "model",
        "Act",
        "native",
        &[],
        None,
        &[],
        None,
        empty_classes(),
        &[],
        None,
        Some(summary),
        "done",
    );

    let json = write_and_parse(&rec);
    assert_eq!(
        json["photon_eval"]["context_pack_id"],
        "cpid-fallback-from-context-pack"
    );
    assert_eq!(json["photon_eval"]["admission_decision"], "accepted");
}
