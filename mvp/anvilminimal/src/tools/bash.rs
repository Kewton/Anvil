use std::path::Path;
use std::process::{Command, Output};

use anyhow::bail;

pub fn run(command: &str, root: &Path, offline: bool) -> anyhow::Result<String> {
    let output = run_command(command, root, offline)?;
    Ok(format_output(&output))
}

pub fn run_checked(command: &str, root: &Path, offline: bool) -> anyhow::Result<String> {
    let output = run_command(command, root, offline)?;
    let formatted = format_output(&output);
    if !output.status.success() {
        bail!("command failed: {command}\n{formatted}");
    }
    Ok(formatted)
}

fn run_command(command: &str, root: &Path, offline: bool) -> anyhow::Result<Output> {
    if let Some(reason) = blocked_reason(command, offline) {
        bail!("{reason}");
    }
    Ok(Command::new("sh")
        .arg("-c")
        .arg(command)
        .current_dir(root)
        .output()?)
}

fn format_output(output: &Output) -> String {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    format!(
        "status: {}\nstdout:\n{}\nstderr:\n{}",
        output.status, stdout, stderr
    )
}

pub fn blocked_reason(command: &str, offline: bool) -> Option<String> {
    let lower = command.to_ascii_lowercase();
    if lower.contains("rm -rf /")
        || lower.contains("sudo ")
        || lower.contains("printenv")
        || lower.contains("cat ~/.ssh")
        || lower.contains("/etc/passwd")
        || lower.contains("curl ") && lower.contains("| sh")
        || lower.contains("wget ") && lower.contains("| sh")
    {
        return Some("dangerous command blocked".to_string());
    }
    if offline
        && (lower.contains("npm install")
            || lower.contains("pnpm install")
            || lower.contains("yarn install")
            || lower.contains("cargo install")
            || lower.contains("curl ")
            || lower.contains("wget "))
    {
        return Some("network/setup command blocked in offline mode".to_string());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_returns_nonzero_output_for_agent_feedback() {
        let dir = tempfile::tempdir().unwrap();
        let output = run("false", dir.path(), false).unwrap();
        assert!(output.contains("status: exit status"));
    }

    #[test]
    fn run_checked_rejects_nonzero_for_verify() {
        let dir = tempfile::tempdir().unwrap();
        assert!(run_checked("false", dir.path(), false).is_err());
    }
}
