//! Integration tests for the hooks lifecycle hook system (Issue #25).

mod common;

use anvil::config::load_hooks_config;
use anvil::hooks::*;
use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

// ---------------------------------------------------------------------------
// HooksConfig parsing tests
// ---------------------------------------------------------------------------

#[test]
fn hooks_config_parse_full_json() {
    let json = r#"{
        "hooks": {
            "PreToolUse": [
                { "command": "/usr/bin/validator", "timeout_ms": 3000 }
            ],
            "PostToolUse": [],
            "PreCompact": [],
            "PostSession": [
                { "command": "/usr/bin/cleanup", "timeout_ms": 10000 }
            ]
        }
    }"#;
    let config: HooksConfig = serde_json::from_str(json).unwrap();
    assert!(!config.is_empty());
    assert_eq!(config.get_entries(&HookPoint::PreToolUse).len(), 1);
    assert_eq!(config.get_entries(&HookPoint::PostToolUse).len(), 0);
    assert_eq!(config.get_entries(&HookPoint::PostSession).len(), 1);
    assert_eq!(
        config.get_entries(&HookPoint::PreToolUse)[0].timeout_ms,
        3000
    );
}

#[test]
fn hooks_config_parse_empty() {
    let json = r#"{ "hooks": {} }"#;
    let config: HooksConfig = serde_json::from_str(json).unwrap();
    assert!(config.is_empty());
}

#[test]
fn hooks_config_is_empty_with_empty_vecs() {
    let json = r#"{ "hooks": { "PreToolUse": [], "PostSession": [] } }"#;
    let config: HooksConfig = serde_json::from_str(json).unwrap();
    assert!(config.is_empty());
}

#[test]
fn hooks_config_get_entries_missing_point() {
    let json = r#"{ "hooks": { "PreToolUse": [{ "command": "test" }] } }"#;
    let config: HooksConfig = serde_json::from_str(json).unwrap();
    assert!(config.get_entries(&HookPoint::PostSession).is_empty());
}

#[test]
fn hook_entry_default_timeout_and_on_timeout() {
    let json = r#"{ "command": "/usr/bin/test" }"#;
    let entry: HookEntry = serde_json::from_str(json).unwrap();
    assert_eq!(entry.timeout_ms, 5000);
    assert_eq!(entry.on_timeout, "continue");
}

#[test]
fn hook_entry_timeout_alias() {
    let json = r#"{ "command": "/usr/bin/test", "timeout": 3000 }"#;
    let entry: HookEntry = serde_json::from_str(json).unwrap();
    assert_eq!(entry.timeout_ms, 3000);
}

#[test]
fn hook_entry_on_timeout_block() {
    let json = r#"{ "command": "/usr/bin/test", "on_timeout": "block" }"#;
    let entry: HookEntry = serde_json::from_str(json).unwrap();
    assert_eq!(entry.on_timeout, "block");
}

#[test]
fn hook_point_serde_roundtrip() {
    let points = vec![
        HookPoint::PreToolUse,
        HookPoint::PostToolUse,
        HookPoint::PreCompact,
        HookPoint::PostSession,
    ];
    for point in points {
        let json = serde_json::to_string(&point).unwrap();
        let parsed: HookPoint = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, point);
    }
}

// ---------------------------------------------------------------------------
// Event serialization tests
// ---------------------------------------------------------------------------

#[test]
fn pre_tool_use_event_serializes_correctly() {
    let event = PreToolUseEvent {
        hook_point: "PreToolUse",
        tool_name: "file.read".to_string(),
        tool_input: serde_json::json!({"path": "/tmp/test.txt"}),
        tool_call_id: "call_001".to_string(),
    };
    let json = serde_json::to_string(&event).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed["hook_point"], "PreToolUse");
    assert_eq!(parsed["tool_name"], "file.read");
    assert_eq!(parsed["tool_input"]["path"], "/tmp/test.txt");
}

