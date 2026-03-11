use crate::ui::interactive::{InteractiveFrame, UiEvent};

pub fn render_banner() -> String {
    [
        "\x1b[38;5;45m      /\\\\\x1b[0m      \x1b[38;5;214m[]\x1b[0m      \x1b[38;5;81m/\\\\\x1b[0m",
        "\x1b[38;5;45m     /  \\\\\x1b[0m   \x1b[38;5;220m[] []\x1b[0m   \x1b[38;5;81m/  \\\\\x1b[0m",
        "\x1b[38;5;51m    / /\\\\ \\\\\x1b[0m   \x1b[38;5;214m==\x1b[0m    \x1b[38;5;87m/ /\\\\ \\\\\x1b[0m",
        "\x1b[38;5;51m   / ____  \\\\\x1b[0m  \x1b[38;5;226m[][]\x1b[0m   \x1b[38;5;87m/ ____  \\\\\x1b[0m",
        "\x1b[38;5;39m  /_/    \\_\\\\x1b[0m   \x1b[38;5;196mA N V I L\x1b[0m",
        "\x1b[38;5;244m  local coding agent for Ollama / LM Studio\x1b[0m",
    ]
    .join("\n")
}

pub fn render_event_log(events: &[UiEvent]) -> String {
    events
        .iter()
        .map(|event| match event {
            UiEvent::UserInput(text) => format!("user> {text}"),
            UiEvent::AgentText(text) => format!("agent> {text}"),
            UiEvent::ToolCall(text) => format!("tool> {text}"),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn render_frame(frame: &InteractiveFrame) -> String {
    format!(
        "{title}\nprovider: {provider}\nmodel: {model}\ncwd: {cwd}\n\n{body}\n\nmode: {mode} | hint: {hint} | tokens: {tokens}",
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
