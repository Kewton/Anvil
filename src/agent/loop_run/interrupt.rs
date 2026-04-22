//! Terminal-level ESC interrupt monitor for agent runs.
//!
//! Wraps `crossterm::event` polling in a daemon thread so `turn::run_actor_loop`
//! can break safely between tool calls and LLM response boundaries. Design:
//! `dev-reports/design/issue-429-esc-interrupt-design-policy.md` §4.
//!
//! Parallels `src/agent/loop_run/spinner.rs`: same `Arc<AtomicBool>` +
//! `Condvar` + `Drop` RAII + `catch_unwind` patterns. Kept in a separate
//! module because the TTY channel (stdin vs stderr), env var
//! (`ANVIL_NO_INTERRUPT` vs `ANVIL_NO_SPINNER`), and responsibility (key
//! detection vs rendering) are orthogonal — common-traiting would
//! re-introduce the provider abstraction CLAUDE.md forbids.
//!
//! Scope (per S5-003): interrupt takes effect at the next `run_actor_loop`
//! boundary — tool completion or LLM response completion. No mid-flight cancel.

use std::io::{self, IsTerminal};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};

/// Poll timeout in the daemon thread. 100ms keeps CPU near-idle while staying
/// responsive to ESC. Also bounded by `Condvar::notify_all()` from pause/Drop.
const POLL_INTERVAL: Duration = Duration::from_millis(100);

// ---------------------------------------------------------------------------
// InterruptFlag — read-only handle checked at `run_actor_loop` boundaries.
// ---------------------------------------------------------------------------

/// Read-only handle distributed to `turn.rs` boundaries. `Clone` so integration
/// tests can inject a preset flag via `InterruptFlag::new_preset(true)`.
#[derive(Clone)]
pub(super) struct InterruptFlag {
    pub(super) flag: Arc<AtomicBool>,
}

impl InterruptFlag {
    pub(super) fn is_set(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }

    /// Test-only constructor used by `turn.rs` integration tests to drive
    /// `run_actor_loop` without a live daemon thread.
    #[cfg(test)]
    pub(super) fn new_preset(value: bool) -> Self {
        Self {
            flag: Arc::new(AtomicBool::new(value)),
        }
    }
}

// ---------------------------------------------------------------------------
// InterruptEnv — detected once per call to `InterruptMonitor::start`.
// ---------------------------------------------------------------------------

pub(super) struct InterruptEnv {
    pub(super) enabled: bool,
}

impl InterruptEnv {
    /// Real detection path: reads `std::env` and checks whether stdin is a TTY.
    pub(super) fn detect() -> Self {
        Self::detect_with(
            |k| std::env::var_os(k).map(|v| v.to_string_lossy().into_owned()),
            io::stdin().is_terminal(),
        )
    }

    /// DI path mirroring `spinner::Env::detect_with` so unit tests avoid
    /// mutating `std::env`. `ANVIL_NO_INTERRUPT` non-empty disables the monitor
    /// (POSIX `NO_COLOR` convention); non-TTY stdin also disables.
    pub(super) fn detect_with(
        get_env: impl Fn(&str) -> Option<String>,
        stdin_is_tty: bool,
    ) -> Self {
        if get_env("ANVIL_NO_INTERRUPT").is_some_and(|v| !v.is_empty()) {
            return Self { enabled: false };
        }
        Self {
            enabled: stdin_is_tty,
        }
    }
}

// ---------------------------------------------------------------------------
// InterruptMonitor — raw mode + daemon thread + wake Condvar (3-in-1 RAII).
// ---------------------------------------------------------------------------

/// Shared mutable state between the daemon thread and the owning
/// `InterruptMonitor`. `parked` lets `pause()` synchronously wait until the
/// worker has yielded the terminal before the caller starts `stdin.read_line`.
struct MonitorState {
    stop_requested: bool,
    paused: bool,
    parked: bool,
}

struct Active {
    flag: Arc<AtomicBool>,
    state: Arc<(Mutex<MonitorState>, Condvar)>,
    handle: Option<JoinHandle<()>>,
    raw_mode_active: bool,
}

/// RAII guard: on start, enables raw mode and spawns the monitor thread. On
/// drop, wakes the thread, joins it (best-effort), and disables raw mode. A
/// no-op `inner: None` variant is returned when the environment is disabled,
/// so callers do not need to branch on `env.enabled` themselves.
pub(super) struct InterruptMonitor {
    inner: Option<Active>,
    /// Returned from `flag()` when disabled so callers always get a live
    /// `InterruptFlag` whose `is_set()` is permanently false.
    dummy_flag: Arc<AtomicBool>,
}