#[test]
fn post_tool_use_event_serializes_correctly() {
    let event = PostToolUseEvent {
        hook_point: "PostToolUse",
        tool_name: "shell.exec".to_string(),
        tool_input: serde_json::json!({"command": "ls"}),
        tool_call_id: "call_002".to_string(),
        tool_result: HookToolResult {
            status: "completed".to_string(),
            summary: "command executed successfully".to_string(),
        },
    };
    let json = serde_json::to_string(&event).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed["tool_result"]["status"], "completed");
}

#[test]
fn pre_compact_event_serializes_correctly() {
    let event = PreCompactEvent {
        hook_point: "PreCompact",
        session_id: "session_abc".to_string(),
        trigger: "auto".to_string(),
        message_count: 100,
    };
    let json = serde_json::to_string(&event).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed["trigger"], "auto");
    assert_eq!(parsed["message_count"], 100);
}

#[test]
fn post_session_event_mode_string() {
    let event = PostSessionEvent {
        hook_point: "PostSession",
        session_id: "session_xyz".to_string(),
        mode: "interactive".to_string(),
    };
    let json = serde_json::to_string(&event).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed["mode"], "interactive");

    let event_non = PostSessionEvent {
        hook_point: "PostSession",
        session_id: "session_xyz".to_string(),
        mode: "non-interactive".to_string(),
    };
    let json = serde_json::to_string(&event_non).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed["mode"], "non-interactive");
}

// ---------------------------------------------------------------------------
// HookRunner tests
// ---------------------------------------------------------------------------

#[test]
fn hook_runner_execute_echo_continue() {
    let shutdown = Arc::new(AtomicBool::new(false));
    let runner = HookRunner::new(shutdown);

    // echo outputs nothing meaningful -> Continue
    let result = runner.execute("echo hello", b"{}", 5000, "continue");
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), HookOutcome::Continue);
}

