use std::io;
use std::process::{Child, Command, ExitStatus, Output};
use std::thread;
use std::time::{Duration, Instant};

pub const DEFAULT_POLL_INTERVAL: Duration = Duration::from_millis(20);
const DEFAULT_KILL_GRACE: Duration = Duration::from_millis(50);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoundedProcessOutcomeKind {
    Exited,
    TimedOut,
    Cancelled,
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
    crate::tui::status_bus::publish_command_started(command, timeout);
    let child = match spawn_child(command) {
        Ok(child) => child,
        Err(err) => {
            crate::tui::status_bus::publish_command_finished();
            return Err(err);
        }
    };
    let output = wait_with_timeout_and_cancel(child, timeout, is_cancelled);
    crate::tui::status_bus::publish_command_finished();
    output
}

pub fn wait_with_timeout(child: Child, timeout: Duration) -> io::Result<BoundedProcessOutput> {
    wait_with_timeout_and_cancel(child, timeout, || false)
}

pub fn wait_with_timeout_and_cancel<F>(
    mut child: Child,
    timeout: Duration,
    is_cancelled: F,
) -> io::Result<BoundedProcessOutput>
where
    F: Fn() -> bool,
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
            terminate_process_group(&mut child);
            return output_from_child(
                child.wait_with_output()?,
                BoundedProcessOutcomeKind::Cancelled,
                started.elapsed(),
            );
        }
        if started.elapsed() >= timeout {
            terminate_process_group(&mut child);
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
    #[cfg(unix)]
    {
        let pgid = -(child.id() as i32);
        unsafe {
            libc::kill(pgid, libc::SIGTERM);
        }
        thread::sleep(DEFAULT_KILL_GRACE);
        unsafe {
            libc::kill(pgid, libc::SIGKILL);
        }
    }
    #[cfg(not(unix))]
    {
        thread::sleep(DEFAULT_KILL_GRACE);
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
}
