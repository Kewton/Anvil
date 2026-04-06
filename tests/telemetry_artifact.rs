//! Tests for AgentTelemetry::write_artifact / write_artifact_to_dir (Issue #271 Phase 4).

use anvil::contracts::AgentTelemetry;
use std::fs;

/// Helper: create a telemetry instance with known values for testing.
fn sample_telemetry() -> AgentTelemetry {
    let mut tel = AgentTelemetry::new();
    tel.premature_final_count = 2;
    tel.total_final_requests = 5;
    tel.plan_registration_count = 1;
    tel.plan_update_count = 1;
    tel.anvil_plan_visible_count = 3;
    tel.last_mutation_turn = 25;
    tel.final_suppressed_with_remaining_targets_count = 1;
    tel.sync_from_touched_files_count = 2;
    tel.initial_plan_miss_count = 1;
    tel.no_op_mutation_count = 1;
    tel.rolled_back_mutation_count = 1;
    tel.record_turn_metrics(3, 2, 100, 1);
    tel.record_turn_metrics(1, 0, 50, 2);
    tel
}

/// Write the artifact and return the parsed JSON object.
fn write_and_parse(tel: &AgentTelemetry, label: &str) -> serde_json::Value {
    let dir = tempfile::tempdir().unwrap();
    let session_id = format!("test_{label}_{}", std::process::id());
    let dir_str = dir.path().to_str().unwrap();

    tel.write_artifact_to_dir(dir_str, &session_id).unwrap();

    let file_path = dir.path().join(format!("{session_id}_telemetry.json"));
    let contents = fs::read_to_string(&file_path).unwrap();
    serde_json::from_str(&contents).unwrap()
}

#[test]
fn telemetry_artifact_written_when_dir_set() {
    let dir = tempfile::tempdir().unwrap();
    let session_id = format!("test_written_{}", std::process::id());
    let dir_str = dir.path().to_str().unwrap();

    let tel = sample_telemetry();
    let result = tel.write_artifact_to_dir(dir_str, &session_id);

    assert!(result.is_ok(), "write_artifact should succeed: {result:?}");

    let file_path = dir.path().join(format!("{session_id}_telemetry.json"));
    assert!(file_path.exists(), "telemetry file should exist");

    let contents = fs::read_to_string(&file_path).unwrap();
    assert!(!contents.is_empty());
}

#[test]
fn telemetry_artifact_not_written_when_dir_unset() {
    // write_artifact reads env; when ANVIL_TELEMETRY_DIR is unset it should be no-op.
    let tel = AgentTelemetry::new();
    let result = tel.write_artifact("test_unset_no_env");
    // This relies on ANVIL_TELEMETRY_DIR not being set in the test environment.
    assert!(result.is_ok(), "should be no-op when dir unset");
}

#[test]
fn telemetry_artifact_schema_version() {
    let json = write_and_parse(&AgentTelemetry::new(), "schema");
    assert_eq!(json["schema_version"], "3");
}

#[test]
fn telemetry_artifact_derived_values() {
    let json = write_and_parse(&sample_telemetry(), "derived");

    // accepted_final_count = total(5) - premature(2) = 3
    assert_eq!(json["accepted_final_count"], 3);
    // late_mutation_flag: last_mutation_turn(25) > LATE_MUTATION_THRESHOLD(20) = true
    assert_eq!(json["late_mutation_flag"], true);
}

#[test]
fn telemetry_artifact_no_overwrite() {
    let dir = tempfile::tempdir().unwrap();
    let session_id = format!("test_nooverwrite_{}", std::process::id());
    let file_path = dir.path().join(format!("{session_id}_telemetry.json"));
    let dir_str = dir.path().to_str().unwrap();

    // Create existing file.
    fs::write(&file_path, "existing").unwrap();

    let tel = AgentTelemetry::new();
    let result = tel.write_artifact_to_dir(dir_str, &session_id);

    assert!(result.is_err(), "should fail when file already exists");

    // Original content should be preserved.
    let contents = fs::read_to_string(&file_path).unwrap();
    assert_eq!(contents, "existing");
}

#[test]
fn telemetry_artifact_relative_path_rejected() {
    let tel = AgentTelemetry::new();
    let result = tel.write_artifact_to_dir("relative/path", "test_relative");

    assert!(result.is_err(), "relative path should be rejected");
}

