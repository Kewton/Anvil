use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use std::io::Write;

const POLL_INTERVAL: Duration = Duration::from_millis(100);
const INTERRUPT_NOTICE: &str = "interrupt requested; stopping after current operation";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InterruptEnv {
    pub enabled: bool,
}

impl InterruptEnv {
    pub fn detect() -> Self {
        Self::detect_with(
            |key| std::env::var_os(key).map(|value| value.to_string_lossy().into_owned()),
            crate::tui::terminal::stdin_is_tty(),
        )
    }

    pub fn detect_with(get_env: impl Fn(&str) -> Option<String>, stdin_is_tty: bool) -> Self {
        if crate::tui::terminal::env_non_empty_with(get_env, "ANVIL_NO_INTERRUPT") {
            return Self { enabled: false };
        }
        Self {
            enabled: stdin_is_tty,
        }
    }
}

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

pub struct InterruptMonitor {
    inner: Option<Active>,
    dummy_flag: Arc<AtomicBool>,
}

impl InterruptMonitor {
    pub fn start() -> Self {
        Self::start_with_env(InterruptEnv::detect())
    }

    pub fn start_with_env(env: InterruptEnv) -> Self {
        let dummy_flag = Arc::new(AtomicBool::new(false));
        if !env.enabled || enable_raw_mode().is_err() {
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
        let thread_flag = flag.clone();
        let thread_state = state.clone();
        let handle = thread::Builder::new()
            .name("anvilminimal-interrupt".to_string())
            .spawn(move || monitor_loop(thread_flag, thread_state))
            .ok();
        match handle {
            Some(handle) => Self {
                inner: Some(Active {
                    flag,
                    state,
                    handle: Some(handle),
                    raw_mode_active: true,
                }),
                dummy_flag,
            },
            None => {
                let _ = disable_raw_mode();
                Self {
                    inner: None,
                    dummy_flag,
                }
            }
        }
    }

    pub fn new_preset(value: bool) -> Self {
        Self {
            inner: None,
            dummy_flag: Arc::new(AtomicBool::new(value)),
        }
    }

    pub fn interrupted(&self) -> bool {
        match &self.inner {
            Some(active) => active.flag.load(Ordering::SeqCst),
            None => self.dummy_flag.load(Ordering::SeqCst),
        }
    }

    pub fn reset(&self) {
        match &self.inner {
            Some(active) => active.flag.store(false, Ordering::SeqCst),
            None => self.dummy_flag.store(false, Ordering::SeqCst),
        }
    }

    fn pause(&mut self) {
        let Some(active) = self.inner.as_mut() else {
            return;
        };
        if !active.raw_mode_active {
            return;
        }
        let (lock, cvar) = &*active.state;
        if let Ok(mut state) = lock.lock() {
            state.paused = true;
            cvar.notify_all();
            while !state.parked && !state.stop_requested {
                state = cvar.wait(state).unwrap();
            }
        }
        let _ = disable_raw_mode();
        active.raw_mode_active = false;
    }

    fn resume(&mut self) {
        let Some(active) = self.inner.as_mut() else {
            return;
        };
        if active.raw_mode_active || enable_raw_mode().is_err() {
            return;
        }
        let (lock, cvar) = &*active.state;
        if let Ok(mut state) = lock.lock() {
            state.paused = false;
            state.parked = false;
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
        {
            let (lock, cvar) = &*active.state;
            if let Ok(mut state) = lock.lock() {
                state.stop_requested = true;
                state.paused = false;
                cvar.notify_all();
            }
        }
        if let Some(handle) = active.handle.take() {
            let _ = handle.join();
        }
        if active.raw_mode_active {
            let _ = disable_raw_mode();
        }
    }
}

pub struct PauseGuard {
    monitor: Arc<Mutex<InterruptMonitor>>,
}

impl PauseGuard {
    pub fn new(monitor: Arc<Mutex<InterruptMonitor>>) -> Option<Self> {
        if let Ok(mut monitor_guard) = monitor.lock() {
            monitor_guard.pause();
        }
        Some(Self { monitor })
    }
}

impl Drop for PauseGuard {
    fn drop(&mut self) {
        if let Ok(mut monitor) = self.monitor.lock() {
            monitor.resume();
        }
    }
}

fn monitor_loop(flag: Arc<AtomicBool>, state: Arc<(Mutex<MonitorState>, Condvar)>) {
    let (lock, cvar) = &*state;
    loop {
        {
            let mut shared = lock.lock().unwrap();
            if shared.stop_requested {
                break;
            }
            while shared.paused {
                shared.parked = true;
                cvar.notify_all();
                shared = cvar.wait(shared).unwrap();
                if shared.stop_requested {
                    return;
                }
            }
            shared.parked = false;
        }
        match event::poll(POLL_INTERVAL) {
            Ok(true) => match event::read() {
                Ok(Event::Key(key)) if key.code == KeyCode::Esc => {
                    emit_interrupt_feedback();
                    flag.store(true, Ordering::SeqCst);
                }
                Ok(_) => {}
                Err(_) => break,
            },
            Ok(false) => {}
            Err(_) => break,
        }
    }
}

fn emit_interrupt_feedback() {
    let mut err = std::io::stderr().lock();
    let _ = err.write_all(b"\r\x1b[2K\r\n");
    let _ = err.write_all(INTERRUPT_NOTICE.as_bytes());
    let _ = err.write_all(b"\r\n");
    let _ = err.flush();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interrupt_env_disable() {
        let env = InterruptEnv::detect_with(
            |key| (key == "ANVIL_NO_INTERRUPT").then(|| "1".to_string()),
            true,
        );
        assert!(!env.enabled);
    }

    #[test]
    fn interrupt_env_non_tty_noop() {
        assert!(!InterruptEnv::detect_with(|_| None, false).enabled);
    }

    #[test]
    fn interrupt_preset_flag() {
        let monitor = InterruptMonitor::new_preset(true);
        assert!(monitor.interrupted());
        monitor.reset();
        assert!(!monitor.interrupted());
    }
}
