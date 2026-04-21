//! Animated terminal spinner for LLM inference and tool execution.
//!
//! See `dev-reports/design/issue-426-spinner-design-policy.md` for the full
//! design rationale. This module is crate-private to `loop_run`; callers
//! outside of `turn.rs` should not depend on it.

use std::io::{self, IsTerminal, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const FRAMES_UTF8: &[&str] = &[
    "\u{280B}", // ⠋
    "\u{2819}", // ⠙
    "\u{2839}", // ⠹
    "\u{2838}", // ⠸
    "\u{283C}", // ⠼
    "\u{2834}", // ⠴
    "\u{2826}", // ⠦
    "\u{2827}", // ⠧
    "\u{2807}", // ⠇
    "\u{280F}", // ⠏
];
const FRAMES_ASCII: &[&str] = &["|", "/", "-", "\\"];
const TICK_MS: u64 = 80;

// --- Env detection (DI-friendly, cached once per process) ------------------

pub(super) struct Env {
    pub(super) enabled: bool,
    pub(super) use_color: bool,
    pub(super) use_utf8: bool,
}

static ENV: OnceLock<Env> = OnceLock::new();

fn env() -> &'static Env {
    ENV.get_or_init(Env::detect)
}

impl Env {
    fn detect() -> Self {
        Self::detect_with(
            |k| std::env::var_os(k).map(|v| v.to_string_lossy().into_owned()),
            io::stderr().is_terminal(),
        )
    }

    /// Pure-function variant for testing: the caller supplies env lookups and
    /// the tty flag, so tests do not need `std::env::set_var` (which is
    /// unsafe-marked in recent Rust and races with parallel tests).
    pub(super) fn detect_with(
        get_env: impl Fn(&str) -> Option<String>,
        stderr_is_tty: bool,
    ) -> Self {
        if get_env("ANVIL_NO_SPINNER").is_some_and(|v| !v.is_empty()) {
            return Self {
                enabled: false,
                use_color: false,
                use_utf8: false,
            };
        }
        // POSIX: `NO_COLOR` disables color only when set to a non-empty value;
        // the empty string is treated as unset. Matches the semantics used by
        // other Anvil helpers so the spinner and progress output never disagree.
        let no_color = get_env("NO_COLOR").is_some_and(|v| !v.is_empty());
        let use_color = !no_color;
        // POSIX: `LC_ALL` overrides `LANG` when set. Use LC_ALL first, fall back
        // to LANG.
        let lang = get_env("LC_ALL")
            .or_else(|| get_env("LANG"))
            .unwrap_or_default();
        let up = lang.to_ascii_uppercase();
        Self {
            enabled: stderr_is_tty,
            use_color,
            use_utf8: up.contains("UTF-8") || up.contains("UTF8"),
        }
    }
}

// --- Sanitize: C0 + DEL + ESC + bidi control chars -> '?' ------------------

/// Replace C0 control characters (0x00-0x1F), DEL (0x7F), and Unicode bidi
/// overrides (U+200E/U+200F/U+202A..U+202E/U+2066..U+2069) with `'?'`.
/// Prevents terminal escape injection and visual spoofing via tool / model
/// names routed through the spinner label.
fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| {
            let u = c as u32;
            let is_bidi = matches!(
                u,
                0x200E | 0x200F | 0x202A..=0x202E | 0x2066..=0x2069
            );
            if u < 0x20 || c == '\u{007F}' || is_bidi {
                '?'
            } else {
                c
            }
        })
        .collect()
}

// --- Spinner / Active (stop signal + join handle) --------------------------

pub(super) struct Spinner {
    inner: Option<Active>,
}

struct Active {
    signal: SpinnerStopSignal,
    handle: Option<JoinHandle<()>>,
}

/// Lightweight stop handle that can be moved into a streaming closure without
/// carrying a mutable borrow of the parent `Spinner`. Calling `trigger()`
/// both sets the stop flag and wakes the render thread immediately via the
/// shared `Condvar`, so the spinner does not wait up to one full tick before
/// exiting.
#[derive(Clone)]
pub(super) struct SpinnerStopSignal {
    stop: Arc<AtomicBool>,
    wake: Arc<(Mutex<()>, Condvar)>,
}

impl Spinner {
    /// Start a spinner with the given label. Returns a no-op handle when the
    /// environment is disabled (non-TTY stderr, `ANVIL_NO_SPINNER`, or thread
    /// spawn failure). The label is sanitized before being rendered, so
    /// adversarial tool / model names cannot inject escapes.
    pub(super) fn start(label: impl Into<String>) -> Self {
        let e = env();
        if !e.enabled {
            return Self { inner: None };
        }
        Self::spawn(e, sanitize(&label.into()))
    }