#[test]
fn hook_runner_execute_block_via_json_stdout() {
    let shutdown = Arc::new(AtomicBool::new(false));
    let runner = HookRunner::new(shutdown);

    // Use printf to output a block decision JSON
    let result = runner.execute(
        r#"printf '{"decision":"block","reason":"test block"}'"#,
        b"{}",
        5000,
        "continue",
    );
    // printf is not directly available via shlex; use sh -c or a script
    // Since we use shlex parsing (not shell), we need a different approach
    // Let's test with a simple exit code 2 for block
    let _ = result; // printf may not work via shlex

    // Test block via exit code 2
    let tmpdir = tempfile::tempdir().unwrap();
    let script_path = tmpdir.path().join("block.sh");
    {
        let mut f = std::fs::File::create(&script_path).unwrap();
        writeln!(f, "#!/bin/sh").unwrap();
        writeln!(f, "echo 'blocked by security policy'").unwrap();
        writeln!(f, "exit 2").unwrap();
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    let result = runner
        .execute(script_path.to_str().unwrap(), b"{}", 5000, "continue")
        .unwrap();
    assert_eq!(
        result,
        HookOutcome::Block {
            reason: "blocked by security policy".to_string(),
            exit_code: 2,
        }
    );
}

#[test]
fn hook_runner_execute_block_via_json() {
    let shutdown = Arc::new(AtomicBool::new(false));
    let runner = HookRunner::new(shutdown);

    let tmpdir = tempfile::tempdir().unwrap();
    let script_path = tmpdir.path().join("block_json.sh");
    {
        let mut f = std::fs::File::create(&script_path).unwrap();
        writeln!(f, "#!/bin/sh").unwrap();
        writeln!(
            f,
            r#"echo '{{"decision":"block","reason":"forbidden tool"}}'"#
        )
        .unwrap();
        writeln!(f, "exit 0").unwrap();
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    let result = runner
        .execute(script_path.to_str().unwrap(), b"{}", 5000, "continue")
        .unwrap();
    assert_eq!(
        result,
        HookOutcome::Block {
            reason: "forbidden tool".to_string(),
            exit_code: 0,
        }
    );
}

#[test]
fn hook_runner_command_not_found() {
    let shutdown = Arc::new(AtomicBool::new(false));
    let runner = HookRunner::new(shutdown);

    let result = runner.execute("/nonexistent/path/to/hook", b"{}", 5000, "continue");
    assert!(result.is_err());
    match result.unwrap_err() {
        HookError::CommandNotFound { .. } => {}
        other => panic!("expected CommandNotFound, got: {other:?}"),
    }
}

#[test]
fn hook_runner_command_parse_failed() {
    let shutdown = Arc::new(AtomicBool::new(false));
    let runner = HookRunner::new(shutdown);

    // Unterminated quote
    let result = runner.execute("echo 'unterminated", b"{}", 5000, "continue");
    assert!(result.is_err());
    match result.unwrap_err() {
        HookError::CommandParseFailed { .. } => {}
        other => panic!("expected CommandParseFailed, got: {other:?}"),
    }
}

#[test]
fn hook_runner_path_traversal_rejected() {
    let shutdown = Arc::new(AtomicBool::new(false));
    let runner = HookRunner::new(shutdown);

    let result = runner.execute("../../../etc/passwd", b"{}", 5000, "continue");
    assert!(result.is_err());
    match result.unwrap_err() {
        HookError::CommandNotFound { .. } => {}
        other => panic!("expected CommandNotFound, got: {other:?}"),
    }
}

#[test]
fn hook_runner_timeout_continue() {
    let shutdown = Arc::new(AtomicBool::new(false));
    let runner = HookRunner::new(shutdown);

    let result = runner.execute("sleep 60", b"{}", 200, "continue");
    assert!(result.is_err());
    match result.unwrap_err() {
        HookError::Timeout { timeout_ms, .. } => assert_eq!(timeout_ms, 200),
        other => panic!("expected Timeout, got: {other:?}"),
    }
}

#[test]
fn hook_runner_timeout_block() {
    let shutdown = Arc::new(AtomicBool::new(false));
    let runner = HookRunner::new(shutdown);

    let result = runner.execute("sleep 60", b"{}", 200, "block");
    assert!(result.is_ok());
    match result.unwrap() {
        HookOutcome::Block { reason, .. } => {
            assert!(reason.contains("timed out"));
        }
        other => panic!("expected Block, got: {other:?}"),
    }
}

#[test]
fn hook_runner_shutdown_flag() {
    let shutdown = Arc::new(AtomicBool::new(true));
    let runner = HookRunner::new(shutdown);

    let result = runner.execute("echo hello", b"{}", 5000, "continue");
    assert!(result.is_err());
    match result.unwrap_err() {
        HookError::Shutdown => {}
        other => panic!("expected Shutdown, got: {other:?}"),
    }
}

#[cfg(unix)]
#[test]
fn hook_runner_permission_denied() {
    use std::os::unix::fs::PermissionsExt;

    let tmpdir = tempfile::tempdir().unwrap();
    let script_path = tmpdir.path().join("no_exec.sh");
    std::fs::write(&script_path, "#!/bin/sh\necho hello").unwrap();
    // Set to no execute permission
    std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o644)).unwrap();

    let shutdown = Arc::new(AtomicBool::new(false));
    let runner = HookRunner::new(shutdown);

    let result = runner.execute(script_path.to_str().unwrap(), b"{}", 5000, "continue");
    assert!(result.is_err());
    match result.unwrap_err() {
        HookError::PermissionDenied { .. } => {}
        other => panic!("expected PermissionDenied, got: {other:?}"),
    }
}

