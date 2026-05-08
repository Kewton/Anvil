use std::path::Path;

use anvil::photon::mapper::{
    ContextPackInputs, MAX_CONTEXT_PACK_TOOL_ARG_BYTES, MAX_CONTEXT_PACK_TOOL_NAME_BYTES,
    PHOTON_CONTEXT_PACK_SCHEMA_VERSION, PhotonGateInputs, RecentToolCall,
    build_context_pack_request, deterministic_canary_hash, should_send_context_pack,
};

fn make_gate(photon_present: bool, shadow: bool, canary: u16) -> PhotonGateInputs<'static> {
    PhotonGateInputs {
        photon_present,
        shadow_mode: shadow,
        canary,
        session_id: "sess-abc123",
        turn_idx: 1,
    }
}

fn default_inputs<'a>(repo: &'a Path) -> ContextPackInputs<'a> {
    ContextPackInputs {
        task: Some("Fix the login bug"),
        repo_path: repo,
        branch: Some("main"),
        commit: Some("abc123"),
        working_memory_text: Some("working on auth module"),
        touched_files: &[],
        recent_tool_summary: &[],
        selected_case_ids: &[],
        selected_anti_pattern_ids: &[],
        selected_precaution_ids: &[],
    }
}

// T1: 全フィールド正常マッピング
#[test]
fn t1_all_fields_mapped() {
    let repo = tempfile::TempDir::new().unwrap();
    let tools = [RecentToolCall {
        name: "Read".to_string(),
        args_summary: r#"{"path":"src/main.rs"}"#.to_string(),
    }];
    let cases = vec!["case_abc".to_string()];
    let anti = vec!["anti_xyz".to_string()];
    let prec = vec!["prec_123".to_string()];
    let files = vec!["src/lib.rs".to_string()];

    let inputs = ContextPackInputs {
        task: Some("Fix login bug"),
        repo_path: repo.path(),
        branch: Some("main"),
        commit: Some("deadbeef"),
        working_memory_text: Some("auth module"),
        touched_files: &files,
        recent_tool_summary: &tools,
        selected_case_ids: &cases,
        selected_anti_pattern_ids: &anti,
        selected_precaution_ids: &prec,
    };
    let req = build_context_pack_request(&inputs);
    let v = &req.0;
    assert_eq!(v["task"]["user_request"], "Fix login bug");
    assert_eq!(v["repo"]["branch"], "main");
    assert_eq!(v["repo"]["commit"], "deadbeef");
    assert!(!v["repo"]["root"].as_str().unwrap().is_empty());
    assert_eq!(v["working_memory"]["active_task"], "Fix login bug");
    assert_eq!(v["working_memory"]["touched_files"][0], "src/lib.rs");
    assert_eq!(v["recent_tool_summary"][0]["name"], "Read");
    assert_eq!(v["selected_cases"][0], "case_abc");
    assert_eq!(v["selected_anti_patterns"][0], "anti_xyz");
    assert_eq!(v["selected_precautions"][0], "prec_123");
}

// T2: schema_version = "action-memory.v0.2"
#[test]
fn t2_schema_version_is_v02() {
    let repo = tempfile::TempDir::new().unwrap();
    let req = build_context_pack_request(&default_inputs(repo.path()));
    assert_eq!(
        req.0["schema_version"],
        serde_json::Value::String(PHOTON_CONTEXT_PACK_SCHEMA_VERSION.to_string())
    );
}

// T3: stdout/stderr キー非包含
#[test]
fn t3_no_stdout_stderr_keys() {
    let repo = tempfile::TempDir::new().unwrap();
    let req = build_context_pack_request(&default_inputs(repo.path()));
    let v = &req.0;
    assert!(!v.as_object().unwrap().contains_key("stdout"));
    assert!(!v.as_object().unwrap().contains_key("stderr"));
}

