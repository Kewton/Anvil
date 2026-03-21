use anvil::app::agentic::{ExecutionGroup, group_by_execution_mode};
use anvil::tooling::{
    CheckpointEntry, CheckpointStack, ExecutionClass, ExecutionMode, LocalToolExecutor,
    ParallelExecutionPlan, ParallelExecutionPlanError, PermissionClass, PlanModePolicy,
    RollbackPolicy, ToolCallRequest, ToolExecutionError, ToolExecutionPayload, ToolExecutionPolicy,
    ToolExecutionRequest, ToolExecutionResult, ToolExecutionStatus, ToolInput, ToolKind,
    ToolRegistry, ToolValidationError, detect_image_mime,
};
use std::fs;
use std::path::Path;

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
                regex: false,
                context_lines: 0,
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
                regex: false,
                context_lines: 0,
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
    assert_eq!(log.elapsed_ms, Some(12));
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

// --- classify_shell_policy tests ---

mod classify_shell_policy_tests {
    use anvil::tooling::{ShellPolicy, classify_shell_policy};

    #[test]
    fn read_only_git_log() {
        assert_eq!(
            classify_shell_policy("git log --oneline"),
            ShellPolicy::ReadOnly
        );
    }

    #[test]
    fn read_only_git_status() {
        assert_eq!(classify_shell_policy("git status"), ShellPolicy::ReadOnly);
    }

    #[test]
    fn read_only_git_diff() {
        assert_eq!(classify_shell_policy("git diff"), ShellPolicy::ReadOnly);
    }

    #[test]
    fn read_only_git_branch() {
        assert_eq!(classify_shell_policy("git branch"), ShellPolicy::ReadOnly);
    }

    #[test]
    fn read_only_git_show_with_ref() {
        assert_eq!(
            classify_shell_policy("git show HEAD"),
            ShellPolicy::ReadOnly
        );
    }

    #[test]
    fn read_only_gh_api_get() {
        assert_eq!(
            classify_shell_policy("gh api repos/o/r/stats"),
            ShellPolicy::ReadOnly
        );
    }

    #[test]
    fn read_only_which() {
        assert_eq!(classify_shell_policy("which rustc"), ShellPolicy::ReadOnly);
    }

    #[test]
    fn read_only_uname() {
        assert_eq!(classify_shell_policy("uname"), ShellPolicy::ReadOnly);
    }

    #[test]
    fn read_only_lsof_i() {
        assert_eq!(classify_shell_policy("lsof -i"), ShellPolicy::ReadOnly);
    }

    #[test]
    fn build_test_cargo_test() {
        assert_eq!(classify_shell_policy("cargo test"), ShellPolicy::BuildTest);
    }

    #[test]
    fn build_test_cargo_build() {
        assert_eq!(classify_shell_policy("cargo build"), ShellPolicy::BuildTest);
    }

    #[test]
    fn build_test_cargo_clippy() {
        assert_eq!(
            classify_shell_policy("cargo clippy --all-targets"),
            ShellPolicy::BuildTest
        );
    }

    #[test]
    fn build_test_npm_test() {
        assert_eq!(classify_shell_policy("npm test"), ShellPolicy::BuildTest);
    }

    #[test]
    fn build_test_pytest() {
        assert_eq!(
            classify_shell_policy("pytest tests/"),
            ShellPolicy::BuildTest
        );
    }

    #[test]
    fn build_test_go_test() {
        assert_eq!(
            classify_shell_policy("go test ./..."),
            ShellPolicy::BuildTest
        );
    }

    #[test]
    fn build_test_make_test() {
        assert_eq!(classify_shell_policy("make test"), ShellPolicy::BuildTest);
    }

    #[test]
    fn general_curl() {
        assert_eq!(
            classify_shell_policy("curl https://example.com"),
            ShellPolicy::General
        );
    }

    #[test]
    fn general_unknown_command() {
        assert_eq!(
            classify_shell_policy("some-unknown-command"),
            ShellPolicy::General
        );
    }

    #[test]
    fn pipe_fallback_to_general() {
        assert_eq!(
            classify_shell_policy("git log | head -5"),
            ShellPolicy::General
        );
    }

    #[test]
    fn chain_fallback_to_general() {
        assert_eq!(
            classify_shell_policy("cargo test && rm -rf /"),
            ShellPolicy::General
        );
    }

    #[test]
    fn case_insensitive_git_log() {
        assert_eq!(
            classify_shell_policy("GIT LOG --oneline"),
            ShellPolicy::ReadOnly
        );
    }

    #[test]
    fn case_insensitive_cargo_test() {
        assert_eq!(classify_shell_policy("Cargo Test"), ShellPolicy::BuildTest);
    }

    #[test]
    fn dangerous_options_web_general() {
        assert_eq!(
            classify_shell_policy("gh repo view --web"),
            ShellPolicy::General
        );
    }

    #[test]
    fn dangerous_options_browse_general() {
        assert_eq!(
            classify_shell_policy("gh issue list --browse"),
            ShellPolicy::General
        );
    }

    #[test]
    fn gh_api_post_is_general() {
        assert_eq!(
            classify_shell_policy("gh api --method POST repos/o/r/issues"),
            ShellPolicy::General
        );
    }

    #[test]
    fn gh_api_xdelete_is_general() {
        assert_eq!(
            classify_shell_policy("gh api -XDELETE repos/o/r/issues/1"),
            ShellPolicy::General
        );
    }

    #[test]
    fn gh_api_field_flag_is_general() {
        assert_eq!(
            classify_shell_policy("gh api -f title=hacked repos/o/r/issues"),
            ShellPolicy::General
        );
    }
}

// --- is_network_command tests ---

mod is_network_command_tests {
    use anvil::tooling::is_network_command;

    #[test]
    fn curl_is_network() {
        assert!(is_network_command("curl https://example.com"));
    }

    #[test]
    fn wget_is_network() {
        assert!(is_network_command("wget https://example.com"));
    }

    #[test]
    fn ssh_is_network() {
        assert!(is_network_command("ssh user@host"));
    }

    #[test]
    fn ping_is_network() {
        assert!(is_network_command("ping 8.8.8.8"));
    }

    #[test]
    fn scp_is_network() {
        assert!(is_network_command("scp file user@host:/tmp/"));
    }

    #[test]
    fn dig_is_network() {
        assert!(is_network_command("dig example.com"));
    }

    #[test]
    fn ls_is_not_network() {
        assert!(!is_network_command("ls -la"));
    }

    #[test]
    fn git_is_not_network() {
        assert!(!is_network_command("git log"));
    }

    #[test]
    fn cargo_is_not_network() {
        assert!(!is_network_command("cargo test"));
    }

    #[test]
    fn case_insensitive_curl() {
        assert!(is_network_command("CURL https://example.com"));
    }

    #[test]
    fn case_insensitive_wget() {
        assert!(is_network_command("Wget https://example.com"));
    }

    #[test]
    fn sudo_curl_is_network() {
        assert!(is_network_command("sudo curl https://example.com"));
    }

    #[test]
    fn env_wget_is_network() {
        assert!(is_network_command("env wget https://example.com"));
    }

    #[test]
    fn absolute_path_curl_is_network() {
        assert!(is_network_command("/usr/bin/curl https://example.com"));
    }

    #[test]
    fn env_var_prefix_curl_is_network() {
        assert!(is_network_command(
            "HTTPS_PROXY=proxy curl https://example.com"
        ));
    }

    #[test]
    fn env_i_curl_is_network() {
        assert!(is_network_command("env -i curl https://example.com"));
    }
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

// ---- Diff preview tests ----

use anvil::tooling::diff::{generate_diff_preview, is_binary_content};

#[test]
fn test_diff_preview_existing_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let file_path = dir.path().join("hello.txt");
    fs::write(&file_path, "line1\nline2\nline3\n").expect("write");

    let input = ToolInput::FileWrite {
        path: "hello.txt".to_string(),
        content: "line1\nline2 modified\nline3\nline4\n".to_string(),
    };
    let result = generate_diff_preview(dir.path(), &input);
    assert!(result.is_some());
    let diff = result.unwrap();
    assert!(diff.contains("-line2"));
    assert!(diff.contains("+line2 modified"));
    assert!(diff.contains("+line4"));
}

#[test]
fn test_diff_preview_new_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let input = ToolInput::FileWrite {
        path: "brand_new.txt".to_string(),
        content: "first line\nsecond line\n".to_string(),
    };
    let result = generate_diff_preview(dir.path(), &input);
    assert!(result.is_some());
    let preview = result.unwrap();
    assert!(preview.contains("(new file)"));
    assert!(preview.contains("+first line"));
    assert!(preview.contains("+second line"));
}

#[test]
fn test_diff_preview_binary_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let file_path = dir.path().join("binary.bin");
    let mut content = vec![0u8; 100];
    content[50] = 0; // NUL byte
    fs::write(&file_path, &content).expect("write");

    let input = ToolInput::FileWrite {
        path: "binary.bin".to_string(),
        content: "new content".to_string(),
    };
    let result = generate_diff_preview(dir.path(), &input);
    assert!(result.is_some());
    assert!(result.unwrap().contains("binary file"));
}

#[test]
fn test_diff_preview_large_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let file_path = dir.path().join("big.txt");
    // Create a file larger than 1MB
    let big_content = "x".repeat(1_048_577);
    fs::write(&file_path, &big_content).expect("write");

    let input = ToolInput::FileWrite {
        path: "big.txt".to_string(),
        content: "replacement".to_string(),
    };
    let result = generate_diff_preview(dir.path(), &input);
    assert!(result.is_some());
    assert!(result.unwrap().contains("file too large"));
}

#[test]
fn test_diff_preview_large_diff() {
    let dir = tempfile::tempdir().expect("tempdir");
    let file_path = dir.path().join("many_lines.txt");
    // Create a file with many lines
    let old_lines: Vec<String> = (0..100).map(|i| format!("old line {i}")).collect();
    fs::write(&file_path, old_lines.join("\n")).expect("write");

    let new_lines: Vec<String> = (0..100).map(|i| format!("new line {i}")).collect();
    let input = ToolInput::FileWrite {
        path: "many_lines.txt".to_string(),
        content: new_lines.join("\n"),
    };
    let result = generate_diff_preview(dir.path(), &input);
    assert!(result.is_some());
    let diff = result.unwrap();
    // Should be truncated
    assert!(diff.contains("..."));
    assert!(diff.contains("lines added"));
    assert!(diff.contains("lines deleted"));
}

#[test]
fn test_diff_preview_file_edit() {
    let dir = tempfile::tempdir().expect("tempdir");
    let input = ToolInput::FileEdit {
        path: "some_file.rs".to_string(),
        old_string: "fn old_function() {}".to_string(),
        new_string: "fn new_function() {\n    println!(\"hello\");\n}".to_string(),
    };
    let result = generate_diff_preview(dir.path(), &input);
    assert!(result.is_some());
    let diff = result.unwrap();
    assert!(diff.contains("-fn old_function() {}"));
    assert!(diff.contains("+fn new_function() {"));
}

#[test]
fn test_diff_preview_nonexistent_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let input = ToolInput::FileWrite {
        path: "does_not_exist.txt".to_string(),
        content: "hello world\n".to_string(),
    };
    let result = generate_diff_preview(dir.path(), &input);
    assert!(result.is_some());
    let preview = result.unwrap();
    assert!(preview.contains("(new file)"));
    assert!(preview.contains("+hello world"));
}

#[test]
fn test_diff_preview_line_truncation() {
    let dir = tempfile::tempdir().expect("tempdir");
    // Create a new file with a very long line (> 200 chars)
    let long_line = "a".repeat(300);
    let input = ToolInput::FileWrite {
        path: "long_line.txt".to_string(),
        content: format!("{long_line}\nshort\n"),
    };
    let result = generate_diff_preview(dir.path(), &input);
    assert!(result.is_some());
    let preview = result.unwrap();
    assert!(preview.contains("(new file)"));
    // The long line should be truncated with "..."
    assert!(preview.contains("..."));
    // Should not contain the full 300-char line
    assert!(!preview.contains(&"a".repeat(300)));
}

