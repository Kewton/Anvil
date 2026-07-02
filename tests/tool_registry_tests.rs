use std::fs;

use anvil::modes::plan_act::{ExecutionMode, PlanStage};
use anvil::tools::registry::{ToolContext, ToolRegistry};
use anvil::util::workspace_paths::WorkspacePolicy;
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
        workspace_policy: WorkspacePolicy::default(),
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
fn protected_workspace_metadata_is_hidden_from_normal_discovery() {
    let dir = tempdir().unwrap();
    fs::write(
        dir.path().join("prompt.md"),
        "build from these instructions",
    )
    .unwrap();
    fs::write(dir.path().join("cmd.txt"), "anvil run").unwrap();
    fs::write(dir.path().join("anvil.out"), "runtime log").unwrap();
    fs::write(dir.path().join("eval.out"), "eval log").unwrap();
    fs::write(dir.path().join("runtime.log"), "runtime log").unwrap();
    fs::write(dir.path().join("sidecar.log"), "sidecar log").unwrap();
    fs::create_dir_all(dir.path().join(".anvil")).unwrap();
    fs::write(dir.path().join(".anvil/session.json"), "{}").unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(dir.path().join("src/main.rs"), "fn main() {}\n").unwrap();

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
        workspace_policy: WorkspacePolicy::default(),
    };

    let listing = registry
        .execute("Read", &json!({"path":"."}), &context)
        .unwrap();
    assert!(!listing.contains("prompt.md"), "got: {listing}");
    assert!(!listing.contains("cmd.txt"), "got: {listing}");
    assert!(!listing.contains("anvil.out"), "got: {listing}");
    assert!(!listing.contains("eval.out"), "got: {listing}");
    assert!(!listing.contains("runtime.log"), "got: {listing}");
    assert!(!listing.contains("sidecar.log"), "got: {listing}");
    assert!(!listing.contains(".anvil"), "got: {listing}");
    assert!(listing.contains("src"), "got: {listing}");

    let globbed = registry
        .execute("Glob", &json!({"pattern":"**/*"}), &context)
        .unwrap();
    assert!(!globbed.contains("prompt.md"), "got: {globbed}");
    assert!(!globbed.contains("cmd.txt"), "got: {globbed}");
    assert!(!globbed.contains("anvil.out"), "got: {globbed}");
    assert!(!globbed.contains("eval.out"), "got: {globbed}");
    assert!(!globbed.contains("runtime.log"), "got: {globbed}");
    assert!(!globbed.contains("sidecar.log"), "got: {globbed}");
    assert!(!globbed.contains(".anvil"), "got: {globbed}");
    assert!(globbed.contains("src/main.rs"), "got: {globbed}");

    let grep = registry
        .execute("Grep", &json!({"pattern":"runtime"}), &context)
        .unwrap();
    assert!(grep.is_empty(), "got: {grep}");
}

#[test]
fn normal_task_rejects_v0430_style_first_reads_of_prompt_and_cmd() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("prompt.md"), "user prompt").unwrap();
    fs::write(dir.path().join("cmd.txt"), "controller command").unwrap();
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
        workspace_policy: WorkspacePolicy::default(),
    };

    for path in ["prompt.md", "cmd.txt"] {
        let err = registry
            .execute("Read", &json!({"path": path}), &context)
            .unwrap_err();
        assert!(
            err.contains("protected workspace metadata rejected Read"),
            "got: {err}"
        );
    }
}

#[test]
fn explicit_log_analysis_policy_can_read_protected_metadata() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("anvil.out"), "runtime log").unwrap();
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
        workspace_policy: WorkspacePolicy::allow_protected_metadata_reads(),
    };

    let read = registry
        .execute("Read", &json!({"path":"anvil.out"}), &context)
        .unwrap();
    assert!(read.contains("runtime log"), "got: {read}");

    let globbed = registry
        .execute("Glob", &json!({"pattern":"anvil.out"}), &context)
        .unwrap();
    assert_eq!(globbed, "anvil.out");
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
        workspace_policy: WorkspacePolicy::default(),
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
        workspace_policy: WorkspacePolicy::default(),
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
        workspace_policy: WorkspacePolicy::default(),
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
        workspace_policy: WorkspacePolicy::default(),
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
        workspace_policy: WorkspacePolicy::default(),
    };

    let result = registry
        .execute("Bash", &json!({"command":"cargo test --help"}), &context)
        .unwrap();
    assert!(result.contains("exit_code=0"));
}
