use std::any::Any;
use std::io::Write;
use std::panic::{AssertUnwindSafe, PanicHookInfo};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use serde_json::{Value, json};

use crate::config::Config;

static PANIC_HOOK_LOCK: Mutex<()> = Mutex::new(());
type SharedPanicHook = Arc<dyn Fn(&PanicHookInfo<'_>) + Sync + Send + 'static>;

#[derive(Debug, Clone, PartialEq, Eq)]
struct PanicDiagnostic {
    message: String,
    location: String,
}

#[derive(Debug, Clone)]
struct PanicBoundaryContext {
    workspace_root: PathBuf,
    eval_events_path: Option<PathBuf>,
    action: String,
    profile: String,
    prompt_layout: String,
    reproduction_command: String,
}

impl PanicBoundaryContext {
    fn from_config(config: &Config) -> Self {
        Self {
            workspace_root: config.workspace_root.clone(),
            eval_events_path: config.eval_events_path.clone(),
            action: format!("{:?}", config.action),
            profile: config.profile.clone(),
            prompt_layout: config.prompt_layout.as_str().to_string(),
            reproduction_command: current_reproduction_command(),
        }
    }
}

struct PanicHookGuard {
    previous: Option<SharedPanicHook>,
    _lock: MutexGuard<'static, ()>,
}

impl PanicHookGuard {
    fn install(diagnostics: Arc<Mutex<Vec<PanicDiagnostic>>>) -> Self {
        let lock = PANIC_HOOK_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let previous: SharedPanicHook = Arc::from(std::panic::take_hook());
        let previous_for_other_threads = Arc::clone(&previous);
        let owner = std::thread::current().id();
        std::panic::set_hook(Box::new(move |info| {
            let on_owner_thread = std::thread::current().id() == owner;
            // Provider workers can resume their panic on the CLI thread, so retain a bounded
            // set and later match the caught payload instead of assuming one panic thread.
            let mut diagnostics = diagnostics
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if diagnostics.len() == 16 {
                diagnostics.remove(0);
            }
            diagnostics.push(PanicDiagnostic::from_hook(info));
            drop(diagnostics);
            if !on_owner_thread {
                previous_for_other_threads(info);
            }
        }));
        Self {
            previous: Some(previous),
            _lock: lock,
        }
    }
}

impl Drop for PanicHookGuard {
    fn drop(&mut self) {
        let _ = std::panic::take_hook();
        if let Some(previous) = self.previous.take() {
            std::panic::set_hook(Box::new(move |info| previous(info)));
        }
    }
}

impl PanicDiagnostic {
    fn from_hook(info: &PanicHookInfo<'_>) -> Self {
        let location = info
            .location()
            .map(|location| {
                format!(
                    "{}:{}:{}",
                    location.file(),
                    location.line(),
                    location.column()
                )
            })
            .unwrap_or_else(|| "unknown".to_string());
        Self {
            message: panic_payload_message(info.payload()),
            location,
        }
    }

    fn from_payload(payload: &(dyn Any + Send)) -> Self {
        Self {
            message: panic_payload_message(payload),
            location: "unknown".to_string(),
        }
    }
}

#[derive(Debug, Default)]
struct RecoveryNoteOutcome {
    relative_path: String,
    error: String,
}

pub(crate) fn catch_cli_run<F>(config: &Config, operation: F) -> anyhow::Result<()>
where
    F: FnOnce() -> anyhow::Result<()>,
{
    catch_with_context(PanicBoundaryContext::from_config(config), operation)
}

fn catch_with_context<F>(context: PanicBoundaryContext, operation: F) -> anyhow::Result<()>
where
    F: FnOnce() -> anyhow::Result<()>,
{
    let captured = Arc::new(Mutex::new(Vec::new()));
    let hook_guard = PanicHookGuard::install(Arc::clone(&captured));
    let outcome = std::panic::catch_unwind(AssertUnwindSafe(operation));
    if let Err(payload) = std::panic::catch_unwind(AssertUnwindSafe(|| drop(hook_guard))) {
        stderr_line(&format!(
            "internal panic boundary: panic while restoring panic hook: {}",
            panic_payload_message(payload.as_ref())
        ));
    }

    match outcome {
        Ok(result) => result,
        Err(payload) => {
            let payload_message = panic_payload_message(payload.as_ref());
            let diagnostic = captured
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .iter()
                .rev()
                .find(|diagnostic| diagnostic.message == payload_message)
                .cloned()
                .or_else(|| nested_panic_diagnostic(context.eval_events_path.as_deref()))
                .unwrap_or_else(|| PanicDiagnostic::from_payload(payload.as_ref()));
            let recovery = finalize_internal_panic(&context, &diagnostic);
            let recovery_suffix = if recovery.relative_path.is_empty() {
                String::new()
            } else {
                format!("; recovery note: {}", recovery.relative_path)
            };
            Err(anyhow::anyhow!(
                "internal_panic: {} at {}{}",
                diagnostic.message,
                diagnostic.location,
                recovery_suffix
            ))
        }
    }
}

fn finalize_internal_panic(
    context: &PanicBoundaryContext,
    diagnostic: &PanicDiagnostic,
) -> RecoveryNoteOutcome {
    stderr_line(&format!(
        "internal panic caught: {} at {}",
        diagnostic.message, diagnostic.location
    ));

    let recovery = save_recovery_note_safely(context, diagnostic);
    run_finalizer_step("reaping registered server children", || {
        crate::bounded_process::reap_registered_server_children_for_workspace(
            context.eval_events_path.as_deref(),
            "internal_panic",
            &context.workspace_root,
        );
        Ok(())
    });
    run_finalizer_step("writing internal panic terminal event", || {
        crate::eval_events::append_event_failsafe(
            context.eval_events_path.as_deref(),
            internal_panic_stop_event(context, diagnostic, &recovery),
        )
    });
    run_finalizer_step("writing internal panic run summary", || {
        write_internal_panic_summary(context, diagnostic, &recovery);
        Ok(())
    });
    recovery
}

fn save_recovery_note_safely(
    context: &PanicBoundaryContext,
    diagnostic: &PanicDiagnostic,
) -> RecoveryNoteOutcome {
    match std::panic::catch_unwind(AssertUnwindSafe(|| save_recovery_note(context, diagnostic))) {
        Ok(Ok(path)) => RecoveryNoteOutcome {
            relative_path: display_workspace_path(&context.workspace_root, &path),
            error: String::new(),
        },
        Ok(Err(err)) => {
            stderr_line(&format!(
                "internal panic boundary: failed to write recovery note: {err:#}"
            ));
            RecoveryNoteOutcome {
                relative_path: String::new(),
                error: err.to_string(),
            }
        }
        Err(payload) => {
            let error = format!(
                "panic while writing recovery note: {}",
                panic_payload_message(payload.as_ref())
            );
            stderr_line(&format!("internal panic boundary: {error}"));
            RecoveryNoteOutcome {
                relative_path: String::new(),
                error,
            }
        }
    }
}

fn save_recovery_note(
    context: &PanicBoundaryContext,
    diagnostic: &PanicDiagnostic,
) -> anyhow::Result<PathBuf> {
    let dir = context.workspace_root.join(".anvil").join("repairs");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("repair-internal-panic-{}.md", uuid::Uuid::now_v7()));
    let message = crate::eval_events::render_stop_reason_text(&diagnostic.message);
    let note = format!(
        "# Internal panic recovery note\n\n\
Status: failed\n\
Reason: internal_panic\n\
Panic message: {message}\n\
Panic location: {}\n\n\
Reproduction command:\n\n    {}\n",
        diagnostic.location, context.reproduction_command
    );
    std::fs::write(&path, note)?;
    Ok(path)
}

