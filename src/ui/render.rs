use crate::ui::interactive::{InteractiveFrame, UiEvent};

pub fn render_banner() -> String {
    [
        "\x1b[38;5;196m    _   _ _   _ _   _ ___ _     \x1b[0m",
        "\x1b[38;5;196m   /_\\ | \\ | | | | |_ _| |    \x1b[0m",
        "\x1b[38;5;203m  / _ \\|  \\| | | |  | || |__  \x1b[0m",
        "\x1b[38;5;209m /_/ \\_\\_|\\_| |_| |_|___|____| \x1b[0m",
        "\x1b[38;5;196m                A N V I L                \x1b[0m",
        "\x1b[38;5;244m offline local coding agent for Ollama / LM Studio \x1b[0m",
    ]
    .join("\n")
}

pub fn render_event_log(events: &[UiEvent]) -> String {
    events
        .iter()
        .map(|event| match event {
            UiEvent::UserInput(text) => format!("You  {text}"),
            UiEvent::AgentText(text) => format!("Anvil  {text}"),
            UiEvent::ToolCall(text) => format!("Tool  {text}"),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn render_frame(frame: &InteractiveFrame) -> String {
    format!(
        "{title}\n🦙 provider  {provider}\n🧠 model     {model}\n📁 cwd       {cwd}\n\n{body}\n\n🔐 mode {mode} | 💡 {hint} | 🧱 {tokens}",
        title = frame.title,
        provider = frame.provider,
        model = frame.model,
        cwd = frame.cwd,
        body = render_event_log(&frame.transcript),
        mode = frame.footer.mode,
        hint = frame.footer.pending_hint,
        tokens = frame.footer.token_status,
    )
}

pub fn render_startup_help() -> String {
    [
        "⌨ Enter send | \"\"\" multiline | /exit quit",
        "📝 Multiline: type \"\"\" on its own line to start and end block input",
    ]
    .join("\n")
}

pub fn render_result_block(summary: &str, details: &[String]) -> String {
    let mut lines = vec![
        "==================== RESULT ====================".to_string(),
        summary.to_string(),
    ];
    if !details.is_empty() {
        lines.push(String::new());
        lines.extend(details.iter().map(|detail| format!("• {detail}")));
    }
    lines.push("================================================".to_string());
    lines.join("\n")
}

pub fn render_rich_diff(before: &str, after: &str) -> String {
    let before_lines = before.lines().collect::<Vec<_>>();
    let after_lines = after.lines().collect::<Vec<_>>();
    let mut out = Vec::new();
    for line in &before_lines {
        if !after_lines.contains(line) {
            out.push(format!("[-] {line}"));
        }
    }
    for line in &after_lines {
        if !before_lines.contains(line) {
            out.push(format!("[+] {line}"));
        }
    }
    out.join("\n")
}

pub fn type_ahead_suggestions<'a>(prefix: &str, commands: &'a [&'a str]) -> Vec<&'a str> {
    commands
        .iter()
        .copied()
        .filter(|command| command.starts_with(prefix))
        .collect()
}