    fn spawn(env: &'static Env, label: String) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let wake = Arc::new((Mutex::new(()), Condvar::new()));
        let signal = SpinnerStopSignal {
            stop: stop.clone(),
            wake: wake.clone(),
        };
        let stop_for_thread = stop.clone();
        let wake_for_thread = wake.clone();
        let start_at = Instant::now();

        let spawn_result = thread::Builder::new()
            .name("anvil-spinner".into())
            .spawn(move || {
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    render_loop(
                        start_at,
                        label,
                        env,
                        stop_for_thread.clone(),
                        wake_for_thread,
                    );
                }));
                stop_for_thread.store(true, Ordering::SeqCst);
            });

        match spawn_result {
            Ok(handle) => Self {
                inner: Some(Active {
                    signal,
                    handle: Some(handle),
                }),
            },
            Err(err) => {
                // Intentionally log only fixed text + the error object to avoid
                // PII / secret leakage (label / model / prompt are NOT logged).
                tracing::warn!(
                    ?err,
                    "spinner thread spawn failed; continuing without spinner"
                );
                Self { inner: None }
            }
        }
    }

    /// Returns a movable stop handle for the streaming `on_chunk` closure.
    /// `None` when no render thread is running (no-op handle).
    pub(super) fn stop_signal(&self) -> Option<SpinnerStopSignal> {
        self.inner.as_ref().map(|a| a.signal.clone())
    }

    /// Explicit stop. Called directly before progress lines / REPL prompts
    /// where the spinner must be silent. Idempotent: subsequent calls are
    /// no-ops.
    pub(super) fn stop(&mut self) {
        let Some(mut active) = self.inner.take() else {
            return;
        };
        active.signal.trigger();
        if let Some(handle) = active.handle.take()
            && let Err(e) = handle.join()
        {
            // Do not panic from Drop-triggered stops. Log only fixed text.
            tracing::warn!(?e, "spinner thread join failed");
        }
        clear_line();
    }
}

impl SpinnerStopSignal {
    /// Stop the render thread now: set the stop flag, wake the thread via
    /// `Condvar::notify_all()` so there is no up-to-one-tick stop latency, and
    /// synchronously erase the last rendered spinner line from stderr so the
    /// caller can immediately print `assistant>` or any other confirmed line
    /// without visual residue. Idempotent: repeated triggers are harmless
    /// (`clear_line` is a single write of `\r\x1b[2K`).
    pub(super) fn trigger(&self) {
        self.stop.store(true, Ordering::SeqCst);
        let (lock, cvar) = &*self.wake;
        {
            let _g = lock.lock().unwrap();
            cvar.notify_all();
        }
        clear_line();
    }
}

impl Drop for Spinner {
    fn drop(&mut self) {
        self.stop();
    }
}

// --- Internal helpers ------------------------------------------------------

fn clear_line() {
    let mut err = io::stderr().lock();
    let _ = err.write_all(b"\r\x1b[2K");
    let _ = err.flush();
}

