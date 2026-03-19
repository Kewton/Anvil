mod common;

use anvil::app::mock::MockAppExt;
use anvil::provider::ProviderRuntimeContext;
use anvil::tui::Tui;

#[test]
fn tui_renders_status_line() {
    let mut app = common::build_app();
    let tui = Tui::new();

    let _ = app
        .initial_snapshot()
        .expect("initial snapshot should build");
    let rendered = app.render_console(&tui).expect("render should succeed");

    assert!(rendered.contains("[A] anvil >"));
    assert!(rendered.contains("Ready."));
    assert!(rendered.contains("provider=ollama"));
    assert!(rendered.contains("Enter to send"));
    // Issue #96: [U] you > is no longer rendered by render_console()
    // for Ready/Done states — the interactive readline prompt handles it.
    assert!(!rendered.contains("[U] you >"));
    assert!(rendered.contains("model:local-default"));
}

#[test]
fn mock_thinking_snapshot_contains_plan_and_reasoning() {
    let mut app = common::build_app();

    let snapshot = app
        .mock_thinking_snapshot()
        .expect("thinking snapshot should build");

    assert_eq!(snapshot.state, anvil::contracts::RuntimeState::Thinking);
    assert!(snapshot.plan.is_some());
    assert!(!snapshot.reasoning_summary.is_empty());
    assert!(snapshot.elapsed_ms.is_some());
    assert!(snapshot.context_usage.is_some());
    let rendered = app
        .render_console(&anvil::tui::Tui::new())
        .expect("console render should succeed");
    assert!(rendered.contains("working on 2/3"));
    assert!(rendered.contains("[U] you > /status /help /plan"));
}

#[test]
fn mock_approval_snapshot_represents_one_tool_call() {
    let mut app = common::build_app();
    let _ = app
        .mock_thinking_snapshot()
        .expect("thinking snapshot should build");

    let snapshot = app
        .mock_approval_snapshot()
        .expect("approval snapshot should build");
    let approval = snapshot.approval.expect("approval should exist");

    assert_eq!(
        snapshot.state,
        anvil::contracts::RuntimeState::AwaitingApproval
    );
    assert_eq!(approval.tool_name, "Write");
    assert_eq!(approval.tool_call_id, "call_001");
}

#[test]
fn mock_interrupted_snapshot_exposes_next_actions() {
    let mut app = common::build_app();
    let _ = app
        .mock_thinking_snapshot()
        .expect("thinking snapshot should build");

    let snapshot = app
        .mock_interrupted_snapshot()
        .expect("interrupted snapshot should build");
    let interrupt = snapshot.interrupt.expect("interrupt details should exist");

    assert_eq!(snapshot.state, anvil::contracts::RuntimeState::Interrupted);
    assert_eq!(interrupt.interrupted_what, "provider turn");
    assert!(!interrupt.next_actions.is_empty());
    assert_eq!(
        app.session().session_event,
        Some(anvil::contracts::AppEvent::SessionNormalizedAfterInterrupt)
    );
    assert!(
        app.session()
            .event_log
            .contains(&anvil::contracts::AppEvent::SessionNormalizedAfterInterrupt)
    );
}

#[test]
fn tui_renders_approval_and_interrupt_sections() {
    let approval_config = common::build_config_in(common::unique_test_dir("tui_approval"));
    let provider =
        ProviderRuntimeContext::bootstrap(&approval_config).expect("provider should bootstrap");
    let mut app = anvil::app::App::new(
        approval_config,
        provider,
        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
    )
    .expect("app should initialize");
    let tui = Tui::new();

    let _ = app
        .mock_thinking_snapshot()
        .expect("thinking snapshot should build");
    let _ = app
        .mock_approval_snapshot()
        .expect("approval snapshot should build");
    let approval_rendered = app.render_console(&tui).expect("render should succeed");

    let interrupt_config = common::build_config_in(common::unique_test_dir("tui_interrupt"));
    let provider =
        ProviderRuntimeContext::bootstrap(&interrupt_config).expect("provider should bootstrap");
    let mut interrupted_app = anvil::app::App::new(
        interrupt_config,
        provider,
        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
    )
    .expect("app should initialize");
    let _ = interrupted_app
        .mock_thinking_snapshot()
        .expect("thinking snapshot should build");
    let _ = interrupted_app
        .mock_interrupted_snapshot()
        .expect("interrupted snapshot should build");
    let interrupted_rendered = interrupted_app
        .render_console(&tui)
        .expect("render should succeed");

    assert!(approval_rendered.contains("[A] anvil > approval"));
    assert!(approval_rendered.contains("tool : Write"));
    assert!(interrupted_rendered.contains("[A] anvil > interrupted"));
    assert!(interrupted_rendered.contains("next :"));
    assert!(interrupted_rendered.contains("/resume"));
}