fn internal_panic_stop_event(
    context: &PanicBoundaryContext,
    diagnostic: &PanicDiagnostic,
    recovery: &RecoveryNoteOutcome,
) -> Value {
    // A direct-command drop may already have emitted its terminal projection during unwind.
    // Mirror those fields so the process-level stop remains compatible while naming the panic.
    let terminal =
        crate::eval_events::latest_tui_command_stop_event(context.eval_events_path.as_deref())
            .filter(|event| event.get("ok").and_then(Value::as_bool) == Some(false));
    let completion_status = terminal_field(&terminal, "completion_status", "failed");
    let task_status = terminal_field(&terminal, "task_status", "failed");
    let assurance_level = terminal_field(&terminal, "assurance_level", "partial");
    let assurance_reason = terminal_field(&terminal, "assurance_reason", "internal_panic");
    let effective_profile = terminal_field(&terminal, "effective_profile", &context.profile);
    let contract_origin = terminal_field(&terminal, "contract_origin", "initial");
    let runtime_acceptance = terminal_field(&terminal, "runtime_acceptance_status", "not_checked");
    let final_acceptance = terminal_field(&terminal, "final_acceptance_status", "not_checked");
    let release_gate = terminal_field(&terminal, "release_gate_status", "not_applicable");
    let release_quality = terminal_field(&terminal, "release_quality_completion", "incomplete");
    let next_action = terminal_field(&terminal, "next_action", "inspect_internal_panic");
    let panic_message = crate::eval_events::body_snippet(&diagnostic.message);

    json!({
        "event": "run_stop",
        "ok": false,
        "lifecycle_stage": "process",
        "action": context.action,
        "reason": "internal_panic",
        "stop_reason": "internal_panic",
        "failure_kind": "internal_panic",
        "status": "failed",
        "completion_status": completion_status,
        "task_status": task_status,
        "profile": context.profile,
        "effective_profile": effective_profile,
        "prompt_layout": context.prompt_layout,
        "contract_origin": contract_origin,
        "assurance_level": assurance_level,
        "assurance_reason": assurance_reason,
        "session_status": "process_exited",
        "repl_status": "not_applicable",
        "process_completion_state": "failed",
        "command_completion_state": "failed",
        "runtime_acceptance_status": runtime_acceptance,
        "final_acceptance_status": final_acceptance,
        "release_gate_status": release_gate,
        "release_quality_completion": release_quality,
        "next_action": next_action,
        "recovery_next_action": "inspect_internal_panic",
        "recovery_prompt_path": recovery.relative_path,
        "recovery_note_path": recovery.relative_path,
        "recovery_note_error": recovery.error,
        "recovery_ultra_plan_path": "",
        "suggested_recovery_command": context.reproduction_command,
        "suggested_recovery_yaml_command": "",
        "panic_message": panic_message,
        "panic_location": diagnostic.location,
        "build_commit": crate::build_info::COMMIT,
        "build_dirty": crate::build_info::dirty(),
        "build_timestamp": crate::build_info::TIMESTAMP,
    })
}