impl InterruptMonitor {
    pub(super) fn start(env: &InterruptEnv) -> Self {
        let dummy_flag = Arc::new(AtomicBool::new(false));
        if !env.enabled {
            return Self {
                inner: None,
                dummy_flag,
            };
        }
        if let Err(err) = enable_raw_mode() {
            tracing::warn!(
                ?err,
                "interrupt: failed to enable raw mode; disabling monitor"
            );
            return Self {
                inner: None,
                dummy_flag,
            };
        }

        let flag = Arc::new(AtomicBool::new(false));
        let state = Arc::new((
            Mutex::new(MonitorState {
                stop_requested: false,
                paused: false,
                parked: false,
            }),
            Condvar::new(),
        ));
        let flag_for_thread = flag.clone();
        let state_for_thread = state.clone();

        let spawn_result = thread::Builder::new()
            .name("anvil-interrupt".into())
            .spawn(move || {
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    render_loop(flag_for_thread, state_for_thread);
                }));
            });

        match spawn_result {
            Ok(handle) => Self {
                inner: Some(Active {
                    flag,
                    state,
                    handle: Some(handle),
                    raw_mode_active: true,
                }),
                dummy_flag,
            },
            Err(err) => {
                tracing::warn!(?err, "interrupt: failed to spawn monitor thread; disabling");
                let _ = disable_raw_mode();
                Self {
                    inner: None,
                    dummy_flag,
                }
            }
        }
    }

    /// Read-only flag handle for boundary checks in `turn::run_actor_loop`.
    /// Disabled monitors return a handle whose `is_set()` is permanently false.
    pub(super) fn flag(&self) -> InterruptFlag {
        match &self.inner {
            Some(active) => InterruptFlag {
                flag: active.flag.clone(),
            },
            None => InterruptFlag {
                flag: self.dummy_flag.clone(),
            },
        }
    }

    /// Yield the terminal to the caller: release raw mode and wait until the
    /// daemon thread has observed `paused=true` and acked via `parked=true`.
    /// Idempotent, order-independent, and no-op on disabled monitors.
    pub(super) fn pause(&mut self) {
        let Some(active) = self.inner.as_mut() else {
            return;
        };
        if !active.raw_mode_active {
            return;
        }
        let (lock, cvar) = &*active.state;
        {
            let mut st = lock.lock().unwrap();
            st.paused = true;
            cvar.notify_all();
            // Wait for the worker to enter its paused wait-loop and ack.
            // The worker also exits early when `stop_requested` is set, so we
            // bail out in that case to avoid an indefinite wait.
            while !st.parked && !st.stop_requested {
                st = cvar.wait(st).unwrap();
            }
        }
        if let Err(err) = disable_raw_mode() {
            tracing::warn!(?err, "interrupt: failed to disable raw mode on pause");
        }
        active.raw_mode_active = false;
    }

    /// Re-enable raw mode and wake the daemon thread back up. Idempotent.
    pub(super) fn resume(&mut self) {
        let Some(active) = self.inner.as_mut() else {
            return;
        };
        if active.raw_mode_active {
            return;
        }
        if let Err(err) = enable_raw_mode() {
            tracing::warn!(?err, "interrupt: failed to re-enable raw mode on resume");
            return;
        }
        let (lock, cvar) = &*active.state;
        {
            let mut st = lock.lock().unwrap();
            st.paused = false;
            st.parked = false;
            cvar.notify_all();
        }
        active.raw_mode_active = true;
    }
}

impl Drop for InterruptMonitor {
    fn drop(&mut self) {
        let Some(mut active) = self.inner.take() else {
            return;
        };
        // Signal stop and wake any wait-loops.
        {
            let (lock, cvar) = &*active.state;
            let mut st = lock.lock().unwrap();
            st.stop_requested = true;
            st.paused = false;
            cvar.notify_all();
        }
        if let Some(handle) = active.handle.take() {
            // Best-effort join; never panic from Drop.
            if let Err(e) = handle.join() {
                tracing::warn!(?e, "interrupt: monitor thread join failed");
            }
        }
        if active.raw_mode_active
            && let Err(err) = disable_raw_mode()
        {
            tracing::warn!(?err, "interrupt: failed to disable raw mode on drop");
        }
    }
}

// ---------------------------------------------------------------------------
// Worker loop
// ---------------------------------------------------------------------------

