use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const FRAMES_UTF8: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const FRAMES_ASCII: &[&str] = &["|", "/", "-", "\\"];
const TICK_MS: u64 = 80;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpinnerEnv {
    pub enabled: bool,
    pub use_color: bool,
    pub use_utf8: bool,
}

impl SpinnerEnv {
    pub fn detect() -> Self {
        Self::detect_with(
            |key| std::env::var_os(key).map(|value| value.to_string_lossy().into_owned()),
            crate::tui::terminal::stderr_is_tty(),
        )
    }

    pub fn detect_with(get_env: impl Fn(&str) -> Option<String>, stderr_is_tty: bool) -> Self {
        if crate::tui::terminal::env_non_empty_with(&get_env, "ANVIL_NO_SPINNER") {
            return Self {
                enabled: false,
                use_color: false,
                use_utf8: false,
            };
        }
        let no_color = crate::tui::terminal::env_non_empty_with(&get_env, "NO_COLOR");
        let lang = get_env("LC_ALL")
            .or_else(|| get_env("LANG"))
            .unwrap_or_default()
            .to_ascii_uppercase();
        Self {
            enabled: stderr_is_tty,
            use_color: !no_color,
            use_utf8: lang.contains("UTF-8") || lang.contains("UTF8"),
        }
    }
}

pub struct Spinner {
    inner: Option<Active>,
}

struct Active {
    stop: Arc<AtomicBool>,
    wake: Arc<(Mutex<()>, Condvar)>,
    handle: Option<JoinHandle<()>>,
}

impl Spinner {
    pub fn start(label: impl Into<String>) -> Option<Self> {
        Self::start_with_env(SpinnerEnv::detect(), label)
    }

    pub fn start_with_env(env: SpinnerEnv, label: impl Into<String>) -> Option<Self> {
        if !env.enabled {
            return None;
        }
        Some(Self::spawn(env, sanitize_label(&label.into())))
    }

    fn spawn(env: SpinnerEnv, label: String) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let wake = Arc::new((Mutex::new(()), Condvar::new()));
        let thread_stop = stop.clone();
        let thread_wake = wake.clone();
        let start = Instant::now();
        let handle = thread::Builder::new()
            .name("anvilminimal-spinner".to_string())
            .spawn(move || render_loop(start, label, env, thread_stop, thread_wake))
            .ok();
        match handle {
            Some(handle) => Self {
                inner: Some(Active {
                    stop,
                    wake,
                    handle: Some(handle),
                }),
            },
            None => Self { inner: None },
        }
    }

    pub fn stop(&mut self) {
        let Some(mut active) = self.inner.take() else {
            return;
        };
        active.stop.store(true, Ordering::SeqCst);
        let (lock, cvar) = &*active.wake;
        if let Ok(_guard) = lock.lock() {
            cvar.notify_all();
        }
        if let Some(handle) = active.handle.take() {
            let _ = handle.join();
        }
        clear_line();
    }
}

impl Drop for Spinner {
    fn drop(&mut self) {
        self.stop();
    }
}

pub fn sanitize_label(s: &str) -> String {
    s.chars()
        .map(|ch| {
            let cp = ch as u32;
            let is_bidi = matches!(
                cp,
                0x200E | 0x200F | 0x202A..=0x202E | 0x2066..=0x2069
            );
            if cp < 0x20 || cp == 0x7F || is_bidi {
                '?'
            } else {
                ch
            }
        })
        .collect()
}

fn clear_line() {
    let mut err = io::stderr().lock();
    let _ = err.write_all(b"\r\x1b[2K");
    let _ = err.flush();
}

fn render_loop(
    start: Instant,
    label: String,
    env: SpinnerEnv,
    stop: Arc<AtomicBool>,
    wake: Arc<(Mutex<()>, Condvar)>,
) {
    let frames = if env.use_utf8 {
        FRAMES_UTF8
    } else {
        FRAMES_ASCII
    };
    let (color_on, color_off) = if env.use_color {
        ("\x1b[36m", "\x1b[0m")
    } else {
        ("", "")
    };
    let mut i = 0usize;
    let (lock, cvar) = &*wake;
    while !stop.load(Ordering::SeqCst) {
        let frame = frames[i % frames.len()];
        let line = format!(
            "\r\x1b[2K{color_on}{frame}{color_off} {label} {}s",
            start.elapsed().as_secs()
        );
        {
            let mut err = io::stderr().lock();
            if err.write_all(line.as_bytes()).is_err() {
                stop.store(true, Ordering::SeqCst);
                return;
            }
            let _ = err.flush();
        }
        i = i.wrapping_add(1);
        if let Ok(guard) = lock.lock() {
            let _ = cvar.wait_timeout(guard, Duration::from_millis(TICK_MS));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spinner_detects_disable_env() {
        let env = SpinnerEnv::detect_with(
            |key| (key == "ANVIL_NO_SPINNER").then(|| "1".to_string()),
            true,
        );
        assert!(!env.enabled);
    }

    #[test]
    fn spinner_detects_non_tty() {
        let env = SpinnerEnv::detect_with(|_| None, false);
        assert!(!env.enabled);
    }

    #[test]
    fn spinner_label_sanitizes_escape_and_bidi() {
        assert_eq!(sanitize_label("x\x1b[31m"), "x?[31m");
        assert_eq!(sanitize_label("a\u{202E}b"), "a?b");
    }

    #[test]
    fn spinner_disabled_env_does_not_spawn() {
        let spinner = Spinner::start_with_env(
            SpinnerEnv {
                enabled: false,
                use_color: false,
                use_utf8: false,
            },
            "x",
        );
        assert!(spinner.is_none());
    }
}