fn write_internal_panic_summary(
    context: &PanicBoundaryContext,
    diagnostic: &PanicDiagnostic,
    recovery: &RecoveryNoteOutcome,
) {
    let recovery_line = if recovery.relative_path.is_empty() {
        format!("Recovery note: unavailable ({})", recovery.error)
    } else {
        format!("Recovery note: {}", recovery.relative_path)
    };
    crate::eval_events::write_run_summary(
        context.eval_events_path.as_deref(),
        &format!(
            "Status: failed\n\
Lifecycle stage: process\n\
Action: {}\n\
Failure kind: internal_panic\n\
Stop reason: internal_panic\n\
Panic message: {}\n\
Panic location: {}\n\
{}\n\
Reproduction command: {}",
            context.action,
            crate::eval_events::body_snippet(&diagnostic.message),
            diagnostic.location,
            recovery_line,
            context.reproduction_command
        ),
    );
}

fn run_finalizer_step(label: &str, operation: impl FnOnce() -> anyhow::Result<()>) {
    match std::panic::catch_unwind(AssertUnwindSafe(operation)) {
        Ok(Ok(())) => {}
        Ok(Err(err)) => stderr_line(&format!(
            "internal panic boundary: failed while {label}: {err:#}"
        )),
        Err(payload) => stderr_line(&format!(
            "internal panic boundary: panic while {label}: {}",
            panic_payload_message(payload.as_ref())
        )),
    }
}

