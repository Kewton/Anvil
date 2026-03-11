use anvil::ui::interactive::{FooterState, InteractiveFrame, UiEvent};
use anvil::ui::render::{
    render_banner, render_event_log, render_frame, render_rich_diff, type_ahead_suggestions,
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
    assert!(rendered.contains("provider: ollama"));
    assert!(rendered.contains("mode: act"));
    assert!(rendered.contains("48k/200k"));
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
    assert!(rendered.contains("user> hello"));
    assert!(rendered.contains("agent> world"));
    assert!(rendered.contains("tool> diff"));
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

    assert!(banner.contains("A N V I L"));
    assert!(banner.contains("\x1b[38;5;196m"));
    assert!(banner.contains("/\\\\"));
}
