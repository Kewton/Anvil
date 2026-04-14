//! Keyboard watcher for ESC-driven interrupt (Issue #379 Phase 1).
//!
//! While `run_live_turn` is executing, this module enables raw mode on
//! stderr, spawns a short-lived watcher thread, and flips the shared
//! stop flag as soon as the user presses ESC. All other key events
//! are discarded in-place per DR4-003 (no logging, no persistence, no
//! telemetry).
//!
//! Design references:
//! - `dev-reports/design/issue-379-tui-improvements-design-policy.md` D1 / P1 / DR4-003
//! - `dev-reports/issue/379/work-plan.md` Task 1.1 / Task 1.2 / Task 1.3

use std::borrow::Cow;
use std::io::{self, Write as _};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Once};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEvent};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};

/// Poll granularity for ESC detection.
///
/// 50ms keeps ESC feel responsive while bounding CPU to a handful of
/// wake-ups per second per active turn.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Tracks whether raw mode is currently enabled globally.
///
/// Accessed by the panic hook to decide whether to call `disable_raw_mode`
/// during unwinding (R1 safety invariant). Using `AtomicBool` keeps the
/// hook lock-free and re-entrant.
static RAW_MODE_ACTIVE: AtomicBool = AtomicBool::new(false);
static PANIC_HOOK_INSTALLED: Once = Once::new();

/// Install a `std::panic::set_hook` that disables raw mode on the way out.
///
/// Idempotent — safe to call from every `KeyboardWatcher::spawn` invocation.
/// Must also be safe to call in tests on TTY-less CI where raw mode is never
/// actually engaged (the `RAW_MODE_ACTIVE` guard short-circuits the call).
pub fn install_raw_mode_panic_hook() {
    PANIC_HOOK_INSTALLED.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if RAW_MODE_ACTIVE.swap(false, Ordering::AcqRel) {
                let _ = disable_raw_mode();
            }
            previous(info);
        }));
    });
}

/// Normalize line endings while raw mode is active.
///
/// In raw mode the terminal no longer expands lone `\n` to `\r\n`, so any
/// interactive output that prints plain line feeds drifts to the right instead
/// of returning to column zero on the next line.
pub fn normalize_output_for_raw_mode(text: &str) -> Cow<'_, str> {
    if !RAW_MODE_ACTIVE.load(Ordering::Acquire) || !text.contains('\n') {
        return Cow::Borrowed(text);
    }

    let mut normalized = String::with_capacity(text.len() + text.matches('\n').count());
    let mut previous_was_cr = false;
    for ch in text.chars() {
        match ch {
            '\n' => {
                if !previous_was_cr {
                    normalized.push('\r');
                }
                normalized.push('\n');
                previous_was_cr = false;
            }
            '\r' => {
                normalized.push('\r');
                previous_was_cr = true;
            }
            other => {
                normalized.push(other);
                previous_was_cr = false;
            }
        }
    }

    Cow::Owned(normalized)
}

/// Write interactive text to stderr, normalizing line endings in raw mode.
pub fn write_stderr(text: &str) {
    let mut stderr = io::stderr();
    let normalized = normalize_output_for_raw_mode(text);
    let _ = stderr.write_all(normalized.as_bytes());
}

/// Write a full line to stderr, normalizing line endings in raw mode.
pub fn writeln_stderr(text: &str) {
    write_stderr(text);
    write_stderr("\n");
}

/// Flush stderr after incremental interactive output.
pub fn flush_stderr() {
    let _ = io::stderr().flush();
}

/// RAII guard that enables raw mode on construction and disables it on
/// drop, no matter how the turn terminates (normal return, `?` bubble,
/// panic unwind — when coupled with `install_raw_mode_panic_hook`).
pub struct RawModeGuard {
    active: bool,
}

impl RawModeGuard {
    /// Attempt to enable raw mode. Returns `Ok(guard)` on success.
    ///
    /// On TTY-less environments (CI, piped I/O) `enable_raw_mode` fails
    /// and this function returns the underlying `io::Error`. Callers
    /// should fall back to `RawModeGuard::inactive()` so downstream code
    /// still sees a usable (no-op) guard without taking over the terminal.
    pub fn enable() -> io::Result<Self> {
        enable_raw_mode()?;
        RAW_MODE_ACTIVE.store(true, Ordering::Release);
        Ok(Self { active: true })
    }

    /// Construct a no-op guard. Safe in TTY-less contexts and tests.
    pub fn inactive() -> Self {
        Self { active: false }
    }

    /// Whether this guard currently owns raw mode.
    pub fn is_active(&self) -> bool {
        self.active
    }
}

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        if self.active && RAW_MODE_ACTIVE.swap(false, Ordering::AcqRel) {
            // Ignore errors: if the terminal has already restored itself
            // (e.g. SIGTERM closed stderr) there is nothing we can do.
            let _ = disable_raw_mode();
            self.active = false;
        }
    }
}

