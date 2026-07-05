use std::collections::BTreeSet;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, bail};

use crate::tools::bash::{BashOutcome, BashOutcomeKind};

const DEFAULT_VERIFY_TIMEOUT: Duration = Duration::from_secs(120);
const PYTHON_CLI_PYTEST_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_STREAM_BYTES: usize = 24_000;
pub const ENV_NODE_ENV_CONFLICT_KIND: &str = "env_node_env_conflict";
pub const ENV_NODE_ENV_REMEDIATION: &str = "unset NODE_ENV or run via env -u NODE_ENV";

pub fn normalized_command<S: AsRef<OsStr>>(program: S) -> Command {
    let mut command = Command::new(program);
    apply_normalized_env(&mut command);
    command
}

pub fn normalized_command_at_root<S: AsRef<OsStr>>(program: S, root: &Path) -> Command {
    let mut command = normalized_command(program);
    apply_workspace_path_env(&mut command, root);
    command
}

pub fn apply_normalized_env(command: &mut Command) -> &mut Command {
    command
        .env_remove("NODE_ENV")
        .env_remove("NODE_OPTIONS")
        .env("NEXT_TELEMETRY_DISABLED", "1")
}

pub fn apply_workspace_path_env<'a>(command: &'a mut Command, root: &Path) -> &'a mut Command {
    if let Some(path) = sanitized_path_for_root(root, std::env::var_os("PATH").as_deref()) {
        command.env("PATH", path);
    }
    command
}

pub(crate) fn sanitized_path_for_root(root: &Path, path: Option<&OsStr>) -> Option<OsString> {
    let workspace_bin = absolute_workspace_root(root).join("node_modules/.bin");
    let mut entries = vec![workspace_bin.clone()];
    let mut seen = BTreeSet::new();
    seen.insert(path_key(&workspace_bin));

    if let Some(path) = path {
        for entry in std::env::split_paths(path) {
            if is_foreign_node_modules_bin_entry(root, &entry) {
                continue;
            }
            if seen.insert(path_key(&entry)) {
                entries.push(entry);
            }
        }
    }

    std::env::join_paths(entries).ok()
}

pub(crate) fn foreign_node_modules_bin_on_path(root: &Path, tool: &str) -> Option<PathBuf> {
    foreign_node_modules_bin_in_path_value(root, tool, std::env::var_os("PATH").as_deref())
}

pub(crate) fn foreign_node_modules_bin_in_path_value(
    root: &Path,
    tool: &str,
    path: Option<&OsStr>,
) -> Option<PathBuf> {
    let path = path?;
    std::env::split_paths(path)
        .filter(|entry| is_foreign_node_modules_bin_entry(root, entry))
        .map(|entry| entry.join(tool))
        .find(|candidate| candidate.is_file())
}

fn is_foreign_node_modules_bin_entry(root: &Path, entry: &Path) -> bool {
    path_contains_node_modules_bin(entry) && !path_is_under_root(entry, root)
}

fn path_contains_node_modules_bin(path: &Path) -> bool {
    let text = path.to_string_lossy();
    text.contains("/node_modules/.bin")
        || text.contains("\\node_modules\\.bin")
        || text.ends_with("node_modules/.bin")
        || text.ends_with("node_modules\\.bin")
}

fn path_is_under_root(path: &Path, root: &Path) -> bool {
    absolute_path(path).starts_with(absolute_workspace_root(root))
}

fn absolute_workspace_root(root: &Path) -> PathBuf {
    absolute_path(root)
}

fn absolute_path(path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(path)
    }
}

fn path_key(path: &Path) -> String {
    path.to_string_lossy().to_string()
}

pub fn host_env_contamination() -> Vec<String> {
    ["NODE_ENV", "NODE_OPTIONS"]
        .into_iter()
        .filter_map(|name| {
            std::env::var(name)
                .ok()
                .filter(|value| !value.is_empty())
                .map(|value| format!("{name}={value}"))
        })
        .collect()
}

