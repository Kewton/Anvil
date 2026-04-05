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
    assert_eq!(json["schema_version"], "1");
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
    ];

    for key in &expected_keys {
        assert!(obj.contains_key(*key), "telemetry JSON missing key: {key}");
    }
}
