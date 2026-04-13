//! Tests for RetryTelemetry (Issue #372).
//!
//! Validates the RetryTelemetry struct, serde compatibility,
//! artifact output, and convenience methods.

use anvil::contracts::{AgentTelemetry, RetryTelemetry};

// ---------------------------------------------------------------------------
// RetryTelemetry Default / Serialize / Deserialize
// ---------------------------------------------------------------------------

#[test]
fn retry_telemetry_default_all_zero() {
    let rt = RetryTelemetry::default();
    assert_eq!(rt.guidance_retry_attempted, 0);
    assert_eq!(rt.guidance_retry_succeeded, 0);
    assert_eq!(rt.final_guard_retry_attempted, 0);
    assert_eq!(rt.final_guard_retry_succeeded, 0);
    assert_eq!(rt.http_retry_attempted, 0);
    assert_eq!(rt.http_retry_final_failure, 0);
    assert_eq!(rt.edit_fallback_strict_count, 0);
    assert_eq!(rt.edit_fallback_trailing_ws_count, 0);
    assert_eq!(rt.edit_fallback_anchor_count, 0);
    assert_eq!(rt.edit_fallback_all_failed_count, 0);
    assert_eq!(rt.edit_write_fallback_attempted, 0);
    assert_eq!(rt.edit_write_fallback_succeeded, 0);
    assert_eq!(rt.plan_gate_suppression_attempted, 0);
    assert_eq!(rt.plan_gate_suppression_succeeded, 0);
    assert_eq!(rt.proactive_delegation_attempted, 0);
    assert_eq!(rt.proactive_delegation_succeeded, 0);
    assert_eq!(rt.parse_failure_recovered, 0);
    assert_eq!(rt.parse_failure_empty_errored, 0);
}

#[test]
fn retry_telemetry_serde_roundtrip() {
    let rt = RetryTelemetry {
        guidance_retry_attempted: 3,
        edit_fallback_anchor_count: 2,
        parse_failure_recovered: 1,
        ..Default::default()
    };

    let json = serde_json::to_string(&rt).unwrap();
    let deserialized: RetryTelemetry = serde_json::from_str(&json).unwrap();

    assert_eq!(deserialized.guidance_retry_attempted, 3);
    assert_eq!(deserialized.edit_fallback_anchor_count, 2);
    assert_eq!(deserialized.parse_failure_recovered, 1);
    // Others should remain zero
    assert_eq!(deserialized.final_guard_retry_attempted, 0);
}

#[test]
fn retry_telemetry_serde_backward_compat_empty_json() {
    // Old JSON without any RetryTelemetry fields should deserialize successfully.
    let json = r#"{}"#;
    let rt: RetryTelemetry = serde_json::from_str(json).unwrap();
    assert_eq!(rt.guidance_retry_attempted, 0);
    assert_eq!(rt.parse_failure_empty_errored, 0);
}

// ---------------------------------------------------------------------------
// AgentTelemetry with flattened RetryTelemetry
// ---------------------------------------------------------------------------

#[test]
fn agent_telemetry_retry_field_accessible() {
    let mut tel = AgentTelemetry::new();
    tel.retry.guidance_retry_attempted = 5;
    tel.retry.parse_failure_recovered = 2;
    assert_eq!(tel.retry.guidance_retry_attempted, 5);
    assert_eq!(tel.retry.parse_failure_recovered, 2);
}

#[test]
fn agent_telemetry_serde_backward_compat_no_retry_fields() {
    // Simulate old JSON without retry fields — should deserialize with defaults.
    let json = r#"{
        "premature_final_count": 1,
        "total_final_requests": 2,
        "plan_registration_count": 0,
        "plan_update_count": 0,
        "sync_from_touched_files_count": 0
    }"#;
    let tel: AgentTelemetry = serde_json::from_str(json).unwrap();
    assert_eq!(tel.premature_final_count, 1);
    assert_eq!(tel.retry.guidance_retry_attempted, 0);
    assert_eq!(tel.retry.parse_failure_empty_errored, 0);
}