pub fn has_host_env_contamination() -> bool {
    !host_env_contamination().is_empty()
}

pub fn next_node_env_marker_present(text: &str) -> bool {
    text.contains("non-standard \"NODE_ENV\"")
        || text.contains("non-standard `NODE_ENV`")
        || text.to_ascii_lowercase().contains("non-standard node_env")
}

pub fn is_env_node_env_conflict_output(text: &str) -> bool {
    has_host_env_contamination() && next_node_env_marker_present(text)
}

pub fn env_node_env_conflict_reason(output: &str) -> String {
    let snippet = crate::eval_events::body_snippet(output);
    if snippet.trim().is_empty() {
        format!("{ENV_NODE_ENV_CONFLICT_KIND}: {ENV_NODE_ENV_REMEDIATION}")
    } else {
        format!("{ENV_NODE_ENV_CONFLICT_KIND}: {ENV_NODE_ENV_REMEDIATION}; {snippet}")
    }
}

pub fn with_env_node_env_remediation(output: &str) -> String {
    if output.contains(ENV_NODE_ENV_REMEDIATION) {
        output.to_string()
    } else if output.trim().is_empty() {
        ENV_NODE_ENV_REMEDIATION.to_string()
    } else {
        format!("{}\n{}", output.trim_end(), ENV_NODE_ENV_REMEDIATION)
    }
}

pub fn run_checked(command: &str, root: &Path, offline: bool) -> anyhow::Result<String> {
    let outcome = run_structured(command, root, offline, DEFAULT_VERIFY_TIMEOUT)?;
    let formatted = format_outcome(&outcome);
    if !outcome.is_success() {
        if is_dev_or_start_verify_command(command) && is_env_node_env_conflict_output(&formatted) {
            bail!("{}", env_node_env_conflict_reason(&formatted));
        }
        bail!("command failed: {command}\n{formatted}");
    }
    Ok(formatted)
}

pub(crate) fn run_structured_for_verify_with_profile(
    command: &str,
    root: &Path,
    profile: Option<&str>,
    offline: bool,
) -> anyhow::Result<BashOutcome> {
    run_structured(
        command,
        root,
        offline,
        verify_timeout_for_command(command, profile),
    )
}

#[cfg(test)]
pub(crate) fn run_structured_for_verify_with_timeout(
    command: &str,
    root: &Path,
    offline: bool,
    timeout: Duration,
) -> anyhow::Result<BashOutcome> {
    run_structured(command, root, offline, timeout)
}

pub(crate) fn verify_timeout_for_command(command: &str, profile: Option<&str>) -> Duration {
    if profile == Some("python-cli") && is_pytest_verify_command(command) {
        return PYTHON_CLI_PYTEST_TIMEOUT;
    }
    DEFAULT_VERIFY_TIMEOUT
}

fn is_pytest_verify_command(command: &str) -> bool {
    let lower = command.trim().to_ascii_lowercase();
    lower == "pytest"
        || lower.starts_with("pytest ")
        || lower == "python -m pytest"
        || lower.starts_with("python -m pytest ")
        || lower == "python3 -m pytest"
        || lower.starts_with("python3 -m pytest ")
}

pub(crate) fn format_verify_outcome(outcome: &BashOutcome) -> String {
    format_outcome(outcome)
}

