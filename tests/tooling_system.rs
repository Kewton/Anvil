use anvil::app::agentic::{ExecutionGroup, group_by_execution_mode};
use anvil::tooling::{
    ExecutionClass, ExecutionMode, LocalToolExecutor, ParallelExecutionPlan,
    ParallelExecutionPlanError, PermissionClass, PlanModePolicy, RollbackPolicy, ToolCallRequest,
    ToolExecutionError, ToolExecutionPayload, ToolExecutionPolicy, ToolExecutionRequest,
    ToolExecutionResult, ToolExecutionStatus, ToolInput, ToolKind, ToolRegistry,
    ToolValidationError,
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

// --- is_safe_shell_command: gh api tests ---

mod safe_shell_gh_api {
    use anvil::tooling::is_safe_shell_command;

    #[test]
    fn get_is_safe() {
        assert!(is_safe_shell_command("gh api repos/o/r/stats/contributors"));
    }

    #[test]
    fn method_post_is_unsafe() {
        assert!(!is_safe_shell_command(
            "gh api --method POST repos/o/r/issues"
        ));
    }

    #[test]
    fn method_eq_post_is_unsafe() {
        assert!(!is_safe_shell_command(
            "gh api --method=POST repos/o/r/issues"
        ));
    }

    #[test]
    fn x_delete_is_unsafe() {
        assert!(!is_safe_shell_command(
            "gh api -X DELETE repos/o/r/issues/1"
        ));
    }

    #[test]
    fn xdelete_combined_is_unsafe() {
        assert!(!is_safe_shell_command("gh api -XDELETE repos/o/r/issues/1"));
    }

    #[test]
    fn pipe_is_unsafe() {
        assert!(!is_safe_shell_command("gh api repos/o/r/issues | jq ."));
    }

    #[test]
    fn semicolon_is_unsafe() {
        assert!(!is_safe_shell_command("gh api repos/o/r/issues; rm -rf /"));
    }

    #[test]
    fn input_flag_is_unsafe() {
        assert!(!is_safe_shell_command(
            "gh api --input data.json repos/o/r/issues"
        ));
    }

    #[test]
    fn input_eq_is_unsafe() {
        assert!(!is_safe_shell_command(
            "gh api --input=data.json repos/o/r/issues"
        ));
    }

    #[test]
    fn xput_is_unsafe() {
        assert!(!is_safe_shell_command("gh api -XPUT repos/o/r/topics"));
    }

    #[test]
    fn xpatch_is_unsafe() {
        assert!(!is_safe_shell_command("gh api -XPATCH repos/o/r/issues/1"));
    }

    #[test]
    fn url_with_method_in_path_is_safe() {
        // Token-based splitting prevents false positive on URLs containing "--method-POST"
        assert!(is_safe_shell_command(
            "gh api repos/o/repo-with--method-POST/stats"
        ));
    }

    #[test]
    fn newline_bypass_is_unsafe() {
        assert!(!is_safe_shell_command("gh api repos/o/r/issues\nrm -rf /"));
    }

    #[test]
    fn f_flag_implicit_post_is_unsafe() {
        assert!(!is_safe_shell_command(
            "gh api -f title=hacked repos/o/r/issues"
        ));
    }

    #[test]
    fn field_flag_implicit_post_is_unsafe() {
        assert!(!is_safe_shell_command(
            "gh api --field title=hacked repos/o/r/issues"
        ));
    }

    #[test]
    fn f_uppercase_flag_implicit_post_is_unsafe() {
        assert!(!is_safe_shell_command(
            "gh api -F body=@file.txt repos/o/r/issues"
        ));
    }

    #[test]
    fn raw_field_implicit_post_is_unsafe() {
        assert!(!is_safe_shell_command(
            "gh api --raw-field body=test repos/o/r/issues"
        ));
    }
}

// --- is_safe_shell_command: gh CLI / git / misc tests ---

mod safe_shell_prefixes {
    use anvil::tooling::is_safe_shell_command;

    #[test]
    fn git_log_is_safe() {
        assert!(is_safe_shell_command("git log --oneline"));
    }

    #[test]
    fn git_status_is_safe() {
        assert!(is_safe_shell_command("git status"));
    }

    #[test]
    fn curl_is_not_safe() {
        assert!(!is_safe_shell_command("curl https://example.com"));
    }

    #[test]
    fn gh_repo_view_web_is_unsafe() {
        assert!(!is_safe_shell_command("gh repo view --web"));
    }

    #[test]
    fn gh_repo_view_json_is_safe() {
        assert!(is_safe_shell_command("gh repo view --json owner,name"));
    }

    #[test]
    fn gh_issue_list_browse_is_unsafe() {
        assert!(!is_safe_shell_command("gh issue list --browse"));
    }

    #[test]
    fn cargo_build_is_safe() {
        assert!(is_safe_shell_command("cargo build"));
    }

    #[test]
    fn cargo_test_is_safe() {
        assert!(is_safe_shell_command("cargo test"));
    }

    #[test]
    fn cargo_clippy_is_safe() {
        assert!(is_safe_shell_command("cargo clippy --all-targets"));
    }

    #[test]
    fn cargo_fmt_check_is_safe() {
        assert!(is_safe_shell_command("cargo fmt --check"));
    }

    #[test]
    fn cargo_check_is_safe() {
        assert!(is_safe_shell_command("cargo check"));
    }

    #[test]
    fn npm_test_is_safe() {
        assert!(is_safe_shell_command("npm test"));
    }

    #[test]
    fn npx_jest_with_args_is_safe() {
        assert!(is_safe_shell_command("npx jest src/tests"));
    }

    #[test]
    fn npx_eslint_with_args_is_safe() {
        assert!(is_safe_shell_command("npx eslint src/"));
    }

    #[test]
    fn npx_prettier_check_is_safe() {
        assert!(is_safe_shell_command("npx prettier --check src/"));
    }

    #[test]
    fn git_branch_is_safe() {
        assert!(is_safe_shell_command("git branch"));
    }

    #[test]
    fn git_show_with_ref_is_safe() {
        assert!(is_safe_shell_command("git show HEAD"));
    }

    #[test]
    fn git_show_alone_is_not_safe() {
        // "git show" without trailing space won't match "git show "
        assert!(!is_safe_shell_command("git show"));
    }

    #[test]
    fn git_remote_v_is_safe() {
        assert!(is_safe_shell_command("git remote -v"));
    }

    #[test]
    fn git_rev_parse_is_safe() {
        assert!(is_safe_shell_command("git rev-parse HEAD"));
    }

    #[test]
    fn gh_pr_view_is_safe() {
        assert!(is_safe_shell_command("gh pr view 123"));
    }

    #[test]
    fn gh_issue_view_is_safe() {
        assert!(is_safe_shell_command("gh issue view 456"));
    }

    #[test]
    fn gh_auth_status_is_safe() {
        assert!(is_safe_shell_command("gh auth status"));
    }

    #[test]
    fn which_is_safe() {
        assert!(is_safe_shell_command("which rustc"));
    }

    #[test]
    fn uname_is_safe() {
        assert!(is_safe_shell_command("uname"));
    }

    #[test]
    fn node_version_is_safe() {
        assert!(is_safe_shell_command("node -v"));
        assert!(is_safe_shell_command("node --version"));
    }

    #[test]
    fn rustc_version_is_safe() {
        assert!(is_safe_shell_command("rustc --version"));
    }

    #[test]
    fn cargo_version_is_safe() {
        assert!(is_safe_shell_command("cargo --version"));
    }

    #[test]
    fn python_version_is_safe() {
        assert!(is_safe_shell_command("python --version"));
    }

    #[test]
    fn go_version_is_safe() {
        assert!(is_safe_shell_command("go version"));
    }

    #[test]
    fn lsof_i_is_safe() {
        assert!(is_safe_shell_command("lsof -i"));
    }

    #[test]
    fn pytest_is_safe() {
        assert!(is_safe_shell_command("pytest"));
        assert!(is_safe_shell_command("pytest tests/"));
    }

    #[test]
    fn ruff_check_is_safe() {
        assert!(is_safe_shell_command("ruff check ."));
    }

    #[test]
    fn flake8_is_safe() {
        assert!(is_safe_shell_command("flake8"));
        assert!(is_safe_shell_command("flake8 src/"));
    }

    #[test]
    fn go_test_is_safe() {
        assert!(is_safe_shell_command("go test ./..."));
    }

    #[test]
    fn go_vet_is_safe() {
        assert!(is_safe_shell_command("go vet ./..."));
    }

    #[test]
    fn golangci_lint_is_safe() {
        assert!(is_safe_shell_command("golangci-lint run"));
    }

    #[test]
    fn make_test_is_safe() {
        assert!(is_safe_shell_command("make test"));
    }

    #[test]
    fn make_check_is_safe() {
        assert!(is_safe_shell_command("make check"));
    }
}

// --- Injection vector tests ---

mod safe_shell_injection {
    use anvil::tooling::is_safe_shell_command;

    #[test]
    fn cargo_test_chain_is_unsafe() {
        assert!(!is_safe_shell_command("cargo test && rm -rf /"));
    }

    #[test]
    fn git_log_redirect_is_unsafe() {
        assert!(!is_safe_shell_command("git log > ~/.bashrc"));
    }

    #[test]
    fn git_status_input_redirect_is_unsafe() {
        assert!(!is_safe_shell_command("git status < /etc/passwd"));
    }
}

// --- effective_permission_class tests ---

mod effective_permission {
    use anvil::tooling::{PermissionClass, ToolInput, ToolRegistry, effective_permission_class};

    fn build_registry() -> ToolRegistry {
        let mut registry = ToolRegistry::new();
        registry.register_standard_tools();
        registry
    }

    #[test]
    fn promotes_safe_shell_to_safe() {
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
    fn keeps_unsafe_shell_as_confirm() {
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
    fn web_search_stays_confirm() {
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
    fn file_read_stays_safe() {
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
}

// --- approval_required uses effective_permission_class ---

mod approval_with_effective_permission {
    use anvil::tooling::{ToolCallRequest, ToolInput, ToolRegistry};

    fn build_registry() -> ToolRegistry {
        let mut registry = ToolRegistry::new();
        registry.register_standard_tools();
        registry
    }

    #[test]
    fn not_required_for_safe_shell_command() {
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
    fn required_for_unsafe_shell_command() {
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
}

// --- Blocked command validation tests ---

mod blocked_commands {
    use anvil::tooling::{ToolCallRequest, ToolInput, ToolRegistry, ToolValidationError};

    fn build_registry() -> ToolRegistry {
        let mut registry = ToolRegistry::new();
        registry.register_standard_tools();
        registry
    }

    fn assert_blocked(command: &str, msg: &str) {
        let registry = build_registry();
        let err = registry
            .validate(ToolCallRequest::new(
                "call_001",
                "shell.exec",
                ToolInput::ShellExec {
                    command: command.to_string(),
                },
            ))
            .expect_err(msg);
        match err {
            ToolValidationError::DangerousCommand { .. } => {}
            other => panic!("expected DangerousCommand, got {other:?}"),
        }
    }

    #[test]
    fn rm_rf_root() {
        assert_blocked("rm -rf /", "rm -rf / should be blocked");
    }

    #[test]
    fn mkfs() {
        assert_blocked("mkfs.ext4 /dev/sda", "mkfs should be blocked");
    }

    #[test]
    fn rm_rf_home() {
        assert_blocked("rm -rf ~", "rm -rf ~ should be blocked");
    }

    #[test]
    fn dd_if() {
        assert_blocked("dd if=/dev/zero of=/dev/sda", "dd if= should be blocked");
    }

    #[test]
    fn fork_bomb() {
        assert_blocked(":(){:|:&};:", "fork bomb should be blocked");
    }

    #[test]
    fn process_substitution() {
        assert_blocked("echo foo >(bar)", "process substitution should be blocked");
    }

    #[test]
    fn git_commit_no_verify() {
        let registry = build_registry();
        let err = registry
            .validate(ToolCallRequest::new(
                "call_001",
                "shell.exec",
                ToolInput::ShellExec {
                    command: "git commit --no-verify -m 'test'".to_string(),
                },
            ))
            .expect_err("git commit --no-verify should be blocked");
        match err {
            ToolValidationError::DangerousCommand { reason, .. } => {
                assert!(reason.contains("git hooks"));
            }
            other => panic!("expected DangerousCommand, got {other:?}"),
        }
    }

    #[test]
    fn git_push_no_verify() {
        assert_blocked(
            "git push --no-verify origin main",
            "git push --no-verify should be blocked",
        );
    }

    #[test]
    fn git_merge_no_verify() {
        assert_blocked(
            "git merge --no-verify feature",
            "git merge --no-verify should be blocked",
        );
    }

    #[test]
    fn git_commit_n_shorthand() {
        let registry = build_registry();
        let err = registry
            .validate(ToolCallRequest::new(
                "call_001",
                "shell.exec",
                ToolInput::ShellExec {
                    command: "git commit -n -m 'test'".to_string(),
                },
            ))
            .expect_err("git commit -n should be blocked");
        match err {
            ToolValidationError::DangerousCommand { reason, .. } => {
                assert!(reason.contains("-n"));
            }
            other => panic!("expected DangerousCommand, got {other:?}"),
        }
    }

    #[test]
    fn npm_publish_no_verify_is_not_blocked() {
        let registry = build_registry();
        let result = registry.validate(ToolCallRequest::new(
            "call_001",
            "shell.exec",
            ToolInput::ShellExec {
                command: "npm publish --no-verify".to_string(),
            },
        ));
        assert!(
            result.is_ok(),
            "npm publish --no-verify should not be blocked by git-specific patterns"
        );
    }

    #[test]
    fn safe_prefix_with_blocked_content_is_still_blocked() {
        assert_blocked(
            "git commit --no-verify",
            "blocked patterns should be checked during validation",
        );
    }
}

// --- file.edit tests ---

#[test]
fn file_edit_validates_typed_tool_input() {
    let registry = build_registry();
    let valid = ToolCallRequest::new(
        "call_edit_001",
        "file.edit",
        ToolInput::FileEdit {
            path: "src/main.rs".to_string(),
            old_string: "fn main()".to_string(),
            new_string: "fn main() -> Result<(), Box<dyn std::error::Error>>".to_string(),
        },
    );
    let validated = registry
        .validate(valid)
        .expect("matching typed input should validate");
    assert_eq!(validated.spec.name, "file.edit");
    assert_eq!(validated.spec.kind, ToolKind::FileEdit);
}

#[test]
fn file_edit_spec_policies() {
    let registry = build_registry();
    let spec = registry.get("file.edit").expect("file.edit should exist");
    assert_eq!(spec.version, 1);
    assert_eq!(spec.execution_class, ExecutionClass::Mutating);
    assert_eq!(spec.permission_class, PermissionClass::Confirm);
    assert_eq!(
        spec.execution_mode,
        anvil::tooling::ExecutionMode::SequentialOnly
    );
    assert_eq!(spec.plan_mode, PlanModePolicy::AllowedWithScope);
    assert_eq!(spec.rollback_policy, RollbackPolicy::CheckpointBeforeWrite);
}

#[test]
fn file_edit_missing_path_error() {
    let registry = build_registry();
    let err = registry
        .validate(ToolCallRequest::new(
            "call_edit_002",
            "file.edit",
            ToolInput::FileEdit {
                path: "".to_string(),
                old_string: "hello".to_string(),
                new_string: "world".to_string(),
            },
        ))
        .expect_err("empty path should be rejected");
    assert_eq!(
        err,
        ToolValidationError::MissingRequiredField("path".to_string())
    );
}

#[test]
fn file_edit_missing_old_string_error() {
    let registry = build_registry();
    let err = registry
        .validate(ToolCallRequest::new(
            "call_edit_003",
            "file.edit",
            ToolInput::FileEdit {
                path: "src/main.rs".to_string(),
                old_string: "".to_string(),
                new_string: "world".to_string(),
            },
        ))
        .expect_err("empty old_string should be rejected");
    assert_eq!(
        err,
        ToolValidationError::MissingRequiredField("old_string".to_string())
    );
}

#[test]
fn file_edit_execution_replaces_unique_match() {
    let root = std::env::temp_dir().join("anvil_file_edit_replace");
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("dir should exist");
    let file_path = root.join("test.txt");
    fs::write(&file_path, "hello world").expect("write should succeed");

    let mut executor = LocalToolExecutor::new_without_rate_limit(root.clone());
    let result = executor
        .execute(ToolExecutionRequest {
            tool_call_id: "call_edit_exec_001".to_string(),
            spec: build_registry()
                .get("file.edit")
                .expect("file.edit spec")
                .clone(),
            input: ToolInput::FileEdit {
                path: "./test.txt".to_string(),
                old_string: "hello".to_string(),
                new_string: "goodbye".to_string(),
            },
        })
        .expect("edit should succeed");

    assert_eq!(result.status, ToolExecutionStatus::Completed);
    let content = fs::read_to_string(&file_path).expect("read should succeed");
    assert_eq!(content, "goodbye world");
}

#[test]
fn file_edit_old_string_not_found() {
    let root = std::env::temp_dir().join("anvil_file_edit_not_found");
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("dir should exist");
    fs::write(root.join("test.txt"), "hello world").expect("write should succeed");

    let mut executor = LocalToolExecutor::new_without_rate_limit(root.clone());
    let err = executor
        .execute(ToolExecutionRequest {
            tool_call_id: "call_edit_nf_001".to_string(),
            spec: build_registry()
                .get("file.edit")
                .expect("file.edit spec")
                .clone(),
            input: ToolInput::FileEdit {
                path: "./test.txt".to_string(),
                old_string: "nonexistent".to_string(),
                new_string: "replacement".to_string(),
            },
        })
        .expect_err("should fail when old_string not found");

    assert!(err.to_string().contains("not found"));
}

#[test]
fn file_edit_old_string_multiple_matches() {
    let root = std::env::temp_dir().join("anvil_file_edit_multi_match");
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("dir should exist");
    fs::write(root.join("test.txt"), "aaa bbb aaa").expect("write should succeed");

    let mut executor = LocalToolExecutor::new_without_rate_limit(root.clone());
    let err = executor
        .execute(ToolExecutionRequest {
            tool_call_id: "call_edit_mm_001".to_string(),
            spec: build_registry()
                .get("file.edit")
                .expect("file.edit spec")
                .clone(),
            input: ToolInput::FileEdit {
                path: "./test.txt".to_string(),
                old_string: "aaa".to_string(),
                new_string: "ccc".to_string(),
            },
        })
        .expect_err("should fail when old_string matches multiple times");

    assert!(err.to_string().contains("found 2 times"));
}

#[test]
fn file_edit_empty_new_string_deletes() {
    let root = std::env::temp_dir().join("anvil_file_edit_delete");
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("dir should exist");
    let file_path = root.join("test.txt");
    fs::write(&file_path, "hello world").expect("write should succeed");

    let mut executor = LocalToolExecutor::new_without_rate_limit(root.clone());
    let result = executor
        .execute(ToolExecutionRequest {
            tool_call_id: "call_edit_del_001".to_string(),
            spec: build_registry()
                .get("file.edit")
                .expect("file.edit spec")
                .clone(),
            input: ToolInput::FileEdit {
                path: "./test.txt".to_string(),
                old_string: " world".to_string(),
                new_string: "".to_string(),
            },
        })
        .expect("edit should succeed");

    assert_eq!(result.status, ToolExecutionStatus::Completed);
    let content = fs::read_to_string(&file_path).expect("read should succeed");
    assert_eq!(content, "hello");
}

#[test]
fn file_edit_noop_when_strings_equal() {
    let root = std::env::temp_dir().join("anvil_file_edit_noop");
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("dir should exist");
    let file_path = root.join("test.txt");
    fs::write(&file_path, "hello world").expect("write should succeed");

    let mut executor = LocalToolExecutor::new_without_rate_limit(root.clone());
    let result = executor
        .execute(ToolExecutionRequest {
            tool_call_id: "call_edit_noop_001".to_string(),
            spec: build_registry()
                .get("file.edit")
                .expect("file.edit spec")
                .clone(),
            input: ToolInput::FileEdit {
                path: "./test.txt".to_string(),
                old_string: "hello".to_string(),
                new_string: "hello".to_string(),
            },
        })
        .expect("noop edit should succeed");

    assert_eq!(result.status, ToolExecutionStatus::Completed);
    assert!(result.summary.contains("no changes"));
    let content = fs::read_to_string(&file_path).expect("read should succeed");
    assert_eq!(content, "hello world");
}

#[test]
fn file_edit_file_not_found() {
    let root = std::env::temp_dir().join("anvil_file_edit_no_file");
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("dir should exist");

    let mut executor = LocalToolExecutor::new_without_rate_limit(root.clone());
    let err = executor
        .execute(ToolExecutionRequest {
            tool_call_id: "call_edit_nofile_001".to_string(),
            spec: build_registry()
                .get("file.edit")
                .expect("file.edit spec")
                .clone(),
            input: ToolInput::FileEdit {
                path: "./nonexistent.txt".to_string(),
                old_string: "hello".to_string(),
                new_string: "world".to_string(),
            },
        })
        .expect_err("should fail for nonexistent file");

    assert!(err.to_string().contains("file.edit failed to read"));
}

#[test]
fn file_edit_sandbox_escape_rejected() {
    let root = std::env::temp_dir().join("anvil_file_edit_sandbox");
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("dir should exist");

    let mut executor = LocalToolExecutor::new_without_rate_limit(root.clone());
    let err = executor
        .execute(ToolExecutionRequest {
            tool_call_id: "call_edit_escape_001".to_string(),
            spec: build_registry()
                .get("file.edit")
                .expect("file.edit spec")
                .clone(),
            input: ToolInput::FileEdit {
                path: "../../../etc/passwd".to_string(),
                old_string: "root".to_string(),
                new_string: "hacked".to_string(),
            },
        })
        .expect_err("should reject sandbox escape");

    assert!(err.to_string().contains("invalid tool path"));
}

// --- Parallel execution grouping tests ---

/// Build a ToolExecutionRequest with a given spec name and execution mode.
fn build_exec_request(name: &str, mode: ExecutionMode) -> ToolExecutionRequest {
    let registry = build_registry();
    // Pick a real spec that matches the desired execution mode.
    let mut spec = match mode {
        ExecutionMode::ParallelSafe => registry.get("file.read").unwrap().clone(),
        ExecutionMode::SequentialOnly => registry.get("file.write").unwrap().clone(),
    };
    // Override the name for test clarity (the spec already has the right execution_mode).
    spec.name = name.to_string();
    ToolExecutionRequest {
        tool_call_id: format!("call_{name}"),
        spec,
        input: ToolInput::FileRead {
            path: "dummy.txt".to_string(),
        },
    }
}

#[test]
fn group_by_execution_mode_all_parallel() {
    let requests: Vec<(usize, ToolExecutionRequest)> = vec![
        (0, build_exec_request("read1", ExecutionMode::ParallelSafe)),
        (1, build_exec_request("read2", ExecutionMode::ParallelSafe)),
        (2, build_exec_request("read3", ExecutionMode::ParallelSafe)),
    ];

    let groups = group_by_execution_mode(&requests);
    assert_eq!(groups.len(), 1);
    match &groups[0] {
        ExecutionGroup::Parallel(items) => assert_eq!(items.len(), 3),
        _ => panic!("expected Parallel group"),
    }
}

#[test]
fn group_by_execution_mode_all_sequential() {
    let requests: Vec<(usize, ToolExecutionRequest)> = vec![
        (
            0,
            build_exec_request("write1", ExecutionMode::SequentialOnly),
        ),
        (
            1,
            build_exec_request("write2", ExecutionMode::SequentialOnly),
        ),
        (
            2,
            build_exec_request("write3", ExecutionMode::SequentialOnly),
        ),
    ];

    let groups = group_by_execution_mode(&requests);
    assert_eq!(groups.len(), 3);
    for group in &groups {
        match group {
            ExecutionGroup::Sequential(_, _) => {}
            _ => panic!("expected Sequential group"),
        }
    }
}

#[test]
fn group_by_execution_mode_mixed() {
    // [read, read, write, read, search] -> [Parallel([read,read]), Sequential(write), Parallel([read,search])]
    let requests: Vec<(usize, ToolExecutionRequest)> = vec![
        (0, build_exec_request("read1", ExecutionMode::ParallelSafe)),
        (1, build_exec_request("read2", ExecutionMode::ParallelSafe)),
        (
            2,
            build_exec_request("write1", ExecutionMode::SequentialOnly),
        ),
        (3, build_exec_request("read3", ExecutionMode::ParallelSafe)),
        (
            4,
            build_exec_request("search1", ExecutionMode::ParallelSafe),
        ),
    ];

    let groups = group_by_execution_mode(&requests);
    assert_eq!(groups.len(), 3);

    match &groups[0] {
        ExecutionGroup::Parallel(items) => {
            assert_eq!(items.len(), 2);
            assert_eq!(items[0].0, 0);
            assert_eq!(items[1].0, 1);
        }
        _ => panic!("expected Parallel group at index 0"),
    }
    match &groups[1] {
        ExecutionGroup::Sequential(idx, _) => assert_eq!(*idx, 2),
        _ => panic!("expected Sequential group at index 1"),
    }
    match &groups[2] {
        ExecutionGroup::Parallel(items) => {
            assert_eq!(items.len(), 2);
            assert_eq!(items[0].0, 3);
            assert_eq!(items[1].0, 4);
        }
        _ => panic!("expected Parallel group at index 2"),
    }
}

#[test]
fn group_by_execution_mode_empty() {
    let requests: Vec<(usize, ToolExecutionRequest)> = vec![];
    let groups = group_by_execution_mode(&requests);
    assert!(groups.is_empty());
}

#[test]
fn group_by_execution_mode_single_parallel() {
    let requests: Vec<(usize, ToolExecutionRequest)> =
        vec![(0, build_exec_request("read1", ExecutionMode::ParallelSafe))];

    let groups = group_by_execution_mode(&requests);
    assert_eq!(groups.len(), 1);
    match &groups[0] {
        ExecutionGroup::Parallel(items) => assert_eq!(items.len(), 1),
        _ => panic!("expected Parallel group with 1 element"),
    }
}

#[test]
fn parallel_execution_preserves_result_order() {
    // Create multiple real files and read them in parallel.
    // Verify results come back in the original request order.
    let root = std::env::temp_dir().join("anvil_parallel_order_test");
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("dir should exist");

    let file_count = 5;
    for i in 0..file_count {
        fs::write(root.join(format!("file_{i}.txt")), format!("content_{i}"))
            .expect("write should succeed");
    }

    let registry = build_registry();
    let read_spec = registry.get("file.read").unwrap().clone();

    let requests: Vec<(usize, ToolExecutionRequest)> = (0..file_count)
        .map(|i| {
            (
                i,
                ToolExecutionRequest {
                    tool_call_id: format!("call_read_{i}"),
                    spec: read_spec.clone(),
                    input: ToolInput::FileRead {
                        path: format!("./file_{i}.txt"),
                    },
                },
            )
        })
        .collect();

    // Execute using LocalToolExecutor in parallel via thread::scope
    let mut results: Vec<(usize, ToolExecutionResult)> = Vec::new();
    std::thread::scope(|s| {
        let handles: Vec<_> = requests
            .iter()
            .map(|(idx, req)| {
                let root = root.clone();
                let idx = *idx;
                let req = req.clone();
                s.spawn(move || {
                    let mut executor = LocalToolExecutor::new_without_rate_limit(root);
                    let result = executor.execute(req).expect("read should succeed");
                    (idx, result)
                })
            })
            .collect();
        for handle in handles {
            results.push(handle.join().expect("thread should not panic"));
        }
    });

    // Sort by index (as the real implementation does)
    results.sort_by_key(|(idx, _)| *idx);

    // Verify order and content
    assert_eq!(results.len(), file_count);
    for (i, (idx, result)) in results.iter().enumerate() {
        assert_eq!(*idx, i);
        assert_eq!(result.status, ToolExecutionStatus::Completed);
        match &result.payload {
            ToolExecutionPayload::Text(content) => {
                assert!(content.contains(&format!("content_{i}")));
            }
            _ => panic!("expected Text payload for file_{i}"),
        }
    }
}
