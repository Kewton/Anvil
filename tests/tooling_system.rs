use anvil::tooling::{
    ExecutionClass, LocalToolExecutor, ParallelExecutionPlan, ParallelExecutionPlanError,
    PermissionClass, PlanModePolicy, RollbackPolicy, ToolCallRequest, ToolExecutionError,
    ToolExecutionPayload, ToolExecutionPolicy, ToolExecutionRequest, ToolExecutionResult,
    ToolExecutionStatus, ToolInput, ToolKind, ToolRegistry, ToolValidationError,
    effective_permission_class, is_safe_shell_command,
};
use std::fs;

fn build_registry() -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    registry.register_standard_tools();
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

    assert_eq!(read_spec.version, 1);
    assert_eq!(read_spec.execution_class, ExecutionClass::ReadOnly);
    assert_eq!(read_spec.permission_class, PermissionClass::Safe);
    assert_eq!(read_spec.plan_mode, PlanModePolicy::Allowed);
    assert_eq!(write_spec.version, 1);
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

    let plan = ParallelExecutionPlan::build(
        vec![read_a.clone(), read_b.clone()],
        ToolExecutionPolicy::default(),
    )
    .expect("parallel-safe approved calls should build");
    let denied_for_permission = ParallelExecutionPlan::build(
        vec![read_a.clone(), write.clone()],
        ToolExecutionPolicy::default(),
    )
    .expect_err("confirm tool should not enter parallel plan before individual approval");
    let denied_for_mode = ParallelExecutionPlan::build(
        vec![read_a, write.approve()],
        ToolExecutionPolicy::default(),
    )
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
        .into_execution_request(ToolExecutionPolicy::default())
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

    // shell.exec is Confirm class: requires approval in default policy
    let err = shell
        .into_execution_request(ToolExecutionPolicy::default())
        .expect_err("confirm tool should require approval");

    assert_eq!(
        err,
        ToolExecutionError::ApprovalRequired("call_shell_001".to_string())
    );
}

#[test]
fn restricted_tool_can_be_allowed_by_explicit_policy_override() {
    let registry = build_registry();
    let shell = registry
        .validate(ToolCallRequest::new(
            "call_shell_001",
            "shell.exec",
            ToolInput::ShellExec {
                command: "git status".to_string(),
            },
        ))
        .expect("shell should validate")
        .approve();

    let execution = shell
        .into_execution_request(ToolExecutionPolicy {
            allow_restricted: true,
            ..ToolExecutionPolicy::default()
        })
        .expect("policy should allow restricted tool");

    assert_eq!(execution.spec.kind, ToolKind::ShellExec);
}

#[test]
fn plan_mode_policy_is_enforced_for_blocked_and_scoped_tools() {
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
        .expect("write should validate")
        .approve();
    let shell = registry
        .validate(ToolCallRequest::new(
            "call_shell_001",
            "shell.exec",
            ToolInput::ShellExec {
                command: "git status".to_string(),
            },
        ))
        .expect("shell should validate")
        .approve();

    let write_err = write
        .clone()
        .into_execution_request(ToolExecutionPolicy {
            plan_mode: true,
            allow_restricted: false,
            plan_scope_granted: false,
            approval_required: true,
        })
        .expect_err("scoped plan-mode tool should require explicit scope");
    // shell.exec is AllowedWithScope: should require scope in plan mode
    let shell_err = shell
        .clone()
        .into_execution_request(ToolExecutionPolicy {
            plan_mode: true,
            allow_restricted: false,
            plan_scope_granted: false,
            approval_required: true,
        })
        .expect_err("shell in plan-mode without scope should be denied");
    let _shell_ok = shell
        .into_execution_request(ToolExecutionPolicy {
            plan_mode: true,
            allow_restricted: false,
            plan_scope_granted: true,
            approval_required: false,
        })
        .expect("shell in plan-mode with scope should pass");
    let write_ok = write
        .into_execution_request(ToolExecutionPolicy {
            plan_mode: true,
            allow_restricted: false,
            plan_scope_granted: true,
            approval_required: true,
        })
        .expect("scoped plan-mode tool should pass when scope is granted");

    assert_eq!(
        write_err,
        ToolExecutionError::PlanModeScopeRequired("file.write".to_string())
    );
    assert_eq!(
        shell_err,
        ToolExecutionError::PlanModeScopeRequired("shell.exec".to_string())
    );
    assert_eq!(write_ok.spec.kind, ToolKind::FileWrite);
}

