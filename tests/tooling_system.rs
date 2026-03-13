use anvil::tooling::{
    ExecutionClass, ParallelExecutionPlan, ParallelExecutionPlanError, PermissionClass,
    PlanModePolicy, RollbackPolicy, ToolCallRequest, ToolExecutionError, ToolExecutionPayload,
    ToolExecutionResult, ToolExecutionStatus, ToolInput, ToolKind, ToolRegistry,
    ToolValidationError,
};

fn build_registry() -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    registry.register_file_read();
    registry.register_file_write();
    registry.register_file_search();
    registry.register_shell_exec();
    registry
}

#[test]
fn registry_validates_typed_tool_input_against_registered_tool() {
    let registry = build_registry();
    let valid = ToolCallRequest::new(
        "call_read_001",
        "file.read",
        ToolInput::FileRead {
            path: "src/app/mod.rs".to_string(),
        },
    );
    let invalid = ToolCallRequest::new(
        "call_read_002",
        "file.read",
        ToolInput::FileWrite {
            path: "src/app/mod.rs".to_string(),
            content: "oops".to_string(),
        },
    );

    let validated = registry
        .validate(valid)
        .expect("matching typed input should validate");
    let err = registry
        .validate(invalid)
        .expect_err("mismatched typed input should fail");

    assert_eq!(validated.spec.name, "file.read");
    assert_eq!(validated.spec.kind, ToolKind::FileRead);
    assert_eq!(err, ToolValidationError::InputKindMismatch);
}

#[test]
fn registry_exposes_explicit_execution_and_permission_policy() {
    let registry = build_registry();
    let read_spec = registry.get("file.read").expect("read spec should exist");
    let write_spec = registry.get("file.write").expect("write spec should exist");

    assert_eq!(read_spec.execution_class, ExecutionClass::ReadOnly);
    assert_eq!(read_spec.permission_class, PermissionClass::Safe);
    assert_eq!(read_spec.plan_mode, PlanModePolicy::Allowed);
    assert_eq!(write_spec.execution_class, ExecutionClass::Mutating);
    assert_eq!(write_spec.permission_class, PermissionClass::Confirm);
    assert_eq!(write_spec.plan_mode, PlanModePolicy::AllowedWithScope);
}

#[test]
fn permission_flow_requires_per_tool_approval_for_confirm_tools() {
    let registry = build_registry();
    let read = registry
        .validate(ToolCallRequest::new(
            "call_read_001",
            "file.read",
            ToolInput::FileRead {
                path: "src/app/mod.rs".to_string(),
            },
        ))
        .expect("read should validate");
    let write = registry
        .validate(ToolCallRequest::new(
            "call_write_001",
            "file.write",
            ToolInput::FileWrite {
                path: "src/app/mod.rs".to_string(),
                content: "updated".to_string(),
            },
        ))
        .expect("write should validate");

    assert!(read.approval_required(true).is_none());
    assert_eq!(
        write
            .approval_required(true)
            .expect("write should require approval")
            .tool_call_id,
        "call_write_001"
    );
    assert!(write.approval_required(false).is_none());
}

#[test]
fn parallel_execution_plan_only_accepts_parallel_safe_and_approved_calls() {
    let registry = build_registry();
    let read_a = registry
        .validate(ToolCallRequest::new(
            "call_read_001",
            "file.read",
            ToolInput::FileRead {
                path: "src/app/mod.rs".to_string(),
            },
        ))
        .expect("read should validate");
    let read_b = registry
        .validate(ToolCallRequest::new(
            "call_search_001",
            "file.search",
            ToolInput::FileSearch {
                root: "src".to_string(),
                pattern: "ProviderClient".to_string(),
            },
        ))
        .expect("search should validate");
    let write = registry
        .validate(ToolCallRequest::new(
            "call_write_001",
            "file.write",
            ToolInput::FileWrite {
                path: "src/app/mod.rs".to_string(),
                content: "updated".to_string(),
            },
        ))
        .expect("write should validate");

    let plan = ParallelExecutionPlan::build(vec![read_a.clone(), read_b.clone()], true)
        .expect("parallel-safe approved calls should build");
    let denied_for_permission =
        ParallelExecutionPlan::build(vec![read_a.clone(), write.clone()], true)
            .expect_err("confirm tool should not enter parallel plan before individual approval");
    let denied_for_mode = ParallelExecutionPlan::build(vec![read_a, write.approve()], true)
        .expect_err("sequential-only tool should not enter parallel-safe batch");

    assert_eq!(plan.calls.len(), 2);
    assert_eq!(
        denied_for_permission,
        ParallelExecutionPlanError::ApprovalRequired("call_write_001".to_string())
    );
    assert_eq!(
        denied_for_mode,
        ParallelExecutionPlanError::SequentialOnly("file.write".to_string())
    );
}

#[test]
fn mutating_tool_carries_rollback_checkpoint_policy() {
    let registry = build_registry();
    let write = registry
        .validate(ToolCallRequest::new(
            "call_write_001",
            "file.write",
            ToolInput::FileWrite {
                path: "src/app/mod.rs".to_string(),
                content: "updated".to_string(),
            },
        ))
        .expect("write should validate");

    assert_eq!(
        write.spec.rollback_policy,
        RollbackPolicy::CheckpointBeforeWrite
    );
}

#[test]
fn validated_tool_call_builds_typed_execution_request_and_result() {
    let registry = build_registry();
    let read = registry
        .validate(ToolCallRequest::new(
            "call_read_001",
            "file.read",
            ToolInput::FileRead {
                path: "src/app/mod.rs".to_string(),
            },
        ))
        .expect("read should validate");
    let execution = read
        .into_execution_request(true)
        .expect("safe tool should become execution request");
    let result = ToolExecutionResult {
        tool_call_id: execution.tool_call_id.clone(),
        tool_name: execution.spec.name.clone(),
        status: ToolExecutionStatus::Completed,
        summary: "Read src/app/mod.rs".to_string(),
        payload: ToolExecutionPayload::Text("mod app".to_string()),
        artifacts: vec!["src/app/mod.rs".to_string()],
        elapsed_ms: 12,
    };

    assert_eq!(execution.spec.kind, ToolKind::FileRead);
    assert_eq!(result.status, ToolExecutionStatus::Completed);
    assert_eq!(result.artifacts, vec!["src/app/mod.rs".to_string()]);
}

#[test]
fn restricted_tool_is_blocked_before_execution() {
    let registry = build_registry();
    let shell = registry
        .validate(ToolCallRequest::new(
            "call_shell_001",
            "shell.exec",
            ToolInput::ShellExec {
                command: "rm -rf target".to_string(),
            },
        ))
        .expect("shell should validate");

    let err = shell
        .into_execution_request(true)
        .expect_err("restricted tool should be blocked");

    assert_eq!(
        err,
        ToolExecutionError::RestrictedTool("shell.exec".to_string())
    );
}
