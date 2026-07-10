use std::io;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Output};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::json;

pub const DEFAULT_POLL_INTERVAL: Duration = Duration::from_millis(20);
const DEFAULT_TIMEOUT_KILL_GRACE: Duration = Duration::from_millis(50);
pub const USER_INTERRUPT_KILL_GRACE: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegisteredServerChild {
    pub pid: u32,
    pub command: String,
    pub origin_phase: String,
    pub workspace_root: PathBuf,
}

static SERVER_CHILDREN: OnceLock<Mutex<Vec<RegisteredServerChild>>> = OnceLock::new();

fn server_children() -> &'static Mutex<Vec<RegisteredServerChild>> {
    SERVER_CHILDREN.get_or_init(|| Mutex::new(Vec::new()))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoundedProcessOutcomeKind {
    Exited,
    TimedOut,
    Cancelled,
    CommandAbortedByUser,
}

#[derive(Debug)]
pub struct BoundedProcessOutput {
    pub kind: BoundedProcessOutcomeKind,
    pub status: Option<ExitStatus>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub elapsed: Duration,
}

impl BoundedProcessOutput {
    pub fn success(&self) -> bool {
        self.status.is_some_and(|status| status.success())
    }
}

pub fn spawn_child(command: &mut Command) -> io::Result<Child> {
    apply_child_env_allowlist(command);
    configure_process_group(command);
    command.spawn()
}

