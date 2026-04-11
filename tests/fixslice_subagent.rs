//! Integration tests for the FixSlice sub-agent (Issue #291).

use anvil::agent::subagent::{
    DEFAULT_FIXSLICE_REPEATED_READ_THRESHOLD, FIXSLICE_MAX_ITERATIONS,
    FIXSLICE_REPEATED_READ_THRESHOLD, FIXSLICE_TIMEOUT_SECS, MAX_FIXSLICE_LINES, SubAgentKind,
    first_file_read_target, is_repeated_read_loop,
};
use anvil::agent::tag_spec::find_spec;
use anvil::app::agentic::{
    build_fixslice_user_prompt, build_rewrite_request, validate_fix_proposal,
};
use anvil::config::{ENV_OVERRIDE_WHITELIST, EffectiveConfig};
use anvil::contracts::{FixSliceFailureReason, FixSliceProposal};
use anvil::tooling::{
    ExecutionClass, ExecutionMode, PermissionClass, ToolCallRequest, ToolInput, ToolKind,
    ToolRegistry, ToolValidationError,
};
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Task 1.1: FixSliceProposal serde
// ---------------------------------------------------------------------------

#[test]
fn test_fixslice_proposal_serde() {
    let proposal = FixSliceProposal {
        target_path: "src/main.rs".to_string(),
        start_line: 10,
        end_line: 15,
        replacement_content: "fn fixed() {}\n".to_string(),
        rationale: "Fix compilation error".to_string(),
    };

    let json = serde_json::to_string(&proposal).expect("serialize");
    let deserialized: FixSliceProposal = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(proposal, deserialized);
}

#[test]
fn test_fixslice_proposal_serde_default_rationale() {
    // rationale has #[serde(default)], so it can be omitted in JSON
    let json = r#"{
        "target_path": "src/lib.rs",
        "start_line": 1,
        "end_line": 5,
        "replacement_content": "new content"
    }"#;
    let proposal: FixSliceProposal = serde_json::from_str(json).expect("deserialize");
    assert_eq!(proposal.target_path, "src/lib.rs");
    assert_eq!(proposal.start_line, 1);
    assert_eq!(proposal.end_line, 5);
    assert_eq!(proposal.replacement_content, "new content");
    assert_eq!(proposal.rationale, ""); // default
}

// ---------------------------------------------------------------------------
// Task 2.1: ToolRegistry
// ---------------------------------------------------------------------------

#[test]
fn test_fixslice_tool_registry() {
    let mut registry = ToolRegistry::new();
    registry.register_fixslice_tools();

    // file.read should be registered
    assert!(
        registry.get("file.read").is_some(),
        "file.read should be registered for FixSlice worker"
    );

    // Other tools should NOT be registered
    assert!(registry.get("file.write").is_none());
    assert!(registry.get("file.edit").is_none());
    assert!(registry.get("file.search").is_none());
    assert!(registry.get("shell.exec").is_none());
    assert!(registry.get("agent.explore").is_none());
    assert!(registry.get("agent.plan").is_none());
}

#[test]
fn test_fixslice_permission_class_confirm() {
    let mut registry = ToolRegistry::new();
    registry.register_agent_fix_slice();

    let spec = registry
        .get("agent.fix_slice")
        .expect("agent.fix_slice should be registered");
    assert_eq!(spec.kind, ToolKind::AgentFixSlice);
    assert_eq!(spec.execution_class, ExecutionClass::Mutating);
    assert_eq!(spec.permission_class, PermissionClass::Confirm);
    assert_eq!(spec.execution_mode, ExecutionMode::SequentialOnly);
}

// ---------------------------------------------------------------------------
// Task 2.3: Budget defaults
// ---------------------------------------------------------------------------

#[test]
fn test_fixslice_config_defaults() {
    assert_eq!(FIXSLICE_MAX_ITERATIONS, 3);
    assert_eq!(FIXSLICE_TIMEOUT_SECS, 60);
    assert_eq!(MAX_FIXSLICE_LINES, 200);
}

// ---------------------------------------------------------------------------
// Task 1.2: SubAgentKind::FixSlice
// ---------------------------------------------------------------------------