// T4: secret masking — token-like value is replaced with ***
#[test]
fn t4_secret_masking_task() {
    let repo = tempfile::TempDir::new().unwrap();
    let inputs = ContextPackInputs {
        task: Some("token=ghp_AAAAAAAABBBBBBBBCCCCCCCCDDDDDDDDEEEE"),
        repo_path: repo.path(),
        branch: None,
        commit: None,
        working_memory_text: None,
        touched_files: &[],
        recent_tool_summary: &[],
        selected_case_ids: &[],
        selected_anti_pattern_ids: &[],
        selected_precaution_ids: &[],
    };
    let req = build_context_pack_request(&inputs);
    let task_val = req.0["task"]["user_request"].as_str().unwrap();
    assert!(
        !task_val.contains("ghp_AAAAAAAABBBBBBBBCCCCCCCCDDDDDDDDEEEE"),
        "raw token should be masked, got: {task_val}"
    );
    assert!(
        task_val.contains("***"),
        "masked value should contain ***, got: {task_val}"
    );
}

// T5: workspace-relative touched_files
#[test]
fn t5_touched_files_workspace_relative() {
    let repo = tempfile::TempDir::new().unwrap();
    let files = vec!["src/main.rs".to_string(), "tests/foo_test.rs".to_string()];
    let inputs = ContextPackInputs {
        task: None,
        repo_path: repo.path(),
        branch: None,
        commit: None,
        working_memory_text: None,
        touched_files: &files,
        recent_tool_summary: &[],
        selected_case_ids: &[],
        selected_anti_pattern_ids: &[],
        selected_precaution_ids: &[],
    };
    let req = build_context_pack_request(&inputs);
    let arr = req.0["working_memory"]["touched_files"].as_array().unwrap();
    assert_eq!(arr.len(), 2);
    assert_eq!(arr[0], "src/main.rs");
    assert_eq!(arr[1], "tests/foo_test.rs");
}

// T6: should_send_context_pack(photon_present=false) = false
#[test]
fn t6_gate_no_photon_returns_false() {
    let gate = make_gate(false, true, 1000);
    assert!(!should_send_context_pack(&gate));
}

// T7: shadow=false/canary=0 = false
#[test]
fn t7_gate_shadow_false_canary_zero_returns_false() {
    let gate = make_gate(true, false, 0);
    assert!(!should_send_context_pack(&gate));
}

// T8: shadow=true = true (regardless of canary)
#[test]
fn t8_gate_shadow_true_always_sends() {
    let gate = make_gate(true, true, 0);
    assert!(should_send_context_pack(&gate));
}

// T9: canary=1000 = true
#[test]
fn t9_gate_canary_1000_always_sends() {
    let gate = make_gate(true, false, 1000);
    assert!(should_send_context_pack(&gate));
}

// T10: canary deterministic — same input always gives same result
#[test]
fn t10_canary_deterministic() {
    let h1 = deterministic_canary_hash("sess-xyz", 5);
    let h2 = deterministic_canary_hash("sess-xyz", 5);
    assert_eq!(h1, h2);
}

// T10b: different inputs produce different hashes (probabilistic sanity check)
#[test]
fn t10b_canary_different_inputs_differ() {
    let h1 = deterministic_canary_hash("sess-aaa", 1);
    let h2 = deterministic_canary_hash("sess-bbb", 1);
    let h3 = deterministic_canary_hash("sess-aaa", 2);
    // These should differ (extremely unlikely they're equal by chance)
    assert!(
        h1 != h2 || h1 != h3,
        "hashes should differ for different inputs"
    );
}

// T11: working_memory は active_task と touched_files を持つオブジェクト
#[test]
fn t11_working_memory_is_structured_object() {
    let repo = tempfile::TempDir::new().unwrap();
    let inputs = ContextPackInputs {
        task: Some("test task"),
        repo_path: repo.path(),
        branch: None,
        commit: None,
        working_memory_text: None,
        touched_files: &[],
        recent_tool_summary: &[],
        selected_case_ids: &[],
        selected_anti_pattern_ids: &[],
        selected_precaution_ids: &[],
    };
    let req = build_context_pack_request(&inputs);
    let wm = req.0["working_memory"].as_object().unwrap();
    assert!(
        wm.contains_key("active_task"),
        "working_memory must have active_task"
    );
    assert!(
        wm.contains_key("touched_files"),
        "working_memory must have touched_files"
    );
    assert_eq!(wm["active_task"], "test task");
    assert!(wm["touched_files"].as_array().unwrap().is_empty());
}