#[test]
fn validation_reports_missing_required_field_details() {
    let registry = build_registry();
    let missing_path = registry
        .validate(ToolCallRequest::new(
            "call_read_001",
            "file.read",
            ToolInput::FileRead {
                path: "".to_string(),
            },
        ))
        .expect_err("empty path should be rejected");
    let missing_pattern = registry
        .validate(ToolCallRequest::new(
            "call_search_001",
            "file.search",
            ToolInput::FileSearch {
                root: "src".to_string(),
                pattern: " ".to_string(),
            },
        ))
        .expect_err("empty pattern should be rejected");

    assert_eq!(
        missing_path,
        ToolValidationError::MissingRequiredField("path".to_string())
    );
    assert_eq!(
        missing_pattern,
        ToolValidationError::MissingRequiredField("pattern".to_string())
    );
}

#[test]
fn tool_execution_result_can_bridge_into_console_tool_log_view() {
    let result = ToolExecutionResult {
        tool_call_id: "call_read_001".to_string(),
        tool_name: "file.read".to_string(),
        status: ToolExecutionStatus::Completed,
        summary: "Read src/app/mod.rs".to_string(),
        payload: ToolExecutionPayload::Text("mod app".to_string()),
        artifacts: vec!["src/app/mod.rs".to_string()],
        elapsed_ms: 12,
    };
    let log = result.to_tool_log_view();

    assert_eq!(log.tool_name, "file.read");
    assert_eq!(log.action, "completed");
    assert_eq!(log.target, "Read src/app/mod.rs");
}

#[test]
fn local_tool_executor_reads_directory_as_listing() {
    let root = std::env::temp_dir().join("anvil_tool_executor_dir_listing");
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("sandbox/test1_001")).expect("dir should exist");
    fs::write(root.join("sandbox/test1_001/index.html"), "<html></html>").expect("file exists");
    let mut executor = LocalToolExecutor::new_without_rate_limit(root.clone());

    let result = executor
        .execute(ToolExecutionRequest {
            tool_call_id: "call_dir_read_001".to_string(),
            spec: build_registry()
                .get("file.read")
                .expect("file.read spec")
                .clone(),
            input: ToolInput::FileRead {
                path: "./sandbox/test1_001".to_string(),
            },
        })
        .expect("directory listing should succeed");

    match result.payload {
        ToolExecutionPayload::Text(listing) => {
            assert!(listing.contains("index.html"));
        }
        other => panic!("unexpected payload: {other:?}"),
    }
}

// --- web.fetch tests ---

#[test]
fn web_fetch_is_registered_after_register_standard_tools() {
    let mut registry = ToolRegistry::new();
    registry.register_standard_tools();
    let spec = registry
        .get("web.fetch")
        .expect("web.fetch should be registered");
    assert_eq!(spec.kind, ToolKind::WebFetch);
    assert_eq!(spec.execution_class, ExecutionClass::Network);
    assert_eq!(spec.permission_class, PermissionClass::Safe);
}

#[test]
fn web_fetch_input_maps_to_web_fetch_kind() {
    let input = ToolInput::WebFetch {
        url: "https://example.com".to_string(),
    };
    assert_eq!(input.kind(), ToolKind::WebFetch);
}

#[test]
fn web_fetch_validation_rejects_empty_url() {
    let registry = build_registry();
    let err = registry
        .validate(ToolCallRequest::new(
            "call_fetch_001",
            "web.fetch",
            ToolInput::WebFetch {
                url: "".to_string(),
            },
        ))
        .expect_err("empty URL should be rejected");
    assert_eq!(
        err,
        ToolValidationError::MissingRequiredField("url".to_string())
    );
}

#[test]
fn web_fetch_validation_accepts_http_url() {
    let registry = build_registry();
    let result = registry.validate(ToolCallRequest::new(
        "call_fetch_002",
        "web.fetch",
        ToolInput::WebFetch {
            url: "http://example.com".to_string(),
        },
    ));
    assert!(result.is_ok(), "http:// URL should be accepted");
}