#[test]
fn test_fixslice_subagent_kind_from_tool_input() {
    let input = ToolInput::AgentFixSlice {
        target_path: "src/main.rs".to_string(),
        goal: "fix bug".to_string(),
        max_lines: 50,
    };
    assert_eq!(
        SubAgentKind::from_tool_input(&input),
        Some(SubAgentKind::FixSlice)
    );

    // Explore/Plan should still work
    let explore = ToolInput::AgentExplore {
        prompt: "test".to_string(),
        scope: None,
    };
    assert_eq!(
        SubAgentKind::from_tool_input(&explore),
        Some(SubAgentKind::Explore)
    );

    // Non-agent tools should return None
    let read = ToolInput::FileRead {
        path: "test".to_string(),
    };
    assert_eq!(SubAgentKind::from_tool_input(&read), None);
}

// ---------------------------------------------------------------------------
// Task 1.3: ToolInput from_json
// ---------------------------------------------------------------------------

#[test]
fn test_fixslice_json_parse() {
    let value = serde_json::json!({
        "target_path": "src/main.rs",
        "goal": "fix the compilation error",
        "max_lines": 100
    });
    let input = ToolInput::from_json("agent.fix_slice", &value).expect("parse should succeed");
    match input {
        ToolInput::AgentFixSlice {
            target_path,
            goal,
            max_lines,
        } => {
            assert_eq!(target_path, "src/main.rs");
            assert_eq!(goal, "fix the compilation error");
            assert_eq!(max_lines, 100);
        }
        _ => panic!("expected AgentFixSlice"),
    }
}

#[test]
fn test_fixslice_json_parse_default_max_lines() {
    let value = serde_json::json!({
        "target_path": "src/main.rs",
        "goal": "fix bug"
    });
    let input = ToolInput::from_json("agent.fix_slice", &value).expect("parse should succeed");
    match input {
        ToolInput::AgentFixSlice { max_lines, .. } => {
            assert_eq!(max_lines, 50, "default max_lines should be 50");
        }
        _ => panic!("expected AgentFixSlice"),
    }
}

#[test]
fn test_fixslice_json_parse_missing_target_path() {
    let value = serde_json::json!({
        "goal": "fix bug"
    });
    let err = ToolInput::from_json("agent.fix_slice", &value).unwrap_err();
    assert!(err.contains("missing target_path"));
}

#[test]
fn test_fixslice_json_parse_missing_goal() {
    let value = serde_json::json!({
        "target_path": "src/main.rs"
    });
    let err = ToolInput::from_json("agent.fix_slice", &value).unwrap_err();
    assert!(err.contains("missing goal"));
}

// ---------------------------------------------------------------------------
// Task 3.2: Validation
// ---------------------------------------------------------------------------

#[test]
fn test_fixslice_validation_good_proposal() {
    let tmp = tempfile::tempdir().unwrap();
    let sandbox_root = tmp.path();
    // Create target directory and file
    std::fs::create_dir_all(sandbox_root.join("src")).unwrap();
    std::fs::write(sandbox_root.join("src/main.rs"), "line1\nline2\nline3\n").unwrap();

    let proposal = FixSliceProposal {
        target_path: "src/main.rs".to_string(),
        start_line: 1,
        end_line: 2,
        replacement_content: "fixed\n".to_string(),
        rationale: "fix bug".to_string(),
    };

    let result = validate_fix_proposal(&proposal, "src/main.rs", 50, sandbox_root);
    assert!(result.is_ok(), "valid proposal should pass: {:?}", result);
}

#[test]
fn test_fixslice_validation_bad_lines() {
    let tmp = tempfile::tempdir().unwrap();
    let sandbox_root = tmp.path();

    let proposal = FixSliceProposal {
        target_path: "src/main.rs".to_string(),
        start_line: 1,
        end_line: 100,
        replacement_content: "fixed\n".to_string(),
        rationale: "".to_string(),
    };

    // max_lines = 50, but range is 100 lines
    let result = validate_fix_proposal(&proposal, "src/main.rs", 50, sandbox_root);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("exceeds max_lines"));
}