fn run_structured(
    command: &str,
    root: &Path,
    offline: bool,
    timeout: Duration,
) -> anyhow::Result<BashOutcome> {
    let started = Instant::now();
    if let Some(reason) = crate::tools::bash::blocked_reason(command, offline) {
        return Ok(BashOutcome {
            kind: BashOutcomeKind::Blocked,
            status: None,
            stdout: String::new(),
            stderr: String::new(),
            elapsed_ms: started.elapsed().as_millis(),
            summary: reason,
        });
    }

    let mut process = normalized_command_at_root("sh", root);
    process.arg("-c").arg(command).current_dir(root);
    process.stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        process.process_group(0);
    }
    let mut child = process
        .spawn()
        .with_context(|| format!("failed to spawn verifier command: {command}"))?;

    loop {
        if let Some(status) = child.try_wait()? {
            let output = child.wait_with_output()?;
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            let kind = if status.success() {
                BashOutcomeKind::Success
            } else {
                BashOutcomeKind::CommandFailed
            };
            return Ok(BashOutcome {
                kind,
                status: Some(status.to_string()),
                stdout: truncate_stream(&stdout),
                stderr: truncate_stream(&stderr),
                elapsed_ms: started.elapsed().as_millis(),
                summary: build_summary(command, kind, &stdout, &stderr),
            });
        }
        if started.elapsed() >= timeout {
            terminate_child(&mut child);
            let output = child.wait_with_output()?;
            return Ok(BashOutcome {
                kind: BashOutcomeKind::Timeout,
                status: None,
                stdout: truncate_stream(&String::from_utf8_lossy(&output.stdout)),
                stderr: truncate_stream(&String::from_utf8_lossy(&output.stderr)),
                elapsed_ms: started.elapsed().as_millis(),
                summary: format!("command timed out after {} ms", timeout.as_millis()),
            });
        }
        thread::sleep(Duration::from_millis(20));
    }
}

fn terminate_child(child: &mut std::process::Child) {
    #[cfg(unix)]
    {
        let _ = Command::new("kill")
            .arg("-TERM")
            .arg(format!("-{}", child.id()))
            .status();
        thread::sleep(Duration::from_millis(50));
    }
    let _ = child.kill();
}

fn format_outcome(outcome: &BashOutcome) -> String {
    format!(
        "outcome: {:?}\nstatus: {}\nelapsed_ms: {}\nsummary: {}\nstdout:\n{}\nstderr:\n{}",
        outcome.kind,
        outcome.status.as_deref().unwrap_or("none"),
        outcome.elapsed_ms,
        outcome.summary,
        outcome.stdout,
        outcome.stderr
    )
}

fn truncate_stream(value: &str) -> String {
    crate::util::excerpt_with_newline_marker(
        value,
        MAX_STREAM_BYTES,
        &format!("[anvilminimal: verifier output truncated at {MAX_STREAM_BYTES} bytes]"),
    )
}

fn build_summary(command: &str, kind: BashOutcomeKind, stdout: &str, stderr: &str) -> String {
    if kind == BashOutcomeKind::Success {
        return "command succeeded".to_string();
    }
    let combined = format!("{stderr}\n{stdout}");
    let mut lines = Vec::new();
    for line in combined.lines() {
        let lower = line.to_ascii_lowercase();
        if lower.contains("error")
            || lower.contains("failed")
            || lower.contains("panic")
            || lower.contains("exception")
            || lower.contains("not found")
            || lower.contains("non-standard")
        {
            lines.push(line.trim().to_string());
        }
        if lines.len() >= 8 {
            break;
        }
    }
    if lines.is_empty() {
        format!("command did not succeed: {command}")
    } else {
        lines.join("\n")
    }
}