fn terminal_field(terminal: &Option<Value>, key: &str, fallback: &str) -> String {
    terminal
        .as_ref()
        .and_then(|event| event.get(key))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(fallback)
        .to_string()
}

fn nested_panic_diagnostic(path: Option<&Path>) -> Option<PanicDiagnostic> {
    let text = std::fs::read_to_string(path?).ok()?;
    text.lines().rev().find_map(|line| {
        let event = serde_json::from_str::<Value>(line).ok()?;
        if event.get("event").and_then(Value::as_str) != Some("panic_caught") {
            return None;
        }
        Some(PanicDiagnostic {
            message: event
                .get("panic_message")
                .or_else(|| event.get("message"))
                .and_then(Value::as_str)
                .unwrap_or("non-string panic payload")
                .to_string(),
            location: event
                .get("panic_location")
                .or_else(|| event.get("location"))
                .and_then(Value::as_str)
                .unwrap_or("unknown")
                .to_string(),
        })
    })
}

fn current_reproduction_command() -> String {
    let command = std::env::args_os()
        .map(|argument| shell_quote(&argument.to_string_lossy()))
        .collect::<Vec<_>>()
        .join(" ");
    if command.is_empty() {
        "anvilminimal".to_string()
    } else {
        command
    }
}

fn shell_quote(value: &str) -> String {
    if !value.is_empty()
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || "_-./:=+,".contains(ch))
    {
        value.to_string()
    } else {
        format!("'{}'", value.replace('\'', "'\"'\"'"))
    }
}

fn display_workspace_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn panic_payload_message(payload: &(dyn Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_string()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "non-string panic payload".to_string()
    }
}

fn stderr_line(message: &str) {
    let _ = writeln!(std::io::stderr().lock(), "{message}");
}

pub(crate) fn report_secondary_panic(label: &str, payload: Box<dyn Any + Send>) {
    stderr_line(&format!(
        "internal panic boundary: panic while {label}: {}",
        panic_payload_message(payload.as_ref())
    ));
}