#[test]
fn test_fixslice_sandbox_violation() {
    let tmp = tempfile::tempdir().unwrap();
    let sandbox_root = tmp.path();

    let proposal = FixSliceProposal {
        target_path: "../../../etc/passwd".to_string(),
        start_line: 1,
        end_line: 2,
        replacement_content: "hacked\n".to_string(),
        rationale: "".to_string(),
    };

    let result = validate_fix_proposal(&proposal, "../../../etc/passwd", 50, sandbox_root);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("sandbox violation"));
}

#[test]
fn test_fixslice_control_char_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    let sandbox_root = tmp.path();

    let proposal = FixSliceProposal {
        target_path: "src/main\0.rs".to_string(),
        start_line: 1,
        end_line: 2,
        replacement_content: "fixed\n".to_string(),
        rationale: "".to_string(),
    };

    let result = validate_fix_proposal(&proposal, "src/main\0.rs", 50, sandbox_root);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("control characters"));
}

#[test]
fn test_fixslice_target_path_mismatch() {
    let tmp = tempfile::tempdir().unwrap();
    let sandbox_root = tmp.path();

    let proposal = FixSliceProposal {
        target_path: "src/other.rs".to_string(),
        start_line: 1,
        end_line: 2,
        replacement_content: "fixed\n".to_string(),
        rationale: "".to_string(),
    };

    let result = validate_fix_proposal(&proposal, "src/main.rs", 50, sandbox_root);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("mismatch"));
}

#[test]
fn test_fixslice_empty_replacement_content() {
    let tmp = tempfile::tempdir().unwrap();
    let sandbox_root = tmp.path();

    let proposal = FixSliceProposal {
        target_path: "src/main.rs".to_string(),
        start_line: 1,
        end_line: 2,
        replacement_content: "".to_string(),
        rationale: "".to_string(),
    };

    let result = validate_fix_proposal(&proposal, "src/main.rs", 50, sandbox_root);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("must not be empty"));
}

// ---------------------------------------------------------------------------
// Task 3.2: build_rewrite_request
// ---------------------------------------------------------------------------

#[test]
fn test_fixslice_build_rewrite_request() {
    let proposal = FixSliceProposal {
        target_path: "src/main.rs".to_string(),
        start_line: 10,
        end_line: 15,
        replacement_content: "new code\n".to_string(),
        rationale: "fix bug".to_string(),
    };

    let request = build_rewrite_request(&proposal, "call_001");
    assert_eq!(request.spec.name, "file.rewrite");
    assert_eq!(request.spec.kind, ToolKind::FileRewrite);
    assert_eq!(request.tool_call_id, "call_001_rewrite");
    match &request.input {
        ToolInput::FileRewrite {
            path,
            start_line,
            end_line,
            content,
        } => {
            assert_eq!(path, "src/main.rs");
            assert_eq!(*start_line, 10);
            assert_eq!(*end_line, 15);
            assert_eq!(content, "new code\n");
        }
        _ => panic!("expected FileRewrite input"),
    }
}

// ---------------------------------------------------------------------------
// No replacement_content in failure summary
// ---------------------------------------------------------------------------

#[test]
fn test_fixslice_no_replacement_content_in_failure() {
    let tmp = tempfile::tempdir().unwrap();
    let sandbox_root = tmp.path();

    let proposal = FixSliceProposal {
        target_path: "src/other.rs".to_string(), // mismatch
        start_line: 1,
        end_line: 2,
        replacement_content: "SECRET_CONTENT_SHOULD_NOT_LEAK".to_string(),
        rationale: "".to_string(),
    };

    let err = validate_fix_proposal(&proposal, "src/main.rs", 50, sandbox_root).unwrap_err();
    assert!(
        !err.contains("SECRET_CONTENT_SHOULD_NOT_LEAK"),
        "replacement_content should not appear in error message"
    );
}

// ---------------------------------------------------------------------------
// Task 4.1: Tag spec and parser
// ---------------------------------------------------------------------------

#[test]
fn test_fixslice_tag_spec() {
    let spec = find_spec("agent.fix_slice").expect("agent.fix_slice should be in TOOL_TAG_SPECS");
    assert_eq!(spec.name, "agent.fix_slice");
    assert_eq!(spec.attributes, &["target_path", "max_lines"]);
    assert_eq!(spec.child_elements, &["goal"]);
    assert!(!spec.example.is_empty());
}