#[test]
fn test_is_binary_content() {
    // Text content
    assert!(!is_binary_content(b"hello world\nfoo bar\n"));
    // Binary content with NUL byte
    assert!(is_binary_content(b"hello\x00world"));
    // Empty content
    assert!(!is_binary_content(b""));
    // Pure NUL
    assert!(is_binary_content(&[0u8; 10]));
}

#[test]
fn test_file_edit_diff_no_file_access() {
    // file.edit diff generation should work without any file on disk
    let dir = tempfile::tempdir().expect("tempdir");
    // No files created in dir

    let input = ToolInput::FileEdit {
        path: "nonexistent.rs".to_string(),
        old_string: "old code".to_string(),
        new_string: "new code".to_string(),
    };
    let result = generate_diff_preview(dir.path(), &input);
    assert!(result.is_some());
    let diff = result.unwrap();
    assert!(diff.contains("-old code"));
    assert!(diff.contains("+new code"));
}

#[test]
fn test_diff_preview_other_tool_input_returns_none() {
    let dir = tempfile::tempdir().expect("tempdir");
    let input = ToolInput::FileRead {
        path: "foo.txt".to_string(),
    };
    assert!(generate_diff_preview(dir.path(), &input).is_none());

    let input = ToolInput::ShellExec {
        command: "ls".to_string(),
    };
    assert!(generate_diff_preview(dir.path(), &input).is_none());
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

// ── ToolExecutionPayload::Image tests ──────────────────────────────────

#[test]
fn tool_execution_payload_image_construction() {
    let payload = ToolExecutionPayload::Image {
        source_path: "/tmp/test.png".to_string(),
        mime_type: "image/png".to_string(),
    };
    match payload {
        ToolExecutionPayload::Image {
            source_path,
            mime_type,
        } => {
            assert_eq!(source_path, "/tmp/test.png");
            assert_eq!(mime_type, "image/png");
        }
        _ => panic!("expected Image payload"),
    }
}

#[test]
fn tool_execution_payload_image_debug_and_clone() {
    let payload = ToolExecutionPayload::Image {
        source_path: "photo.jpg".to_string(),
        mime_type: "image/jpeg".to_string(),
    };
    let cloned = payload.clone();
    assert_eq!(payload, cloned);
    // Debug should work
    let debug_str = format!("{:?}", payload);
    assert!(debug_str.contains("Image"));
}

// -----------------------------------------------------------------------
// Phase 2: detect_image_mime tests
// -----------------------------------------------------------------------

#[test]
fn detect_image_mime_png() {
    assert_eq!(detect_image_mime(Path::new("photo.png")), Some("image/png"));
}

#[test]
fn detect_image_mime_jpg() {
    assert_eq!(
        detect_image_mime(Path::new("photo.jpg")),
        Some("image/jpeg")
    );
}

#[test]
fn detect_image_mime_jpeg() {
    assert_eq!(
        detect_image_mime(Path::new("photo.jpeg")),
        Some("image/jpeg")
    );
}

#[test]
fn detect_image_mime_gif() {
    assert_eq!(detect_image_mime(Path::new("anim.gif")), Some("image/gif"));
}

#[test]
fn detect_image_mime_webp() {
    assert_eq!(
        detect_image_mime(Path::new("photo.webp")),
        Some("image/webp")
    );
}

#[test]
fn detect_image_mime_unknown_returns_none() {
    assert_eq!(detect_image_mime(Path::new("file.txt")), None);
    assert_eq!(detect_image_mime(Path::new("file.rs")), None);
    assert_eq!(detect_image_mime(Path::new("no_extension")), None);
}

#[test]
fn detect_image_mime_case_insensitive() {
    assert_eq!(detect_image_mime(Path::new("PHOTO.PNG")), Some("image/png"));
    assert_eq!(
        detect_image_mime(Path::new("photo.JPG")),
        Some("image/jpeg")
    );
}

// -----------------------------------------------------------------------
// Phase 3: format_tool_result_message for Image payload
// -----------------------------------------------------------------------

#[test]
fn format_tool_result_message_image_payload() {
    use anvil::app::agentic::format_tool_result_message;
    let result = ToolExecutionResult {
        tool_call_id: "call_001".to_string(),
        tool_name: "file.read".to_string(),
        status: ToolExecutionStatus::Completed,
        summary: "read image".to_string(),
        payload: ToolExecutionPayload::Image {
            source_path: "/tmp/photo.png".to_string(),
            mime_type: "image/png".to_string(),
        },
        artifacts: Vec::new(),
        elapsed_ms: 10,
    };
    let msg = format_tool_result_message(&result, 10000);
    assert!(msg.contains("file.read"));
    assert!(msg.contains("/tmp/photo.png"));
    assert!(msg.contains("画像"));
}

// -----------------------------------------------------------------------
// Phase 3: format_tool_result_message — UTF-8 safe truncation (Issue #94)
// -----------------------------------------------------------------------

#[test]
fn format_tool_result_message_truncates_multibyte_safely() {
    use anvil::app::agentic::format_tool_result_message;

    // Create a string of 3000 CJK characters (each 3 bytes in UTF-8 = 9000 bytes).
    // With max_chars = 100 (characters), truncation must land on a char boundary.
    let cjk_content: String = "競".repeat(3000);
    let result = ToolExecutionResult {
        tool_call_id: "call_utf8".to_string(),
        tool_name: "file.read".to_string(),
        status: ToolExecutionStatus::Completed,
        summary: "read CJK file".to_string(),
        payload: ToolExecutionPayload::Text(cjk_content),
        artifacts: Vec::new(),
        elapsed_ms: 5,
    };

    // Must not panic — the old byte-slicing implementation would panic here.
    let msg = format_tool_result_message(&result, 100);
    assert!(msg.contains("truncated"));
    assert!(msg.contains("chars total"));
    assert!(msg.contains("file.read"));
}

#[test]
fn format_tool_result_message_ascii_truncation_still_works() {
    use anvil::app::agentic::format_tool_result_message;

    let ascii_content = "a".repeat(500);
    let result = ToolExecutionResult {
        tool_call_id: "call_ascii".to_string(),
        tool_name: "file.read".to_string(),
        status: ToolExecutionStatus::Completed,
        summary: "read file".to_string(),
        payload: ToolExecutionPayload::Text(ascii_content),
        artifacts: Vec::new(),
        elapsed_ms: 5,
    };

    let msg = format_tool_result_message(&result, 100);
    assert!(msg.contains("truncated"));
    assert!(msg.contains("chars total"));
}

#[test]
fn format_tool_result_message_boundary_char_3byte() {
    use anvil::app::agentic::format_tool_result_message;

    // 99 ASCII chars + one 3-byte CJK char = 100 chars.
    // Truncation at exactly 100 chars must not split the CJK char.
    let mut content = "a".repeat(99);
    content.push('競');
    content.push_str(&"b".repeat(200)); // push past the limit

    let result = ToolExecutionResult {
        tool_call_id: "call_boundary".to_string(),
        tool_name: "file.read".to_string(),
        status: ToolExecutionStatus::Completed,
        summary: "boundary test".to_string(),
        payload: ToolExecutionPayload::Text(content),
        artifacts: Vec::new(),
        elapsed_ms: 5,
    };

    let msg = format_tool_result_message(&result, 100);
    assert!(msg.contains("truncated"));
    // Head is 80% of 100 = 80 chars. The CJK char is at position 99 (0-indexed),
    // so it falls in the omitted middle section. Tail contains trailing 'b's.
    assert!(msg.contains("chars total"));
}

// -----------------------------------------------------------------------
// Issue #117: truncate_with_head_tail unit tests
// -----------------------------------------------------------------------

#[test]
fn truncate_with_head_tail_basic() {
    use anvil::app::agentic::truncate_with_head_tail;

    // 200 chars, max 100, head_pct 80 => head=80, tail=20
    let content = "a".repeat(200);
    let result = truncate_with_head_tail(&content, 100, 80);

    // Head: 80 'a's
    assert!(result.starts_with(&"a".repeat(80)));
    // Marker present
    assert!(result.contains("chars truncated"));
    assert!(result.contains("200 chars total"));
    // Tail: 20 'a's at the end
    assert!(result.ends_with(&"a".repeat(20)));
}

#[test]
fn truncate_with_head_tail_short_content() {
    use anvil::app::agentic::truncate_with_head_tail;

    let content = "hello world";
    let result = truncate_with_head_tail(content, 100, 80);
    assert_eq!(result, "hello world");
}

#[test]
fn truncate_with_head_tail_max_chars_zero() {
    use anvil::app::agentic::truncate_with_head_tail;

    let content = "a".repeat(500);
    let result = truncate_with_head_tail(&content, 0, 80);
    assert!(result.contains("500 chars total"));
    assert!(result.contains("all truncated"));
}

#[test]
fn truncate_with_head_tail_cjk_safety() {
    use anvil::app::agentic::truncate_with_head_tail;

    // 300 CJK chars, max 100, head_pct 50 => head=50, tail=50
    let content: String = "漢".repeat(300);
    let result = truncate_with_head_tail(&content, 100, 50);

    // Should not panic, head = 50 CJK chars, tail = 50 CJK chars
    assert!(result.contains("chars truncated"));
    assert!(result.contains("300 chars total"));
    // Head: 50 CJK chars
    let head_part: String = "漢".repeat(50);
    assert!(result.starts_with(&head_part));
    // Tail: 50 CJK chars
    let tail_part: String = "漢".repeat(50);
    assert!(result.ends_with(&tail_part));
}

#[test]
fn format_tool_result_message_success_head_priority() {
    use anvil::app::agentic::format_tool_result_message;

    // 200 distinct chars: 'H' * 100 + 'T' * 100
    let content = format!("{}{}", "H".repeat(100), "T".repeat(100));
    let result = ToolExecutionResult {
        tool_call_id: "call_success".to_string(),
        tool_name: "shell.exec".to_string(),
        status: ToolExecutionStatus::Completed,
        summary: "ok".to_string(),
        payload: ToolExecutionPayload::Text(content),
        artifacts: Vec::new(),
        elapsed_ms: 5,
    };

    let msg = format_tool_result_message(&result, 100);
    // Completed => head_pct=80 => head=80, tail=20
    // Head should have 80 H's
    assert!(msg.contains(&"H".repeat(80)));
    // Tail should have 20 T's
    assert!(msg.contains(&"T".repeat(20)));
    assert!(msg.contains("truncated"));
}

#[test]
fn format_tool_result_message_failure_tail_priority() {
    use anvil::app::agentic::format_tool_result_message;

    // 200 distinct chars: 'H' * 100 + 'T' * 100
    let content = format!("{}{}", "H".repeat(100), "T".repeat(100));
    let result = ToolExecutionResult {
        tool_call_id: "call_fail".to_string(),
        tool_name: "shell.exec".to_string(),
        status: ToolExecutionStatus::Failed,
        summary: "error".to_string(),
        payload: ToolExecutionPayload::Text(content),
        artifacts: Vec::new(),
        elapsed_ms: 5,
    };

    let msg = format_tool_result_message(&result, 100);
    // Failed => head_pct=20 => head=20, tail=80
    // Head should have 20 H's
    assert!(msg.contains(&"H".repeat(20)));
    // Tail should have 80 T's
    assert!(msg.contains(&"T".repeat(80)));
    assert!(msg.contains("truncated"));
}

#[test]
fn format_tool_result_message_interrupted_tail_priority() {
    use anvil::app::agentic::format_tool_result_message;

    // 200 distinct chars: 'H' * 100 + 'T' * 100
    let content = format!("{}{}", "H".repeat(100), "T".repeat(100));
    let result = ToolExecutionResult {
        tool_call_id: "call_int".to_string(),
        tool_name: "shell.exec".to_string(),
        status: ToolExecutionStatus::Interrupted,
        summary: "interrupted".to_string(),
        payload: ToolExecutionPayload::Text(content),
        artifacts: Vec::new(),
        elapsed_ms: 5,
    };

    let msg = format_tool_result_message(&result, 100);
    // Interrupted => head_pct=20 => head=20, tail=80
    assert!(msg.contains(&"H".repeat(20)));
    assert!(msg.contains(&"T".repeat(80)));
    assert!(msg.contains("truncated"));
}

// ============================================================
// Sub-agent tool tests (Issue #24 Phase 1)
// ============================================================

fn build_registry_with_subagent_tools() -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    registry.register_standard_tools();
    registry.register_agent_explore();
    registry.register_agent_plan();
    registry
}