#[test]
fn startup_screen_shows_logo_model_and_project() {
    let config = common::build_config_in(common::unique_test_dir("startup"));
    let provider = ProviderRuntimeContext::bootstrap(&config).expect("provider should bootstrap");
    let mut app = anvil::app::App::new(
        config.clone(),
        provider,
        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
    )
    .expect("app should initialize");
    let tui = Tui::new();

    let snapshot = app
        .initial_snapshot()
        .expect("initial snapshot should build");
    let rendered = tui.render_startup(
        &config,
        &snapshot,
        &config.runtime.model,
        config.runtime.context_window,
    );

    assert!(rendered.contains("local coding agent for serious terminal work"));
    assert!(rendered.contains("Model   :"));
    assert!(rendered.contains("Project :"));
    assert!(rendered.contains("[U] you >"));
}

#[test]
fn tui_renders_working_and_done_views_with_tool_logs() {
    let mut app = common::build_app();
    let tui = Tui::new();

    app.record_user_input("msg_001", "inspect state handling")
        .expect("user input should persist");
    let _ = app
        .mock_thinking_snapshot()
        .expect("thinking snapshot should build");

    let _ = app
        .mock_working_snapshot()
        .expect("working snapshot should build");
    let working = app
        .render_console(&tui)
        .expect("working render should succeed");
    let _ = app
        .mock_done_snapshot()
        .expect("done snapshot should build");
    let done = app
        .render_console(&tui)
        .expect("done render should succeed");

    // All messages (user, tool, assistant) are excluded from live-turn
    // frames because they were already shown during the turn via stderr
    // streaming and tool execution output (Issue #1).
    assert!(
        !working.contains("[U] you > inspect state handling"),
        "user message should not appear in working frame (already shown)"
    );
    assert!(working.contains("[T] tool  > progress"));
    assert!(working.contains("completed:2"));
    assert!(working.contains("[T] tool  > Read"));
    assert!(working.contains("tools active"));
    assert!(
        !done.contains("[A] anvil > 調査結果を整理しました。"),
        "assistant message should not appear in done frame"
    );
    assert!(
        app.session()
            .messages
            .iter()
            .any(|m| m.content.contains("調査結果を整理しました。")),
        "assistant message should still be in session history"
    );
    assert!(done.contains("[A] anvil > result"));
    assert!(done.contains("session saved"));
    assert!(done.contains("/continue"));
    assert!(done.contains("/compact"));
}

#[test]
fn app_can_render_console_from_runtime_state_without_manual_message_plumbing() {
    let mut app = common::build_app();
    let tui = Tui::new();

    app.record_user_input("msg_001", "trace runtime-driven rendering")
        .expect("user input should persist");
    let _ = app
        .mock_thinking_snapshot()
        .expect("thinking snapshot should build");

    let rendered = app
        .render_console(&tui)
        .expect("console render should succeed");

    // Messages are excluded from live-turn frames (Issue #1).
    assert!(
        !rendered.contains("[U] you > trace runtime-driven rendering"),
        "user message should not appear in live-turn frame"
    );
    assert!(rendered.contains("[A] anvil > Thinking."));
    assert!(rendered.contains("typeahead enabled"));
}

#[test]
fn tui_limits_rendered_history_to_recent_messages() {
    let mut app = common::build_app();
    let tui = Tui::new();

    for index in 0..8 {
        app.record_user_input(format!("msg_{index:03}"), format!("message {index}"))
            .expect("user input should persist");
    }
    let _ = app
        .mock_thinking_snapshot()
        .expect("thinking snapshot should build");

    // Live-turn frames exclude all messages (Issue #1), so verify via
    // startup rendering where history is displayed.
    let startup = app
        .startup_console(&tui)
        .expect("startup render should succeed");

    assert!(!startup.contains("[U] you > message 0"));
    assert!(startup.contains("[U] you > message 7"));
    assert!(startup.contains("history: recent 5 messages"));
}