// ---------------------------------------------------------------------------
// Task 2.2: System prompt
// ---------------------------------------------------------------------------

#[test]
fn test_fixslice_system_prompt_contents() {
    use anvil::agent::subagent::{SubAgentPromptOptions, build_subagent_system_prompt};

    let opts = SubAgentPromptOptions {
        offline: false,
        ui_language: None,
    };
    let prompt = build_subagent_system_prompt(&SubAgentKind::FixSlice, &opts);

    assert!(
        prompt.contains("ANVIL_FINAL"),
        "FixSlice prompt should mention ANVIL_FINAL"
    );
    assert!(
        prompt.contains("start_line"),
        "FixSlice prompt should mention start_line"
    );
    assert!(
        prompt.contains("end_line"),
        "FixSlice prompt should mention end_line"
    );
    assert!(
        prompt.contains("target_path"),
        "FixSlice prompt should mention target_path"
    );
    assert!(
        prompt.contains("replacement_content"),
        "FixSlice prompt should mention replacement_content"
    );
    assert!(
        prompt.contains("file.read"),
        "FixSlice prompt should describe file.read tool"
    );
    assert!(
        prompt.contains("FixSlice"),
        "FixSlice prompt should mention FixSlice role"
    );
}

// ---------------------------------------------------------------------------
// Validation via ToolRegistry
// ---------------------------------------------------------------------------

#[test]
fn test_fixslice_validation_empty_target_path() {
    let mut registry = ToolRegistry::new();
    registry.register_agent_fix_slice();

    let call = ToolCallRequest::new(
        "call_001",
        "agent.fix_slice",
        ToolInput::AgentFixSlice {
            target_path: "".to_string(),
            goal: "fix bug".to_string(),
            max_lines: 50,
        },
    );

    let err = registry.validate(call).unwrap_err();
    assert_eq!(
        err,
        ToolValidationError::MissingRequiredField("target_path".to_string())
    );
}

#[test]
fn test_fixslice_validation_empty_goal() {
    let mut registry = ToolRegistry::new();
    registry.register_agent_fix_slice();

    let call = ToolCallRequest::new(
        "call_001",
        "agent.fix_slice",
        ToolInput::AgentFixSlice {
            target_path: "src/main.rs".to_string(),
            goal: "".to_string(),
            max_lines: 50,
        },
    );

    let err = registry.validate(call).unwrap_err();
    assert_eq!(
        err,
        ToolValidationError::MissingRequiredField("goal".to_string())
    );
}

#[test]
fn test_fixslice_validation_max_lines_zero() {
    let mut registry = ToolRegistry::new();
    registry.register_agent_fix_slice();

    let call = ToolCallRequest::new(
        "call_001",
        "agent.fix_slice",
        ToolInput::AgentFixSlice {
            target_path: "src/main.rs".to_string(),
            goal: "fix bug".to_string(),
            max_lines: 0,
        },
    );

    let err = registry.validate(call).unwrap_err();
    match err {
        ToolValidationError::InvalidFieldValue { field, .. } => {
            assert_eq!(field, "max_lines");
        }
        _ => panic!("expected InvalidFieldValue for max_lines"),
    }
}

#[test]
fn test_fixslice_validation_max_lines_exceeds_limit() {
    let mut registry = ToolRegistry::new();
    registry.register_agent_fix_slice();

    let call = ToolCallRequest::new(
        "call_001",
        "agent.fix_slice",
        ToolInput::AgentFixSlice {
            target_path: "src/main.rs".to_string(),
            goal: "fix bug".to_string(),
            max_lines: MAX_FIXSLICE_LINES + 1,
        },
    );

    let err = registry.validate(call).unwrap_err();
    match err {
        ToolValidationError::InvalidFieldValue { field, .. } => {
            assert_eq!(field, "max_lines");
        }
        _ => panic!("expected InvalidFieldValue for max_lines"),
    }
}