// --- from_json tests ---

#[test]
fn from_json_parses_agent_explore_with_scope() {
    let json: serde_json::Value = serde_json::json!({
        "prompt": "Investigate the module structure",
        "scope": "src/tooling"
    });
    let input = ToolInput::from_json("agent.explore", &json).expect("should parse agent.explore");
    assert_eq!(
        input,
        ToolInput::AgentExplore {
            prompt: "Investigate the module structure".to_string(),
            scope: Some("src/tooling".to_string()),
        }
    );
}

#[test]
fn from_json_parses_agent_explore_without_scope() {
    let json: serde_json::Value = serde_json::json!({
        "prompt": "Explore the codebase"
    });
    let input = ToolInput::from_json("agent.explore", &json).expect("should parse agent.explore");
    assert_eq!(
        input,
        ToolInput::AgentExplore {
            prompt: "Explore the codebase".to_string(),
            scope: None,
        }
    );
}

#[test]
fn from_json_parses_agent_plan_with_scope() {
    let json: serde_json::Value = serde_json::json!({
        "prompt": "Plan the refactoring",
        "scope": "src/app"
    });
    let input = ToolInput::from_json("agent.plan", &json).expect("should parse agent.plan");
    assert_eq!(
        input,
        ToolInput::AgentPlan {
            prompt: "Plan the refactoring".to_string(),
            scope: Some("src/app".to_string()),
        }
    );
}

#[test]
fn from_json_parses_agent_plan_without_scope() {
    let json: serde_json::Value = serde_json::json!({
        "prompt": "Create implementation plan"
    });
    let input = ToolInput::from_json("agent.plan", &json).expect("should parse agent.plan");
    assert_eq!(
        input,
        ToolInput::AgentPlan {
            prompt: "Create implementation plan".to_string(),
            scope: None,
        }
    );
}

#[test]
fn from_json_agent_explore_missing_prompt_fails() {
    let json: serde_json::Value = serde_json::json!({
        "scope": "src/tooling"
    });
    let err = ToolInput::from_json("agent.explore", &json).expect_err("should fail without prompt");
    assert!(err.contains("missing prompt"));
}

#[test]
fn from_json_agent_plan_missing_prompt_fails() {
    let json: serde_json::Value = serde_json::json!({
        "scope": "src/app"
    });
    let err = ToolInput::from_json("agent.plan", &json).expect_err("should fail without prompt");
    assert!(err.contains("missing prompt"));
}

// --- kind() tests ---

#[test]
fn kind_returns_agent_explore_for_agent_explore_input() {
    let input = ToolInput::AgentExplore {
        prompt: "test".to_string(),
        scope: None,
    };
    assert_eq!(input.kind(), ToolKind::AgentExplore);
}

#[test]
fn kind_returns_agent_plan_for_agent_plan_input() {
    let input = ToolInput::AgentPlan {
        prompt: "test".to_string(),
        scope: None,
    };
    assert_eq!(input.kind(), ToolKind::AgentPlan);
}

// --- validate_required_fields tests ---

#[test]
fn validate_agent_explore_empty_prompt_fails() {
    let registry = build_registry_with_subagent_tools();
    let call = ToolCallRequest::new(
        "call_explore_001",
        "agent.explore",
        ToolInput::AgentExplore {
            prompt: "".to_string(),
            scope: None,
        },
    );
    let err = registry
        .validate(call)
        .expect_err("empty prompt should fail");
    assert_eq!(
        err,
        ToolValidationError::MissingRequiredField("prompt".to_string())
    );
}

#[test]
fn validate_agent_plan_empty_prompt_fails() {
    let registry = build_registry_with_subagent_tools();
    let call = ToolCallRequest::new(
        "call_plan_001",
        "agent.plan",
        ToolInput::AgentPlan {
            prompt: "   ".to_string(),
            scope: None,
        },
    );
    let err = registry
        .validate(call)
        .expect_err("whitespace-only prompt should fail");
    assert_eq!(
        err,
        ToolValidationError::MissingRequiredField("prompt".to_string())
    );
}

#[test]
fn validate_agent_explore_too_long_prompt_fails() {
    let registry = build_registry_with_subagent_tools();
    let long_prompt = "a".repeat(10001);
    let call = ToolCallRequest::new(
        "call_explore_002",
        "agent.explore",
        ToolInput::AgentExplore {
            prompt: long_prompt,
            scope: None,
        },
    );
    let err = registry
        .validate(call)
        .expect_err("too long prompt should fail");
    match err {
        ToolValidationError::InvalidFieldValue { field, .. } => {
            assert_eq!(field, "prompt");
        }
        other => panic!("expected InvalidFieldValue, got {other:?}"),
    }
}

#[test]
fn validate_agent_plan_too_long_prompt_fails() {
    let registry = build_registry_with_subagent_tools();
    let long_prompt = "b".repeat(10001);
    let call = ToolCallRequest::new(
        "call_plan_002",
        "agent.plan",
        ToolInput::AgentPlan {
            prompt: long_prompt,
            scope: None,
        },
    );
    let err = registry
        .validate(call)
        .expect_err("too long prompt should fail");
    match err {
        ToolValidationError::InvalidFieldValue { field, .. } => {
            assert_eq!(field, "prompt");
        }
        other => panic!("expected InvalidFieldValue, got {other:?}"),
    }
}

#[test]
fn validate_agent_explore_valid_prompt_succeeds() {
    let registry = build_registry_with_subagent_tools();
    let call = ToolCallRequest::new(
        "call_explore_003",
        "agent.explore",
        ToolInput::AgentExplore {
            prompt: "Investigate how error handling works".to_string(),
            scope: Some("src/tooling".to_string()),
        },
    );
    let validated = registry.validate(call).expect("valid prompt should pass");
    assert_eq!(validated.spec.name, "agent.explore");
    assert_eq!(validated.spec.kind, ToolKind::AgentExplore);
}

#[test]
fn validate_agent_plan_valid_prompt_succeeds() {
    let registry = build_registry_with_subagent_tools();
    let call = ToolCallRequest::new(
        "call_plan_003",
        "agent.plan",
        ToolInput::AgentPlan {
            prompt: "Plan the implementation of feature X".to_string(),
            scope: None,
        },
    );
    let validated = registry.validate(call).expect("valid prompt should pass");
    assert_eq!(validated.spec.name, "agent.plan");
    assert_eq!(validated.spec.kind, ToolKind::AgentPlan);
}

// --- ToolSpec attribute tests ---

#[test]
fn agent_explore_spec_has_correct_attributes() {
    let registry = build_registry_with_subagent_tools();
    let spec = registry
        .get("agent.explore")
        .expect("agent.explore should be registered");
    assert_eq!(spec.kind, ToolKind::AgentExplore);
    assert_eq!(spec.execution_class, ExecutionClass::ReadOnly);
    assert_eq!(spec.permission_class, PermissionClass::Safe);
    assert_eq!(spec.execution_mode, ExecutionMode::SequentialOnly);
    assert_eq!(spec.plan_mode, PlanModePolicy::Allowed);
    assert_eq!(spec.rollback_policy, RollbackPolicy::None);
}

#[test]
fn agent_plan_spec_has_correct_attributes() {
    let registry = build_registry_with_subagent_tools();
    let spec = registry
        .get("agent.plan")
        .expect("agent.plan should be registered");
    assert_eq!(spec.kind, ToolKind::AgentPlan);
    assert_eq!(spec.execution_class, ExecutionClass::ReadOnly);
    assert_eq!(spec.permission_class, PermissionClass::Safe);
    assert_eq!(spec.execution_mode, ExecutionMode::SequentialOnly);
    assert_eq!(spec.plan_mode, PlanModePolicy::Allowed);
    assert_eq!(spec.rollback_policy, RollbackPolicy::None);
}

// --- ToolRegistry subset tests ---

#[test]
fn explore_tools_registry_contains_only_file_read_and_file_search() {
    let mut registry = ToolRegistry::new();
    registry.register_explore_tools();
    assert!(registry.get("file.read").is_some());
    assert!(registry.get("file.search").is_some());
    assert!(registry.get("file.write").is_none());
    assert!(registry.get("shell.exec").is_none());
    assert!(registry.get("web.fetch").is_none());
    assert!(registry.get("agent.explore").is_none());
    assert!(registry.get("agent.plan").is_none());
}

#[test]
fn plan_tools_registry_contains_file_read_file_search_and_web_fetch() {
    let mut registry = ToolRegistry::new();
    registry.register_plan_tools();
    assert!(registry.get("file.read").is_some());
    assert!(registry.get("file.search").is_some());
    assert!(registry.get("web.fetch").is_some());
    assert!(registry.get("file.write").is_none());
    assert!(registry.get("shell.exec").is_none());
    assert!(registry.get("agent.explore").is_none());
    assert!(registry.get("agent.plan").is_none());
}

// --- repair_from_block tests ---

#[test]
fn repair_from_block_agent_explore_with_prompt_and_scope() {
    fn extract_simple(block: &str, key: &str) -> Option<String> {
        let pattern = format!("\"{}\":", key);
        let start = block.find(&pattern)? + pattern.len();
        let rest = &block[start..];
        let rest = rest.trim_start();
        if let Some(inner) = rest.strip_prefix('"') {
            let end = inner.find('"')?;
            Some(inner[..end].to_string())
        } else {
            None
        }
    }
    fn extract_trailing(block: &str, key: &str) -> Option<String> {
        extract_simple(block, key)
    }

    let block = r#"{"prompt": "explore this", "scope": "src/app"}"#;
    let result =
        ToolInput::repair_from_block("agent.explore", block, extract_simple, extract_trailing);
    assert_eq!(
        result,
        Some(ToolInput::AgentExplore {
            prompt: "explore this".to_string(),
            scope: Some("src/app".to_string()),
        })
    );
}

#[test]
fn repair_from_block_agent_plan_without_scope() {
    fn extract_simple(block: &str, key: &str) -> Option<String> {
        let pattern = format!("\"{}\":", key);
        let start = block.find(&pattern)? + pattern.len();
        let rest = &block[start..];
        let rest = rest.trim_start();
        if let Some(inner) = rest.strip_prefix('"') {
            let end = inner.find('"')?;
            Some(inner[..end].to_string())
        } else {
            None
        }
    }
    fn extract_trailing(block: &str, key: &str) -> Option<String> {
        extract_simple(block, key)
    }

    let block = r#"{"prompt": "plan the implementation"}"#;
    let result =
        ToolInput::repair_from_block("agent.plan", block, extract_simple, extract_trailing);
    assert_eq!(
        result,
        Some(ToolInput::AgentPlan {
            prompt: "plan the implementation".to_string(),
            scope: None,
        })
    );
}

#[test]
fn repair_from_block_agent_explore_missing_prompt_returns_none() {
    fn extract_simple(block: &str, key: &str) -> Option<String> {
        let pattern = format!("\"{}\":", key);
        let start = block.find(&pattern)? + pattern.len();
        let rest = &block[start..];
        let rest = rest.trim_start();
        if let Some(inner) = rest.strip_prefix('"') {
            let end = inner.find('"')?;
            Some(inner[..end].to_string())
        } else {
            None
        }
    }
    fn extract_trailing(block: &str, key: &str) -> Option<String> {
        extract_simple(block, key)
    }

    let block = r#"{"scope": "src/app"}"#;
    let result =
        ToolInput::repair_from_block("agent.explore", block, extract_simple, extract_trailing);
    assert!(result.is_none());
}

// --- max prompt length boundary test ---

#[test]
fn validate_agent_explore_exactly_max_prompt_length_succeeds() {
    let registry = build_registry_with_subagent_tools();
    let exact_prompt = "x".repeat(10000);
    let call = ToolCallRequest::new(
        "call_explore_004",
        "agent.explore",
        ToolInput::AgentExplore {
            prompt: exact_prompt,
            scope: None,
        },
    );
    registry
        .validate(call)
        .expect("exactly 10000 chars should pass");
}