fn render_loop(
    start: Instant,
    label: String,
    env: &Env,
    stop: Arc<AtomicBool>,
    wake: Arc<(Mutex<()>, Condvar)>,
) {
    let frames: &[&str] = if env.use_utf8 {
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
        let secs = start.elapsed().as_secs();
        let frame = frames[i % frames.len()];
        let line = format!("\r\x1b[2K{color_on}{frame}{color_off} {label} {secs}s");
        {
            let mut err = io::stderr().lock();
            if err.write_all(line.as_bytes()).is_err() {
                // Broken pipe or similar: self-terminate without further IO.
                stop.store(true, Ordering::SeqCst);
                return;
            }
            let _ = err.flush();
        }
        i = i.wrapping_add(1);
        let g = lock.lock().unwrap();
        let (_g, _) = cvar
            .wait_timeout(g, Duration::from_millis(TICK_MS))
            .unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- Env::detect_with branches ------------------------------------------

    fn empty_env(_k: &str) -> Option<String> {
        None
    }

    #[test]
    fn test_detect_with_disabled_by_anvil_no_spinner() {
        let e = Env::detect_with(
            |k| match k {
                "ANVIL_NO_SPINNER" => Some("1".to_string()),
                _ => None,
            },
            true, // TTY but still disabled by env
        );
        assert!(!e.enabled);
        assert!(!e.use_color);
        assert!(!e.use_utf8);
    }

    #[test]
    fn test_detect_with_anvil_no_spinner_empty_value_does_not_disable() {
        // Empty value must NOT disable (matches `is_some_and(|v| !v.is_empty())`).
        let e = Env::detect_with(
            |k| match k {
                "ANVIL_NO_SPINNER" => Some(String::new()),
                _ => None,
            },
            true,
        );
        assert!(e.enabled);
    }

    #[test]
    fn test_detect_with_disabled_by_non_tty() {
        let e = Env::detect_with(empty_env, false);
        assert!(!e.enabled);
    }

    #[test]
    fn test_detect_with_no_color() {
        let e = Env::detect_with(
            |k| match k {
                "NO_COLOR" => Some("1".to_string()),
                _ => None,
            },
            true,
        );
        assert!(e.enabled);
        assert!(!e.use_color);
    }

    #[test]
    fn test_detect_with_no_color_empty_value_does_not_disable_color() {
        // Per POSIX: `NO_COLOR` disables color only when set to a non-empty
        // value. Empty string must be treated as "unset".
        let e = Env::detect_with(
            |k| match k {
                "NO_COLOR" => Some(String::new()),
                _ => None,
            },
            true,
        );
        assert!(e.enabled);
        assert!(e.use_color, "empty NO_COLOR must NOT disable color");
    }

    #[test]
    fn test_detect_with_utf8_locale() {
        let e = Env::detect_with(
            |k| match k {
                "LANG" => Some("en_US.UTF-8".to_string()),
                _ => None,
            },
            true,
        );
        assert!(e.use_utf8);
    }

    #[test]
    fn test_detect_with_c_locale() {
        let e = Env::detect_with(
            |k| match k {
                "LANG" => Some("C".to_string()),
                _ => None,
            },
            true,
        );
        assert!(!e.use_utf8);
    }

    #[test]
    fn test_detect_with_lc_all_utf8_when_lang_missing() {
        let e = Env::detect_with(
            |k| match k {
                "LC_ALL" => Some("ja_JP.UTF8".to_string()),
                _ => None,
            },
            true,
        );
        assert!(e.use_utf8);
    }

    #[test]
    fn test_detect_with_lc_all_takes_precedence_over_lang() {
        // POSIX: LC_ALL overrides LANG. LC_ALL=C must mask LANG=*.UTF-8.
        let e = Env::detect_with(
            |k| match k {
                "LANG" => Some("en_US.UTF-8".to_string()),
                "LC_ALL" => Some("C".to_string()),
                _ => None,
            },
            true,
        );
        assert!(
            !e.use_utf8,
            "LC_ALL=C must mask LANG=en_US.UTF-8 per POSIX precedence"
        );
    }

    #[test]
    fn test_detect_with_lc_all_utf8_wins_over_lang_c() {
        // Inverse case: LC_ALL=UTF-8 must win even when LANG=C.
        let e = Env::detect_with(
            |k| match k {
                "LANG" => Some("C".to_string()),
                "LC_ALL" => Some("en_US.UTF-8".to_string()),
                _ => None,
            },
            true,
        );
        assert!(
            e.use_utf8,
            "LC_ALL=en_US.UTF-8 must win over LANG=C per POSIX precedence"
        );
    }

    // -- sanitize branches --------------------------------------------------

    #[test]
    fn test_sanitize_c0_control() {
        // NUL, bell, newline, tab all below 0x20.
        assert_eq!(sanitize("a\x00b"), "a?b");
        assert_eq!(sanitize("a\nb"), "a?b");
        assert_eq!(sanitize("a\tb"), "a?b");
    }

    #[test]
    fn test_sanitize_esc() {
        // ESC (0x1B) falls under C0 range; check explicitly.
        assert_eq!(sanitize("x\x1b[31mred"), "x?[31mred");
    }

    #[test]
    fn test_sanitize_del() {
        assert_eq!(sanitize("x\x7fy"), "x?y");
    }

    #[test]
    fn test_sanitize_bidi_override() {
        // LRM, RLM, LRE..RLO, LRI..PDI should all be replaced.
        for &cp in &[
            0x200Eu32, 0x200F, 0x202A, 0x202B, 0x202C, 0x202D, 0x202E, 0x2066, 0x2067, 0x2068,
            0x2069,
        ] {
            let c = char::from_u32(cp).unwrap();
            let input = format!("a{c}b");
            assert_eq!(sanitize(&input), "a?b", "bidi cp U+{cp:04X} not sanitized");
        }
    }

    #[test]
    fn test_sanitize_passthrough_normal_chars() {
        assert_eq!(sanitize("hello world 日本語"), "hello world 日本語");
    }

    // -- Spinner lifecycle (relies on CI non-TTY) ---------------------------

    // NOTE: `Spinner::start("x")` consults `env()` which is backed by a
    // process-wide `OnceLock<Env>`. In CI / test harness runs stderr is
    // non-TTY, so `env().enabled == false` and `Spinner::start` takes the
    // early-return branch BEFORE `thread::Builder::spawn` is reached (see
    // `Spinner::start` impl above). Therefore `stop_signal().is_none()` is
    // equivalent to "no worker thread was spawned" in this environment.
    #[test]
    fn test_spinner_start_returns_noop_when_env_disabled() {
        let sp = Spinner::start("x");
        assert!(
            sp.stop_signal().is_none(),
            "CI runs under non-TTY stderr; Spinner::start must take the early-return branch"
        );
    }

    #[test]
    fn test_spinner_drop_is_idempotent_for_noop() {
        // Explicit no-op handle: dropping must not panic (idempotent stop).
        let sp = Spinner { inner: None };
        drop(sp); // must not panic
    }

    #[test]
    fn test_spinner_stop_twice_is_noop() {
        let mut sp = Spinner { inner: None };
        sp.stop();
        sp.stop(); // second call must not panic
    }

    #[test]
    fn test_stop_signal_none_when_inner_is_none() {
        let sp = Spinner { inner: None };
        assert!(sp.stop_signal().is_none());
    }
}
