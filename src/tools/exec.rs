use std::path::PathBuf;
use std::process::Command;

use anyhow::Context;

#[derive(Debug, Default)]
pub struct ExecTool;

impl ExecTool {
    pub fn run(&self, request: &ExecRequest) -> anyhow::Result<ExecResult> {
        let output = Command::new(&request.program)
            .args(&request.args)
            .current_dir(&request.cwd)
            .output()
            .with_context(|| format!("failed to run command {}", request.program))?;

        Ok(ExecResult {
            exit_code: output.status.code(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }
}

#[derive(Debug, Clone)]
pub struct ExecRequest {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
}

#[derive(Debug, Clone)]
pub struct ExecResult {
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}