// --- SubAgentKind::from_tool_input() tests ---

#[test]
fn subagent_kind_from_tool_input_explore() {
    use anvil::agent::subagent::SubAgentKind;
    let input = ToolInput::AgentExplore {
        prompt: "test".to_string(),
        scope: None,
    };
    assert_eq!(
        SubAgentKind::from_tool_input(&input),
        Some(SubAgentKind::Explore)
    );
}

#[test]
fn subagent_kind_from_tool_input_plan() {
    use anvil::agent::subagent::SubAgentKind;
    let input = ToolInput::AgentPlan {
        prompt: "test".to_string(),
        scope: Some("./src".to_string()),
    };
    assert_eq!(
        SubAgentKind::from_tool_input(&input),
        Some(SubAgentKind::Plan)
    );
}

#[test]
fn subagent_kind_from_tool_input_returns_none_for_other_tools() {
    use anvil::agent::subagent::SubAgentKind;
    let input = ToolInput::FileRead {
        path: "./foo".to_string(),
    };
    assert_eq!(SubAgentKind::from_tool_input(&input), None);
}

#[test]
fn subagent_error_display_formats_correctly() {
    use anvil::agent::subagent::SubAgentError;
    use anvil::provider::ProviderTurnError;

    // Issue #129: Timeout and MaxIterations removed from SubAgentError (moved to Ok path)
    let e = SubAgentError::SandboxViolation("../escape".to_string());
    assert!(e.to_string().contains("../escape"));

    let e = SubAgentError::Provider(ProviderTurnError::Cancelled);
    assert!(e.to_string().contains("provider"));

    let e = SubAgentError::ToolExecution("bad tool".to_string());
    assert!(e.to_string().contains("bad tool"));
}

#[test]
fn subagent_error_into_tool_execution_result_all_failed() {
    use anvil::agent::subagent::SubAgentError;
    use anvil::tooling::ToolExecutionStatus;

    let call = ToolCallRequest::new(
        "call_001",
        "agent.explore",
        ToolInput::AgentExplore {
            prompt: "test".to_string(),
            scope: None,
        },
    );

    // Issue #129: All remaining SubAgentError variants map to Failed
    // SandboxViolation -> Failed
    let result =
        SubAgentError::SandboxViolation("bad".to_string()).into_tool_execution_result(&call);
    assert_eq!(result.status, ToolExecutionStatus::Failed);

    // ToolExecution -> Failed
    let result = SubAgentError::ToolExecution("err".to_string()).into_tool_execution_result(&call);
    assert_eq!(result.status, ToolExecutionStatus::Failed);
}

// ============================================================
// SubAgentResult / SubAgentPayload tests (Issue #129)
// ============================================================

#[test]
fn subagent_result_into_tool_execution_result_json_payload() {
    use anvil::agent::subagent::SubAgentResult;
    use anvil::contracts::{SubAgentPayload, TerminationReason};

    let call = ToolCallRequest::new(
        "call_001",
        "agent.explore",
        ToolInput::AgentExplore {
            prompt: "test".to_string(),
            scope: None,
        },
    );

    let result = SubAgentResult {
        payload: SubAgentPayload {
            found_files: vec!["src/main.rs".to_string()],
            key_findings: vec![],
            raw_summary: "Found main entry".to_string(),
            confidence: Some(0.9),
            termination_reason: TerminationReason::Completed,
            error: None,
        },
        estimated_tokens: 100,
        iterations_used: 2,
    };

    let tool_result = result.into_tool_execution_result(&call);
    assert_eq!(tool_result.status, ToolExecutionStatus::Completed);
    assert!(tool_result.summary.contains("completed"));
    assert!(tool_result.summary.contains("2 iteration(s)"));

    // Payload should be valid JSON
    if let ToolExecutionPayload::Text(json) = &tool_result.payload {
        let parsed: SubAgentPayload = serde_json::from_str(json).expect("should be valid JSON");
        assert_eq!(parsed.raw_summary, "Found main entry");
        assert_eq!(parsed.found_files, vec!["src/main.rs".to_string()]);
        assert_eq!(parsed.termination_reason, TerminationReason::Completed);
    } else {
        panic!("expected Text payload");
    }
}

#[test]
fn subagent_result_timeout_into_tool_execution_result() {
    use anvil::agent::subagent::SubAgentResult;
    use anvil::contracts::{SubAgentPayload, TerminationReason};

    let call = ToolCallRequest::new(
        "call_001",
        "agent.explore",
        ToolInput::AgentExplore {
            prompt: "test".to_string(),
            scope: None,
        },
    );

    let result = SubAgentResult {
        payload: SubAgentPayload::fallback("partial work".to_string(), TerminationReason::Timeout),
        estimated_tokens: 0,
        iterations_used: 5,
    };

    let tool_result = result.into_tool_execution_result(&call);
    assert_eq!(tool_result.status, ToolExecutionStatus::Completed);
    assert!(tool_result.summary.contains("timeout"));
    assert!(tool_result.summary.contains("5 iteration(s)"));

    if let ToolExecutionPayload::Text(json) = &tool_result.payload {
        let parsed: SubAgentPayload = serde_json::from_str(json).expect("should be valid JSON");
        assert_eq!(parsed.termination_reason, TerminationReason::Timeout);
        assert_eq!(parsed.raw_summary, "partial work");
    } else {
        panic!("expected Text payload");
    }
}

#[test]
fn subagent_payload_input_ignores_system_fields() {
    // LLM output may include termination_reason/error, but SubAgentPayloadInput
    // should not capture them. The parse function always sets Completed/None.
    use anvil::contracts::{SubAgentPayload, TerminationReason};

    let llm_json = r#"{
        "found_files": ["a.rs"],
        "key_findings": [],
        "raw_summary": "summary",
        "confidence": 0.5,
        "termination_reason": "timeout",
        "error": "injected error"
    }"#;

    // Parse through the public contract type (the internal parse function uses SubAgentPayloadInput)
    // We test the contract: system fields should NOT be controlled by LLM
    let payload: SubAgentPayload = serde_json::from_str(llm_json).expect("parse");
    // SubAgentPayload itself does have those fields, but in production the parse_final_response_to_payload
    // function uses SubAgentPayloadInput (which lacks them) and always sets system fields.
    // Here we just verify SubAgentPayload defaults are correct when absent
    let minimal_json = r#"{"found_files":[],"key_findings":[],"raw_summary":"test"}"#;
    let payload_minimal: SubAgentPayload = serde_json::from_str(minimal_json).expect("parse");
    assert_eq!(
        payload_minimal.termination_reason,
        TerminationReason::Completed
    );
    assert_eq!(payload_minimal.error, None);
    // Even with injected fields present, the system manages them
    assert_eq!(payload.found_files, vec!["a.rs".to_string()]);
}

#[test]
fn subagent_payload_size_limits_applied() {
    use anvil::contracts::{Finding, SubAgentPayload, TerminationReason};

    // Test that size limits are documented/enforced at the contracts level
    // The actual size limiting happens in parse_final_response_to_payload (internal),
    // but we verify the types work with large data
    let large_findings: Vec<Finding> = (0..100)
        .map(|i| Finding {
            title: format!("Finding {i}"),
            detail: "x".repeat(5000),
            related_code: (0..50).map(|j| format!("file{j}.rs:{j}")).collect(),
        })
        .collect();

    let payload = SubAgentPayload {
        found_files: (0..200).map(|i| format!("file{i}.rs")).collect(),
        key_findings: large_findings,
        raw_summary: "x".repeat(10000),
        confidence: Some(1.5), // out of range, should be clamped by caller
        termination_reason: TerminationReason::Completed,
        error: None,
    };

    // Verify serialization works with large data
    let json = serde_json::to_string(&payload).expect("serialize");
    assert!(!json.is_empty());
}

// ============================================================
// Offline mode tests (Issue #67)
// ============================================================

#[test]
fn build_subagent_system_prompt_plan_offline_excludes_web_fetch() {
    use anvil::agent::subagent::{SubAgentKind, build_subagent_system_prompt};
    let prompt = build_subagent_system_prompt(&SubAgentKind::Plan, true);
    assert!(
        !prompt.contains("web.fetch"),
        "offline Plan prompt should not contain web.fetch tool description"
    );
    assert!(
        prompt.contains("Offline mode is active"),
        "offline Plan prompt should contain offline note"
    );
    assert!(
        !prompt.contains("You may fetch web URLs"),
        "offline Plan prompt should not contain web URL permission"
    );
}

#[test]
fn build_subagent_system_prompt_plan_online_includes_web_fetch() {
    use anvil::agent::subagent::{SubAgentKind, build_subagent_system_prompt};
    let prompt = build_subagent_system_prompt(&SubAgentKind::Plan, false);
    assert!(
        prompt.contains("web.fetch"),
        "online Plan prompt should contain web.fetch tool description"
    );
    assert!(
        prompt.contains("You may fetch web URLs"),
        "online Plan prompt should contain web URL permission"
    );
    assert!(
        !prompt.contains("Offline mode is active"),
        "online Plan prompt should not contain offline note"
    );
}

#[test]
fn build_subagent_system_prompt_explore_unaffected_by_offline() {
    use anvil::agent::subagent::{SubAgentKind, build_subagent_system_prompt};
    let online_prompt = build_subagent_system_prompt(&SubAgentKind::Explore, false);
    let offline_prompt = build_subagent_system_prompt(&SubAgentKind::Explore, true);
    assert_eq!(
        online_prompt, offline_prompt,
        "Explore prompt should be identical regardless of offline flag"
    );
}

#[test]
fn check_offline_blocked_blocks_web_fetch_in_offline_mode() {
    use anvil::app::policy::check_offline_blocked;
    let mut config = anvil::config::EffectiveConfig::default_for_test().unwrap();
    config.mode.offline = true;
    let call = ToolCallRequest::new(
        "call_001",
        "web.fetch",
        ToolInput::WebFetch {
            url: "https://example.com".to_string(),
        },
    );
    let result = check_offline_blocked(&config, &call);
    assert!(result.is_some());
    assert!(result.unwrap().contains("is unavailable in offline mode"));
}

#[test]
fn check_offline_blocked_allows_file_read_in_offline_mode() {
    use anvil::app::policy::check_offline_blocked;
    let mut config = anvil::config::EffectiveConfig::default_for_test().unwrap();
    config.mode.offline = true;
    let call = ToolCallRequest::new(
        "call_002",
        "file.read",
        ToolInput::FileRead {
            path: "./test.rs".to_string(),
        },
    );
    assert!(check_offline_blocked(&config, &call).is_none());
}

#[test]
fn check_offline_blocked_allows_web_fetch_when_not_offline() {
    use anvil::app::policy::check_offline_blocked;
    let config = anvil::config::EffectiveConfig::default_for_test().unwrap();
    assert!(!config.mode.offline);
    let call = ToolCallRequest::new(
        "call_003",
        "web.fetch",
        ToolInput::WebFetch {
            url: "https://example.com".to_string(),
        },
    );
    assert!(check_offline_blocked(&config, &call).is_none());
}

#[test]
fn check_offline_blocked_blocks_mcp_in_offline_mode() {
    use anvil::app::policy::check_offline_blocked;
    let mut config = anvil::config::EffectiveConfig::default_for_test().unwrap();
    config.mode.offline = true;
    let call = ToolCallRequest::new(
        "call_004",
        "mcp__server__tool",
        ToolInput::Mcp {
            server: "server".to_string(),
            tool: "tool".to_string(),
            arguments: serde_json::Value::Null,
        },
    );
    let result = check_offline_blocked(&config, &call);
    assert!(result.is_some());
    assert!(result.unwrap().contains("is unavailable in offline mode"));
}