/// Background thread that polls for ESC key presses and flips the shared
/// shutdown flag on match.
///
/// Lifetime: `spawn` → user presses ESC (or `stop()` is called for the
/// normal-completion path) → thread exits and raw mode is released via
/// `RawModeGuard::Drop`.
pub struct KeyboardWatcher {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl KeyboardWatcher {
    /// Spawn a watcher if `enabled=true` and raw mode can be acquired.
    ///
    /// Returns `None` when:
    /// - `enabled == false` (non-interactive turn / caller policy opt-out).
    /// - `enable_raw_mode` fails (TTY-less CI).
    ///
    /// Callers MUST treat `None` as a successful no-op (Task 1.3 / DR1-006).
    pub fn spawn(shutdown_flag: Arc<AtomicBool>, enabled: bool) -> Option<Self> {
        if !enabled {
            return None;
        }

        install_raw_mode_panic_hook();

        let guard = match RawModeGuard::enable() {
            Ok(guard) => guard,
            Err(err) => {
                tracing::debug!(
                    error = %err,
                    "raw mode unavailable; ESC watcher disabled for this turn"
                );
                return None;
            }
        };

        let stop = Arc::new(AtomicBool::new(false));
        let stop_for_thread = Arc::clone(&stop);
        let handle = thread::Builder::new()
            .name("anvil-esc-watcher".to_string())
            .spawn(move || watch_esc(guard, shutdown_flag, stop_for_thread))
            .ok()?;
        Some(Self {
            stop,
            handle: Some(handle),
        })
    }

    /// Signal the watcher to stop and join its thread.
    ///
    /// Idempotent: calling this after a natural exit (ESC press) simply
    /// joins the already-finished thread. Always releases raw mode via the
    /// watcher thread's `RawModeGuard::Drop`.
    pub fn stop(mut self) {
        self.signal_and_join();
    }

    /// Shared shutdown implementation for `stop()` and `Drop`.
    ///
    /// Sets the stop flag and joins the thread handle if it has not already
    /// been consumed. Errors from `join` are intentionally swallowed — the
    /// watcher thread performs only in-memory work and any panic it raised
    /// would already have surfaced via the global panic hook.
    fn signal_and_join(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for KeyboardWatcher {
    fn drop(&mut self) {
        // Defensive path: if a caller forgets to call `stop()` explicitly,
        // ensure we still signal the thread and join it before the owning
        // scope ends. This keeps raw-mode release deterministic (R1).
        self.signal_and_join();
    }
}

/// The actual polling loop, split out for readability.
///
/// DR4-003: every key other than ESC is dropped on the floor immediately
/// — no persistence, no logging, no forwarding.
fn watch_esc(guard: RawModeGuard, shutdown_flag: Arc<AtomicBool>, stop: Arc<AtomicBool>) {
    while !stop.load(Ordering::Acquire) && !shutdown_flag.load(Ordering::Acquire) {
        match event::poll(POLL_INTERVAL) {
            Ok(true) => match event::read() {
                Ok(Event::Key(KeyEvent { code, .. })) => {
                    if matches!(code, KeyCode::Esc) {
                        shutdown_flag.store(true, Ordering::Release);
                        break;
                    }
                    // Any non-ESC key is discarded in place per DR4-003.
                }
                Ok(_) | Err(_) => {
                    // Non-key events (Resize, Mouse, …) and read errors
                    // are ignored — they must not influence control flow.
                }
            },
            Ok(false) => {
                // Timeout: loop back and re-check both flags.
            }
            Err(err) => {
                tracing::debug!(error = %err, "event::poll failed; stopping ESC watcher");
                break;
            }
        }
    }
    drop(guard);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spawn_is_none_when_disabled() {
        let flag = Arc::new(AtomicBool::new(false));
        let handle = KeyboardWatcher::spawn(Arc::clone(&flag), false);
        assert!(handle.is_none());
        assert!(!flag.load(Ordering::Acquire));
    }

    #[test]
    fn normalize_output_leaves_lf_when_raw_mode_inactive() {
        RAW_MODE_ACTIVE.store(false, Ordering::Release);
        assert_eq!(normalize_output_for_raw_mode("a\nb"), "a\nb");
    }

    #[test]
    fn normalize_output_converts_lf_to_crlf_when_raw_mode_active() {
        RAW_MODE_ACTIVE.store(true, Ordering::Release);
        assert_eq!(normalize_output_for_raw_mode("a\nb\n"), "a\r\nb\r\n");
        RAW_MODE_ACTIVE.store(false, Ordering::Release);
    }

    #[test]
    fn normalize_output_preserves_existing_crlf_pairs() {
        RAW_MODE_ACTIVE.store(true, Ordering::Release);
        assert_eq!(
            normalize_output_for_raw_mode("a\r\nb\nc\r"),
            "a\r\nb\r\nc\r"
        );
        RAW_MODE_ACTIVE.store(false, Ordering::Release);
    }

    #[test]
    fn inactive_guard_is_safe_to_drop() {
        let g = RawModeGuard::inactive();
        assert!(!g.is_active());
        drop(g);
    }

    #[test]
    fn install_panic_hook_is_idempotent() {
        install_raw_mode_panic_hook();
        install_raw_mode_panic_hook();
    }
}