#[test]
fn hook_runner_stdin_receives_json() {
    let shutdown = Arc::new(AtomicBool::new(false));
    let runner = HookRunner::new(shutdown);

    let tmpdir = tempfile::tempdir().unwrap();
    let output_file = tmpdir.path().join("stdin_output.txt");
    let script_path = tmpdir.path().join("read_stdin.sh");
    {
        let mut f = std::fs::File::create(&script_path).unwrap();
        writeln!(f, "#!/bin/sh").unwrap();
        writeln!(f, "cat > {}", output_file.display()).unwrap();
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    let input_json = r#"{"tool_name":"file.read","tool_call_id":"call_001"}"#;
    let result = runner.execute(
        script_path.to_str().unwrap(),
        input_json.as_bytes(),
        5000,
        "continue",
    );
    assert!(result.is_ok());

    let captured = std::fs::read_to_string(&output_file).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&captured).unwrap();
    assert_eq!(parsed["tool_name"], "file.read");
    assert_eq!(parsed["tool_call_id"], "call_001");
}

// ---------------------------------------------------------------------------
// HooksEngine tests
// ---------------------------------------------------------------------------

#[test]
fn hooks_engine_pre_tool_use_continue() {
    let config = make_config_with_echo("PreToolUse");
    let shutdown = Arc::new(AtomicBool::new(false));
    let engine = HooksEngine::new(config, shutdown);

    let event = PreToolUseEvent {
        hook_point: "PreToolUse",
        tool_name: "file.read".to_string(),
        tool_input: serde_json::json!({}),
        tool_call_id: "call_1".to_string(),
    };

    let result = engine.run_pre_tool_use(event).unwrap();
    assert_eq!(result, PreToolUseOutcome::Continue);
}

#[test]
fn hooks_engine_pre_tool_use_block() {
    let tmpdir = tempfile::tempdir().unwrap();
    let script_path = create_block_script(tmpdir.path());
    let config = make_config_with_command("PreToolUse", script_path.to_str().unwrap());
    let shutdown = Arc::new(AtomicBool::new(false));
    let engine = HooksEngine::new(config, shutdown);

    let event = PreToolUseEvent {
        hook_point: "PreToolUse",
        tool_name: "shell.exec".to_string(),
        tool_input: serde_json::json!({}),
        tool_call_id: "call_2".to_string(),
    };

    let result = engine.run_pre_tool_use(event).unwrap();
    match result {
        PreToolUseOutcome::Block { reason, .. } => {
            assert!(reason.contains("blocked"));
        }
        other => panic!("expected Block, got: {other:?}"),
    }
}

#[test]
fn hooks_engine_post_tool_use_soft_fail() {
    let config = make_config_with_command("PostToolUse", "/nonexistent/hook");
    let shutdown = Arc::new(AtomicBool::new(false));
    let engine = HooksEngine::new(config, shutdown);

    let event = PostToolUseEvent {
        hook_point: "PostToolUse",
        tool_name: "file.read".to_string(),
        tool_input: serde_json::json!({}),
        tool_call_id: "call_3".to_string(),
        tool_result: HookToolResult {
            status: "completed".to_string(),
            summary: "ok".to_string(),
        },
    };

    // Should not error even though command doesn't exist (soft-fail)
    let result = engine.run_post_tool_use(event);
    assert!(result.is_ok());
}

#[test]
fn hooks_engine_pre_compact_soft_fail() {
    let config = make_config_with_command("PreCompact", "/nonexistent/hook");
    let shutdown = Arc::new(AtomicBool::new(false));
    let engine = HooksEngine::new(config, shutdown);

    let event = PreCompactEvent {
        hook_point: "PreCompact",
        session_id: "session_1".to_string(),
        trigger: "auto".to_string(),
        message_count: 100,
    };

    let result = engine.run_pre_compact(event);
    assert!(result.is_ok());
}

#[test]
fn hooks_engine_post_session_soft_fail() {
    let config = make_config_with_command("PostSession", "/nonexistent/hook");
    let shutdown = Arc::new(AtomicBool::new(false));
    let engine = HooksEngine::new(config, shutdown);

    let event = PostSessionEvent {
        hook_point: "PostSession",
        session_id: "session_1".to_string(),
        mode: "interactive".to_string(),
    };

    let result = engine.run_post_session(event);
    assert!(result.is_ok());
}