fn is_dev_or_start_verify_command(command: &str) -> bool {
    let lower = command.trim().to_ascii_lowercase();
    lower == "npm run dev"
        || lower.starts_with("npm run dev ")
        || lower == "npm run start"
        || lower.starts_with("npm run start ")
        || lower == "npm start"
        || lower.starts_with("npm start ")
        || lower == "pnpm dev"
        || lower.starts_with("pnpm dev ")
        || lower == "pnpm start"
        || lower.starts_with("pnpm start ")
        || lower == "yarn dev"
        || lower.starts_with("yarn dev ")
        || lower == "yarn start"
        || lower.starts_with("yarn start ")
        || lower.contains("next dev")
        || lower.contains("next start")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalized_command_removes_node_env_and_options() {
        let status = run_ignored_self_test(
            "minimal_loop::verifier_env::tests::normalized_command_removes_node_env_and_options_child",
            &[
                ("NODE_ENV", "production"),
                ("NODE_OPTIONS", "--require ./host-hook.js"),
            ],
        );
        assert!(status.success(), "{status}");
    }

    #[test]
    #[ignore]
    fn normalized_command_removes_node_env_and_options_child() {
        let output = normalized_command("sh")
            .arg("-c")
            .arg("printf '%s|%s|%s' \"${NODE_ENV-unset}\" \"${NODE_OPTIONS-unset}\" \"${NEXT_TELEMETRY_DISABLED-unset}\"")
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        assert_eq!(String::from_utf8_lossy(&output.stdout), "unset|unset|1");
    }

    #[test]
    fn host_env_contamination_reports_node_env() {
        let status = run_ignored_self_test(
            "minimal_loop::verifier_env::tests::host_env_contamination_reports_node_env_child",
            &[("NODE_ENV", "production")],
        );
        assert!(status.success(), "{status}");
    }

    #[test]
    #[ignore]
    fn host_env_contamination_reports_node_env_child() {
        let contamination = host_env_contamination();
        assert!(
            contamination
                .iter()
                .any(|entry| entry == "NODE_ENV=production"),
            "{contamination:?}"
        );
    }

    #[test]
    fn sanitized_path_removes_foreign_node_modules_bin_and_prepends_workspace_bin() {
        let root = Path::new("/tmp/anvil-workspace");
        let current = std::env::join_paths([
            Path::new("/usr/bin"),
            Path::new("/tmp/other/node_modules/.bin"),
            Path::new("/bin"),
            Path::new("/tmp/anvil-workspace/node_modules/.bin"),
        ])
        .unwrap();

        let sanitized = sanitized_path_for_root(root, Some(current.as_os_str())).unwrap();
        let entries = std::env::split_paths(&sanitized).collect::<Vec<_>>();

        assert_eq!(
            entries[0],
            PathBuf::from("/tmp/anvil-workspace/node_modules/.bin")
        );
        assert!(entries.contains(&PathBuf::from("/usr/bin")));
        assert!(entries.contains(&PathBuf::from("/bin")));
        assert!(!entries.contains(&PathBuf::from("/tmp/other/node_modules/.bin")));
        assert_eq!(
            entries
                .iter()
                .filter(
                    |entry| entry.as_path() == Path::new("/tmp/anvil-workspace/node_modules/.bin")
                )
                .count(),
            1
        );
    }

    #[test]
    fn verifier_stream_truncation_handles_multibyte_boundary() {
        let value = format!("{}{}", "x".repeat(MAX_STREAM_BYTES - 1), "除外");
        let truncated = truncate_stream(&value);
        assert!(truncated.contains("verifier output truncated"));
        assert!(truncated.starts_with(&"x".repeat(MAX_STREAM_BYTES - 1)));
    }

    #[test]
    fn verify_timeout_kills_process_group_and_reports_timeout() {
        let dir = tempfile::tempdir().unwrap();
        let outcome = run_structured_for_verify_with_timeout(
            "sleep 5",
            dir.path(),
            false,
            Duration::from_millis(20),
        )
        .unwrap();

        assert_eq!(outcome.kind, BashOutcomeKind::Timeout);
        assert!(outcome.elapsed_ms < 2_000, "{outcome:?}");
        assert!(outcome.summary.contains("command timed out"), "{outcome:?}");
    }

    #[test]
    fn python_cli_pytest_uses_profile_timeout_cap() {
        assert_eq!(
            verify_timeout_for_command("python -m pytest", Some("python-cli")),
            PYTHON_CLI_PYTEST_TIMEOUT
        );
        assert_eq!(
            verify_timeout_for_command("python -m pytest", Some("generic")),
            DEFAULT_VERIFY_TIMEOUT
        );
    }

    fn run_ignored_self_test(test_name: &str, envs: &[(&str, &str)]) -> std::process::ExitStatus {
        let exe = std::env::current_exe().unwrap();
        let mut command = Command::new(exe);
        command.args(["--ignored", "--exact", test_name, "--nocapture"]);
        for (key, value) in envs {
            command.env(key, value);
        }
        command.status().unwrap()
    }
}