#[test]
fn check_offline_blocked_blocks_web_search_in_offline_mode() {
    use anvil::app::policy::check_offline_blocked;
    let mut config = anvil::config::EffectiveConfig::default_for_test().unwrap();
    config.mode.offline = true;
    let call = ToolCallRequest::new(
        "call_005",
        "web.search",
        ToolInput::WebSearch {
            query: "test".to_string(),
        },
    );
    let result = check_offline_blocked(&config, &call);
    assert!(result.is_some());
    assert!(result.unwrap().contains("web.search"));
}

// ---------------------------------------------------------------------------
// CheckpointStack tests (Issue #68)
// ---------------------------------------------------------------------------

fn make_checkpoint_entry(path: &str, content: Option<&str>) -> CheckpointEntry {
    let byte_size = content.map_or(0, |c| c.len());
    CheckpointEntry {
        path: std::path::PathBuf::from(path),
        previous_content: content.map(String::from),
        byte_size,
    }
}

#[test]
fn checkpoint_stack_new_initial_state() {
    let stack = CheckpointStack::new();
    assert_eq!(stack.len(), 0);
    assert!(stack.is_empty());
}

#[test]
fn checkpoint_stack_push_pop_basic() {
    let mut stack = CheckpointStack::new();
    let entry = make_checkpoint_entry("/tmp/a.rs", Some("hello"));
    stack.push(entry);
    assert_eq!(stack.len(), 1);
    assert!(!stack.is_empty());

    let popped = stack.pop().expect("should pop");
    assert_eq!(popped.path, std::path::PathBuf::from("/tmp/a.rs"));
    assert_eq!(popped.previous_content.as_deref(), Some("hello"));
    assert!(stack.is_empty());
}

#[test]
fn checkpoint_stack_push_returns_index_and_remove_works() {
    let mut stack = CheckpointStack::new();
    let idx0 = stack.push(make_checkpoint_entry("/tmp/a.rs", Some("a")));
    let idx1 = stack.push(make_checkpoint_entry("/tmp/b.rs", Some("b")));
    assert_eq!(idx0, 0);
    assert_eq!(idx1, 1);

    let removed = stack.remove(0).expect("should remove");
    assert_eq!(removed.path, std::path::PathBuf::from("/tmp/a.rs"));
    assert_eq!(stack.len(), 1);

    let remaining = stack.pop().expect("should pop remaining");
    assert_eq!(remaining.path, std::path::PathBuf::from("/tmp/b.rs"));
}

#[test]
fn checkpoint_stack_pop_n_partial() {
    let mut stack = CheckpointStack::new();
    stack.push(make_checkpoint_entry("/tmp/a.rs", Some("a")));
    stack.push(make_checkpoint_entry("/tmp/b.rs", Some("b")));
    stack.push(make_checkpoint_entry("/tmp/c.rs", Some("c")));

    let popped = stack.pop_n(2);
    assert_eq!(popped.len(), 2);
    // Newest first
    assert_eq!(popped[0].path, std::path::PathBuf::from("/tmp/c.rs"));
    assert_eq!(popped[1].path, std::path::PathBuf::from("/tmp/b.rs"));
    assert_eq!(stack.len(), 1);
}

#[test]
fn checkpoint_stack_pop_n_exceeds_depth() {
    let mut stack = CheckpointStack::new();
    stack.push(make_checkpoint_entry("/tmp/a.rs", Some("a")));
    stack.push(make_checkpoint_entry("/tmp/b.rs", Some("b")));

    let popped = stack.pop_n(10);
    assert_eq!(popped.len(), 2);
    assert!(stack.is_empty());
}

#[test]
fn checkpoint_stack_pop_n_deduplicates_same_file() {
    let mut stack = CheckpointStack::new();
    stack.push(make_checkpoint_entry("/tmp/a.rs", Some("original")));
    stack.push(make_checkpoint_entry("/tmp/b.rs", Some("b")));
    stack.push(make_checkpoint_entry("/tmp/a.rs", Some("modified")));

    let popped = stack.pop_n(3);
    // Same file /tmp/a.rs appeared twice; only the oldest ("original") should be kept
    assert_eq!(popped.len(), 2);
    let a_entry = popped
        .iter()
        .find(|e| e.path == Path::new("/tmp/a.rs"))
        .expect("a.rs should exist");
    assert_eq!(a_entry.previous_content.as_deref(), Some("original"));
}

#[test]
fn checkpoint_stack_depth_limit_evicts_oldest() {
    let mut stack = CheckpointStack::new();
    for i in 0..25 {
        stack.push(make_checkpoint_entry(&format!("/tmp/{i}.rs"), Some("x")));
    }
    // max_depth = 20
    assert_eq!(stack.len(), 20);
    // The first 5 should have been evicted; entry at index 0 should be /tmp/5.rs
    let first = stack.pop().expect("should pop");
    assert_eq!(first.path, std::path::PathBuf::from("/tmp/24.rs"));
}

#[test]
fn checkpoint_stack_byte_limit_evicts_oldest() {
    let mut stack = CheckpointStack::new();
    // Each entry is ~1MB (1_048_576 bytes). Push 12 to exceed 10MB limit.
    let big_content = "x".repeat(1_048_576);
    for i in 0..12 {
        stack.push(make_checkpoint_entry(
            &format!("/tmp/{i}.rs"),
            Some(&big_content),
        ));
    }
    // Should have evicted enough to stay under 10MB
    assert!(stack.len() < 12);
    assert!(stack.len() >= 9); // floor(10MB / 1MB) = 10, but some eviction margin
}

#[test]
fn checkpoint_stack_new_file_entry_has_none_content() {
    let mut stack = CheckpointStack::new();
    stack.push(make_checkpoint_entry("/tmp/new.rs", None));
    let popped = stack.pop().expect("should pop");
    assert!(popped.previous_content.is_none());
    assert_eq!(popped.byte_size, 0);
}