#[test]
fn web_fetch_validation_accepts_https_url() {
    let registry = build_registry();
    let result = registry.validate(ToolCallRequest::new(
        "call_fetch_003",
        "web.fetch",
        ToolInput::WebFetch {
            url: "https://example.com".to_string(),
        },
    ));
    assert!(result.is_ok(), "https:// URL should be accepted");
}

#[test]
fn web_fetch_validation_rejects_file_scheme() {
    let registry = build_registry();
    let err = registry
        .validate(ToolCallRequest::new(
            "call_fetch_004",
            "web.fetch",
            ToolInput::WebFetch {
                url: "file:///etc/passwd".to_string(),
            },
        ))
        .expect_err("file:// URL should be rejected");
    assert_eq!(
        err,
        ToolValidationError::InvalidFieldValue {
            field: "url".to_string(),
            reason: "must start with http:// or https://".to_string(),
        }
    );
}

#[test]
fn web_fetch_validation_rejects_ftp_scheme() {
    let registry = build_registry();
    let err = registry
        .validate(ToolCallRequest::new(
            "call_fetch_005",
            "web.fetch",
            ToolInput::WebFetch {
                url: "ftp://example.com/file".to_string(),
            },
        ))
        .expect_err("ftp:// URL should be rejected");
    assert_eq!(
        err,
        ToolValidationError::InvalidFieldValue {
            field: "url".to_string(),
            reason: "must start with http:// or https://".to_string(),
        }
    );
}

#[test]
fn web_fetch_serde_round_trip() {
    let input = ToolInput::WebFetch {
        url: "https://example.com/page".to_string(),
    };
    let json = serde_json::to_string(&input).expect("serialize should succeed");
    let deserialized: ToolInput = serde_json::from_str(&json).expect("deserialize should succeed");
    assert_eq!(input, deserialized);
}

#[test]
fn web_fetch_spec_has_correct_policies() {
    let registry = build_registry();
    let spec = registry.get("web.fetch").expect("web.fetch should exist");
    assert_eq!(spec.version, 1);
    assert_eq!(spec.permission_class, PermissionClass::Safe);
    assert_eq!(spec.execution_class, ExecutionClass::Network);
    assert_eq!(
        spec.execution_mode,
        anvil::tooling::ExecutionMode::ParallelSafe
    );
    assert_eq!(spec.plan_mode, PlanModePolicy::Allowed);
    assert_eq!(spec.rollback_policy, RollbackPolicy::None);
}

// --- web.search tests ---

#[test]
fn web_search_is_registered_after_register_standard_tools() {
    let mut registry = ToolRegistry::new();
    registry.register_standard_tools();
    let spec = registry
        .get("web.search")
        .expect("web.search should be registered");
    assert_eq!(spec.kind, ToolKind::WebSearch);
    assert_eq!(spec.execution_class, ExecutionClass::Network);
    assert_eq!(spec.permission_class, PermissionClass::Confirm);
    assert_eq!(
        spec.execution_mode,
        anvil::tooling::ExecutionMode::SequentialOnly
    );
    assert_eq!(spec.plan_mode, PlanModePolicy::Allowed);
    assert_eq!(spec.rollback_policy, RollbackPolicy::None);
}

#[test]
fn web_search_input_maps_to_web_search_kind() {
    let input = ToolInput::WebSearch {
        query: "rust error handling".to_string(),
    };
    assert_eq!(input.kind(), ToolKind::WebSearch);
}

#[test]
fn web_search_validation_rejects_empty_query() {
    let registry = build_registry();
    let err = registry
        .validate(ToolCallRequest::new(
            "call_search_001",
            "web.search",
            ToolInput::WebSearch {
                query: "".to_string(),
            },
        ))
        .expect_err("empty query should be rejected");
    assert_eq!(
        err,
        ToolValidationError::MissingRequiredField("query".to_string())
    );
}

#[test]
fn web_search_validation_rejects_whitespace_only_query() {
    let registry = build_registry();
    let err = registry
        .validate(ToolCallRequest::new(
            "call_search_002",
            "web.search",
            ToolInput::WebSearch {
                query: "   ".to_string(),
            },
        ))
        .expect_err("whitespace-only query should be rejected");
    assert_eq!(
        err,
        ToolValidationError::MissingRequiredField("query".to_string())
    );
}