#[test]
fn tui_rendering_uses_runtime_render_path_not_snapshot_model_fields() {
    let mut app = common::build_app();
    let tui = Tui::new();
    let _ = app
        .initial_snapshot()
        .expect("initial snapshot should build");

    let rendered = app.render_console(&tui).expect("render should succeed");

    assert!(rendered.contains("model:local-default"));
    assert!(rendered.contains("event:StateChanged"));
}

#[test]
fn done_frame_excludes_streamed_assistant_messages() {
    let mut app = common::build_app();
    let tui = Tui::new();

    app.record_user_input("msg_001", "ask a question")
        .expect("user input should persist");
    let _ = app
        .mock_thinking_snapshot()
        .expect("thinking snapshot should build");

    // Simulate the Done event which records an assistant message via
    // record_assistant_output (same as the streaming path).
    let _ = app
        .mock_done_snapshot()
        .expect("done snapshot should build");

    let rendered = app
        .render_console(&tui)
        .expect("done render should succeed");

    // All messages are excluded from live-turn frames because they were
    // already shown via stderr streaming and tool output (Issue #1).
    assert!(
        !rendered.contains("[U] you > ask a question"),
        "user message should not appear in done frame (already shown)"
    );
    assert!(
        !rendered.contains("[A] anvil > 調査結果を整理しました。"),
        "assistant message should be excluded from done frame"
    );
    // The frame should still contain the status/result sections
    assert!(rendered.contains("[A] anvil > result"));
    assert!(rendered.contains("[A] anvil > Done."));
}

#[test]
fn startup_shows_anvil_md_loaded() {
    let mut config = common::build_config_in(common::unique_test_dir("startup_anvil"));
    config.set_project_instructions_for_test(Some("test instructions".to_string()));
    let provider = ProviderRuntimeContext::bootstrap(&config).expect("provider should bootstrap");
    let mut app = anvil::app::App::new(
        config.clone(),
        provider,
        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
    )
    .expect("app should initialize");
    let tui = Tui::new();

    let snapshot = app
        .initial_snapshot()
        .expect("initial snapshot should build");
    let rendered = tui.render_startup(
        &config,
        &snapshot,
        &config.runtime.model,
        config.runtime.context_window,
    );

    assert!(rendered.contains("ANVIL.md: loaded"));
}

#[test]
fn startup_without_anvil_md() {
    let config = common::build_config_in(common::unique_test_dir("startup_no_anvil"));
    let provider = ProviderRuntimeContext::bootstrap(&config).expect("provider should bootstrap");
    let mut app = anvil::app::App::new(
        config.clone(),
        provider,
        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
    )
    .expect("app should initialize");
    let tui = Tui::new();

    let snapshot = app
        .initial_snapshot()
        .expect("initial snapshot should build");
    let rendered = tui.render_startup(
        &config,
        &snapshot,
        &config.runtime.model,
        config.runtime.context_window,
    );

    assert!(!rendered.contains("ANVIL.md: loaded"));
}

#[test]
fn context_warning_bar_displayed_for_warning_level() {
    let tui = Tui::new();
    let snapshot = anvil::contracts::AppStateSnapshot::new(anvil::contracts::RuntimeState::Done)
        .with_status("Done. session saved".to_string())
        .with_completion_summary("completed task", "session saved")
        .with_context_usage(8500, 10000)
        .with_context_warning(anvil::contracts::ContextWarningLevel::Warning);

    let context = anvil::contracts::ConsoleRenderContext {
        snapshot,
        model_name: "test-model".to_string(),
        messages: vec![],
        history_summary: None,
    };

    let rendered = tui.render_console(&context);
    assert!(
        rendered.contains("[!] Warning: Context usage at 85%"),
        "warning bar should display at 85% usage"
    );
    assert!(
        rendered.contains("/compact"),
        "warning should suggest /compact"
    );
}