fn apply_child_env_allowlist(command: &mut Command) {
    let explicit_env = command
        .get_envs()
        .map(|(key, value)| (key.to_os_string(), value.map(|value| value.to_os_string())))
        .collect::<Vec<_>>();
    command.env_clear();
    for (key, value) in std::env::vars_os() {
        if child_env_allowed(&key, EnvSource::Parent) {
            command.env(key, value);
        }
    }
    for (key, value) in explicit_env {
        match value {
            Some(value) if child_env_allowed(&key, EnvSource::Explicit) => {
                command.env(key, value);
            }
            Some(_) => {}
            None => {
                command.env_remove(key);
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EnvSource {
    Parent,
    Explicit,
}

fn child_env_allowed(key: &std::ffi::OsStr, source: EnvSource) -> bool {
    let Some(key) = key.to_str() else {
        return false;
    };
    matches!(key, "PATH" | "HOME" | "LANG" | "TERM" | "NODE_ENV")
        || key.starts_with("LC_")
        || key.starts_with("npm_config_")
        || (source == EnvSource::Explicit
            && matches!(
                key,
                "NODE_OPTIONS"
                    | "NODE_PATH"
                    | "NEXT_TELEMETRY_DISABLED"
                    | "PORT"
                    | "ANVIL_FOREIGN_TOOLCHAIN_ROOT"
                    | "ANVIL_FOREIGN_TOOLCHAIN_EVENTS"
            ))
        || (source == EnvSource::Explicit && test_control_env_allowed(key))
}

#[cfg(test)]
fn test_control_env_allowed(key: &str) -> bool {
    matches!(
        key,
        "ANVIL_BROWSER_PROBE_MOCK_CHILD"
            | "ANVIL_BROWSER_PROBE_MOCK_PORT"
            | "ANVIL_BROWSER_PROBE_MOCK_STATUS"
            | "ANVIL_BROWSER_PROBE_MOCK_DELAY_MS"
    )
}

#[cfg(not(test))]
fn test_control_env_allowed(_key: &str) -> bool {
    false
}

pub fn register_server_child(
    child: &Child,
    command: impl Into<String>,
    origin_phase: impl Into<String>,
    workspace_root: &Path,
) {
    let entry = RegisteredServerChild {
        pid: child.id(),
        command: command.into(),
        origin_phase: origin_phase.into(),
        workspace_root: workspace_root.to_path_buf(),
    };
    let mut children = server_children().lock().expect("server child registry");
    children.retain(|child| child.pid != entry.pid);
    children.push(entry);
}

pub fn unregister_server_child(pid: u32) {
    let mut children = server_children().lock().expect("server child registry");
    children.retain(|child| child.pid != pid);
}

pub fn registered_server_child(pid: u32) -> Option<RegisteredServerChild> {
    server_children()
        .lock()
        .expect("server child registry")
        .iter()
        .find(|child| child.pid == pid)
        .cloned()
}

pub fn reap_registered_server_child(
    pid: u32,
    eval_events_path: Option<&Path>,
    reason: &str,
) -> bool {
    let Some(child) = registered_server_child(pid) else {
        return false;
    };
    let reaped = terminate_process_group_by_pid(pid).is_ok();
    unregister_server_child(pid);
    emit_server_reaped(eval_events_path, &child, reason, reaped);
    reaped
}

pub fn reap_registered_server_children_for_workspace(
    eval_events_path: Option<&Path>,
    reason: &str,
    workspace_root: &Path,
) -> usize {
    let children = {
        let mut guard = server_children().lock().expect("server child registry");
        let mut remaining = Vec::new();
        let mut matched = Vec::new();
        for child in std::mem::take(&mut *guard) {
            if same_workspace_root(&child.workspace_root, workspace_root) {
                matched.push(child);
            } else {
                remaining.push(child);
            }
        }
        *guard = remaining;
        matched
    };
    reap_server_children(children, eval_events_path, reason)
}

fn reap_server_children(
    children: Vec<RegisteredServerChild>,
    eval_events_path: Option<&Path>,
    reason: &str,
) -> usize {
    let mut count = 0usize;
    for child in children {
        let reaped = terminate_process_group_by_pid(child.pid).is_ok();
        if reaped {
            count += 1;
        }
        emit_server_reaped(eval_events_path, &child, reason, reaped);
    }
    count
}

fn same_workspace_root(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

fn emit_server_reaped(
    eval_events_path: Option<&Path>,
    child: &RegisteredServerChild,
    reason: &str,
    ok: bool,
) {
    crate::eval_events::emit(
        eval_events_path,
        json!({
            "event": "server_reaped",
            "pid": child.pid,
            "command": child.command,
            "origin_phase": child.origin_phase,
            "workspace_root": child.workspace_root.display().to_string(),
            "reason": reason,
            "ok": ok,
        }),
    );
}

pub fn run_with_timeout(
    command: &mut Command,
    timeout: Duration,
) -> io::Result<BoundedProcessOutput> {
    run_with_timeout_and_cancel(command, timeout, || false)
}

pub fn run_with_timeout_and_cancel<F>(
    command: &mut Command,
    timeout: Duration,
    is_cancelled: F,
) -> io::Result<BoundedProcessOutput>
where
    F: Fn() -> bool,
{
    run_with_timeout_cancel_and_force(command, timeout, is_cancelled, || false)
}

pub fn run_with_timeout_cancel_and_force<F, G>(
    command: &mut Command,
    timeout: Duration,
    is_cancelled: F,
    is_force_cancelled: G,
) -> io::Result<BoundedProcessOutput>
where
    F: Fn() -> bool,
    G: Fn() -> bool,
{
    crate::tui::status_bus::publish_command_started(command, timeout);
    let child = match spawn_child(command) {
        Ok(child) => child,
        Err(err) => {
            crate::tui::status_bus::publish_command_finished();
            return Err(err);
        }
    };
    let output =
        wait_with_timeout_cancel_and_force(child, timeout, is_cancelled, is_force_cancelled);
    crate::tui::status_bus::publish_command_finished();
    output
}

pub fn wait_with_timeout(child: Child, timeout: Duration) -> io::Result<BoundedProcessOutput> {
    wait_with_timeout_and_cancel(child, timeout, || false)
}

pub fn wait_with_timeout_and_cancel<F>(
    child: Child,
    timeout: Duration,
    is_cancelled: F,
) -> io::Result<BoundedProcessOutput>
where
    F: Fn() -> bool,
{
    wait_with_timeout_cancel_and_force(child, timeout, is_cancelled, || false)
}

pub fn wait_with_timeout_cancel_and_force<F, G>(
    child: Child,
    timeout: Duration,
    is_cancelled: F,
    is_force_cancelled: G,
) -> io::Result<BoundedProcessOutput>
where
    F: Fn() -> bool,
    G: Fn() -> bool,
{
    wait_with_timeout_cancel_force_and_grace(
        child,
        timeout,
        is_cancelled,
        is_force_cancelled,
        USER_INTERRUPT_KILL_GRACE,
    )
}

fn wait_with_timeout_cancel_force_and_grace<F, G>(
    mut child: Child,
    timeout: Duration,
    is_cancelled: F,
    is_force_cancelled: G,
    user_interrupt_grace: Duration,
) -> io::Result<BoundedProcessOutput>
where
    F: Fn() -> bool,
    G: Fn() -> bool,
{
    let started = Instant::now();
    loop {
        if child.try_wait()?.is_some() {
            return output_from_child(
                child.wait_with_output()?,
                BoundedProcessOutcomeKind::Exited,
                started.elapsed(),
            );
        }
        if is_cancelled() {
            terminate_process_group_with_grace(
                &mut child,
                user_interrupt_grace,
                &is_force_cancelled,
            );
            return output_from_child(
                child.wait_with_output()?,
                BoundedProcessOutcomeKind::CommandAbortedByUser,
                started.elapsed(),
            );
        }
        if started.elapsed() >= timeout {
            terminate_process_group_with_grace(&mut child, DEFAULT_TIMEOUT_KILL_GRACE, || false);
            return output_from_child(
                child.wait_with_output()?,
                BoundedProcessOutcomeKind::TimedOut,
                started.elapsed(),
            );
        }
        thread::sleep(DEFAULT_POLL_INTERVAL);
    }
}

pub fn terminate_process_group(child: &mut Child) {
    terminate_process_group_with_grace(child, DEFAULT_TIMEOUT_KILL_GRACE, || false);
}

pub fn terminate_process_group_by_pid(pid: u32) -> io::Result<()> {
    #[cfg(unix)]
    {
        let pgid = i32::try_from(pid)
            .map_err(|_| io::Error::other("server child pid does not fit pid_t"))?;
        unsafe {
            libc::kill(-pgid, libc::SIGTERM);
        }
        thread::sleep(DEFAULT_TIMEOUT_KILL_GRACE);
        unsafe {
            libc::kill(-pgid, libc::SIGKILL);
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        Err(io::Error::other(
            "process-group reaping by pid is only supported on Unix",
        ))
    }
}

fn terminate_process_group_with_grace<F>(child: &mut Child, grace: Duration, is_force_cancelled: F)
where
    F: Fn() -> bool,
{
    #[cfg(unix)]
    {
        let pgid = -(child.id() as i32);
        unsafe {
            libc::kill(pgid, libc::SIGTERM);
        }
    }
    let grace_started = Instant::now();
    loop {
        if child.try_wait().ok().flatten().is_some() {
            return;
        }
        if is_force_cancelled() || grace_started.elapsed() >= grace {
            break;
        }
        let remaining = grace.saturating_sub(grace_started.elapsed());
        thread::sleep(DEFAULT_POLL_INTERVAL.min(remaining));
    }
    #[cfg(unix)]
    {
        let pgid = -(child.id() as i32);
        unsafe {
            libc::kill(pgid, libc::SIGKILL);
        }
    }
    let _ = child.kill();
}

fn output_from_child(
    output: Output,
    kind: BoundedProcessOutcomeKind,
    elapsed: Duration,
) -> io::Result<BoundedProcessOutput> {
    Ok(BoundedProcessOutput {
        kind,
        status: Some(output.status),
        stdout: output.stdout,
        stderr: output.stderr,
        elapsed,
    })
}

fn configure_process_group(command: &mut Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Stdio;
    use std::sync::Mutex;

    static ENV_TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn run_with_timeout_kills_hanging_child() {
        let mut command = Command::new("sh");
        command
            .arg("-c")
            .arg("sleep 5")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let output = run_with_timeout(&mut command, Duration::from_millis(100)).unwrap();
        assert_eq!(output.kind, BoundedProcessOutcomeKind::TimedOut);
        assert!(output.elapsed < Duration::from_secs(2), "{output:?}");
    }

    #[test]
    fn child_env_uses_allowlist_without_provider_keys() {
        let _guard = ENV_TEST_LOCK.lock().unwrap();
        let _env_guard = TestEnvGuard::set([
            ("OLLAMA_API_KEY", "ollama-secret"),
            ("GEMINI_API_KEY", "gemini-secret"),
            ("OPENAI_API_KEY", "openai-secret"),
            ("ANVIL_TEST_UNRELATED_PARENT_SECRET", "parent-secret"),
        ]);
        let cache_dir = tempfile::tempdir().unwrap();
        let mut command = Command::new("sh");
        command
            .arg("-c")
            .arg("env | sort")
            .env("NODE_ENV", "test")
            .env("npm_config_cache", cache_dir.path())
            .env("GEMINI_API_KEY", "explicit-secret")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let output = run_with_timeout(&mut command, Duration::from_secs(2)).unwrap();

        assert!(output.success(), "{output:?}");
        let env = String::from_utf8(output.stdout).unwrap();
        assert!(env.contains("PATH="), "{env}");
        assert!(env.contains("NODE_ENV=test"), "{env}");
        assert!(env.contains("npm_config_cache="), "{env}");
        assert!(!env.contains("OLLAMA_API_KEY"), "{env}");
        assert!(!env.contains("GEMINI_API_KEY"), "{env}");
        assert!(!env.contains("OPENAI_API_KEY"), "{env}");
        assert!(!env.contains("ANVIL_TEST_UNRELATED_PARENT_SECRET"), "{env}");
    }

    #[test]
    fn cancelled_child_is_classified_as_user_abort() {
        let mut command = Command::new("sh");
        command
            .arg("-c")
            .arg("trap '' TERM; while :; do :; done")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let started = Instant::now();
        let cancel_after = Instant::now() + Duration::from_millis(50);
        let output = wait_with_timeout_cancel_force_and_grace(
            spawn_child(&mut command).unwrap(),
            Duration::from_secs(30),
            || Instant::now() >= cancel_after,
            || false,
            Duration::from_millis(100),
        )
        .unwrap();

        assert_eq!(output.kind, BoundedProcessOutcomeKind::CommandAbortedByUser);
        assert!(started.elapsed() >= Duration::from_millis(80), "{output:?}");
        assert!(started.elapsed() < Duration::from_secs(2), "{output:?}");
        assert_eq!(USER_INTERRUPT_KILL_GRACE, Duration::from_secs(5));
    }

    #[test]
    fn registered_server_children_are_reaped_with_origin_telemetry() {
        let dir = tempfile::tempdir().unwrap();
        let events = dir.path().join("events.jsonl");
        let mut command = Command::new("sh");
        command
            .arg("-c")
            .arg("sleep 5")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let mut child = spawn_child(&mut command).unwrap();
        let pid = child.id();

        register_server_child(&child, "npm run dev", "phase-a", dir.path());
        assert_eq!(
            registered_server_child(pid).unwrap().origin_phase,
            "phase-a"
        );

        assert_eq!(
            reap_registered_server_children_for_workspace(
                Some(&events),
                "terminal_guard",
                dir.path()
            ),
            1
        );
        let _ = child.wait();
        assert!(registered_server_child(pid).is_none());
        let event_text = std::fs::read_to_string(events).unwrap();
        assert!(event_text.contains("\"event\":\"server_reaped\""));
        assert!(event_text.contains("\"origin_phase\":\"phase-a\""));
        assert!(event_text.contains("\"reason\":\"terminal_guard\""));
    }

    #[test]
    fn workspace_scoped_reap_preserves_other_run_children() {
        let dir_a = tempfile::tempdir().unwrap();
        let dir_b = tempfile::tempdir().unwrap();
        let events_a = dir_a.path().join("events.jsonl");
        let events_b = dir_b.path().join("events.jsonl");
        let mut command_a = Command::new("sh");
        command_a
            .arg("-c")
            .arg("sleep 5")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let mut command_b = Command::new("sh");
        command_b
            .arg("-c")
            .arg("sleep 5")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let mut child_a = spawn_child(&mut command_a).unwrap();
        let mut child_b = spawn_child(&mut command_b).unwrap();
        let pid_a = child_a.id();
        let pid_b = child_b.id();

        register_server_child(&child_a, "npm run dev", "phase-a", dir_a.path());
        register_server_child(&child_b, "npm run dev", "phase-b", dir_b.path());

        assert_eq!(
            reap_registered_server_children_for_workspace(
                Some(&events_a),
                "phase_transition",
                dir_a.path()
            ),
            1
        );
        let _ = child_a.wait();
        assert!(registered_server_child(pid_a).is_none());
        assert!(registered_server_child(pid_b).is_some());

        assert_eq!(
            reap_registered_server_children_for_workspace(Some(&events_b), "cleanup", dir_b.path()),
            1
        );
        let _ = child_b.wait();
        assert!(registered_server_child(pid_b).is_none());
    }

    struct TestEnvGuard {
        keys: Vec<&'static str>,
    }

    impl TestEnvGuard {
        fn set<const N: usize>(pairs: [(&'static str, &'static str); N]) -> Self {
            let keys = pairs.iter().map(|(key, _)| *key).collect();
            for (key, value) in pairs {
                unsafe {
                    std::env::set_var(key, value);
                }
            }
            Self { keys }
        }
    }

    impl Drop for TestEnvGuard {
        fn drop(&mut self) {
            for key in &self.keys {
                unsafe {
                    std::env::remove_var(key);
                }
            }
        }
    }
}
