use anvil::ui::interactive::{FooterState, InteractiveFrame, UiEvent};
use anvil::ui::render::{
    render_banner, render_event_log, render_frame, render_result_block, render_rich_diff,
    render_startup_help, type_ahead_suggestions,
};

#[test]
fn renderer_snapshot_contains_claude_code_like_sections() {
    let frame = InteractiveFrame {
        title: "Anvil".to_string(),
        provider: "ollama".to_string(),
        model: "qwen3.5:35b".to_string(),
        cwd: "/tmp/project".to_string(),
        transcript: vec![
            UiEvent::UserInput("fix tests".to_string()),
            UiEvent::AgentText("Inspecting files".to_string()),
            UiEvent::ToolCall("search".to_string()),
        ],
        footer: FooterState {
            mode: "act".to_string(),
            pending_hint: "/memory show".to_string(),
            token_status: "48k/200k".to_string(),
        },
    };

    let rendered = render_frame(&frame);

    assert!(rendered.contains("Anvil"));
    assert!(rendered.contains("🦙 provider"));
    assert!(rendered.contains("🔐 mode"));
    assert!(rendered.contains("48k/200k"));
    assert!(rendered.contains("📁 cwd"));
}

#[test]
fn renderer_event_sequence_and_contract_are_stable() {
    let events = vec![
        UiEvent::UserInput("hello".to_string()),
        UiEvent::AgentText("world".to_string()),
        UiEvent::ToolCall("diff".to_string()),
    ];
    let rendered = render_event_log(&events);

    assert!(rendered.lines().count() >= 3);
    assert!(rendered.contains("👤 You"));
    assert!(rendered.contains("🧱 Anvil"));
    assert!(rendered.contains("🛠 Tool"));
    assert!(rendered.contains("hello"));
    assert!(rendered.contains("world"));
    assert!(rendered.contains("diff"));
    assert!(rendered.contains("\x1b[48;5;52m"));
}

#[test]
fn rich_diff_and_type_ahead_are_available() {
    let diff = render_rich_diff("before\nline2\n", "after\nline2\n");
    let suggestions = type_ahead_suggestions(
        "/me",
        &["/memory add", "/memory show", "/plan create", "/subagent"],
    );

    assert!(diff.contains("[-] before"));
    assert!(diff.contains("[+] after"));
    assert_eq!(suggestions, vec!["/memory add", "/memory show"]);
}

#[test]
fn startup_banner_contains_colored_anvil_wordmark() {
    let banner = render_banner();

    assert!(banner.contains("A N V I L") || banner.contains("ANVIL"));
    assert!(banner.contains("\x1b[38;5;196m"));
    assert!(banner.contains("_   _"));
}

#[test]
fn startup_help_describes_multiline_input() {
    let help = render_startup_help();

    assert!(help.contains("\"\"\""));
    assert!(help.contains("multiline"));
    assert!(help.contains("/exit"));
}

#[test]
fn result_block_is_visually_separated() {
    let rendered =
        render_result_block("Created output", &["./sandbox/demo/index.html".to_string()]);

    assert!(rendered.contains("RESULT"));
    assert!(rendered.contains("Created output"));
    assert!(rendered.contains("./sandbox/demo/index.html"));
}
