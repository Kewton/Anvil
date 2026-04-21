use std::process::Command;

use crate::tools::registry::truncate_output;

const BLOCKED_SNIPPETS: &[&str] = &["rm -rf /", "rm -rf ~", "mkfs", "dd if=", ":(){"];

pub fn run(command: &str, cwd: &std::path::Path) -> Result<String, String> {
    for snippet in BLOCKED_SNIPPETS {
        if command.contains(snippet) {
            return Err(format!("blocked dangerous command fragment: {snippet}"));
        }
    }

    let output = Command::new("sh")
        .args(["-lc", command])
        .current_dir(cwd)
        .output()
        .map_err(|err| format!("failed to run shell command: {err}"))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = if stderr.trim().is_empty() {
        stdout.into_owned()
    } else if stdout.trim().is_empty() {
        stderr.into_owned()
    } else {
        format!("{stdout}\n{stderr}")
    };
    Ok(format!(
        "exit_code={}\n{}",
        output.status.code().unwrap_or(-1),
        truncate_output(&combined, 20_000)
    ))
}