#[test]
fn telemetry_artifact_all_fields_present() {
    let json = write_and_parse(&sample_telemetry(), "allfields");
    let obj = json.as_object().expect("should be a JSON object");

    let expected_keys = [
        "schema_version",
        "session_id",
        "completion_kind",
        "premature_final_count",
        "total_final_requests",
        "accepted_final_count",
        "plan_registration_count",
        "plan_update_count",
        "anvil_plan_visible_count",
        "last_mutation_turn",
        "late_mutation_flag",
        "final_suppressed_with_remaining_targets_count",
        "sync_from_touched_files_count",
        "forced_workset_transition_count",
        "initial_plan_miss_count",
        "no_op_mutation_count",
        "rolled_back_mutation_count",
        "plan_repair_request_count",
        "mutations_per_turn",
        "items_advanced_per_turn",
        "guidance_chars_per_turn",
        "workset_size_per_turn",
        "first_mutation_event_turn",
        "first_mutation_event_elapsed_s",
        "first_mutation_event_tool",
        "first_mutation_event_semantic_basis",
        "recovery_telemetry",
        "first_successful_mutation_file_role",
        "first_non_test_mutation_file_role",
        "mutation_role_sequence",
        "peripheral_mutation_before_core_count",
        "role_transition_rework_count",
        "files_touched_before_first_core_mutation",
        "plan_order_vs_actual_mutation_divergence",
    ];

    for key in &expected_keys {
        assert!(obj.contains_key(*key), "telemetry JSON missing key: {key}");
    }
}

// ---------------------------------------------------------------------------
// Issue #273 Phase 1.5: first_mutation_event_* tests
// ---------------------------------------------------------------------------

/// (c) record_mutation_turn sets first_mutation_event_* on the first call.
#[test]
fn first_mutation_event_recorded_on_first_call() {
    let mut tel = AgentTelemetry::new();
    tel.record_mutation_turn(3, Some(1.5), "file.write");
    assert_eq!(tel.first_mutation_event_turn, Some(3));
    assert_eq!(tel.first_mutation_event_elapsed_s, Some(1.5));
    assert_eq!(tel.first_mutation_event_tool.as_deref(), Some("file.write"));
}

/// (d) Subsequent calls do NOT overwrite first_mutation_event_*.
#[test]
fn first_mutation_event_not_overwritten_by_later_calls() {
    let mut tel = AgentTelemetry::new();
    tel.record_mutation_turn(3, Some(1.5), "file.write");
    tel.record_mutation_turn(7, Some(5.0), "file.edit");
    // first_mutation_event_* should still reflect the first call
    assert_eq!(tel.first_mutation_event_turn, Some(3));
    assert_eq!(tel.first_mutation_event_elapsed_s, Some(1.5));
    assert_eq!(tel.first_mutation_event_tool.as_deref(), Some("file.write"));
    // last_mutation_turn should reflect the latest call
    assert_eq!(tel.last_mutation_turn, 7);
}

/// (e) Artifact payload includes first_mutation_event_* when mutations occurred.
#[test]
fn first_mutation_event_in_artifact_payload() {
    let mut tel = sample_telemetry();
    tel.record_mutation_turn(2, Some(0.8), "file.edit");
    let json = write_and_parse(&tel, "first_mut_present");
    assert_eq!(json["first_mutation_event_turn"], 2);
    assert!((json["first_mutation_event_elapsed_s"].as_f64().unwrap() - 0.8).abs() < 1e-9);
    assert_eq!(json["first_mutation_event_tool"], "file.edit");
    assert_eq!(
        json["first_mutation_event_semantic_basis"],
        "runtime_lower_bound"
    );
}

// ---------------------------------------------------------------------------
// Issue #277: backward compatibility test
// ---------------------------------------------------------------------------

/// Deserializing schema_version 2 JSON (without Issue #277 fields) should
/// produce default values for all new fields.
#[test]
fn backward_compat_schema_v2_deserializes_with_defaults() {
    let v2_json = r#"{
        "premature_final_count": 1,
        "total_final_requests": 3,
        "plan_registration_count": 1,
        "plan_update_count": 0,
        "sync_from_touched_files_count": 0,
        "last_mutation_turn": 5,
        "anvil_plan_visible_count": 1,
        "final_suppressed_with_remaining_targets_count": 0,
        "initial_plan_miss_count": 0,
        "no_op_mutation_count": 0,
        "rolled_back_mutation_count": 0
    }"#;
    let tel: AgentTelemetry = serde_json::from_str(v2_json).unwrap();

    // All Issue #277 fields should have their default values
    assert!(tel.first_successful_mutation_file_role.is_none());
    assert!(tel.first_non_test_mutation_file_role.is_none());
    assert!(tel.mutation_role_sequence.is_empty());
    assert_eq!(tel.peripheral_mutation_before_core_count, 0);
    assert_eq!(tel.role_transition_rework_count, 0);
    assert_eq!(tel.files_touched_before_first_core_mutation, 0);
    assert!(tel.plan_order_vs_actual_mutation_divergence.is_none());
}

/// (f) Artifact payload has null first_mutation_event_* when no mutations occurred.
#[test]
fn first_mutation_event_null_when_no_mutations() {
    let tel = AgentTelemetry::new();
    let json = write_and_parse(&tel, "first_mut_null");
    assert!(json["first_mutation_event_turn"].is_null());
    assert!(json["first_mutation_event_elapsed_s"].is_null());
    assert!(json["first_mutation_event_tool"].is_null());
    // semantic_basis is always present as a constant string
    assert_eq!(
        json["first_mutation_event_semantic_basis"],
        "runtime_lower_bound"
    );
}
