use std::io::{self, IsTerminal};

pub fn stdin_is_tty() -> bool {
    io::stdin().is_terminal()
}

pub fn stdout_is_tty() -> bool {
    io::stdout().is_terminal()
}

pub fn stderr_is_tty() -> bool {
    io::stderr().is_terminal()
}

pub fn env_non_empty(name: &str) -> bool {
    std::env::var_os(name).is_some_and(|value| !value.is_empty())
}

pub fn no_color() -> bool {
    env_non_empty("NO_COLOR")
}

pub fn utf8_locale() -> bool {
    let value = std::env::var("LC_ALL")
        .ok()
        .or_else(|| std::env::var("LANG").ok())
        .unwrap_or_default()
        .to_ascii_uppercase();
    value.contains("UTF-8") || value.contains("UTF8")
}

pub fn env_non_empty_with(get_env: impl Fn(&str) -> Option<String>, name: &str) -> bool {
    get_env(name).is_some_and(|value| !value.is_empty())
}