fn render_loop(flag: Arc<AtomicBool>, state: Arc<(Mutex<MonitorState>, Condvar)>) {
    let (lock, cvar) = &*state;
    loop {
        // 1. Gate on shared state: check stop / enter park on pause.
        {
            let mut st = lock.lock().unwrap();
            if st.stop_requested {
                break;
            }
            while st.paused {
                st.parked = true;
                cvar.notify_all();
                st = cvar.wait(st).unwrap();
                if st.stop_requested {
                    return;
                }
            }
            st.parked = false;
        }
        // 2. Poll for a key event; always consume with `event::read` to avoid
        //    leaving the same event pending. Non-ESC keys/mouse/resize events
        //    are intentionally discarded (no type-ahead buffer, per R5).
        match event::poll(POLL_INTERVAL) {
            Ok(true) => match event::read() {
                Ok(Event::Key(k)) if k.code == KeyCode::Esc => {
                    flag.store(true, Ordering::SeqCst);
                    break;
                }
                Ok(_) => {}
                Err(_) => break,
            },
            Ok(false) => {}
            Err(_) => break,
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- InterruptEnv::detect_with branches ---------------------------------

    fn empty_env(_k: &str) -> Option<String> {
        None
    }

    #[test]
    fn env_disabled_when_non_tty() {
        let e = InterruptEnv::detect_with(empty_env, false);
        assert!(!e.enabled);
    }

    #[test]
    fn env_enabled_when_tty_and_no_flag() {
        let e = InterruptEnv::detect_with(empty_env, true);
        assert!(e.enabled);
    }

    #[test]
    fn env_disabled_by_anvil_no_interrupt_one() {
        let e = InterruptEnv::detect_with(
            |k| match k {
                "ANVIL_NO_INTERRUPT" => Some("1".to_string()),
                _ => None,
            },
            true,
        );
        assert!(!e.enabled);
    }

    #[test]
    fn env_disabled_by_anvil_no_interrupt_zero() {
        // POSIX NO_COLOR convention: any non-empty value disables.
        let e = InterruptEnv::detect_with(
            |k| match k {
                "ANVIL_NO_INTERRUPT" => Some("0".to_string()),
                _ => None,
            },
            true,
        );
        assert!(!e.enabled);
    }

    #[test]
    fn env_empty_anvil_no_interrupt_does_not_disable() {
        // Empty string must be treated as unset.
        let e = InterruptEnv::detect_with(
            |k| match k {
                "ANVIL_NO_INTERRUPT" => Some(String::new()),
                _ => None,
            },
            true,
        );
        assert!(e.enabled, "empty ANVIL_NO_INTERRUPT must not disable");
    }

    // -- InterruptFlag::new_preset -----------------------------------------

    #[test]
    fn flag_preset_true_reports_set() {
        assert!(InterruptFlag::new_preset(true).is_set());
    }

    #[test]
    fn flag_preset_false_reports_unset() {
        assert!(!InterruptFlag::new_preset(false).is_set());
    }

    // -- Disabled monitor behaviour ----------------------------------------

    #[test]
    fn disabled_monitor_has_none_inner() {
        let env = InterruptEnv { enabled: false };
        let monitor = InterruptMonitor::start(&env);
        assert!(monitor.inner.is_none());
    }

    #[test]
    fn disabled_monitor_flag_is_permanently_false() {
        let env = InterruptEnv { enabled: false };
        let monitor = InterruptMonitor::start(&env);
        assert!(!monitor.flag().is_set());
    }

    #[test]
    fn disabled_monitor_pause_and_resume_are_noops() {
        let env = InterruptEnv { enabled: false };
        let mut monitor = InterruptMonitor::start(&env);
        // Both must complete without touching the terminal — no panic, no hang.
        monitor.pause();
        monitor.resume();
        monitor.pause();
        monitor.pause();
    }

    #[test]
    fn disabled_monitor_drop_is_noop() {
        let env = InterruptEnv { enabled: false };
        let monitor = InterruptMonitor::start(&env);
        drop(monitor); // must not panic
    }

    // NOTE: enabled-monitor lifecycle (start/pause/resume/drop) is not unit
    // tested here because it calls `enable_raw_mode()` on the current process
    // terminal. The CI harness runs under non-TTY stderr/stdout, so
    // `detect().enabled` is already false in tests. End-to-end behaviour is
    // covered by `tests/e2e/interrupt_pty.rs` (ANVIL_E2E=1 gated).
}