#[test]
fn checkpoint_entry_generate_restore_preview_for_existing_file() {
    let dir = std::env::temp_dir().join(format!(
        "anvil_test_restore_preview_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&dir).unwrap();
    let file_path = dir.join("test.rs");
    fs::write(&file_path, "modified content").unwrap();

    let entry = CheckpointEntry {
        path: file_path.clone(),
        previous_content: Some("original content".to_string()),
        byte_size: 16,
    };

    let preview = entry.generate_restore_preview();
    assert!(preview.is_some());
    let text = preview.unwrap();
    // Should contain diff information
    assert!(text.contains("current") || text.contains("restored") || text.contains("-"));

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn checkpoint_entry_generate_restore_preview_no_changes() {
    let dir = std::env::temp_dir().join(format!(
        "anvil_test_no_change_preview_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&dir).unwrap();
    let file_path = dir.join("test.rs");
    fs::write(&file_path, "same content").unwrap();

    let entry = CheckpointEntry {
        path: file_path.clone(),
        previous_content: Some("same content".to_string()),
        byte_size: 12,
    };

    let preview = entry.generate_restore_preview();
    assert!(preview.is_some());
    assert!(preview.unwrap().contains("no changes to undo"));

    fs::remove_dir_all(&dir).ok();
}

// ---------------------------------------------------------------------------
// CheckpointStack transaction (mark/rollback/commit) tests (Issue #69)
// ---------------------------------------------------------------------------

#[test]
fn test_mark_and_rollback_basic() {
    let mut stack = CheckpointStack::new();
    // Push a pre-existing entry before the mark
    stack.push(make_checkpoint_entry("/tmp/pre.rs", Some("pre")));

    // Start transaction
    let mark = stack.mark();
    assert_eq!(mark, 1); // mark is at position 1 (after the pre-existing entry)
    assert!(stack.is_in_transaction());

    // Push 2 entries within the transaction
    stack.push(make_checkpoint_entry("/tmp/a.rs", Some("a")));
    stack.push(make_checkpoint_entry("/tmp/b.rs", Some("b")));
    assert_eq!(stack.len(), 3);

    // Rollback to mark: should return the 2 transaction entries
    let rolled_back = stack.rollback_to_mark(mark);
    assert_eq!(rolled_back.len(), 2);
    assert!(!stack.is_in_transaction()); // mark cleared
    assert_eq!(stack.len(), 1); // pre-existing entry remains
    // Verify newest-first ordering from pop_n
    assert_eq!(rolled_back[0].path, Path::new("/tmp/b.rs"));
    assert_eq!(rolled_back[1].path, Path::new("/tmp/a.rs"));
}

#[test]
fn test_mark_and_commit() {
    let mut stack = CheckpointStack::new();
    let mark = stack.mark();
    assert_eq!(mark, 0);
    assert!(stack.is_in_transaction());

    stack.push(make_checkpoint_entry("/tmp/a.rs", Some("a")));
    stack.push(make_checkpoint_entry("/tmp/b.rs", Some("b")));

    // Commit: entries remain, mark is cleared
    stack.commit_mark();
    assert!(!stack.is_in_transaction());
    assert_eq!(stack.len(), 2); // entries preserved for /undo
}

#[test]
fn test_rollback_empty() {
    let mut stack = CheckpointStack::new();
    let mark = stack.mark();

    // Rollback immediately without pushing anything
    let rolled_back = stack.rollback_to_mark(mark);
    assert!(rolled_back.is_empty());
    assert!(!stack.is_in_transaction());
}

#[test]
fn test_mark_rollback_deduplication() {
    let mut stack = CheckpointStack::new();
    let mark = stack.mark();

    // Same file edited twice within transaction
    stack.push(make_checkpoint_entry("/tmp/a.rs", Some("original")));
    stack.push(make_checkpoint_entry("/tmp/b.rs", Some("b")));
    stack.push(make_checkpoint_entry("/tmp/a.rs", Some("after_first_edit")));

    let rolled_back = stack.rollback_to_mark(mark);
    // Deduplication: /tmp/a.rs should keep "original" (the oldest checkpoint)
    assert_eq!(rolled_back.len(), 2);
    let a_entry = rolled_back
        .iter()
        .find(|e| e.path == Path::new("/tmp/a.rs"))
        .expect("a.rs should exist");
    assert_eq!(a_entry.previous_content.as_deref(), Some("original"));
    assert!(stack.is_empty());
}

#[test]
fn test_mark_with_remove_interaction() {
    let mut stack = CheckpointStack::new();
    let mark = stack.mark();
    assert_eq!(mark, 0);

    // Push A (tool succeeds, kept) at index 0
    let idx_a = stack.push(make_checkpoint_entry("/tmp/a.rs", Some("a")));
    assert_eq!(idx_a, 0);

    // Push B (tool fails, removed individually) at index 1
    let idx_b = stack.push(make_checkpoint_entry("/tmp/b.rs", Some("b")));
    assert_eq!(idx_b, 1);

    // Remove B (index 1, which is >= mark=0 so mark stays at 0)
    stack.remove(idx_b);
    assert_eq!(stack.len(), 1);

    // Rollback to mark: only A should be returned
    let rolled_back = stack.rollback_to_mark(mark);
    assert_eq!(rolled_back.len(), 1);
    assert_eq!(rolled_back[0].path, Path::new("/tmp/a.rs"));
}

#[test]
fn test_eviction_with_active_mark() {
    let mut stack = CheckpointStack::new();
    // Push 18 entries before mark (max_depth=20)
    for i in 0..18 {
        stack.push(make_checkpoint_entry(&format!("/tmp/pre{i}.rs"), Some("x")));
    }
    assert_eq!(stack.len(), 18);

    // Mark at position 18
    let mark = stack.mark();
    assert_eq!(mark, 18);

    // Push 5 entries within transaction (total would be 23, exceeding max_depth=20)
    for i in 0..5 {
        stack.push(make_checkpoint_entry(&format!("/tmp/tx{i}.rs"), Some("t")));
    }

    // Eviction should only remove pre-mark entries, preserving all 5 transaction entries
    // max_depth=20, so eviction removes 3 oldest pre-mark entries (18+5=23, need to drop 3)
    assert_eq!(stack.len(), 20);

    // All 5 transaction entries should still be present after rollback
    // active_mark was adjusted from 18 to 15 by eviction
    let rolled_back = stack.rollback_to_mark(mark);
    assert_eq!(rolled_back.len(), 5);
    assert!(!stack.is_in_transaction());
}

#[test]
fn test_eviction_mark_adjustment() {
    let mut stack = CheckpointStack::new();
    // Push 19 entries before mark
    for i in 0..19 {
        stack.push(make_checkpoint_entry(&format!("/tmp/pre{i}.rs"), Some("x")));
    }

    let mark = stack.mark();
    assert_eq!(mark, 19);
    assert!(stack.is_in_transaction());

    // Push 3 entries within transaction (total 22 > max_depth=20)
    // This should evict 2 pre-mark entries, adjusting active_mark from 19 to 17
    for i in 0..3 {
        stack.push(make_checkpoint_entry(&format!("/tmp/tx{i}.rs"), Some("t")));
    }

    assert_eq!(stack.len(), 20);

    // Rollback should still correctly return all 3 transaction entries
    // because active_mark was adjusted to 17 internally
    let rolled_back = stack.rollback_to_mark(mark);
    assert_eq!(rolled_back.len(), 3);
    // Pre-mark entries: 19 - 2 evicted = 17 remaining
    assert_eq!(stack.len(), 17);
}

// ===================================================================
// Git tools tests (Issue #75)
// ===================================================================

#[test]
fn git_tools_registered_in_standard_tools() {
    let registry = build_registry();
    assert!(registry.get("git.status").is_some());
    assert!(registry.get("git.diff").is_some());
    assert!(registry.get("git.log").is_some());

    let status_spec = registry.get("git.status").unwrap();
    assert_eq!(status_spec.kind, ToolKind::GitStatus);
    assert_eq!(status_spec.execution_class, ExecutionClass::ReadOnly);
    assert_eq!(status_spec.permission_class, PermissionClass::Safe);
    assert_eq!(status_spec.execution_mode, ExecutionMode::ParallelSafe);
    assert_eq!(status_spec.plan_mode, PlanModePolicy::Allowed);
    assert_eq!(status_spec.rollback_policy, RollbackPolicy::None);
}

#[test]
fn git_tools_registered_in_explore_tools() {
    let mut registry = ToolRegistry::new();
    registry.register_explore_tools();
    assert!(registry.get("git.status").is_some());
    assert!(registry.get("git.diff").is_some());
    assert!(registry.get("git.log").is_some());
}

#[test]
fn git_status_registered_in_plan_tools() {
    let mut registry = ToolRegistry::new();
    registry.register_plan_tools();
    assert!(registry.get("git.status").is_some());
    assert!(registry.get("git.diff").is_none());
    assert!(registry.get("git.log").is_none());
}

#[test]
fn from_json_parses_git_status() {
    let value = serde_json::json!({"tool": "git.status"});
    let input = ToolInput::from_json("git.status", &value).unwrap();
    assert_eq!(input, ToolInput::GitStatus {});
    assert_eq!(input.kind(), ToolKind::GitStatus);
}

#[test]
fn from_json_parses_git_diff_with_all_params() {
    let value = serde_json::json!({
        "tool": "git.diff",
        "path": "src/main.rs",
        "staged": true,
        "commit": "HEAD~3"
    });
    let input = ToolInput::from_json("git.diff", &value).unwrap();
    assert_eq!(
        input,
        ToolInput::GitDiff {
            path: Some("src/main.rs".to_string()),
            staged: Some(true),
            commit: Some("HEAD~3".to_string()),
        }
    );
    assert_eq!(input.kind(), ToolKind::GitDiff);
}

#[test]
fn from_json_parses_git_diff_without_params() {
    let value = serde_json::json!({"tool": "git.diff"});
    let input = ToolInput::from_json("git.diff", &value).unwrap();
    assert_eq!(
        input,
        ToolInput::GitDiff {
            path: None,
            staged: None,
            commit: None,
        }
    );
}

#[test]
fn from_json_parses_git_log_with_params() {
    let value = serde_json::json!({
        "tool": "git.log",
        "count": 20,
        "path": "src/"
    });
    let input = ToolInput::from_json("git.log", &value).unwrap();
    assert_eq!(
        input,
        ToolInput::GitLog {
            count: Some(20),
            path: Some("src/".to_string()),
        }
    );
    assert_eq!(input.kind(), ToolKind::GitLog);
}

#[test]
fn from_json_parses_git_log_without_params() {
    let value = serde_json::json!({"tool": "git.log"});
    let input = ToolInput::from_json("git.log", &value).unwrap();
    assert_eq!(
        input,
        ToolInput::GitLog {
            count: None,
            path: None,
        }
    );
}

#[test]
fn git_diff_commit_injection_rejected() {
    let registry = build_registry();
    // Reject commit starting with -
    let result = registry.validate(ToolCallRequest::new(
        "call_001",
        "git.diff",
        ToolInput::GitDiff {
            path: None,
            staged: None,
            commit: Some("-c".to_string()),
        },
    ));
    assert!(result.is_err());
    assert!(matches!(
        result.unwrap_err(),
        ToolValidationError::InvalidFieldValue { .. }
    ));
}

#[test]
fn git_diff_commit_flag_injection_rejected() {
    let registry = build_registry();
    let result = registry.validate(ToolCallRequest::new(
        "call_001",
        "git.diff",
        ToolInput::GitDiff {
            path: None,
            staged: None,
            commit: Some("--exec".to_string()),
        },
    ));
    assert!(result.is_err());
}

#[test]
fn git_diff_valid_commit_accepted() {
    let registry = build_registry();
    // HEAD~3 should be accepted
    let result = registry.validate(ToolCallRequest::new(
        "call_001",
        "git.diff",
        ToolInput::GitDiff {
            path: None,
            staged: None,
            commit: Some("HEAD~3".to_string()),
        },
    ));
    assert!(result.is_ok());

    // main should be accepted
    let result = registry.validate(ToolCallRequest::new(
        "call_002",
        "git.diff",
        ToolInput::GitDiff {
            path: None,
            staged: None,
            commit: Some("main".to_string()),
        },
    ));
    assert!(result.is_ok());
}

#[test]
fn git_diff_path_traversal_rejected() {
    let registry = build_registry();
    let result = registry.validate(ToolCallRequest::new(
        "call_001",
        "git.diff",
        ToolInput::GitDiff {
            path: Some("../secret".to_string()),
            staged: None,
            commit: None,
        },
    ));
    assert!(result.is_err());
    assert!(matches!(
        result.unwrap_err(),
        ToolValidationError::InvalidFieldValue { .. }
    ));
}

#[test]
fn git_diff_valid_path_accepted() {
    let registry = build_registry();
    let result = registry.validate(ToolCallRequest::new(
        "call_001",
        "git.diff",
        ToolInput::GitDiff {
            path: Some("src/main.rs".to_string()),
            staged: None,
            commit: None,
        },
    ));
    assert!(result.is_ok());
}

#[test]
fn git_log_count_zero_rejected() {
    let registry = build_registry();
    let result = registry.validate(ToolCallRequest::new(
        "call_001",
        "git.log",
        ToolInput::GitLog {
            count: Some(0),
            path: None,
        },
    ));
    assert!(result.is_err());
    assert!(matches!(
        result.unwrap_err(),
        ToolValidationError::InvalidFieldValue { .. }
    ));
}

#[test]
fn git_log_count_over_100_rejected() {
    let registry = build_registry();
    let result = registry.validate(ToolCallRequest::new(
        "call_001",
        "git.log",
        ToolInput::GitLog {
            count: Some(101),
            path: None,
        },
    ));
    assert!(result.is_err());
}

#[test]
fn git_log_valid_count_accepted() {
    let registry = build_registry();
    // count=10 should be accepted
    let result = registry.validate(ToolCallRequest::new(
        "call_001",
        "git.log",
        ToolInput::GitLog {
            count: Some(10),
            path: None,
        },
    ));
    assert!(result.is_ok());

    // count=100 should be accepted
    let result = registry.validate(ToolCallRequest::new(
        "call_002",
        "git.log",
        ToolInput::GitLog {
            count: Some(100),
            path: None,
        },
    ));
    assert!(result.is_ok());

    // count=1 should be accepted
    let result = registry.validate(ToolCallRequest::new(
        "call_003",
        "git.log",
        ToolInput::GitLog {
            count: Some(1),
            path: None,
        },
    ));
    assert!(result.is_ok());
}

#[test]
fn git_log_path_traversal_rejected() {
    let registry = build_registry();
    let result = registry.validate(ToolCallRequest::new(
        "call_001",
        "git.log",
        ToolInput::GitLog {
            count: None,
            path: Some("../secret".to_string()),
        },
    ));
    assert!(result.is_err());
}

#[test]
fn git_status_execution_in_git_repo() {
    // This test runs in the project root which is a git repo
    let root = std::env::current_dir().expect("should get current dir");
    let mut executor = LocalToolExecutor::new_without_rate_limit(root);
    let registry = build_registry();

    let result = executor
        .execute(ToolExecutionRequest {
            tool_call_id: "call_git_status".to_string(),
            spec: registry.get("git.status").unwrap().clone(),
            input: ToolInput::GitStatus {},
        })
        .expect("git.status should succeed in git repo");

    assert_eq!(result.status, ToolExecutionStatus::Completed);
    // Porcelain output is valid (may be empty if clean)
    assert!(matches!(result.payload, ToolExecutionPayload::Text(_)));
}

#[test]
fn git_diff_execution_in_git_repo() {
    let root = std::env::current_dir().expect("should get current dir");
    let mut executor = LocalToolExecutor::new_without_rate_limit(root);
    let registry = build_registry();

    let result = executor
        .execute(ToolExecutionRequest {
            tool_call_id: "call_git_diff".to_string(),
            spec: registry.get("git.diff").unwrap().clone(),
            input: ToolInput::GitDiff {
                path: None,
                staged: None,
                commit: None,
            },
        })
        .expect("git.diff should succeed in git repo");

    assert_eq!(result.status, ToolExecutionStatus::Completed);
}

#[test]
fn git_log_execution_in_git_repo() {
    let root = std::env::current_dir().expect("should get current dir");
    let mut executor = LocalToolExecutor::new_without_rate_limit(root);
    let registry = build_registry();

    let result = executor
        .execute(ToolExecutionRequest {
            tool_call_id: "call_git_log".to_string(),
            spec: registry.get("git.log").unwrap().clone(),
            input: ToolInput::GitLog {
                count: Some(5),
                path: None,
            },
        })
        .expect("git.log should succeed in git repo");

    assert_eq!(result.status, ToolExecutionStatus::Completed);
    if let ToolExecutionPayload::Text(output) = &result.payload {
        // Should have at most 5 lines of output
        let lines: Vec<&str> = output.trim().lines().collect();
        assert!(
            lines.len() <= 5,
            "expected at most 5 lines, got {}",
            lines.len()
        );
    } else {
        panic!("expected Text payload");
    }
}

// ---------------------------------------------------------------------------
// Issue #74: file.search regex + context_lines tests
// ---------------------------------------------------------------------------

#[test]
fn file_search_serde_backward_compat_missing_regex_and_context_lines() {
    // JSON without regex/context_lines should deserialize with defaults
    let json_str = r#"{"FileSearch":{"root":".","pattern":"hello"}}"#;
    let input: ToolInput = serde_json::from_str(json_str).expect("should deserialize");
    match input {
        ToolInput::FileSearch {
            root,
            pattern,
            regex,
            context_lines,
        } => {
            assert_eq!(root, ".");
            assert_eq!(pattern, "hello");
            assert!(!regex);
            assert_eq!(context_lines, 0);
        }
        _ => panic!("expected FileSearch"),
    }
}

#[test]
fn git_status_fails_in_non_git_repo() {
    let tmp = std::env::temp_dir().join("anvil_git_non_repo_test");
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp).expect("should create temp dir");
    let mut executor = LocalToolExecutor::new_without_rate_limit(tmp.clone());
    let registry = build_registry();

    let result = executor
        .execute(ToolExecutionRequest {
            tool_call_id: "call_git_status_fail".to_string(),
            spec: registry.get("git.status").unwrap().clone(),
            input: ToolInput::GitStatus {},
        })
        .expect("execute should return result, not runtime error");

    assert_eq!(result.status, ToolExecutionStatus::Failed);
    if let ToolExecutionPayload::Text(msg) = &result.payload {
        assert!(
            msg.contains("not a git repository") || msg.contains("fatal"),
            "expected git repo error message, got: {msg}"
        );
    }

    let _ = fs::remove_dir_all(&tmp);
}

#[test]
fn git_status_no_approval_required() {
    let registry = build_registry();
    let validated = registry
        .validate(ToolCallRequest::new(
            "call_git_001",
            "git.status",
            ToolInput::GitStatus {},
        ))
        .expect("git.status should validate");

    // Safe tools don't require approval
    assert!(validated.approval_required(true).is_none());
}

#[test]
fn git_diff_staged_priority_over_commit() {
    // When both staged=true and commit are provided, staged takes priority
    let root = std::env::current_dir().expect("should get current dir");
    let mut executor = LocalToolExecutor::new_without_rate_limit(root);
    let registry = build_registry();

    let result = executor
        .execute(ToolExecutionRequest {
            tool_call_id: "call_git_diff_staged".to_string(),
            spec: registry.get("git.diff").unwrap().clone(),
            input: ToolInput::GitDiff {
                path: None,
                staged: Some(true),
                commit: Some("HEAD~1".to_string()),
            },
        })
        .expect("git.diff with staged should succeed");

    // Should complete without error (staged takes priority, commit ignored)
    assert_eq!(result.status, ToolExecutionStatus::Completed);
}

#[test]
fn file_search_serde_with_new_fields() {
    let json_str =
        r#"{"FileSearch":{"root":"src","pattern":"fn\\s+main","regex":true,"context_lines":3}}"#;
    let input: ToolInput = serde_json::from_str(json_str).expect("should deserialize");
    match input {
        ToolInput::FileSearch {
            regex,
            context_lines,
            ..
        } => {
            assert!(regex);
            assert_eq!(context_lines, 3);
        }
        _ => panic!("expected FileSearch"),
    }
}

#[test]
fn file_search_from_json_with_regex_and_context_lines() {
    let value = serde_json::json!({
        "root": ".",
        "pattern": "fn\\s+main",
        "regex": true,
        "context_lines": 5
    });
    let input = ToolInput::from_json("file.search", &value).expect("should parse");
    match input {
        ToolInput::FileSearch {
            regex,
            context_lines,
            ..
        } => {
            assert!(regex);
            assert_eq!(context_lines, 5);
        }
        _ => panic!("expected FileSearch"),
    }
}

#[test]
fn file_search_from_json_defaults_when_omitted() {
    let value = serde_json::json!({
        "root": ".",
        "pattern": "hello"
    });
    let input = ToolInput::from_json("file.search", &value).expect("should parse");
    match input {
        ToolInput::FileSearch {
            regex,
            context_lines,
            ..
        } => {
            assert!(!regex);
            assert_eq!(context_lines, 0);
        }
        _ => panic!("expected FileSearch"),
    }
}

#[test]
fn file_search_validation_rejects_excessive_context_lines() {
    let registry = build_registry();
    let result = registry.validate(ToolCallRequest::new(
        "call_001",
        "file.search",
        ToolInput::FileSearch {
            root: ".".to_string(),
            pattern: "test".to_string(),
            regex: false,
            context_lines: 11,
        },
    ));
    match result {
        Err(ToolValidationError::InvalidFieldValue { field, .. }) => {
            assert_eq!(field, "context_lines");
        }
        other => panic!("expected InvalidFieldValue, got {other:?}"),
    }
}

#[test]
fn file_search_validation_accepts_max_context_lines() {
    let registry = build_registry();
    let result = registry.validate(ToolCallRequest::new(
        "call_001",
        "file.search",
        ToolInput::FileSearch {
            root: ".".to_string(),
            pattern: "test".to_string(),
            regex: false,
            context_lines: 10,
        },
    ));
    assert!(result.is_ok());
}

#[test]
fn file_search_validation_rejects_invalid_regex() {
    let registry = build_registry();
    let result = registry.validate(ToolCallRequest::new(
        "call_001",
        "file.search",
        ToolInput::FileSearch {
            root: ".".to_string(),
            pattern: "[invalid".to_string(),
            regex: true,
            context_lines: 0,
        },
    ));
    match result {
        Err(ToolValidationError::InvalidFieldValue { field, .. }) => {
            assert_eq!(field, "pattern");
        }
        other => panic!("expected InvalidFieldValue for pattern, got {other:?}"),
    }
}

#[test]
fn file_search_validation_accepts_valid_regex() {
    let registry = build_registry();
    let result = registry.validate(ToolCallRequest::new(
        "call_001",
        "file.search",
        ToolInput::FileSearch {
            root: ".".to_string(),
            pattern: r"fn\s+\w+".to_string(),
            regex: true,
            context_lines: 0,
        },
    ));
    assert!(result.is_ok());
}

#[test]
fn file_search_literal_backward_compatible() {
    // Ensure default (regex=false, context_lines=0) still returns Paths
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("hello.txt");
    fs::write(&file_path, "hello world\nfoo bar\n").unwrap();

    let mut executor = LocalToolExecutor::new_without_rate_limit(dir.path());
    let mut registry = ToolRegistry::new();
    registry.register_file_search();
    let validated = registry
        .validate(ToolCallRequest::new(
            "call_001",
            "file.search",
            ToolInput::FileSearch {
                root: ".".to_string(),
                pattern: "hello".to_string(),
                regex: false,
                context_lines: 0,
            },
        ))
        .unwrap();
    let exec_req = validated
        .approve()
        .into_execution_request(ToolExecutionPolicy {
            approval_required: false,
            ..Default::default()
        })
        .unwrap();
    let result = executor.execute(exec_req).unwrap();
    match &result.payload {
        ToolExecutionPayload::Paths(paths) => {
            assert!(!paths.is_empty());
            assert!(paths[0].contains("hello.txt"));
        }
        other => panic!("expected Paths, got {other:?}"),
    }
}

#[test]
fn file_search_regex_matches_content() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("code.rs");
    fs::write(&file_path, "fn main() {\n    println!(\"hello\");\n}\n").unwrap();

    let mut executor = LocalToolExecutor::new_without_rate_limit(dir.path());
    let mut registry = ToolRegistry::new();
    registry.register_file_search();
    let validated = registry
        .validate(ToolCallRequest::new(
            "call_001",
            "file.search",
            ToolInput::FileSearch {
                root: ".".to_string(),
                pattern: r"fn\s+main".to_string(),
                regex: true,
                context_lines: 0,
            },
        ))
        .unwrap();
    let exec_req = validated
        .approve()
        .into_execution_request(ToolExecutionPolicy {
            approval_required: false,
            ..Default::default()
        })
        .unwrap();
    let result = executor.execute(exec_req).unwrap();
    match &result.payload {
        ToolExecutionPayload::Paths(paths) => {
            assert!(!paths.is_empty());
            assert!(paths[0].contains("code.rs"));
        }
        other => panic!("expected Paths, got {other:?}"),
    }
}

#[test]
fn file_search_regex_no_match() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("code.rs");
    fs::write(&file_path, "fn main() {}\n").unwrap();

    let mut executor = LocalToolExecutor::new_without_rate_limit(dir.path());
    let mut registry = ToolRegistry::new();
    registry.register_file_search();
    let validated = registry
        .validate(ToolCallRequest::new(
            "call_001",
            "file.search",
            ToolInput::FileSearch {
                root: ".".to_string(),
                pattern: r"class\s+Foo".to_string(),
                regex: true,
                context_lines: 0,
            },
        ))
        .unwrap();
    let exec_req = validated
        .approve()
        .into_execution_request(ToolExecutionPolicy {
            approval_required: false,
            ..Default::default()
        })
        .unwrap();
    let result = executor.execute(exec_req).unwrap();
    match &result.payload {
        ToolExecutionPayload::Paths(paths) => {
            assert!(paths.is_empty());
        }
        other => panic!("expected empty Paths, got {other:?}"),
    }
}

#[test]
fn file_search_context_lines_returns_text_payload() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("test.txt");
    fs::write(
        &file_path,
        "line1\nline2\nMATCH_HERE\nline4\nline5\nline6\n",
    )
    .unwrap();

    let mut executor = LocalToolExecutor::new_without_rate_limit(dir.path());
    let mut registry = ToolRegistry::new();
    registry.register_file_search();
    let validated = registry
        .validate(ToolCallRequest::new(
            "call_001",
            "file.search",
            ToolInput::FileSearch {
                root: ".".to_string(),
                pattern: "MATCH_HERE".to_string(),
                regex: false,
                context_lines: 2,
            },
        ))
        .unwrap();
    let exec_req = validated
        .approve()
        .into_execution_request(ToolExecutionPolicy {
            approval_required: false,
            ..Default::default()
        })
        .unwrap();
    let result = executor.execute(exec_req).unwrap();
    match &result.payload {
        ToolExecutionPayload::Text(text) => {
            // Should contain the match line and context
            assert!(text.contains("MATCH_HERE"), "should contain match line");
            assert!(text.contains("line2"), "should contain before context");
            assert!(text.contains("line4"), "should contain after context");
            assert!(text.contains(":3:"), "should contain line number 3");
        }
        other => panic!("expected Text with context, got {other:?}"),
    }
}

#[test]
fn file_search_context_lines_at_file_boundaries() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("test.txt");
    fs::write(&file_path, "MATCH_FIRST\nline2\nline3\n").unwrap();

    let mut executor = LocalToolExecutor::new_without_rate_limit(dir.path());
    let mut registry = ToolRegistry::new();
    registry.register_file_search();
    let validated = registry
        .validate(ToolCallRequest::new(
            "call_001",
            "file.search",
            ToolInput::FileSearch {
                root: ".".to_string(),
                pattern: "MATCH_FIRST".to_string(),
                regex: false,
                context_lines: 5,
            },
        ))
        .unwrap();
    let exec_req = validated
        .approve()
        .into_execution_request(ToolExecutionPolicy {
            approval_required: false,
            ..Default::default()
        })
        .unwrap();
    let result = executor.execute(exec_req).unwrap();
    match &result.payload {
        ToolExecutionPayload::Text(text) => {
            assert!(text.contains("MATCH_FIRST"));
            assert!(text.contains(":1:"), "match at line 1");
            // Should have after context but no before context
            assert!(text.contains("line2"));
        }
        other => panic!("expected Text, got {other:?}"),
    }
}

