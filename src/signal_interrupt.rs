use std::io::Write;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use serde_json::{Value, json};

use crate::logging::log_llm_event;

const TERMINAL_RUNNING: u8 = 0;
const TERMINAL_INTERRUPTED: u8 = 1;
const TERMINAL_COMPLETED: u8 = 2;
const TERMINAL_FAILED: u8 = 3;
const WATCH_INTERVAL: Duration = Duration::from_millis(50);

static SIGINT_COUNT: AtomicUsize = AtomicUsize::new(0);
static SIGINT_HANDLER_INSTALLED: AtomicBool = AtomicBool::new(false);
static DIRECT_COMMAND_FINALIZED: AtomicBool = AtomicBool::new(false);

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

pub(crate) struct DirectCommandTerminalGuard {
    command: &'static str,
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
                    if second_sigint_requested() && !DIRECT_COMMAND_FINALIZED.load(Ordering::SeqCst)
                    {
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

impl DirectCommandTerminalGuard {
    pub(crate) fn start(command: &'static str) -> Self {
        DIRECT_COMMAND_FINALIZED.store(false, Ordering::SeqCst);
        log_llm_event(
            "tui_command_start",
            direct_tui_command_start_payload(command),
        );
        emit_status_line("running");

        let state = Arc::new(AtomicU8::new(TERMINAL_RUNNING));
        let stop_watcher = Arc::new(AtomicBool::new(false));
        let state_for_thread = state.clone();
        let stop_for_thread = stop_watcher.clone();
        let watcher = thread::Builder::new()
            .name("anvil-direct-command-finalizer".to_string())
            .spawn(move || {
                while !stop_for_thread.load(Ordering::SeqCst) {
                    if interrupted() {
                        finalize_direct_command_status(
                            &state_for_thread,
                            command,
                            "interrupted",
                            Some("sigint"),
                            None,
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
            command,
            state,
            stop_watcher,
            watcher,
        }
    }

    pub(crate) fn finish_result<T>(mut self, result: Result<T, String>) -> Result<T, String> {
        match &result {
            Ok(_) => {
                finalize_direct_command_status(
                    &self.state,
                    self.command,
                    "completed",
                    None,
                    None,
                    |event, payload| {
                        log_llm_event(event, payload);
                    },
                );
            }
            Err(err) if interrupted() => {
                finalize_direct_command_status(
                    &self.state,
                    self.command,
                    "interrupted",
                    Some("sigint"),
                    Some(err),
                    |event, payload| {
                        log_llm_event(event, payload);
                    },
                );
            }
            Err(err) => {
                let (status, reason) = direct_command_failure_status(err);
                finalize_direct_command_status(
                    &self.state,
                    self.command,
                    status,
                    Some(reason),
                    Some(err),
                    |event, payload| {
                        log_llm_event(event, payload);
                    },
                );
            }
        }
        self.stop_watcher.store(true, Ordering::SeqCst);
        if let Some(watcher) = self.watcher.take() {
            let _ = watcher.join();
        }
        result
    }
}

impl Drop for DirectCommandTerminalGuard {
    fn drop(&mut self) {
        self.stop_watcher.store(true, Ordering::SeqCst);
        if let Some(watcher) = self.watcher.take() {
            let _ = watcher.join();
        }
        if self.state.load(Ordering::SeqCst) == TERMINAL_RUNNING {
            let status = if interrupted() {
                "interrupted"
            } else {
                "failed"
            };
            let reason = if interrupted() {
                Some("sigint")
            } else {
                Some("guard_drop")
            };
            finalize_direct_command_status(
                &self.state,
                self.command,
                status,
                reason,
                None,
                |event, payload| {
                    log_llm_event(event, payload);
                },
            );
        }
    }
}

impl Drop for TerminalInterruptGuard {
    fn drop(&mut self) {
        self.stop_watcher.store(true, Ordering::SeqCst);
        if let Some(watcher) = self.watcher.take() {
            let _ = watcher.join();
        }
        if interrupted() && !DIRECT_COMMAND_FINALIZED.load(Ordering::SeqCst) {
            finalize_interrupted_status(&self.state, "exit", |event, payload| {
                log_llm_event(event, payload);
            });
        }
    }
}

pub(crate) fn run_direct_cli_command<T>(
    command: &'static str,
    run: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    let guard = DirectCommandTerminalGuard::start(command);
    guard.finish_result(run())
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

fn finalize_direct_command_status(
    state: &AtomicU8,
    command: &'static str,
    status: &'static str,
    reason: Option<&str>,
    error: Option<&str>,
    mut emit: impl FnMut(&'static str, Value),
) -> bool {
    let final_state = direct_command_state_for_status(status);
    if state
        .compare_exchange(
            TERMINAL_RUNNING,
            final_state,
            Ordering::SeqCst,
            Ordering::SeqCst,
        )
        .is_err()
    {
        return false;
    }
    emit(
        "tui_command_stop",
        direct_tui_command_stop_payload(command, status, reason, error),
    );
    DIRECT_COMMAND_FINALIZED.store(true, Ordering::SeqCst);
    emit_status_line(status);
    true
}

fn direct_command_state_for_status(status: &str) -> u8 {
    match status {
        "completed" => TERMINAL_COMPLETED,
        "interrupted" => TERMINAL_INTERRUPTED,
        _ => TERMINAL_FAILED,
    }
}

fn direct_command_failure_status(error: &str) -> (&'static str, &'static str) {
    let lower = error.to_ascii_lowercase();
    if lower.contains("phase_step_planner_timeout") {
        ("phase_step_planner_timeout", "phase_step_planner_timeout")
    } else if lower.contains("planner_ultra_timeout") {
        ("planner_ultra_timeout", "planner_ultra_timeout")
    } else if lower.contains("provider_turn_timeout") {
        ("provider_turn_timeout", "provider_turn_timeout")
    } else {
        ("failed", "error")
    }
}

fn direct_tui_command_start_payload(command: &'static str) -> Value {
    json!({
        "command": command,
        "source": "direct_cli",
        "status": "running",
    })
}

fn direct_tui_command_stop_payload(
    command: &'static str,
    status: &'static str,
    reason: Option<&str>,
    error: Option<&str>,
) -> Value {
    json!({
        "command": command,
        "source": "direct_cli",
        "status": status,
        "reason": reason,
        "error": error,
    })
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

    #[test]
    fn direct_cli_finalizer_marks_interrupted_for_ultra_plan_run() {
        let state = AtomicU8::new(TERMINAL_RUNNING);
        let mut emitted = Vec::<(&'static str, Value)>::new();

        assert!(finalize_direct_command_status(
            &state,
            "ultra_plan_run",
            "interrupted",
            Some("sigint"),
            None,
            |event, payload| {
                emitted.push((event, payload));
            },
        ));

        assert_eq!(state.load(Ordering::SeqCst), TERMINAL_INTERRUPTED);
        assert_eq!(emitted.len(), 1);
        assert_eq!(emitted[0].0, "tui_command_stop");
        assert_eq!(emitted[0].1["source"], "direct_cli");
        assert_eq!(emitted[0].1["command"], "ultra_plan_run");
        assert_eq!(emitted[0].1["status"], "interrupted");
        assert_eq!(emitted[0].1["reason"], "sigint");
    }

    #[test]
    fn direct_cli_failure_status_preserves_planner_timeout_kind() {
        assert_eq!(
            direct_command_failure_status("phase_step_planner_timeout: provider call timed out"),
            ("phase_step_planner_timeout", "phase_step_planner_timeout")
        );
        assert_eq!(
            direct_command_failure_status("planner_ultra_timeout: provider call timed out"),
            ("planner_ultra_timeout", "planner_ultra_timeout")
        );
        assert_eq!(
            direct_command_failure_status("provider_turn_timeout: provider call timed out"),
            ("provider_turn_timeout", "provider_turn_timeout")
        );
        assert_eq!(direct_command_failure_status("other"), ("failed", "error"));
    }
}