#[test]
fn test_fixslice_validation_control_char_in_target_path() {
    let mut registry = ToolRegistry::new();
    registry.register_agent_fix_slice();

    let call = ToolCallRequest::new(
        "call_001",
        "agent.fix_slice",
        ToolInput::AgentFixSlice {
            target_path: "src/main\x00.rs".to_string(),
            goal: "fix bug".to_string(),
            max_lines: 50,
        },
    );

    let err = registry.validate(call).unwrap_err();
    match err {
        ToolValidationError::InvalidFieldValue { field, .. } => {
            assert_eq!(field, "target_path");
        }
        _ => panic!("expected InvalidFieldValue for target_path"),
    }
}

// ---------------------------------------------------------------------------
// ToolInput::kind() mapping
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Issue #341: FixSlice worker must receive target_path and max_lines in its
// user prompt, not only the free-form goal.
// ---------------------------------------------------------------------------

#[test]
fn test_fixslice_user_prompt_contains_target_and_budget() {
    let prompt = build_fixslice_user_prompt(
        "src/lib/auto-yes-manager.ts",
        "remove stale branch and add regression test",
        80,
    );
    assert!(
        prompt.contains("src/lib/auto-yes-manager.ts"),
        "prompt must embed the exact target_path: {prompt}"
    );
    assert!(
        prompt.contains("80"),
        "prompt must embed the max_lines budget: {prompt}"
    );
    assert!(
        prompt.contains("remove stale branch and add regression test"),
        "prompt must embed the goal verbatim: {prompt}"
    );
}

#[test]
fn test_fixslice_user_prompt_anchors_worker_to_target() {
    let prompt =
        build_fixslice_user_prompt("crates/foo/src/bar.rs", "fix panic on empty input", 40);
    // The worker must be told to read the target first and to match the
    // target_path in its proposal, otherwise exploration drifts (Issue #341).
    assert!(
        prompt.contains("file.read"),
        "prompt should instruct the worker to read the target first"
    );
    assert!(
        prompt.contains("ANVIL_FINAL"),
        "prompt should reference the ANVIL_FINAL proposal format"
    );
    let target_occurrences = prompt.matches("crates/foo/src/bar.rs").count();
    assert!(
        target_occurrences >= 2,
        "target_path should appear multiple times for redundancy, got {target_occurrences}: {prompt}"
    );
}

#[test]
fn test_fixslice_tool_kind_mapping() {
    let input = ToolInput::AgentFixSlice {
        target_path: "src/main.rs".to_string(),
        goal: "fix bug".to_string(),
        max_lines: 50,
    };
    assert_eq!(input.kind(), ToolKind::AgentFixSlice);
}

// ---------------------------------------------------------------------------
// Issue #351: same-target `file.read` oscillation guard
// ---------------------------------------------------------------------------

#[test]
fn test_fixslice_repeated_read_threshold_default() {
    // Issue #353: the default threshold is 5 — cycle-11 regressed on the
    // previous value of 2, which killed legitimate qwen3.5:122b
    // exploration. The back-compat alias tracks the default.
    assert_eq!(DEFAULT_FIXSLICE_REPEATED_READ_THRESHOLD, 5);
    assert_eq!(
        FIXSLICE_REPEATED_READ_THRESHOLD,
        DEFAULT_FIXSLICE_REPEATED_READ_THRESHOLD
    );
}

// ---------------------------------------------------------------------------
// Issue #353: no-progress detector tunable — runtime config, env, and file
// ---------------------------------------------------------------------------

#[test]
fn test_fixslice_no_progress_detector_default_is_raised_threshold() {
    // Issue #353: the baseline runtime config must ship with the new
    // raised default so legitimate A1/A2 exploration is not aborted.
    let config = EffectiveConfig::default_for_test().expect("default config");
    assert_eq!(
        config.runtime.fixslice_no_progress_detector,
        DEFAULT_FIXSLICE_REPEATED_READ_THRESHOLD
    );
}

#[test]
fn test_fixslice_no_progress_detector_allows_five_reads_before_firing() {
    // Issue #353: with the raised default, five same-path reads in a row
    // are needed before the guard trips. Four are still fine.
    let threshold = DEFAULT_FIXSLICE_REPEATED_READ_THRESHOLD;
    for prior in 0..(threshold - 1) {
        assert!(
            !is_repeated_read_loop(Some("src/main.rs"), Some("src/main.rs"), prior, threshold,),
            "prior={prior} must not trip at default threshold"
        );
    }
    assert!(is_repeated_read_loop(
        Some("src/main.rs"),
        Some("src/main.rs"),
        threshold - 1,
        threshold,
    ));
}