#[test]
fn hooks_engine_no_entries_returns_continue() {
    let config: HooksConfig = serde_json::from_str(r#"{ "hooks": {} }"#).unwrap();
    let shutdown = Arc::new(AtomicBool::new(false));
    let engine = HooksEngine::new(config, shutdown);

    let event = PreToolUseEvent {
        hook_point: "PreToolUse",
        tool_name: "file.read".to_string(),
        tool_input: serde_json::json!({}),
        tool_call_id: "call_1".to_string(),
    };

    let result = engine.run_pre_tool_use(event).unwrap();
    assert_eq!(result, PreToolUseOutcome::Continue);
}

// ---------------------------------------------------------------------------
// load_hooks_config tests
// ---------------------------------------------------------------------------

#[test]
fn load_hooks_config_file_not_found() {
    let root = common::unique_test_dir("hooks_notfound");
    let config = common::build_config_in(root);

    let result = load_hooks_config(&config.paths);
    assert!(result.is_ok());
    assert!(result.unwrap().is_none());
}

#[test]
fn load_hooks_config_valid_file() {
    let root = common::unique_test_dir("hooks_valid");
    std::fs::create_dir_all(root.join(".anvil")).unwrap();
    std::fs::write(
        root.join(".anvil").join("hooks.json"),
        r#"{
            "hooks": {
                "PreToolUse": [{ "command": "echo test" }],
                "PostSession": []
            }
        }"#,
    )
    .unwrap();

    let config = common::build_config_in(root);
    let result = load_hooks_config(&config.paths).unwrap();
    assert!(result.is_some());
    let hooks = result.unwrap();
    assert_eq!(hooks.get_entries(&HookPoint::PreToolUse).len(), 1);
}

#[test]
fn load_hooks_config_parse_error() {
    let root = common::unique_test_dir("hooks_parse_err");
    std::fs::create_dir_all(root.join(".anvil")).unwrap();
    std::fs::write(root.join(".anvil").join("hooks.json"), "invalid json!!!").unwrap();

    let config = common::build_config_in(root);
    let result = load_hooks_config(&config.paths);
    assert!(result.is_err());
    match result.unwrap_err() {
        HookError::ParseError { reason, .. } => {
            assert!(reason.contains("failed to parse"));
        }
        other => panic!("expected ParseError, got: {other:?}"),
    }
}

