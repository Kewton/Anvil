use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::config::Config;
use crate::tui::status::UiStatus;

const TICK: Duration = Duration::from_millis(200);

pub mod ansi {
    pub fn build_decstbm(rows: u16) -> Option<String> {
        if rows < 2 {
            None
        } else {
            Some(format!("\x1b[1;{}r", rows - 1))
        }
    }

    pub fn reset_decstbm() -> &'static str {
        "\x1b[r"
    }

    pub fn clear_line() -> &'static str {
        "\r\x1b[2K"
    }

    pub fn save_cursor() -> &'static str {
        "\x1b[s"
    }

    pub fn restore_cursor() -> &'static str {
        "\x1b[u"
    }

    pub fn move_to(row: u16, col: u16) -> String {
        format!("\x1b[{row};{col}H")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FooterEnv {
    pub enabled: bool,
    pub use_color: bool,
}

impl FooterEnv {
    pub fn detect(config: &Config) -> Self {
        Self::detect_with(
            |key| std::env::var_os(key).map(|value| value.to_string_lossy().into_owned()),
            crate::tui::terminal::stdout_is_tty(),
            config.no_footer,
        )
    }

    pub fn detect_with(
        get_env: impl Fn(&str) -> Option<String>,
        stdout_is_tty: bool,
        no_footer: bool,
    ) -> Self {
        if no_footer || crate::tui::terminal::env_non_empty_with(&get_env, "ANVIL_NO_FOOTER") {
            return Self {
                enabled: false,
                use_color: false,
            };
        }
        let no_color = crate::tui::terminal::env_non_empty_with(get_env, "NO_COLOR");
        Self {
            enabled: stdout_is_tty,
            use_color: !no_color,
        }
    }
}

#[derive(Debug)]
struct FooterState {
    status: Mutex<UiStatus>,
    freeze: AtomicBool,
    stop: AtomicBool,
    wake: (Mutex<()>, Condvar),
}

pub struct Footer {
    inner: Option<Active>,
}

struct Active {
    state: Arc<FooterState>,
    handle: Option<JoinHandle<()>>,
    rows: u16,
}

impl Footer {
    pub fn start(config: &Config) -> Self {
        let env = FooterEnv::detect(config);
        if !env.enabled {
            return Self { inner: None };
        }
        let Ok((_cols, rows)) = crossterm::terminal::size() else {
            return Self { inner: None };
        };
        if rows < 2 {
            return Self { inner: None };
        }
        let Some(decstbm) = ansi::build_decstbm(rows) else {
            return Self { inner: None };
        };
        {
            let mut stdout = io::stdout().lock();
            if stdout.write_all(decstbm.as_bytes()).is_err() {
                return Self { inner: None };
            }
            let _ = stdout.write_all(b"\x1b[H");
            let _ = stdout.flush();
        }
        let state = Arc::new(FooterState {
            status: Mutex::new(UiStatus::from_config(config)),
            freeze: AtomicBool::new(false),
            stop: AtomicBool::new(false),
            wake: (Mutex::new(()), Condvar::new()),
        });
        let thread_state = state.clone();
        let handle = thread::Builder::new()
            .name("anvilminimal-footer".to_string())
            .spawn(move || render_loop(thread_state, rows, env))
            .ok();
        match handle {
            Some(handle) => Self {
                inner: Some(Active {
                    state,
                    handle: Some(handle),
                    rows,
                }),
            },
            None => {
                let _ = io::stdout().write_all(ansi::reset_decstbm().as_bytes());
                Self { inner: None }
            }
        }
    }

    pub fn publish(&self, status: UiStatus) {
        let Some(active) = &self.inner else {
            return;
        };
        if let Ok(mut current) = active.state.status.lock() {
            if status.prompt_tokens.is_some() || status.completion_tokens.is_some() {
                current.prompt_tokens = add_known(current.prompt_tokens, status.prompt_tokens);
                current.completion_tokens =
                    add_known(current.completion_tokens, status.completion_tokens);
            }
            current.mode = status.mode;
            current.provider = status.provider;
            current.model = status.model;
            current.context_budget = status.context_budget;
            current.yes = status.yes;
        }
        active.state.wake.1.notify_all();
    }

    pub fn freeze(&self) -> Option<FreezeGuard> {
        let state = self.inner.as_ref()?.state.clone();
        state.freeze.store(true, Ordering::SeqCst);
        state.wake.1.notify_all();
        Some(FreezeGuard { state })
    }
}

impl Drop for Footer {
    fn drop(&mut self) {
        let Some(mut active) = self.inner.take() else {
            return;
        };
        active.state.stop.store(true, Ordering::SeqCst);
        active.state.wake.1.notify_all();
        if let Some(handle) = active.handle.take() {
            let _ = handle.join();
        }
        let mut stdout = io::stdout().lock();
        let _ = stdout.write_all(ansi::reset_decstbm().as_bytes());
        let _ = stdout.write_all(ansi::move_to(active.rows, 1).as_bytes());
        let _ = stdout.write_all(ansi::clear_line().as_bytes());
        let _ = stdout.flush();
    }
}

pub struct FreezeGuard {
    state: Arc<FooterState>,
}

impl Drop for FreezeGuard {
    fn drop(&mut self) {
        self.state.freeze.store(false, Ordering::SeqCst);
        self.state.wake.1.notify_all();
    }
}

fn add_known(current: Option<u64>, delta: Option<u64>) -> Option<u64> {
    match (current, delta) {
        (None, None) => None,
        (current, delta) => Some(current.unwrap_or(0) + delta.unwrap_or(0)),
    }
}

pub fn build_footer_line(status: &UiStatus, use_color: bool) -> String {
    let tokens = match status.token_total() {
        Some(value) => format_token_count(value),
        None => "n/a".to_string(),
    };
    let yes = if status.yes { " [yes]" } else { "" };
    let body = format!(
        "[{}] provider:{} model:{} ctx:{} tokens:{}{}",
        status.mode, status.provider, status.model, status.context_budget, tokens, yes
    );
    if use_color {
        format!("\x1b[2m{body}\x1b[0m")
    } else {
        body
    }
}

fn format_token_count(value: u64) -> String {
    if value < 1_000 {
        value.to_string()
    } else if value.div_euclid(1_000) * 1_000 == value {
        format!("{}k", value / 1_000)
    } else {
        format!("{:.1}k", value as f64 / 1_000.0)
    }
}

fn render_loop(state: Arc<FooterState>, rows: u16, env: FooterEnv) {
    loop {
        if state.stop.load(Ordering::SeqCst) {
            break;
        }
        if !state.freeze.load(Ordering::SeqCst) {
            let status = match state.status.lock() {
                Ok(status) => status.clone(),
                Err(poisoned) => poisoned.into_inner().clone(),
            };
            let line = build_footer_line(&status, env.use_color);
            let mut stdout = io::stdout().lock();
            let _ = stdout.write_all(ansi::save_cursor().as_bytes());
            let _ = stdout.write_all(ansi::move_to(rows, 1).as_bytes());
            let _ = stdout.write_all(ansi::clear_line().as_bytes());
            let _ = stdout.write_all(line.as_bytes());
            let _ = stdout.write_all(ansi::restore_cursor().as_bytes());
            let _ = stdout.flush();
        }
        if let Ok(guard) = state.wake.0.lock() {
            let _ = state.wake.1.wait_timeout(guard, TICK);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(tokens: Option<u64>) -> UiStatus {
        UiStatus {
            mode: "act".to_string(),
            provider: "ollama".to_string(),
            model: "m".to_string(),
            context_budget: 65536,
            yes: true,
            prompt_tokens: tokens,
            completion_tokens: None,
        }
    }

    #[test]
    fn footer_env_disable_by_flag() {
        assert!(!FooterEnv::detect_with(|_| None, true, true).enabled);
    }

    #[test]
    fn footer_env_disable_by_env() {
        let env = FooterEnv::detect_with(
            |key| (key == "ANVIL_NO_FOOTER").then(|| "1".to_string()),
            true,
            false,
        );
        assert!(!env.enabled);
    }

    #[test]
    fn footer_env_non_tty_noop() {
        assert!(!FooterEnv::detect_with(|_| None, false, false).enabled);
    }

    #[test]
    fn footer_decstbm_rows_under_two_self_disable() {
        assert_eq!(ansi::build_decstbm(1), None);
        assert_eq!(ansi::build_decstbm(2).as_deref(), Some("\x1b[1;1r"));
    }

    #[test]
    fn footer_unknown_tokens_are_na() {
        let line = build_footer_line(&status(None), false);
        assert!(line.contains("tokens:n/a"));
    }

    #[test]
    fn footer_known_tokens_are_formatted() {
        let line = build_footer_line(&status(Some(1500)), false);
        assert!(line.contains("tokens:1.5k"));
    }
}
