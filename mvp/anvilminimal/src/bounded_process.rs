use std::io;
use std::process::{Child, Command, ExitStatus, Output};
use std::thread;
use std::time::{Duration, Instant};

pub const DEFAULT_POLL_INTERVAL: Duration = Duration::from_millis(20);
const DEFAULT_TIMEOUT_KILL_GRACE: Duration = Duration::from_millis(50);
pub const USER_INTERRUPT_KILL_GRACE: Duration = Duration::from_secs(5);

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
    configure_process_group(command);
    command.spawn()
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
}