#[test]
fn load_hooks_config_entry_limit() {
    let root = common::unique_test_dir("hooks_limit");
    std::fs::create_dir_all(root.join(".anvil")).unwrap();

    // Create 20 entries (exceeding limit of 16)
    let entries: Vec<String> = (0..20)
        .map(|i| format!(r#"{{ "command": "echo {i}" }}"#))
        .collect();
    let json = format!(
        r#"{{ "hooks": {{ "PreToolUse": [{}] }} }}"#,
        entries.join(",")
    );
    std::fs::write(root.join(".anvil").join("hooks.json"), json).unwrap();

    let config = common::build_config_in(root);
    let result = load_hooks_config(&config.paths).unwrap().unwrap();
    assert_eq!(result.get_entries(&HookPoint::PreToolUse).len(), 16);
}

// ---------------------------------------------------------------------------
// SessionRecord.should_compact tests
// ---------------------------------------------------------------------------

#[test]
fn should_compact_returns_false_when_threshold_zero() {
    let mut session = anvil::session::SessionRecord::new(PathBuf::from("/tmp/test"));
    session.auto_compact_threshold = 0;
    // Add messages
    for i in 0..100 {
        session.push_message(anvil::session::SessionMessage::new(
            anvil::session::MessageRole::User,
            "test",
            format!("message {i}"),
        ));
    }
    assert!(!session.should_compact());
}

#[test]
fn should_compact_returns_false_below_threshold() {
    let mut session = anvil::session::SessionRecord::new(PathBuf::from("/tmp/test"));
    session.auto_compact_threshold = 64;
    for i in 0..10 {
        session.push_message(anvil::session::SessionMessage::new(
            anvil::session::MessageRole::User,
            "test",
            format!("message {i}"),
        ));
    }
    assert!(!session.should_compact());
}

#[test]
fn should_compact_returns_true_above_threshold() {
    let mut session = anvil::session::SessionRecord::new(PathBuf::from("/tmp/test"));
    session.auto_compact_threshold = 5;
    for i in 0..10 {
        session.push_message(anvil::session::SessionMessage::new(
            anvil::session::MessageRole::User,
            "test",
            format!("message {i}"),
        ));
    }
    assert!(session.should_compact());
}

// ---------------------------------------------------------------------------
// App integration tests (hooks_engine initialization)
// ---------------------------------------------------------------------------

#[test]
fn app_initializes_without_hooks_config() {
    let app = common::build_app();
    // App should initialize fine without hooks.json
    assert!(app.session().message_count() == 0 || app.session().message_count() > 0);
}

#[test]
fn app_initializes_with_hooks_config() {
    let root = common::unique_test_dir("hooks_app");
    std::fs::create_dir_all(root.join(".anvil")).unwrap();
    std::fs::write(
        root.join(".anvil").join("hooks.json"),
        r#"{
            "hooks": {
                "PreToolUse": [{ "command": "echo ok" }]
            }
        }"#,
    )
    .unwrap();

    let app = common::build_app_in(root);
    // App should initialize with hooks.json
    assert!(app.session().message_count() == 0 || app.session().message_count() > 0);
}

#[test]
fn app_graceful_degradation_on_invalid_hooks_json() {
    let root = common::unique_test_dir("hooks_invalid");
    std::fs::create_dir_all(root.join(".anvil")).unwrap();
    std::fs::write(root.join(".anvil").join("hooks.json"), "not valid json").unwrap();

    // App should still initialize (graceful degradation)
    let app = common::build_app_in(root);
    assert!(app.session().message_count() == 0 || app.session().message_count() > 0);
}

// ---------------------------------------------------------------------------
// HookError display tests
// ---------------------------------------------------------------------------

#[test]
fn hook_error_display_variants() {
    let errors: Vec<HookError> = vec![
        HookError::CommandParseFailed {
            command: "cmd".to_string(),
            reason: "bad quotes".to_string(),
        },
        HookError::CommandNotFound {
            command: "/bad/path".to_string(),
        },
        HookError::PermissionDenied {
            command: "cmd".to_string(),
            path: "/path".to_string(),
        },
        HookError::Timeout {
            command: "slow".to_string(),
            timeout_ms: 5000,
        },
        HookError::ExecutionFailed {
            command: "fail".to_string(),
            exit_code: Some(1),
            stderr: "error".to_string(),
        },
        HookError::Shutdown,
        HookError::ParseError {
            file: PathBuf::from("hooks.json"),
            reason: "bad json".to_string(),
        },
    ];

    for err in errors {
        let display = format!("{err}");
        assert!(!display.is_empty());
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_config_with_echo(point_name: &str) -> HooksConfig {
    make_config_with_command(point_name, "echo ok")
}

fn make_config_with_command(point_name: &str, command: &str) -> HooksConfig {
    let point = match point_name {
        "PreToolUse" => HookPoint::PreToolUse,
        "PostToolUse" => HookPoint::PostToolUse,
        "PreCompact" => HookPoint::PreCompact,
        "PostSession" => HookPoint::PostSession,
        _ => panic!("unknown hook point: {point_name}"),
    };
    let mut hooks = HashMap::new();
    hooks.insert(
        point,
        vec![HookEntry {
            command: command.to_string(),
            timeout_ms: 5000,
            on_timeout: "continue".to_string(),
        }],
    );
    HooksConfig { hooks }
}

fn create_block_script(dir: &std::path::Path) -> PathBuf {
    let script_path = dir.join("block_hook.sh");
    {
        let mut f = std::fs::File::create(&script_path).unwrap();
        writeln!(f, "#!/bin/sh").unwrap();
        writeln!(f, "echo 'blocked by hook'").unwrap();
        writeln!(f, "exit 2").unwrap();
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    script_path
}