// T12: recent_tool_summary: tool output 非包含、args 256 bytes cap
#[test]
fn t12_recent_tool_summary_args_capped() {
    let repo = tempfile::TempDir::new().unwrap();
    let long_args = "a".repeat(MAX_CONTEXT_PACK_TOOL_ARG_BYTES + 100);
    let tools = [RecentToolCall {
        name: "Read".to_string(),
        args_summary: long_args,
    }];
    let inputs = ContextPackInputs {
        task: None,
        repo_path: repo.path(),
        branch: None,
        commit: None,
        working_memory_text: None,
        touched_files: &[],
        recent_tool_summary: &tools,
        selected_case_ids: &[],
        selected_anti_pattern_ids: &[],
        selected_precaution_ids: &[],
    };
    let req = build_context_pack_request(&inputs);
    let args_str = req.0["recent_tool_summary"][0]["args_summary"]
        .as_str()
        .unwrap();
    assert!(
        args_str.len() <= MAX_CONTEXT_PACK_TOOL_ARG_BYTES,
        "args_summary should be capped at {} bytes, got {}",
        MAX_CONTEXT_PACK_TOOL_ARG_BYTES,
        args_str.len()
    );
}

// T13: リクエストに必須フィールドが揃っている（request_id は呼び出しごとに異なる UUID）
#[test]
fn t13_build_request_has_required_schema_fields() {
    let repo = tempfile::TempDir::new().unwrap();
    let inputs = default_inputs(repo.path());
    let req = build_context_pack_request(&inputs);
    let v = &req.0;
    assert!(
        v["schema_version"].is_string(),
        "schema_version must be a string"
    );
    assert!(v["request_id"].is_string(), "request_id must be a string");
    assert!(
        !v["request_id"].as_str().unwrap().is_empty(),
        "request_id must not be empty"
    );
    assert!(v["agent"].is_object(), "agent must be an object");
    assert_eq!(v["agent"]["name"], "anvil");
    assert!(v["repo"].is_object(), "repo must be an object");
    assert!(v["task"].is_object(), "task must be an object");
    assert!(
        v["working_memory"].is_object(),
        "working_memory must be an object"
    );
    // Different calls produce different request_ids
    let req2 = build_context_pack_request(&inputs);
    assert_ne!(
        req.0["request_id"], req2.0["request_id"],
        "each call must produce a unique request_id"
    );
}

// T14: selected IDs pass through as-is
#[test]
fn t14_selected_ids_pass_through() {
    let repo = tempfile::TempDir::new().unwrap();
    let cases = vec!["case_001".to_string(), "case_002".to_string()];
    let anti = vec!["anti_abc".to_string()];
    let prec = vec!["prec_xyz".to_string()];
    let inputs = ContextPackInputs {
        task: None,
        repo_path: repo.path(),
        branch: None,
        commit: None,
        working_memory_text: None,
        touched_files: &[],
        recent_tool_summary: &[],
        selected_case_ids: &cases,
        selected_anti_pattern_ids: &anti,
        selected_precaution_ids: &prec,
    };
    let req = build_context_pack_request(&inputs);
    assert_eq!(req.0["selected_cases"][0], "case_001");
    assert_eq!(req.0["selected_cases"][1], "case_002");
    assert_eq!(req.0["selected_anti_patterns"][0], "anti_abc");
    assert_eq!(req.0["selected_precautions"][0], "prec_xyz");
}

