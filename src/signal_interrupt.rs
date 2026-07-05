use std::io::Write;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use serde_json::{Value, json};

use crate::logging::log_llm_event;

const TERMINAL_RUNNING: u8 = 0;
const TERMINAL_INTERRUPTED: u8 = 1;
const WATCH_INTERVAL: Duration = Duration::from_millis(50);

static SIGINT_COUNT: AtomicUsize = AtomicUsize::new(0);
static SIGINT_HANDLER_INSTALLED: AtomicBool = AtomicBool::new(false);

extern "C" fn handle_sigint(_signal: libc::c_int) {
    SIGINT_COUNT.fetch_add(1, Ordering::SeqCst);
}

pub(crate) fn install_sigint_handler() -> Result<(), String> {
    if SIGINT_HANDLER_INSTALLED.swap(true, Ordering::SeqCst) {
        return Ok(());
    }
    install_sigint_handler_inner().inspect_err(|_| {
        SIGINT_HANDLER_INSTALLED.store(false, Ordering::SeqCst);
    })
}

#[cfg(unix)]
fn install_sigint_handler_inner() -> Result<(), String> {
    unsafe {
        let mut action: libc::sigaction = std::mem::zeroed();
        action.sa_sigaction = handle_sigint as *const () as usize;
        action.sa_flags = 0;
        libc::sigemptyset(&mut action.sa_mask);
        if libc::sigaction(libc::SIGINT, &action, std::ptr::null_mut()) != 0 {
            return Err(format!(
                "failed to install SIGINT handler: {}",
                std::io::Error::last_os_error()
            ));
        }
    }
    Ok(())
}

#[cfg(not(unix))]
fn install_sigint_handler_inner() -> Result<(), String> {
    Ok(())
}

pub(crate) fn interrupted() -> bool {
    SIGINT_COUNT.load(Ordering::SeqCst) > 0
}

fn second_sigint_requested() -> bool {
    SIGINT_COUNT.load(Ordering::SeqCst) >= 2
}

pub(crate) struct TerminalInterruptGuard {
    state: Arc<AtomicU8>,
    stop_watcher: Arc<AtomicBool>,
    watcher: Option<JoinHandle<()>>,
}

impl TerminalInterruptGuard {
    pub(crate) fn start() -> Self {
        let state = Arc::new(AtomicU8::new(TERMINAL_RUNNING));
        let stop_watcher = Arc::new(AtomicBool::new(false));
        let state_for_thread = state.clone();
        let stop_for_thread = stop_watcher.clone();
        let watcher = thread::Builder::new()
            .name("anvil-sigint-finalizer".to_string())
            .spawn(move || {
                while !stop_for_thread.load(Ordering::SeqCst) {
                    if second_sigint_requested() {
                        finalize_interrupted_status(
                            &state_for_thread,
                            "second_sigint",
                            |event, payload| {
                                log_llm_event(event, payload);
                            },
                        );
                        break;
                    }
                    thread::sleep(WATCH_INTERVAL);
                }
            })
            .ok();
        Self {
            state,
            stop_watcher,
            watcher,
        }
    }
}

impl Drop for TerminalInterruptGuard {
    fn drop(&mut self) {
        self.stop_watcher.store(true, Ordering::SeqCst);
        if let Some(watcher) = self.watcher.take() {
            let _ = watcher.join();
        }
        if interrupted() {
            finalize_interrupted_status(&self.state, "exit", |event, payload| {
                log_llm_event(event, payload);
            });
        }
    }
}

fn finalize_interrupted_status(
    state: &AtomicU8,
    reason: &'static str,
    mut emit: impl FnMut(&'static str, Value),
) -> bool {
    if state
        .compare_exchange(
            TERMINAL_RUNNING,
            TERMINAL_INTERRUPTED,
            Ordering::SeqCst,
            Ordering::SeqCst,
        )
        .is_err()
    {
        return false;
    }
    emit(
        "tui_command_stop",
        tui_command_stop_payload("interrupted", reason),
    );
    emit_status_line("interrupted");
    true
}

fn tui_command_stop_payload(status: &'static str, reason: &'static str) -> Value {
    json!({
        "status": status,
        "reason": reason,
    })
}

fn emit_status_line(status: &str) {
    let mut stderr = std::io::stderr().lock();
    let _ = writeln!(stderr, "Status: {status}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finalizer_marks_interrupted_and_emits_stop_once() {
        let state = AtomicU8::new(TERMINAL_RUNNING);
        let mut emitted = Vec::<(&'static str, Value)>::new();

        assert!(finalize_interrupted_status(
            &state,
            "unit_test",
            |event, payload| {
                emitted.push((event, payload));
            }
        ));
        assert_eq!(state.load(Ordering::SeqCst), TERMINAL_INTERRUPTED);
        assert_eq!(emitted.len(), 1);
        assert_eq!(emitted[0].0, "tui_command_stop");
        assert_eq!(emitted[0].1["status"], "interrupted");
        assert_eq!(emitted[0].1["reason"], "unit_test");

        assert!(!finalize_interrupted_status(
            &state,
            "unit_test",
            |event, payload| {
                emitted.push((event, payload));
            }
        ));
        assert_eq!(emitted.len(), 1);
        assert_ne!(state.load(Ordering::SeqCst), TERMINAL_RUNNING);
    }
}