#[test]
fn web_search_validation_rejects_query_exceeding_500_chars() {
    let registry = build_registry();
    let long_query = "a".repeat(501);
    let err = registry
        .validate(ToolCallRequest::new(
            "call_search_003",
            "web.search",
            ToolInput::WebSearch { query: long_query },
        ))
        .expect_err("query exceeding 500 characters should be rejected");
    match err {
        ToolValidationError::InvalidFieldValue { field, reason } => {
            assert_eq!(field, "query");
            assert!(reason.contains("501"));
            assert!(reason.contains("500"));
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[test]
fn web_search_validation_accepts_query_at_500_chars() {
    let registry = build_registry();
    let query = "a".repeat(500);
    let result = registry.validate(ToolCallRequest::new(
        "call_search_004",
        "web.search",
        ToolInput::WebSearch { query },
    ));
    assert!(
        result.is_ok(),
        "query at exactly 500 characters should be accepted"
    );
}

#[test]
fn web_search_validation_accepts_normal_query() {
    let registry = build_registry();
    let result = registry.validate(ToolCallRequest::new(
        "call_search_005",
        "web.search",
        ToolInput::WebSearch {
            query: "rust error handling best practices".to_string(),
        },
    ));
    assert!(result.is_ok(), "normal query should be accepted");
}

#[test]
fn web_search_serde_round_trip() {
    let input = ToolInput::WebSearch {
        query: "rust serde tutorial".to_string(),
    };
    let json = serde_json::to_string(&input).expect("serialize should succeed");
    let deserialized: ToolInput = serde_json::from_str(&json).expect("deserialize should succeed");
    assert_eq!(input, deserialized);
}

// --- is_safe_shell_command tests (23 cases) ---

#[test]
fn safe_shell_01_gh_api_get_is_safe() {
    assert!(is_safe_shell_command("gh api repos/o/r/stats/contributors"));
}

#[test]
fn safe_shell_02_gh_api_method_post_is_unsafe() {
    assert!(!is_safe_shell_command(
        "gh api --method POST repos/o/r/issues"
    ));
}

#[test]
fn safe_shell_03_gh_api_method_eq_post_is_unsafe() {
    assert!(!is_safe_shell_command(
        "gh api --method=POST repos/o/r/issues"
    ));
}

#[test]
fn safe_shell_04_gh_api_x_delete_is_unsafe() {
    assert!(!is_safe_shell_command(
        "gh api -X DELETE repos/o/r/issues/1"
    ));
}

#[test]
fn safe_shell_05_gh_api_xdelete_combined_is_unsafe() {
    assert!(!is_safe_shell_command("gh api -XDELETE repos/o/r/issues/1"));
}

#[test]
fn safe_shell_06_gh_api_with_pipe_is_unsafe() {
    assert!(!is_safe_shell_command("gh api repos/o/r/issues | jq ."));
}

#[test]
fn safe_shell_07_gh_api_with_semicolon_is_unsafe() {
    assert!(!is_safe_shell_command("gh api repos/o/r/issues; rm -rf /"));
}

#[test]
fn safe_shell_08_git_log_is_safe() {
    assert!(is_safe_shell_command("git log --oneline"));
}

#[test]
fn safe_shell_09_git_status_is_safe() {
    assert!(is_safe_shell_command("git status"));
}

#[test]
fn safe_shell_10_curl_is_not_safe() {
    assert!(!is_safe_shell_command("curl https://example.com"));
}

#[test]
fn safe_shell_11_gh_api_input_flag_is_unsafe() {
    assert!(!is_safe_shell_command(
        "gh api --input data.json repos/o/r/issues"
    ));
}

#[test]
fn safe_shell_12_gh_api_input_eq_is_unsafe() {
    assert!(!is_safe_shell_command(
        "gh api --input=data.json repos/o/r/issues"
    ));
}

#[test]
fn safe_shell_13_gh_api_xput_is_unsafe() {
    assert!(!is_safe_shell_command("gh api -XPUT repos/o/r/topics"));
}

#[test]
fn safe_shell_14_gh_api_xpatch_is_unsafe() {
    assert!(!is_safe_shell_command("gh api -XPATCH repos/o/r/issues/1"));
}

#[test]
fn safe_shell_15_gh_api_url_with_method_in_path_is_safe() {
    // Token-based splitting prevents false positive on URLs containing "--method-POST"
    assert!(is_safe_shell_command(
        "gh api repos/o/repo-with--method-POST/stats"
    ));
}

#[test]
fn safe_shell_16_gh_api_newline_bypass_is_unsafe() {
    assert!(!is_safe_shell_command("gh api repos/o/r/issues\nrm -rf /"));
}

#[test]
fn safe_shell_17_gh_api_f_flag_implicit_post_is_unsafe() {
    assert!(!is_safe_shell_command(
        "gh api -f title=hacked repos/o/r/issues"
    ));
}

#[test]
fn safe_shell_18_gh_api_field_flag_implicit_post_is_unsafe() {
    assert!(!is_safe_shell_command(
        "gh api --field title=hacked repos/o/r/issues"
    ));
}

#[test]
fn safe_shell_19_gh_api_f_uppercase_flag_implicit_post_is_unsafe() {
    assert!(!is_safe_shell_command(
        "gh api -F body=@file.txt repos/o/r/issues"
    ));
}

#[test]
fn safe_shell_20_gh_api_raw_field_implicit_post_is_unsafe() {
    assert!(!is_safe_shell_command(
        "gh api --raw-field body=test repos/o/r/issues"
    ));
}

#[test]
fn safe_shell_21_gh_repo_view_web_is_unsafe() {
    assert!(!is_safe_shell_command("gh repo view --web"));
}

#[test]
fn safe_shell_22_gh_repo_view_json_is_safe() {
    assert!(is_safe_shell_command("gh repo view --json owner,name"));
}

#[test]
fn safe_shell_23_gh_issue_list_browse_is_unsafe() {
    assert!(!is_safe_shell_command("gh issue list --browse"));
}

// --- effective_permission_class tests ---

#[test]
fn effective_permission_class_promotes_safe_shell_to_safe() {
    let registry = build_registry();
    let spec = registry.get("shell.exec").expect("shell.exec should exist");
    let input = ToolInput::ShellExec {
        command: "git status".to_string(),
    };
    assert_eq!(
        effective_permission_class(&input, spec),
        PermissionClass::Safe
    );
}

#[test]
fn effective_permission_class_keeps_unsafe_shell_as_confirm() {
    let registry = build_registry();
    let spec = registry.get("shell.exec").expect("shell.exec should exist");
    let input = ToolInput::ShellExec {
        command: "rm -rf target".to_string(),
    };
    assert_eq!(
        effective_permission_class(&input, spec),
        PermissionClass::Confirm
    );
}

#[test]
fn effective_permission_class_web_search_stays_confirm() {
    let registry = build_registry();
    let spec = registry.get("web.search").expect("web.search should exist");
    let input = ToolInput::WebSearch {
        query: "rust error".to_string(),
    };
    assert_eq!(
        effective_permission_class(&input, spec),
        PermissionClass::Confirm
    );
}

#[test]
fn effective_permission_class_file_read_stays_safe() {
    let registry = build_registry();
    let spec = registry.get("file.read").expect("file.read should exist");
    let input = ToolInput::FileRead {
        path: "src/main.rs".to_string(),
    };
    assert_eq!(
        effective_permission_class(&input, spec),
        PermissionClass::Safe
    );
}

// --- approval_required uses effective_permission_class ---

#[test]
fn approval_not_required_for_safe_shell_command() {
    let registry = build_registry();
    let validated = registry
        .validate(ToolCallRequest::new(
            "call_shell_safe",
            "shell.exec",
            ToolInput::ShellExec {
                command: "git status".to_string(),
            },
        ))
        .expect("should validate");
    assert!(
        validated.approval_required(true).is_none(),
        "safe shell commands should not require approval"
    );
}

#[test]
fn approval_required_for_unsafe_shell_command() {
    let registry = build_registry();
    let validated = registry
        .validate(ToolCallRequest::new(
            "call_shell_unsafe",
            "shell.exec",
            ToolInput::ShellExec {
                command: "curl https://example.com".to_string(),
            },
        ))
        .expect("should validate");
    assert!(
        validated.approval_required(true).is_some(),
        "unsafe shell commands should require approval"
    );
}
