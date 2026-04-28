use std::fs;

use anvil::modes::plan_act::{ExecutionMode, PlanStage};
use anvil::tools::registry::{ToolContext, ToolRegistry};
use serde_json::json;
use tempfile::tempdir;

#[test]
fn read_write_edit_glob_and_grep_work() {
    let dir = tempdir().unwrap();
    let registry = ToolRegistry::default();
    let context = ToolContext {
        root: dir.path().to_path_buf(),
        mode: ExecutionMode::Act,
        plan_path: None,
        plan_stage: PlanStage::Stage1,
        auto_approve: true,
        interactive_approval: false,
        offline: false,
        cancel_flag: None,
        tmp_tests_root: None,
        tester_active: false,
    };

    registry
        .execute(
            "Write",
            &json!({"path":"src/main.rs","content":"fn main() { println!(\"hi\"); }\n"}),
            &context,
        )
        .unwrap();
    let read = registry
        .execute("Read", &json!({"path":"src/main.rs"}), &context)
        .unwrap();
    assert!(read.contains("println!"));

    registry
        .execute(
            "Edit",
            &json!({"path":"src/main.rs","old_string":"hi","new_string":"bye"}),
            &context,
        )
        .unwrap();
    assert!(
        fs::read_to_string(dir.path().join("src/main.rs"))
            .unwrap()
            .contains("bye")
    );

    let globbed = registry
        .execute("Glob", &json!({"pattern":"src/**/*.rs"}), &context)
        .unwrap();
    assert!(globbed.contains("src/main.rs"));

    let grep = registry
        .execute("Grep", &json!({"pattern":"bye"}), &context)
        .unwrap();
    assert!(grep.contains("src/main.rs:1"));
}

#[test]
fn edit_tool_salvages_token_anchor_drift() {
    let dir = tempdir().unwrap();
    let registry = ToolRegistry::default();
    let context = ToolContext {
        root: dir.path().to_path_buf(),
        mode: ExecutionMode::Act,
        plan_path: None,
        plan_stage: PlanStage::Stage1,
        auto_approve: true,
        interactive_approval: false,
        offline: false,
        cancel_flag: None,
        tmp_tests_root: None,
        tester_active: false,
    };

    registry
        .execute(
            "Write",
            &json!({"path":"src/main.rs","content":"fn main() {\n    let my_special_variable = compute_result(42);\n}\n"}),
            &context,
        )
        .unwrap();

    let result = registry
        .execute(
            "Edit",
            &json!({
                "path":"src/main.rs",
                "old_string":"let my_special_variable = compute_result(input_value);",
                "new_string":"let my_special_variable = compute_result(7);"
            }),
            &context,
        )
        .unwrap();

    assert!(result.contains("token-anchor fallback"));
    let updated = fs::read_to_string(dir.path().join("src/main.rs")).unwrap();
    assert!(updated.contains("compute_result(7)"));
}

#[test]
fn plan_mode_only_allows_plan_file_writes() {
    let dir = tempdir().unwrap();
    let plan_path = dir.path().join(".anvil/plans/plan.md");
    std::fs::create_dir_all(plan_path.parent().unwrap()).unwrap();

    let registry = ToolRegistry::default();
    let context = ToolContext {
        root: dir.path().to_path_buf(),
        mode: ExecutionMode::Plan,
        plan_path: Some(plan_path.clone()),
        plan_stage: PlanStage::Stage1,
        auto_approve: true,
        interactive_approval: false,
        offline: false,
        cancel_flag: None,
        tmp_tests_root: None,
        tester_active: false,
    };

    registry
        .execute(
            "Write",
            &json!({"path": plan_path.display().to_string(), "content":"# Plan\n\n## Goal\n- keep writes confined"}),
            &context,
        )
        .unwrap();
    let err = registry
        .execute(
            "Write",
            &json!({"path":"src/lib.rs","content":"oops"}),
            &context,
        )
        .unwrap_err();
    assert!(err.contains("plan file"));
}

#[test]
fn plan_mode_allows_plan_file_outside_workspace() {
    let workspace = tempdir().unwrap();
    let state_root = tempdir().unwrap();
    let plan_path = state_root
        .path()
        .join("sessions/some-session/plans/plan-1.md");
    std::fs::create_dir_all(plan_path.parent().unwrap()).unwrap();

    let registry = ToolRegistry::default();
    let context = ToolContext {
        root: workspace.path().to_path_buf(),
        mode: ExecutionMode::Plan,
        plan_path: Some(plan_path.clone()),
        plan_stage: PlanStage::Stage1,
        auto_approve: true,
        interactive_approval: false,
        offline: false,
        cancel_flag: None,
        tmp_tests_root: None,
        tester_active: false,
    };

    registry
        .execute(
            "Write",
            &json!({"path": plan_path.display().to_string(), "content":"# Plan\n\n## Goal\n- outside plan"}),
            &context,
        )
        .unwrap();
    let contents = fs::read_to_string(&plan_path).unwrap();
    assert!(contents.contains("outside plan"));

    let err = registry
        .execute(
            "Write",
            &json!({"path":"src/lib.rs","content":"oops"}),
            &context,
        )
        .unwrap_err();
    assert!(err.contains("plan file"));
}

#[test]
fn offline_mode_blocks_network_bash_commands() {
    let dir = tempdir().unwrap();
    let registry = ToolRegistry::default();
    let context = ToolContext {
        root: dir.path().to_path_buf(),
        mode: ExecutionMode::Act,
        plan_path: None,
        plan_stage: PlanStage::Stage1,
        auto_approve: true,
        interactive_approval: false,
        offline: true,
        cancel_flag: None,
        tmp_tests_root: None,
        tester_active: false,
    };

    let err = registry
        .execute(
            "Bash",
            &json!({"command":"curl -I https://example.com"}),
            &context,
        )
        .unwrap_err();
    assert!(err.contains("offline mode blocks network shell commands"));
}

#[test]
fn offline_mode_allows_build_test_bash_commands() {
    let dir = tempdir().unwrap();
    let registry = ToolRegistry::default();
    let context = ToolContext {
        root: dir.path().to_path_buf(),
        mode: ExecutionMode::Act,
        plan_path: None,
        plan_stage: PlanStage::Stage1,
        auto_approve: true,
        interactive_approval: false,
        offline: true,
        cancel_flag: None,
        tmp_tests_root: None,
        tester_active: false,
    };

    let result = registry
        .execute("Bash", &json!({"command":"cargo test --help"}), &context)
        .unwrap();
    assert!(result.contains("exit_code=0"));
}