#[test]
fn test_fixslice_no_progress_detector_env_off_disables_guard() {
    // Issue #353: `ANVIL_FIXSLICE_NO_PROGRESS_DETECTOR=off` must route to
    // threshold=0 in RuntimeConfig; the SubAgentSession treats `0` as
    // "detector disabled" and never aborts on read repetition.
    let mut config = EffectiveConfig::default_for_test().expect("default config");
    let file_values: HashMap<String, String> = HashMap::new();
    let mut env_values: HashMap<String, String> = HashMap::new();
    env_values.insert(
        "ANVIL_FIXSLICE_NO_PROGRESS_DETECTOR".to_string(),
        "off".to_string(),
    );
    let cli_values: HashMap<String, String> = HashMap::new();
    config
        .apply_overrides_for_test(&file_values, &env_values, &cli_values)
        .expect("env override applies");
    assert_eq!(config.runtime.fixslice_no_progress_detector, 0);
}

#[test]
fn test_fixslice_no_progress_detector_env_numeric_override() {
    // Numeric env values are accepted and clamped to [0, 20] by validate.
    let mut config = EffectiveConfig::default_for_test().expect("default config");
    let mut env = HashMap::new();
    env.insert(
        "ANVIL_FIXSLICE_NO_PROGRESS_DETECTOR".to_string(),
        "7".to_string(),
    );
    config
        .apply_overrides_for_test(&HashMap::new(), &env, &HashMap::new())
        .expect("numeric env override applies");
    assert_eq!(config.runtime.fixslice_no_progress_detector, 7);
}

#[test]
fn test_fixslice_no_progress_detector_env_on_restores_default() {
    let mut config = EffectiveConfig::default_for_test().expect("default config");
    // Force a non-default state first.
    config.runtime.fixslice_no_progress_detector = 0;
    let mut env = HashMap::new();
    env.insert(
        "ANVIL_FIXSLICE_NO_PROGRESS_DETECTOR".to_string(),
        "on".to_string(),
    );
    config
        .apply_overrides_for_test(&HashMap::new(), &env, &HashMap::new())
        .expect("on override applies");
    assert_eq!(
        config.runtime.fixslice_no_progress_detector,
        DEFAULT_FIXSLICE_REPEATED_READ_THRESHOLD
    );
}

#[test]
fn test_fixslice_no_progress_detector_file_config_key_honored() {
    // Issue #353: the file-config key must match the env name so users
    // can set it via `.anvil/config`.
    let mut config = EffectiveConfig::default_for_test().expect("default config");
    let mut file = HashMap::new();
    file.insert("fixslice_no_progress_detector".to_string(), "3".to_string());
    config
        .apply_overrides_for_test(&file, &HashMap::new(), &HashMap::new())
        .expect("file override applies");
    assert_eq!(config.runtime.fixslice_no_progress_detector, 3);
}

#[test]
fn test_fixslice_no_progress_detector_env_whitelisted() {
    // Issue #347 regression guard: env vars must be in the whitelist, or
    // `apply_env_overrides()` silently drops them.
    assert!(
        ENV_OVERRIDE_WHITELIST.contains(&"ANVIL_FIXSLICE_NO_PROGRESS_DETECTOR"),
        "ANVIL_FIXSLICE_NO_PROGRESS_DETECTOR must be in the env override \
         whitelist so it reaches RuntimeConfig via apply_env_overrides"
    );
}

#[test]
fn test_fixslice_no_progress_detector_validate_clamps_upper_bound() {
    // Values above 20 are clamped (misconfig guard — would exceed the
    // max FixSlice iteration cap anyway).
    let mut config = EffectiveConfig::default_for_test().expect("default config");
    config.runtime.fixslice_no_progress_detector = 999;
    config.validate_for_test().expect("validate");
    assert_eq!(config.runtime.fixslice_no_progress_detector, 20);
}

#[test]
fn test_is_repeated_read_loop_first_read_never_triggers() {
    // Prior count = 0, no last target: the first read should never trip
    // the guard, regardless of threshold.
    assert!(!is_repeated_read_loop(Some("src/main.rs"), None, 0, 2));
}