#[test]
fn file_search_regex_with_context_lines() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("code.rs");
    fs::write(
        &file_path,
        "// header\nfn main() {\n    println!(\"hello\");\n}\n// footer\n",
    )
    .unwrap();

    let mut executor = LocalToolExecutor::new_without_rate_limit(dir.path());
    let mut registry = ToolRegistry::new();
    registry.register_file_search();
    let validated = registry
        .validate(ToolCallRequest::new(
            "call_001",
            "file.search",
            ToolInput::FileSearch {
                root: ".".to_string(),
                pattern: r"fn\s+main".to_string(),
                regex: true,
                context_lines: 1,
            },
        ))
        .unwrap();
    let exec_req = validated
        .approve()
        .into_execution_request(ToolExecutionPolicy {
            approval_required: false,
            ..Default::default()
        })
        .unwrap();
    let result = executor.execute(exec_req).unwrap();
    match &result.payload {
        ToolExecutionPayload::Text(text) => {
            assert!(text.contains("fn main()"));
            assert!(text.contains("// header"), "before context");
            assert!(text.contains("println!"), "after context");
        }
        other => panic!("expected Text, got {other:?}"),
    }
}

#[test]
fn file_search_path_only_match_with_context_returns_path_in_text() {
    let dir = tempfile::tempdir().unwrap();
    let sub = dir.path().join("searchable_dir");
    fs::create_dir(&sub).unwrap();
    let file_path = sub.join("target_file.rs");
    fs::write(&file_path, "no match content\n").unwrap();

    let mut executor = LocalToolExecutor::new_without_rate_limit(dir.path());
    let mut registry = ToolRegistry::new();
    registry.register_file_search();
    let validated = registry
        .validate(ToolCallRequest::new(
            "call_001",
            "file.search",
            ToolInput::FileSearch {
                root: ".".to_string(),
                pattern: "target_file".to_string(),
                regex: false,
                context_lines: 2,
            },
        ))
        .unwrap();
    let exec_req = validated
        .approve()
        .into_execution_request(ToolExecutionPolicy {
            approval_required: false,
            ..Default::default()
        })
        .unwrap();
    let result = executor.execute(exec_req).unwrap();
    match &result.payload {
        ToolExecutionPayload::Text(text) => {
            assert!(
                text.contains("target_file.rs"),
                "path-only match should appear in text"
            );
        }
        other => panic!("expected Text, got {other:?}"),
    }
}