#[cfg(test)]
thread_local! {
    static TEST_FAULT_REQUESTED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static TEST_FINALIZER_FAULT_REQUESTED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[cfg(test)]
pub(crate) fn request_test_fault() {
    TEST_FAULT_REQUESTED.with(|requested| requested.set(true));
}

#[cfg(test)]
pub(crate) fn inject_test_fault_if_requested() {
    let requested = TEST_FAULT_REQUESTED.with(|requested| requested.replace(false));
    if requested {
        panic!("fault injection: CLI run panic boundary");
    }
}

#[cfg(test)]
pub(crate) fn request_test_finalizer_fault() {
    TEST_FINALIZER_FAULT_REQUESTED.with(|requested| requested.set(true));
}

#[cfg(test)]
pub(crate) fn inject_test_finalizer_fault_if_requested() {
    let requested = TEST_FINALIZER_FAULT_REQUESTED.with(|requested| requested.replace(false));
    if requested {
        panic!("fault injection: direct completion finalizer panic");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context(root: &Path, events: &Path) -> PanicBoundaryContext {
        PanicBoundaryContext {
            workspace_root: root.to_path_buf(),
            eval_events_path: Some(events.to_path_buf()),
            action: "UltraPlanRun(\"fault injection\")".to_string(),
            profile: "nextjs".to_string(),
            prompt_layout: "legacy".to_string(),
            reproduction_command: "anvilminimal --ultra-plan-run 'fault injection'".to_string(),
        }
    }

    fn read_events(path: &Path) -> Vec<Value> {
        std::fs::read_to_string(path)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    #[test]
    fn fault_injection_writes_terminal_event_and_recovery_note() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join(".anvil/runs/fault/events.jsonl");
        request_test_fault();

        let result = catch_with_context(context(dir.path(), &events), || {
            inject_test_fault_if_requested();
            Ok(())
        });

        let err = result.expect_err("fault injection must fail");
        assert!(err.to_string().contains("internal_panic"), "{err:#}");
        let events = read_events(&events);
        let stop = events.last().unwrap();
        assert_eq!(stop.get("event").and_then(Value::as_str), Some("run_stop"));
        assert_eq!(
            stop.get("reason").and_then(Value::as_str),
            Some("internal_panic")
        );
        assert_eq!(stop.get("ok").and_then(Value::as_bool), Some(false));
        assert_eq!(
            stop.get("panic_message").and_then(Value::as_str),
            Some("fault injection: CLI run panic boundary")
        );
        assert!(
            stop.get("panic_location")
                .and_then(Value::as_str)
                .is_some_and(|location| location.contains("cli_panic_boundary.rs")),
            "{stop:#}"
        );
        let recovery_path = stop
            .get("recovery_note_path")
            .and_then(Value::as_str)
            .unwrap();
        assert!(recovery_path.starts_with(".anvil/repairs/repair-internal-panic-"));
        let note = std::fs::read_to_string(dir.path().join(recovery_path)).unwrap();
        assert!(note.contains("Reason: internal_panic"), "{note}");
        assert!(
            note.contains("Panic message: fault injection: CLI run panic boundary"),
            "{note}"
        );
        assert!(note.contains("Panic location:"), "{note}");
        assert!(
            note.contains("anvilminimal --ultra-plan-run 'fault injection'"),
            "{note}"
        );
    }

    #[test]
    fn resumed_worker_panic_keeps_original_location() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join(".anvil/runs/worker-fault/events.jsonl");

        let result = catch_with_context(context(dir.path(), &events), || {
            let payload = std::thread::spawn(|| {
                panic!("fault injection: resumed worker panic");
            })
            .join()
            .expect_err("worker must panic");
            std::panic::resume_unwind(payload)
        });

        assert!(result.unwrap_err().to_string().contains("internal_panic"));
        let events = read_events(&events);
        let stop = events.last().unwrap();
        assert_eq!(
            stop.get("panic_message").and_then(Value::as_str),
            Some("fault injection: resumed worker panic")
        );
        assert!(
            stop.get("panic_location")
                .and_then(Value::as_str)
                .is_some_and(|location| location.contains("cli_panic_boundary.rs")),
            "{stop:#}"
        );
        assert_ne!(
            stop.get("panic_location").and_then(Value::as_str),
            Some("unknown")
        );
    }

    #[test]
    fn normal_result_does_not_add_terminal_event_or_recovery_note() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join(".anvil/runs/normal/events.jsonl");
        let event_path = events.clone();

        let result = catch_with_context(context(dir.path(), &events), || {
            crate::eval_events::emit(
                Some(&event_path),
                json!({"event": "normal_marker", "status": "completed"}),
            );
            Ok(())
        });

        result.unwrap();
        let events = read_events(&events);
        assert_eq!(events.len(), 1, "{events:#?}");
        assert_eq!(
            events[0].get("event").and_then(Value::as_str),
            Some("normal_marker")
        );
        assert!(!dir.path().join(".anvil/repairs").exists());
    }

    #[test]
    fn recovery_note_io_failure_is_recorded_without_second_unwind() {
        let dir = tempfile::tempdir().unwrap();
        let workspace_file = dir.path().join("workspace-file");
        std::fs::write(&workspace_file, "not a directory").unwrap();
        let events = dir.path().join("events.jsonl");
        request_test_fault();

        let result = catch_with_context(context(&workspace_file, &events), || {
            inject_test_fault_if_requested();
            Ok(())
        });

        assert!(result.unwrap_err().to_string().contains("internal_panic"));
        let events = read_events(&events);
        let stop = events.last().unwrap();
        assert_eq!(
            stop.get("recovery_note_path").and_then(Value::as_str),
            Some("")
        );
        assert!(
            stop.get("recovery_note_error")
                .and_then(Value::as_str)
                .is_some_and(|error| !error.is_empty()),
            "{stop:#}"
        );
    }
}