#[test]
fn agent_telemetry_serde_flattened_roundtrip() {
    let mut tel = AgentTelemetry::new();
    tel.premature_final_count = 3;
    tel.retry.final_guard_retry_attempted = 2;
    tel.retry.edit_fallback_strict_count = 7;

    let json = serde_json::to_string(&tel).unwrap();
    let deserialized: AgentTelemetry = serde_json::from_str(&json).unwrap();

    assert_eq!(deserialized.premature_final_count, 3);
    assert_eq!(deserialized.retry.final_guard_retry_attempted, 2);
    assert_eq!(deserialized.retry.edit_fallback_strict_count, 7);
}

// ---------------------------------------------------------------------------
// Artifact output includes retry fields (Issue #372)
// ---------------------------------------------------------------------------

fn write_and_parse(tel: &AgentTelemetry, label: &str) -> serde_json::Value {
    let dir = tempfile::tempdir().unwrap();
    let session_id = format!("test_{label}_{}", std::process::id());
    let dir_str = dir.path().to_str().unwrap();

    tel.write_artifact_to_dir(dir_str, &session_id).unwrap();

    let file_path = dir.path().join(format!("{session_id}_telemetry.json"));
    let contents = std::fs::read_to_string(&file_path).unwrap();
    serde_json::from_str(&contents).unwrap()
}

#[test]
fn retry_telemetry_artifact_output_has_retry_fields() {
    let mut tel = AgentTelemetry::new();
    tel.retry.guidance_retry_attempted = 2;
    tel.retry.guidance_retry_succeeded = 1;
    tel.retry.parse_failure_recovered = 3;
    tel.retry.edit_fallback_strict_count = 5;

    let json = write_and_parse(&tel, "retry_fields");
    let obj = json.as_object().unwrap();

    assert!(
        obj.contains_key("guidance_retry_attempted"),
        "artifact should contain guidance_retry_attempted"
    );
    assert_eq!(json["guidance_retry_attempted"], 2);
    assert_eq!(json["guidance_retry_succeeded"], 1);
    assert_eq!(json["parse_failure_recovered"], 3);
    assert_eq!(json["edit_fallback_strict_count"], 5);
    // Default zero fields should also be present
    assert_eq!(json["final_guard_retry_attempted"], 0);
}

#[test]
fn retry_telemetry_artifact_preserves_existing_fields() {
    // Ensure the serde migration does not break existing artifact fields.
    let mut tel = AgentTelemetry::new();
    tel.premature_final_count = 2;
    tel.total_final_requests = 5;
    tel.last_mutation_turn = 25;
    tel.retry.guidance_retry_attempted = 1;

    let json = write_and_parse(&tel, "preserve_existing");

    // Derived fields
    assert_eq!(json["schema_version"], "2");
    assert_eq!(json["accepted_final_count"], 3);
    assert_eq!(json["late_mutation_flag"], true);
    assert_eq!(
        json["first_mutation_event_semantic_basis"],
        "runtime_lower_bound"
    );
    // completion_kind None -> "none"
    assert_eq!(json["completion_kind"], "none");
    // Existing struct fields
    assert_eq!(json["premature_final_count"], 2);
    assert_eq!(json["total_final_requests"], 5);
}

// ---------------------------------------------------------------------------
// Convenience methods
// ---------------------------------------------------------------------------

#[test]
fn retry_telemetry_record_methods() {
    let mut rt = RetryTelemetry::default();

    rt.record_guidance_retry_attempted();
    assert_eq!(rt.guidance_retry_attempted, 1);

    rt.record_guidance_retry_succeeded();
    assert_eq!(rt.guidance_retry_succeeded, 1);

    rt.record_final_guard_retry_attempted();
    assert_eq!(rt.final_guard_retry_attempted, 1);

    rt.record_final_guard_retry_succeeded();
    assert_eq!(rt.final_guard_retry_succeeded, 1);

    rt.record_plan_gate_suppression_attempted();
    assert_eq!(rt.plan_gate_suppression_attempted, 1);

    rt.record_proactive_delegation_attempted();
    assert_eq!(rt.proactive_delegation_attempted, 1);

    rt.record_parse_failure_recovered();
    assert_eq!(rt.parse_failure_recovered, 1);

    rt.record_parse_failure_empty_errored();
    assert_eq!(rt.parse_failure_empty_errored, 1);

    rt.record_edit_write_fallback_attempted();
    assert_eq!(rt.edit_write_fallback_attempted, 1);
}