#[test]
fn context_warning_bar_displayed_for_critical_level() {
    let tui = Tui::new();
    let snapshot = anvil::contracts::AppStateSnapshot::new(anvil::contracts::RuntimeState::Done)
        .with_status("Done. session saved".to_string())
        .with_completion_summary("completed task", "session saved")
        .with_context_usage(9500, 10000)
        .with_context_warning(anvil::contracts::ContextWarningLevel::Critical);

    let context = anvil::contracts::ConsoleRenderContext {
        snapshot,
        model_name: "test-model".to_string(),
        messages: vec![],
        history_summary: None,
    };

    let rendered = tui.render_console(&context);
    assert!(
        rendered.contains("[!] CRITICAL: Context usage at 95%"),
        "critical bar should display at 95% usage"
    );
    assert!(
        rendered.contains("/compact immediately"),
        "critical warning should suggest immediate /compact"
    );
}

#[test]
fn context_warning_bar_absent_when_no_warning() {
    let tui = Tui::new();
    let snapshot = anvil::contracts::AppStateSnapshot::new(anvil::contracts::RuntimeState::Done)
        .with_status("Done. session saved".to_string())
        .with_completion_summary("completed task", "session saved")
        .with_context_usage(5000, 10000);

    let context = anvil::contracts::ConsoleRenderContext {
        snapshot,
        model_name: "test-model".to_string(),
        messages: vec![],
        history_summary: None,
    };

    let rendered = tui.render_console(&context);
    assert!(
        !rendered.contains("[!]"),
        "no warning bar should appear when usage is below threshold"
    );
}

#[test]
fn done_hint_line_includes_compact() {
    let tui = Tui::new();
    let snapshot = anvil::contracts::AppStateSnapshot::new(anvil::contracts::RuntimeState::Done)
        .with_status("Done. session saved".to_string())
        .with_completion_summary("completed task", "session saved")
        .with_context_usage(5000, 10000);

    let context = anvil::contracts::ConsoleRenderContext {
        snapshot,
        model_name: "test-model".to_string(),
        messages: vec![],
        history_summary: None,
    };

    let rendered = tui.render_console(&context);
    assert!(
        rendered.contains("/compact"),
        "Done hint line should include /compact"
    );
}

#[test]
fn status_detail_shows_token_usage() {
    let snapshot = anvil::contracts::AppStateSnapshot::new(anvil::contracts::RuntimeState::Done)
        .with_status("Done. session saved".to_string())
        .with_completion_summary("completed task", "session saved")
        .with_context_usage(45000, 200000);

    let detail = anvil::app::render::render_status_detail(&snapshot);
    assert!(detail.contains("45000"));
    assert!(detail.contains("200000"));
    assert!(detail.contains("22%") || detail.contains("23%"));
}

#[test]
fn busy_prompt_hints_include_slash_commands() {
    let mut app = common::build_app();
    let tui = Tui::new();
    let _ = app
        .mock_thinking_snapshot()
        .expect("thinking snapshot should build");

    let rendered = app.render_console(&tui).expect("render should succeed");

    assert!(rendered.contains("/help"));
    assert!(rendered.contains("/status"));
    assert!(rendered.contains("[U] you > /status /help /plan"));
}

/// Issue #96: render_console() must NOT include `[U] you >` when state is
/// Done or Ready, because the interactive readline prompt already displays it.
#[test]
fn done_frame_omits_user_prompt_to_avoid_duplicate() {
    let mut app = common::build_app();
    let tui = Tui::new();

    app.record_user_input("msg_001", "hello")
        .expect("user input should persist");
    let _ = app
        .mock_thinking_snapshot()
        .expect("thinking snapshot should build");
    let _ = app
        .mock_working_snapshot()
        .expect("working snapshot should build");
    let _ = app
        .mock_done_snapshot()
        .expect("done snapshot should build");

    let rendered = app.render_console(&tui).expect("render should succeed");

    assert!(
        !rendered.contains("[U] you >"),
        "Done frame should not contain [U] you > (readline handles it). Got:\n{rendered}"
    );
}

/// Issue #96: render_console() must NOT include `[U] you >` when state is
/// Ready (initial state), because the interactive readline prompt handles it.
#[test]
fn ready_frame_omits_user_prompt_to_avoid_duplicate() {
    let mut app = common::build_app();
    let tui = Tui::new();

    let _ = app
        .initial_snapshot()
        .expect("initial snapshot should build");
    let rendered = app.render_console(&tui).expect("render should succeed");

    assert!(
        !rendered.contains("[U] you >"),
        "Ready frame should not contain [U] you > (readline handles it). Got:\n{rendered}"
    );
}

