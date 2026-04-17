use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutoTestResult {
    pub command: String,
    pub exit_code: i32,
    pub output: String,
}

#[derive(Debug, Clone, Default)]
pub struct AutoTestRunner {
    command: Option<String>,
}

impl AutoTestRunner {
    pub fn new(command: Option<String>) -> Self {
        Self { command }
    }

    pub fn command(&self) -> Option<&str> {
        self.command.as_deref()
    }

    pub fn set_command(&mut self, command: Option<String>) {
        self.command = command;
    }

    pub fn is_enabled(&self) -> bool {
        self.command.is_some()
    }

    pub fn run_if_enabled(&self, cwd: &Path) -> Result<Option<AutoTestResult>, String> {
        let Some(command) = &self.command else {
            return Ok(None);
        };
        let output = Command::new("sh")
            .args(["-lc", command])
            .current_dir(cwd)
            .output()
            .map_err(|err| format!("failed to run auto-test command: {err}"))?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let combined = if stderr.trim().is_empty() {
            stdout.into_owned()
        } else if stdout.trim().is_empty() {
            stderr.into_owned()
        } else {
            format!("{stdout}\n{stderr}")
        };
        Ok(Some(AutoTestResult {
            command: command.clone(),
            exit_code: output.status.code().unwrap_or(-1),
            output: combined.chars().take(8_000).collect(),
        }))
    }
}