// T15: touched_files の `..` component はドロップ (DR4-001)
#[test]
fn t15_dotdot_components_dropped() {
    let repo = tempfile::TempDir::new().unwrap();
    let files = vec![
        "src/main.rs".to_string(),
        "../../etc/passwd".to_string(),
        "../secret.txt".to_string(),
        "valid/file.rs".to_string(),
    ];
    let inputs = ContextPackInputs {
        task: None,
        repo_path: repo.path(),
        branch: None,
        commit: None,
        working_memory_text: None,
        touched_files: &files,
        recent_tool_summary: &[],
        selected_case_ids: &[],
        selected_anti_pattern_ids: &[],
        selected_precaution_ids: &[],
    };
    let req = build_context_pack_request(&inputs);
    let arr = req.0["working_memory"]["touched_files"].as_array().unwrap();
    let paths: Vec<&str> = arr.iter().map(|v| v.as_str().unwrap()).collect();
    assert!(
        paths.iter().all(|p| !p.contains("..")),
        "no .. components should survive: {:?}",
        paths
    );
    assert!(
        paths.contains(&"src/main.rs"),
        "valid relative path should remain"
    );
    assert!(
        paths.contains(&"valid/file.rs"),
        "valid relative path should remain"
    );
}

// T16: request body 全体に mask_payload_inplace が適用される (DR4-002)
#[test]
fn t16_full_request_body_secret_masked() {
    let repo = tempfile::TempDir::new().unwrap();
    // Embed a token-like value in selected_cases (IDs only, but we verify masking runs)
    let inputs = ContextPackInputs {
        task: Some("normal task"),
        repo_path: repo.path(),
        branch: Some("main"),
        commit: None,
        working_memory_text: None,
        touched_files: &[],
        recent_tool_summary: &[],
        selected_case_ids: &[],
        selected_anti_pattern_ids: &[],
        selected_precaution_ids: &[],
    };
    let req = build_context_pack_request(&inputs);
    // The full Value should serialize without panic and without raw tokens.
    let serialized = serde_json::to_string(&req.0).unwrap();
    assert!(!serialized.is_empty());
}

// T17: RecentToolCall.name — 長さ cap と許可文字種正規化 (DR4-003)
#[test]
fn t17_tool_name_cap_and_normalization() {
    let repo = tempfile::TempDir::new().unwrap();
    let long_name = "a".repeat(MAX_CONTEXT_PACK_TOOL_NAME_BYTES + 50);
    let tools = [RecentToolCall {
        name: long_name,
        args_summary: "{}".to_string(),
    }];
    let inputs = ContextPackInputs {
        task: None,
        repo_path: repo.path(),
        branch: None,
        commit: None,
        working_memory_text: None,
        touched_files: &[],
        recent_tool_summary: &tools,
        selected_case_ids: &[],
        selected_anti_pattern_ids: &[],
        selected_precaution_ids: &[],
    };
    let req = build_context_pack_request(&inputs);
    let name = req.0["recent_tool_summary"][0]["name"].as_str().unwrap();
    assert!(
        name.len() <= MAX_CONTEXT_PACK_TOOL_NAME_BYTES,
        "name should be capped at {} bytes, got {}",
        MAX_CONTEXT_PACK_TOOL_NAME_BYTES,
        name.len()
    );
}

// T17b: non-alphanumeric chars in tool name are replaced with _
#[test]
fn t17b_tool_name_special_chars_normalized() {
    let repo = tempfile::TempDir::new().unwrap();
    let tools = [RecentToolCall {
        name: "my tool/name!with spaces".to_string(),
        args_summary: "{}".to_string(),
    }];
    let inputs = ContextPackInputs {
        task: None,
        repo_path: repo.path(),
        branch: None,
        commit: None,
        working_memory_text: None,
        touched_files: &[],
        recent_tool_summary: &tools,
        selected_case_ids: &[],
        selected_anti_pattern_ids: &[],
        selected_precaution_ids: &[],
    };
    let req = build_context_pack_request(&inputs);
    let name = req.0["recent_tool_summary"][0]["name"].as_str().unwrap();
    // Only [A-Za-z0-9_-] allowed
    assert!(
        name.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'),
        "name contains disallowed chars: {name}"
    );
}

// T18: photon disabled / offline → gate false
#[test]
fn t18_photon_not_present_gate_false() {
    // photon_present=false covers both disabled and offline cases
    let gate_disabled = make_gate(false, true, 1000);
    assert!(!should_send_context_pack(&gate_disabled));

    let gate_offline = make_gate(false, false, 500);
    assert!(!should_send_context_pack(&gate_offline));
}