#[test]
fn test_approval_view_serialize_skips_diff_preview() {
    let view = anvil::contracts::ApprovalView {
        tool_name: "file.write".to_string(),
        summary: "write foo.txt".to_string(),
        risk: "medium".to_string(),
        tool_call_id: "call_001".to_string(),
        diff_preview: Some("--- a\n+++ b\n-old\n+new\n".to_string()),
    };
    let json = serde_json::to_string(&view).expect("serialize");
    // #[serde(skip)] means diff_preview should not appear in output
    assert!(!json.contains("diff_preview"));
    assert!(!json.contains("old"));
}

#[test]
fn test_approval_view_deserialize_without_diff_preview() {
    // Old JSON without diff_preview field should still deserialize
    let json = r#"{"tool_name":"file.write","summary":"write foo.txt","risk":"medium","tool_call_id":"call_001"}"#;
    let view: anvil::contracts::ApprovalView =
        serde_json::from_str(json).expect("deserialize should succeed");
    assert_eq!(view.tool_name, "file.write");
    assert!(view.diff_preview.is_none());
}

#[test]
fn test_approval_view_with_diff_preview() {
    use anvil::contracts::{AppStateSnapshot, RuntimeState};
    let snapshot = AppStateSnapshot::new(RuntimeState::AwaitingApproval)
        .with_approval(
            "file.write".to_string(),
            "write test.txt".to_string(),
            "medium".to_string(),
            "call_002".to_string(),
        )
        .with_diff_preview(Some("-old line\n+new line\n".to_string()));

    let approval = snapshot.approval.as_ref().expect("approval present");
    assert_eq!(
        approval.diff_preview.as_deref(),
        Some("-old line\n+new line\n")
    );
}

#[test]
fn test_approval_view_without_diff_preview() {
    use anvil::contracts::{AppStateSnapshot, RuntimeState};
    let snapshot = AppStateSnapshot::new(RuntimeState::AwaitingApproval).with_approval(
        "file.write".to_string(),
        "write test.txt".to_string(),
        "medium".to_string(),
        "call_003".to_string(),
    );

    let approval = snapshot.approval.as_ref().expect("approval present");
    assert!(approval.diff_preview.is_none());
}

#[test]
fn footer_shows_perf_with_metrics() {
    let tui = Tui::new();
    let snapshot = anvil::contracts::AppStateSnapshot::new(anvil::contracts::RuntimeState::Done)
        .with_status("Done. session saved".to_string())
        .with_completion_summary("completed task", "session saved")
        .with_context_usage(5000, 10000)
        .with_inference_performance(anvil::contracts::InferencePerformanceView {
            tokens_per_sec_tenths: Some(325),
            eval_tokens: Some(100),
            eval_duration_ms: Some(3077),
        });

    let context = anvil::contracts::ConsoleRenderContext {
        snapshot,
        model_name: "test-model".to_string(),
        messages: vec![],
        history_summary: None,
    };

    let rendered = tui.render_console(&context);
    assert!(
        rendered.contains("perf:32.5tok/s"),
        "footer should display perf:32.5tok/s when metrics are present"
    );
}

#[test]
fn footer_shows_perf_dash_without_metrics() {
    let tui = Tui::new();
    let snapshot = anvil::contracts::AppStateSnapshot::new(anvil::contracts::RuntimeState::Done)
        .with_status("Done. session saved".to_string())
        .with_completion_summary("completed task", "session saved")
        .with_context_usage(5000, 10000);

    let context = anvil::contracts::ConsoleRenderContext {
        snapshot,
        model_name: "test-model".to_string(),
        messages: vec![],
        history_summary: None,
    };

    let rendered = tui.render_console(&context);
    assert!(
        rendered.contains("perf:-"),
        "footer should display perf:- when no metrics are present"
    );
}

#[test]
fn mock_done_snapshot_has_perf_in_footer() {
    let mut app = common::build_app();
    let tui = Tui::new();

    app.record_user_input("msg_001", "test perf display")
        .expect("user input should persist");
    let _ = app
        .mock_thinking_snapshot()
        .expect("thinking snapshot should build");
    let _ = app
        .mock_done_snapshot()
        .expect("done snapshot should build");

    let rendered = app
        .render_console(&tui)
        .expect("done render should succeed");
    assert!(
        rendered.contains("perf:32.5tok/s"),
        "mock done snapshot should show perf:32.5tok/s in footer"
    );
}