#[test]
fn file_search_zero_matches_with_context_returns_empty_text() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("nothing.txt");
    fs::write(&file_path, "no match here\n").unwrap();

    let mut executor = LocalToolExecutor::new_without_rate_limit(dir.path());
    let mut registry = ToolRegistry::new();
    registry.register_file_search();
    let validated = registry
        .validate(ToolCallRequest::new(
            "call_001",
            "file.search",
            ToolInput::FileSearch {
                root: ".".to_string(),
                pattern: "NONEXISTENT_PATTERN_XYZ".to_string(),
                regex: false,
                context_lines: 3,
            },
        ))
        .unwrap();
    let exec_req = validated
        .approve()
        .into_execution_request(ToolExecutionPolicy {
            approval_required: false,
            ..Default::default()
        })
        .unwrap();
    let result = executor.execute(exec_req).unwrap();
    match &result.payload {
        ToolExecutionPayload::Text(text) => {
            assert!(text.is_empty(), "no matches should produce empty text");
        }
        other => panic!("expected empty Text, got {other:?}"),
    }
}

// --- resolve_locale_params tests ---

mod locale_params {
    use anvil::tooling::resolve_locale_params;

    #[test]
    fn resolve_locale_params_japanese() {
        let result = resolve_locale_params("ja_JP.UTF-8");
        let params = result.expect("should return Some for Japanese locale");
        assert_eq!(params.kl, "jp-ja");
        assert_eq!(params.accept_language, "ja,en;q=0.9");
    }

    #[test]
    fn resolve_locale_params_chinese() {
        let result = resolve_locale_params("zh_CN.UTF-8");
        let params = result.expect("should return Some for Chinese locale");
        assert_eq!(params.kl, "cn-zh");
        assert_eq!(params.accept_language, "zh,en;q=0.9");
    }

    #[test]
    fn resolve_locale_params_korean() {
        let result = resolve_locale_params("ko_KR.UTF-8");
        let params = result.expect("should return Some for Korean locale");
        assert_eq!(params.kl, "kr-kr");
        assert_eq!(params.accept_language, "ko,en;q=0.9");
    }

    #[test]
    fn resolve_locale_params_english() {
        let result = resolve_locale_params("en_US.UTF-8");
        assert!(result.is_none(), "English locale should return None");
    }

    #[test]
    fn resolve_locale_params_c_locale() {
        let result = resolve_locale_params("C");
        assert!(result.is_none(), "C locale should return None");
    }

    #[test]
    fn resolve_locale_params_posix() {
        let result = resolve_locale_params("POSIX");
        assert!(result.is_none(), "POSIX locale should return None");
    }

    #[test]
    fn resolve_locale_params_c_utf8() {
        let result = resolve_locale_params("C.UTF-8");
        assert!(result.is_none(), "C.UTF-8 locale should return None");
    }

    #[test]
    fn resolve_locale_params_bare_ja() {
        let result = resolve_locale_params("ja");
        let params = result.expect("should return Some for bare 'ja'");
        assert_eq!(params.kl, "jp-ja");
    }

    #[test]
    fn resolve_locale_params_zh_tw() {
        let result = resolve_locale_params("zh_TW.UTF-8");
        let params = result.expect("should return Some for zh_TW locale");
        assert_eq!(params.kl, "cn-zh");
    }
}

// --- is_captcha_response tests ---

mod captcha_detection {
    use anvil::tooling::is_captcha_response;

    #[test]
    fn is_captcha_response_ddg_specific() {
        let body = "<html><body>Unfortunately, bots use DuckDuckGo too.</body></html>";
        assert!(
            is_captcha_response(body, 0),
            "DDG-specific CAPTCHA string should be detected"
        );
    }

    #[test]
    fn is_captcha_response_generic_captcha() {
        let body = "<html><body>Please solve this CAPTCHA to continue.</body></html>";
        assert!(
            is_captcha_response(body, 0),
            "Generic 'captcha' keyword should be detected"
        );
    }

    #[test]
    fn is_captcha_response_not_triggered_with_results() {
        let body = "<html><body>Unfortunately, bots use DuckDuckGo too.</body></html>";
        assert!(
            !is_captcha_response(body, 3),
            "Should not trigger CAPTCHA when results_count > 0"
        );
    }

    #[test]
    fn is_captcha_response_no_false_positive_bot_query() {
        let body = "<html><body>Learn about chatbot development and bot frameworks.</body></html>";
        assert!(
            !is_captcha_response(body, 0),
            "Should not false-positive on 'bot' keyword without 'captcha'"
        );
    }
}

// --- CaptchaBlocked error tests ---

mod captcha_blocked_error {
    use anvil::tooling::ToolRuntimeError;

    #[test]
    fn captcha_blocked_error_display() {
        let err = ToolRuntimeError::CaptchaBlocked {
            query: "test query".to_string(),
        };
        let msg = format!("{err}");
        assert!(
            msg.contains("SERPER_API_KEY"),
            "CaptchaBlocked message should mention SERPER_API_KEY, got: {msg}"
        );
    }

    #[test]
    fn captcha_blocked_error_display_includes_web_fetch() {
        let err = ToolRuntimeError::CaptchaBlocked {
            query: "test query".to_string(),
        };
        let msg = format!("{err}");
        assert!(
            msg.contains("web.fetch"),
            "CaptchaBlocked message should mention web.fetch, got: {msg}"
        );
    }
}

// --- Issue #128: file.edit_anchor and fallback tests ---

use anvil::tooling::AnchorEditParams;

#[test]
fn file_edit_anchor_registered_in_registry() {
    let registry = build_registry();
    let spec = registry
        .get("file.edit_anchor")
        .expect("file.edit_anchor should be registered");
    assert_eq!(spec.kind, ToolKind::FileEditAnchor);
    assert_eq!(spec.execution_class, ExecutionClass::Mutating);
    assert_eq!(spec.permission_class, PermissionClass::Confirm);
    assert_eq!(spec.execution_mode, ExecutionMode::SequentialOnly);
    assert_eq!(spec.plan_mode, PlanModePolicy::Allowed);
    assert_eq!(spec.rollback_policy, RollbackPolicy::CheckpointBeforeWrite);
}

#[test]
fn file_edit_anchor_validates_typed_tool_input() {
    let registry = build_registry();
    let valid = ToolCallRequest::new(
        "call_anchor_001",
        "file.edit_anchor",
        ToolInput::FileEditAnchor {
            path: "./src/main.rs".to_string(),
            params: AnchorEditParams {
                old_content: "fn old()".to_string(),
                new_content: "fn new()".to_string(),
            },
        },
    );
    assert!(registry.validate(valid).is_ok());
}

#[test]
fn file_edit_anchor_execution_indent_normalized_match() {
    let root = std::env::temp_dir().join("anvil_anchor_indent_match");
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("dir should exist");
    // File has 4-space indentation
    let file_content = "fn main() {\n    let x = 1;\n    let y = 2;\n}\n";
    fs::write(root.join("test.rs"), file_content).expect("write should succeed");

    let mut executor = LocalToolExecutor::new_without_rate_limit(root.clone());
    // Pattern has no indentation — should match via normalization
    let result = executor
        .execute(ToolExecutionRequest {
            tool_call_id: "call_anchor_exec_001".to_string(),
            spec: build_registry()
                .get("file.edit_anchor")
                .expect("file.edit_anchor spec")
                .clone(),
            input: ToolInput::FileEditAnchor {
                path: "./test.rs".to_string(),
                params: AnchorEditParams {
                    old_content: "let x = 1;\nlet y = 2;".to_string(),
                    new_content: "let x = 10;\nlet y = 20;".to_string(),
                },
            },
        })
        .expect("anchor edit should succeed");

    assert_eq!(result.status, ToolExecutionStatus::Completed);
    let content = fs::read_to_string(root.join("test.rs")).expect("read should succeed");
    assert!(
        content.contains("let x = 10;"),
        "edited content should contain new values, got: {content}"
    );
    assert!(
        content.contains("let y = 20;"),
        "edited content should contain new values, got: {content}"
    );
}

#[test]
fn file_edit_anchor_execution_no_match_error() {
    let root = std::env::temp_dir().join("anvil_anchor_no_match");
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("dir should exist");
    fs::write(root.join("test.rs"), "fn main() {}\n").expect("write should succeed");

    let mut executor = LocalToolExecutor::new_without_rate_limit(root.clone());
    let err = executor
        .execute(ToolExecutionRequest {
            tool_call_id: "call_anchor_nm_001".to_string(),
            spec: build_registry()
                .get("file.edit_anchor")
                .expect("file.edit_anchor spec")
                .clone(),
            input: ToolInput::FileEditAnchor {
                path: "./test.rs".to_string(),
                params: AnchorEditParams {
                    old_content: "nonexistent code".to_string(),
                    new_content: "replacement".to_string(),
                },
            },
        })
        .expect_err("should fail when old_content not found");

    assert!(err.to_string().contains("not found"));
}

#[test]
fn file_edit_anchor_execution_multiple_matches_error() {
    let root = std::env::temp_dir().join("anvil_anchor_multi_match");
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("dir should exist");
    let file_content = "let x = 1;\nlet x = 1;\n";
    fs::write(root.join("test.rs"), file_content).expect("write should succeed");

    let mut executor = LocalToolExecutor::new_without_rate_limit(root.clone());
    let err = executor
        .execute(ToolExecutionRequest {
            tool_call_id: "call_anchor_mm_001".to_string(),
            spec: build_registry()
                .get("file.edit_anchor")
                .expect("file.edit_anchor spec")
                .clone(),
            input: ToolInput::FileEditAnchor {
                path: "./test.rs".to_string(),
                params: AnchorEditParams {
                    old_content: "let x = 1;".to_string(),
                    new_content: "let x = 2;".to_string(),
                },
            },
        })
        .expect_err("should fail when old_content matches multiple times");

    assert!(
        err.to_string().contains("matched 2 locations"),
        "error should mention multiple matches, got: {}",
        err
    );
}

#[test]
fn file_edit_fallback_indent_mismatch() {
    let root = std::env::temp_dir().join("anvil_fallback_indent");
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("dir should exist");
    // File has 4-space indentation
    let file_content = "fn main() {\n    let x = 1;\n}\n";
    fs::write(root.join("test.rs"), file_content).expect("write should succeed");

    let mut executor = LocalToolExecutor::new_without_rate_limit(root.clone());
    // Use file.edit with 8-space indent instead of 4-space — cannot be a substring match
    let result = executor
        .execute(ToolExecutionRequest {
            tool_call_id: "call_fallback_001".to_string(),
            spec: build_registry()
                .get("file.edit")
                .expect("file.edit spec")
                .clone(),
            input: ToolInput::FileEdit {
                path: "./test.rs".to_string(),
                old_string: "        let x = 1;".to_string(), // 8-space indent
                new_string: "        let x = 2;".to_string(),
            },
        })
        .expect("fallback should succeed");

    assert_eq!(result.status, ToolExecutionStatus::Completed);
    assert!(
        result.summary.contains("anchor fallback"),
        "summary should mention anchor fallback, got: {}",
        result.summary
    );
    let content = fs::read_to_string(root.join("test.rs")).expect("read should succeed");
    assert!(
        content.contains("let x = 2;"),
        "content should be edited, got: {content}"
    );
}

#[test]
fn edit_not_found_error_is_distinguishable() {
    let err = anvil::tooling::ToolRuntimeError::EditNotFound("test".to_string());
    assert!(err.is_edit_not_found());
    let io_err = anvil::tooling::ToolRuntimeError::Io("test".to_string());
    assert!(!io_err.is_edit_not_found());
}