#[test]
fn test_is_repeated_read_loop_different_paths_never_trigger() {
    // Different target this iteration resets the chain.
    assert!(!is_repeated_read_loop(
        Some("src/other.rs"),
        Some("src/main.rs"),
        1,
        2
    ));
    assert!(!is_repeated_read_loop(
        Some("src/other.rs"),
        Some("src/main.rs"),
        5,
        2
    ));
}

#[test]
fn test_is_repeated_read_loop_same_path_reaches_threshold() {
    // Same target, prior count = 1, threshold = 2 -> next = 2 -> trip.
    assert!(is_repeated_read_loop(
        Some("src/main.rs"),
        Some("src/main.rs"),
        1,
        2
    ));
}

#[test]
fn test_is_repeated_read_loop_same_path_below_threshold() {
    // Same target but prior count = 0 -> next = 1 -> no trip at threshold 2.
    assert!(!is_repeated_read_loop(
        Some("src/main.rs"),
        Some("src/main.rs"),
        0,
        2
    ));
}

#[test]
fn test_is_repeated_read_loop_no_current_read_is_safe() {
    // Iteration with no file.read at all never trips the guard.
    assert!(!is_repeated_read_loop(None, Some("src/main.rs"), 1, 2));
}

#[test]
fn test_is_repeated_read_loop_custom_threshold() {
    // Threshold = 3 — first two same-path iterations should not trip;
    // the third should.
    assert!(!is_repeated_read_loop(
        Some("src/main.rs"),
        Some("src/main.rs"),
        0,
        3
    ));
    assert!(!is_repeated_read_loop(
        Some("src/main.rs"),
        Some("src/main.rs"),
        1,
        3
    ));
    assert!(is_repeated_read_loop(
        Some("src/main.rs"),
        Some("src/main.rs"),
        2,
        3
    ));
}

#[test]
fn test_first_file_read_target_empty_calls() {
    assert_eq!(first_file_read_target(&[]), None);
}

#[test]
fn test_first_file_read_target_picks_file_read() {
    let calls = vec![ToolCallRequest::new(
        "call_001",
        "file.read",
        ToolInput::FileRead {
            path: "src/main.rs".to_string(),
        },
    )];
    assert_eq!(
        first_file_read_target(&calls),
        Some("src/main.rs".to_string())
    );
}

#[test]
fn test_first_file_read_target_picks_first_of_multiple() {
    // When multiple file.read calls are present, the first one wins — it's
    // the stable fingerprint for the iteration's dominant target.
    let calls = vec![
        ToolCallRequest::new(
            "call_001",
            "file.read",
            ToolInput::FileRead {
                path: "src/first.rs".to_string(),
            },
        ),
        ToolCallRequest::new(
            "call_002",
            "file.read",
            ToolInput::FileRead {
                path: "src/second.rs".to_string(),
            },
        ),
    ];
    assert_eq!(
        first_file_read_target(&calls),
        Some("src/first.rs".to_string())
    );
}

#[test]
fn test_first_file_read_target_ignores_non_file_read() {
    // Non-file.read tools in the batch are ignored.
    let calls = vec![
        ToolCallRequest::new(
            "call_001",
            "agent.fix_slice",
            ToolInput::AgentFixSlice {
                target_path: "src/main.rs".to_string(),
                goal: "fix bug".to_string(),
                max_lines: 50,
            },
        ),
        ToolCallRequest::new(
            "call_002",
            "file.read",
            ToolInput::FileRead {
                path: "src/target.rs".to_string(),
            },
        ),
    ];
    assert_eq!(
        first_file_read_target(&calls),
        Some("src/target.rs".to_string())
    );
}

// ---------------------------------------------------------------------------
// Issue #351: FixSliceFailureReason::RepeatedReadLoop Display form
// ---------------------------------------------------------------------------

#[test]
fn test_fixslice_failure_reason_repeated_read_loop_display() {
    // The Display form is what gets stored in telemetry.
    assert_eq!(
        FixSliceFailureReason::RepeatedReadLoop.to_string(),
        "repeated_read_loop"
    );
}
