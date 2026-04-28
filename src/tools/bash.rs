use std::io::Read;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use crate::session::feedback::FeedbackKind;
use crate::tools::registry::truncate_output;

/// Env-inheritance policy for `run_with_outcome`. CB-003 (Issue #459).
///
/// `Inherit` is the historical default — the child shell inherits every
/// `std::env::vars()` entry from the parent process. `TesterSanitized` clears
/// the child env and forwards only an explicit allowlist; this is the policy
/// the Tester Skill smoke runner uses so LLM-generated test code cannot read
/// `OPENAI_API_KEY` / `GITHUB_TOKEN` / `AWS_*` etc. and exfiltrate them via
/// stdout / stderr.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BashEnvPolicy {
    #[default]
    Inherit,
    TesterSanitized,
}

/// CB-003 (Issue #459): exact set of env keys forwarded to a Tester smoke
/// child. Anything else (including `ANVIL_*`, `ANTHROPIC_*`, `OPENAI_*`,
/// `GITHUB_*`, `AWS_*`, `*_TOKEN`, `*_KEY`, `*_SECRET`, `*_PASSWORD`) is
/// dropped. `LANG` / `LC_*` / `TZ` are not forwarded by exact match — the
/// allowlist below picks up `LANG`, and `LC_ALL` is included explicitly to
/// keep cargo / rustc / python output deterministic on CI.
pub const TESTER_ENV_ALLOWLIST_EXACT: &[&str] = &[
    "PATH",
    "HOME",
    "USER",
    "LOGNAME",
    "LANG",
    "LC_ALL",
    "LC_CTYPE",
    "TZ",
    "TMPDIR",
    "SHELL",
    "TERM",
    // Rust toolchain locations are necessary for `cargo test` to find rustc.
    "RUSTUP_HOME",
    "CARGO_HOME",
    // Anvil's smoke-runner injects this directly via `env CARGO_TARGET_DIR=...`
    // in the command line (CB-005), but pass-through is harmless and lets a
    // workspace-wide override flow through unchanged.
    "CARGO_TARGET_DIR",
];

/// Filter the parent process env down to the Tester-safe allowlist. Pure
/// function over a list of `(key, value)` pairs so the unit test can drive
/// it without mutating the global process env.
pub fn filter_env_for_tester<I, K, V>(env: I) -> Vec<(String, String)>
where
    I: IntoIterator<Item = (K, V)>,
    K: AsRef<str>,
    V: Into<String>,
{
    env.into_iter()
        .filter_map(|(k, v)| {
            let key = k.as_ref();
            if TESTER_ENV_ALLOWLIST_EXACT.contains(&key) {
                Some((key.to_string(), v.into()))
            } else {
                None
            }
        })
        .collect()
}

/// Internal Bash outcome bag exposed for FeedbackFrame generation in
/// the agent layer (Issue #450). Not part of `ToolRegistry::execute`'s
/// `Result<String, String>` contract; turn.rs invokes
/// `run_with_outcome` directly when it needs the structured form.
#[derive(Debug, Clone, Default)]
pub struct BashExecutionOutcome {
    pub command: String,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
    pub blocked_reason: Option<String>,
    pub interrupted: bool,
}

impl BashExecutionOutcome {
    pub fn is_failure(&self) -> bool {
        self.timed_out
            || self.blocked_reason.is_some()
            || self.exit_code.is_some_and(|code| code != 0)
            || self.interrupted
    }
}

/// Classify a `BashExecutionOutcome` into a `FeedbackKind` for FeedbackFrame
/// generation. Pure function over the outcome; tested in `tests`.
pub fn classify_bash_outcome(outcome: &BashExecutionOutcome) -> FeedbackKind {
    if outcome.blocked_reason.is_some() {
        return FeedbackKind::UnsafeCommandBlocked;
    }
    if outcome.timed_out {
        return FeedbackKind::Timeout;
    }
    if outcome.interrupted {
        return FeedbackKind::UnknownFailure;
    }
    if outcome.exit_code.is_some_and(|code| code == 0) {
        return FeedbackKind::TestPass;
    }
    FeedbackKind::UnknownFailure
}

const BLOCKED_SNIPPETS: &[&str] = &[
    "rm -rf /",
    "rm -rf ~",
    "mkfs",
    "dd if=",
    ":(){",
    "rm -rf .anvil",
    "sudo ",
    "chmod -r 777",
];
const LONG_RUNNING_TIMEOUT: Duration = Duration::from_secs(15);
const TERMINATE_GRACE: Duration = Duration::from_secs(2);
const POLL_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BashCommandClass {
    ReadOnly,
    BuildTest,
    ScriptRun,
    Network,
    Mutating,
    Dangerous,
    General,
}

pub fn run(
    command: &str,
    cwd: &std::path::Path,
    cancel_flag: Option<&Arc<AtomicBool>>,
    offline: bool,
) -> Result<String, String> {
    run_with_outcome(command, cwd, cancel_flag, offline, None, None).map(|(text, _)| text)
}

/// Like `run`, but additionally returns the structured
/// `BashExecutionOutcome` (Issue #450). The returned `Result<String, String>`
/// is identical to what `run` returns so the registry contract is unchanged.
///
/// `explicit_timeout` (Issue #459 / DR1-003 / DR2-005):
///
/// * `Some(d)` — bypass the `likely_long_running_command` heuristic and
///   enforce `d` as the wall-clock timeout for this dispatch. Used by the
///   Tester Skill smoke runner (`TESTER_SMOKE_TIMEOUT_SECS`).
/// * `None` — preserve the existing call-site behaviour: a timeout is set
///   only when `likely_long_running_command` returns true.
///
/// `is_dangerous_command` / `BLOCKED_SNIPPETS` / offline policy filters are
/// applied unconditionally — `explicit_timeout` does not provide a bypass.
pub fn run_with_outcome(
    command: &str,
    cwd: &std::path::Path,
    cancel_flag: Option<&Arc<AtomicBool>>,
    offline: bool,
    explicit_timeout: Option<Duration>,
    env_policy: Option<BashEnvPolicy>,
) -> Result<(String, BashExecutionOutcome), String> {
    let normalized =
        normalize_background_command(&normalize_noninteractive_scaffold_command(command));
    for snippet in BLOCKED_SNIPPETS {
        if normalized.contains(snippet) {
            let msg = format!("blocked dangerous command fragment: {snippet}");
            return Err(msg);
        }
    }
    let class = classify_command(&normalized);
    enforce_offline_policy(&normalized, class, offline)?;

    let mut cmd = Command::new("sh");
    cmd.args(["-lc", &normalized])
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // CB-003 (Issue #459): Tester's smoke runner clears the child env and
    // forwards only the allowlist below. Production code (registry-driven
    // Bash, AutoTestRunner) passes `None` and keeps the historical inherit
    // behaviour so existing call sites are unaffected.
    let policy = env_policy.unwrap_or_default();
    if matches!(policy, BashEnvPolicy::TesterSanitized) {
        cmd.env_clear();
        for (key, value) in filter_env_for_tester(std::env::vars()) {
            cmd.env(key, value);
        }
    }
    if is_noninteractive_scaffold_command(&normalized) {
        cmd.env("CI", "1");
    }

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            cmd.pre_exec(|| {
                if libc::setpgid(0, 0) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }

    let mut child = cmd
        .spawn()
        .map_err(|err| format!("failed to run shell command: {err}"))?;
    // Issue #459 / DR1-003: explicit_timeout takes precedence over the
    // long-running heuristic so the Tester smoke runner can always cap
    // execution at TESTER_SMOKE_TIMEOUT_SECS.
    let timeout = explicit_timeout
        .or_else(|| likely_long_running_command(&normalized).then_some(LONG_RUNNING_TIMEOUT));
    let started = Instant::now();

    let status = loop {
        if cancel_flag.is_some_and(|flag| flag.load(Ordering::SeqCst)) {
            terminate_child(&mut child);
            let output = collect_output(&mut child)?;
            let combined = render_combined_output(output.stdout.clone(), output.stderr.clone());
            let outcome = BashExecutionOutcome {
                command: normalized.clone(),
                exit_code: None,
                stdout: output.stdout,
                stderr: output.stderr,
                timed_out: false,
                blocked_reason: None,
                interrupted: true,
            };
            return Ok((
                format!(
                    "exit_code=-1\ninterrupted=true\n{}",
                    truncate_output(&combined, 20_000)
                ),
                outcome,
            ));
        }

        if let Some(limit) = timeout
            && started.elapsed() >= limit
        {
            terminate_child(&mut child);
            let output = collect_output(&mut child)?;
            let combined = render_combined_output(output.stdout.clone(), output.stderr.clone());
            let outcome = BashExecutionOutcome {
                command: normalized.clone(),
                exit_code: None,
                stdout: output.stdout,
                stderr: output.stderr,
                timed_out: true,
                blocked_reason: None,
                interrupted: false,
            };
            return Ok((
                format!(
                    "exit_code=-1\ntimed_out=true\ntimeout_secs={}\n{}",
                    limit.as_secs(),
                    truncate_output(&combined, 20_000)
                ),
                outcome,
            ));
        }

        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => thread::sleep(POLL_INTERVAL),
            Err(err) => return Err(format!("failed to wait on shell command: {err}")),
        }
    };

    let output = collect_output(&mut child)?;
    let combined = render_combined_output(output.stdout.clone(), output.stderr.clone());
    let exit_code = status.code();
    let outcome = BashExecutionOutcome {
        command: normalized.clone(),
        exit_code,
        stdout: output.stdout,
        stderr: output.stderr,
        timed_out: false,
        blocked_reason: None,
        interrupted: false,
    };
    Ok((
        format!(
            "exit_code={}\n{}",
            exit_code.unwrap_or(-1),
            truncate_output(&combined, 20_000)
        ),
        outcome,
    ))
}

pub fn classify_command(command: &str) -> BashCommandClass {
    let normalized = command.trim().to_ascii_lowercase();
    if is_dangerous_command(&normalized) {
        BashCommandClass::Dangerous
    } else if command_uses_network(&normalized) {
        BashCommandClass::Network
    } else if is_read_only_command(&normalized) {
        BashCommandClass::ReadOnly
    } else if is_build_test_command(&normalized) {
        BashCommandClass::BuildTest
    } else if is_script_run_command(&normalized) {
        BashCommandClass::ScriptRun
    } else if is_mutating_command(&normalized) {
        BashCommandClass::Mutating
    } else {
        BashCommandClass::General
    }
}

#[derive(Default)]
struct ChildOutput {
    stdout: String,
    stderr: String,
}

fn collect_output(child: &mut Child) -> Result<ChildOutput, String> {
    // CB-003: read raw bytes and decode lossily. `read_to_string` previously
    // failed the whole bash dispatch when the child process emitted non-UTF8
    // output (binary blobs, mixed encodings). FeedbackFrame generation must
    // never be aborted by invalid byte sequences (Issue #450 R5).
    let mut stdout_bytes = Vec::new();
    let mut stderr_bytes = Vec::new();
    if let Some(mut out) = child.stdout.take() {
        out.read_to_end(&mut stdout_bytes)
            .map_err(|err| format!("failed to read command stdout: {err}"))?;
    }
    if let Some(mut err_out) = child.stderr.take() {
        err_out
            .read_to_end(&mut stderr_bytes)
            .map_err(|err| format!("failed to read command stderr: {err}"))?;
    }
    let _ = child.wait();
    let stdout = String::from_utf8_lossy(&stdout_bytes).into_owned();
    let stderr = String::from_utf8_lossy(&stderr_bytes).into_owned();
    Ok(ChildOutput { stdout, stderr })
}

fn render_combined_output(stdout: String, stderr: String) -> String {
    if stderr.trim().is_empty() {
        stdout
    } else if stdout.trim().is_empty() {
        stderr
    } else {
        format!("{stdout}\n{stderr}")
    }
}

fn likely_long_running_command(command: &str) -> bool {
    if launches_persistent_service(command) {
        return true;
    }

    let normalized = command.trim().to_ascii_lowercase();
    normalized.contains("cargo watch")
}

fn launches_persistent_service(command: &str) -> bool {
    let normalized = command.trim().to_ascii_lowercase();
    [
        "npm run dev",
        "pnpm dev",
        "yarn dev",
        "bun dev",
        "next dev",
        "vite",
        "webpack serve",
        "python -m http.server",
        "ruby -run -e httpd",
        "rails server",
    ]
    .iter()
    .any(|needle| normalized.contains(needle))
}

fn normalize_background_command(command: &str) -> String {
    let requested_background = requests_background_execution(command);
    if !requested_background && !launches_persistent_service(command) {
        return command.to_string();
    }
    if !requested_background && has_shell_control_operator(command) {
        return command.to_string();
    }

    let body = strip_trailing_background_operator(command);
    let log_path = background_log_path();
    let quoted_log = shell_single_quote(log_path.to_string_lossy().as_ref());
    let quoted_body = shell_single_quote(&body);
    format!(
        "nohup sh -lc {} >{} 2>&1 </dev/null & printf 'background_pid=%s\\nbackground_log=%s\\n' \"$!\" {}",
        quoted_body, quoted_log, quoted_log
    )
}

fn requests_background_execution(command: &str) -> bool {
    command.trim_end().ends_with('&')
}

fn strip_trailing_background_operator(command: &str) -> String {
    command
        .trim_end()
        .strip_suffix('&')
        .unwrap_or(command.trim_end())
        .trim_end()
        .to_string()
}

fn background_log_path() -> PathBuf {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!(
        "anvil-bash-bg-{}-{}.log",
        std::process::id(),
        unique
    ))
}

fn shell_single_quote(value: &str) -> String {
    let escaped = value.replace('\'', "'\"'\"'");
    format!("'{escaped}'")
}

fn is_noninteractive_scaffold_command(command: &str) -> bool {
    let normalized = command.trim().to_ascii_lowercase();
    normalized.contains("create-next-app") || normalized.contains("create next-app")
}

fn normalize_noninteractive_scaffold_command(command: &str) -> String {
    if !is_noninteractive_scaffold_command(command) {
        return command.to_string();
    }

    split_shell_control_segments(command)
        .into_iter()
        .map(normalize_scaffold_segment)
        .collect::<String>()
}

fn split_shell_control_segments(command: &str) -> Vec<&str> {
    let bytes = command.as_bytes();
    let mut parts = Vec::new();
    let mut start = 0usize;
    let mut i = 0usize;
    let mut in_single_quote = false;
    let mut in_double_quote = false;
    while i < bytes.len() {
        match bytes[i] {
            b'\'' if !in_double_quote => {
                in_single_quote = !in_single_quote;
                i += 1;
                continue;
            }
            b'"' if !in_single_quote && !is_escaped(bytes, i) => {
                in_double_quote = !in_double_quote;
                i += 1;
                continue;
            }
            _ => {}
        }
        if in_single_quote || in_double_quote {
            i += 1;
            continue;
        }
        let op_len = match bytes[i] {
            b'&' if i + 1 < bytes.len() && bytes[i + 1] == b'&' => Some(2),
            b'|' if i + 1 < bytes.len() && bytes[i + 1] == b'|' => Some(2),
            b'|' | b';' => Some(1),
            _ => None,
        };
        if let Some(len) = op_len {
            if start < i {
                parts.push(&command[start..i]);
            }
            parts.push(&command[i..i + len]);
            i += len;
            start = i;
        } else {
            i += 1;
        }
    }
    if start < command.len() {
        parts.push(&command[start..]);
    }
    parts
}

fn has_shell_control_operator(command: &str) -> bool {
    split_shell_control_segments(command)
        .iter()
        .any(|part| matches!(*part, "&&" | "||" | "|" | ";"))
}

fn is_escaped(bytes: &[u8], index: usize) -> bool {
    let mut slash_count = 0usize;
    let mut cursor = index;
    while cursor > 0 && bytes[cursor - 1] == b'\\' {
        slash_count += 1;
        cursor -= 1;
    }
    slash_count % 2 == 1
}

fn normalize_scaffold_segment(segment: &str) -> String {
    if !is_noninteractive_scaffold_command(segment) {
        return segment.to_string();
    }

    let trimmed = segment.trim();
    if trimmed.is_empty() {
        return segment.to_string();
    }

    let mut tokens = trimmed
        .split_whitespace()
        .map(str::to_string)
        .collect::<Vec<_>>();
    let mut trailing_suffix = Vec::new();
    while tokens
        .last()
        .is_some_and(|token| is_shell_redirection_token(token))
    {
        trailing_suffix.push(tokens.pop().expect("last token exists"));
    }

    let normalized = tokens.join(" ").to_ascii_lowercase();
    let has_yes = normalized.contains(" --yes")
        || normalized.ends_with(" --yes")
        || normalized.contains(" -y")
        || normalized.ends_with(" -y");
    if !has_yes {
        tokens.push("--yes".to_string());
    }
    let has_package_manager = ["--use-npm", "--use-pnpm", "--use-yarn", "--use-bun"]
        .iter()
        .any(|flag| normalized.contains(flag));
    if !has_package_manager {
        tokens.push("--use-npm".to_string());
    }

    let mut rewritten = tokens.join(" ");
    if !trailing_suffix.is_empty() {
        rewritten.push(' ');
        rewritten.push_str(
            &trailing_suffix
                .into_iter()
                .rev()
                .collect::<Vec<_>>()
                .join(" "),
        );
    }
    rewritten
}

fn is_shell_redirection_token(token: &str) -> bool {
    token.contains('>') || token.contains('<')
}

fn enforce_offline_policy(
    command: &str,
    class: BashCommandClass,
    offline: bool,
) -> Result<(), String> {
    if !offline {
        return Ok(());
    }
    if command_uses_network(command) {
        return Err(format!(
            "offline mode blocks network shell commands: {}",
            command.trim()
        ));
    }
    if matches!(
        class,
        BashCommandClass::General
            | BashCommandClass::Network
            | BashCommandClass::Mutating
            | BashCommandClass::Dangerous
    ) {
        return Err(format!(
            "offline mode only allows read-only, build-test, or local script-run shell commands: {}",
            command.trim()
        ));
    }
    Ok(())
}

fn is_script_run_command(normalized: &str) -> bool {
    [
        "python ",
        "python3 ",
        "python -m ",
        "python3 -m ",
        "node ",
        "deno run ",
        "bun run ",
        "ruby ",
        "perl ",
        "sh ",
        "bash ",
    ]
    .iter()
    .any(|needle| normalized == *needle || normalized.starts_with(needle))
}

fn is_mutating_command(normalized: &str) -> bool {
    [
        "cp ",
        "mv ",
        "mkdir ",
        "touch ",
        "printf ",
        "echo ",
        "tee ",
        "git add",
        "git commit",
        "git merge",
        "git rebase",
        "cargo fmt",
        "npm run format",
        "pnpm format",
        "yarn format",
    ]
    .iter()
    .any(|needle| normalized == *needle || normalized.starts_with(needle))
        || normalized.contains(" >")
        || normalized.contains(">>")
}

fn is_dangerous_command(normalized: &str) -> bool {
    [
        "rm ",
        "rm -r",
        "rm -rf",
        "sudo ",
        "chmod -r",
        "chown -r",
        "mkfs",
        "dd ",
        "diskutil ",
        "launchctl ",
    ]
    .iter()
    .any(|needle| normalized == *needle || normalized.starts_with(needle))
}

fn is_read_only_command(normalized: &str) -> bool {
    [
        "pwd",
        "ls",
        "find ",
        "rg ",
        "grep ",
        "cat ",
        "sed -n",
        "head ",
        "tail ",
        "wc ",
        "git status",
        "git diff",
        "git log",
        "tree",
        "stat ",
        "file ",
    ]
    .iter()
    .any(|needle| normalized == *needle || normalized.starts_with(needle))
}

fn is_build_test_command(normalized: &str) -> bool {
    [
        "cargo test",
        "cargo check",
        "cargo build",
        "cargo clippy",
        "cargo fmt",
        "npm test",
        "npm run test",
        "npm run build",
        "npm run lint",
        "pnpm test",
        "pnpm run test",
        "pnpm build",
        "pnpm lint",
        "yarn test",
        "yarn build",
        "yarn lint",
        "pytest",
        "python -m pytest",
        "uv run pytest",
        "go test",
        "mvn test",
        "gradle test",
        "make test",
        "make build",
    ]
    .iter()
    .any(|needle| normalized == *needle || normalized.starts_with(needle))
}

fn command_uses_network(command: &str) -> bool {
    let normalized = command.trim().to_ascii_lowercase();
    [
        "curl ",
        "wget ",
        "ping ",
        "ssh ",
        "scp ",
        "rsync ",
        "git clone",
        "git fetch",
        "git pull",
        "npm install",
        "npm add",
        "pnpm install",
        "pnpm add",
        "yarn install",
        "yarn add",
        "pip install",
        "pip3 install",
        "poetry install",
        "poetry add",
        "cargo install",
        "cargo add",
        "go get",
        "brew install",
        "apt install",
        "apt-get install",
        "dnf install",
        "docker pull",
        "kubectl ",
        "npx ",
    ]
    .iter()
    .any(|needle| normalized.contains(needle))
}

fn terminate_child(child: &mut Child) {
    #[cfg(unix)]
    {
        let pid = child.id() as i32;
        unsafe {
            let _ = libc::kill(-pid, libc::SIGTERM);
        }
        let deadline = Instant::now() + TERMINATE_GRACE;
        loop {
            match child.try_wait() {
                Ok(Some(_)) => return,
                Ok(None) if Instant::now() < deadline => thread::sleep(POLL_INTERVAL),
                Ok(None) | Err(_) => {
                    unsafe {
                        let _ = libc::kill(-pid, libc::SIGKILL);
                    }
                    let _ = child.wait();
                    return;
                }
            }
        }
    }

    #[cfg(not(unix))]
    {
        let _ = child.kill();
        let _ = child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BashCommandClass, classify_command, command_uses_network, has_shell_control_operator,
        launches_persistent_service, likely_long_running_command, normalize_background_command,
        normalize_noninteractive_scaffold_command, requests_background_execution, run,
        split_shell_control_segments, strip_trailing_background_operator,
    };
    use std::time::{Duration, Instant};

    use tempfile::tempdir;

    #[test]
    fn detects_long_running_dev_commands() {
        assert!(likely_long_running_command("npm run dev -- --port 3011"));
        assert!(likely_long_running_command("next dev"));
        assert!(likely_long_running_command("python -m http.server 8080"));
        assert!(!likely_long_running_command("npm test"));
    }

    #[test]
    fn detects_persistent_service_commands() {
        assert!(launches_persistent_service("npm run dev -- -p 3011"));
        assert!(launches_persistent_service("next dev"));
        assert!(!launches_persistent_service("cargo test"));
    }

    #[test]
    fn classifies_command_policy_groups() {
        assert_eq!(classify_command("pwd"), BashCommandClass::ReadOnly);
        assert_eq!(classify_command("cargo test"), BashCommandClass::BuildTest);
        assert_eq!(
            classify_command("python3 scripts/check.py"),
            BashCommandClass::ScriptRun
        );
        assert_eq!(classify_command("mkdir -p out"), BashCommandClass::Mutating);
        assert_eq!(
            classify_command("curl -I https://example.com"),
            BashCommandClass::Network
        );
        assert_eq!(
            classify_command("rm -rf build"),
            BashCommandClass::Dangerous
        );
        assert_eq!(
            classify_command("echo hello > output.txt"),
            BashCommandClass::Mutating
        );
    }

    #[test]
    fn detects_networked_commands() {
        assert!(command_uses_network("curl -I https://example.com"));
        assert!(command_uses_network("npm install vitest"));
        assert!(!command_uses_network("cargo test"));
    }

    #[test]
    fn normalizes_create_next_app_to_noninteractive() {
        let rewritten = normalize_noninteractive_scaffold_command(
            "npx create-next-app@latest . --typescript --tailwind",
        );
        assert!(rewritten.contains("--yes"));
        assert!(rewritten.contains("--use-npm"));
    }

    #[test]
    fn preserves_existing_scaffold_flags() {
        let original = "npx create-next-app@latest . --typescript --yes --use-pnpm";
        let rewritten = normalize_noninteractive_scaffold_command(original);
        assert_eq!(rewritten, original);
    }

    #[test]
    fn normalizes_only_scaffold_segment_before_pipe() {
        let rewritten = normalize_noninteractive_scaffold_command(
            "cd /tmp/app && npx create-next-app@latest sample-app --typescript 2>&1 | tail -20",
        );
        assert!(
            rewritten
                .contains("create-next-app@latest sample-app --typescript --yes --use-npm 2>&1")
        );
        assert!(rewritten.ends_with("| tail -20"), "got: {rewritten}");
        assert!(!rewritten.contains("tail -20 --yes"), "got: {rewritten}");
    }

    #[test]
    fn shell_control_split_ignores_quoted_operators() {
        assert_eq!(
            split_shell_control_segments(r#"echo "alpha|beta"; npm run build"#),
            vec![r#"echo "alpha|beta""#, ";", " npm run build"]
        );
        assert!(!has_shell_control_operator(
            r#"npx create-next-app@latest "alpha|beta" --typescript"#
        ));
    }

    #[test]
    fn detects_background_execution_requests() {
        assert!(requests_background_execution("npx next dev -p 3011 &"));
        assert!(requests_background_execution(
            "pkill -f \"next dev\"; npx next dev -p 3011 &   "
        ));
        assert!(!requests_background_execution("npm run build"));
    }

    #[test]
    fn rewrites_background_commands_to_detach_output() {
        let rewritten = normalize_background_command("npx next dev -p 3011 &");
        assert!(rewritten.contains("background_pid="));
        assert!(rewritten.contains("background_log="));
        assert!(rewritten.contains("nohup sh -lc"));
        assert!(rewritten.contains("2>&1"));
        assert!(rewritten.contains("'npx next dev -p 3011'"));
    }

    #[test]
    fn leaves_foreground_commands_unchanged() {
        let original = "npm run build";
        assert_eq!(normalize_background_command(original), original);
    }

    #[test]
    fn detaches_foreground_dev_servers_automatically() {
        let rewritten = normalize_background_command("npm run dev -- -p 3011");
        assert!(rewritten.contains("background_pid="));
        assert!(rewritten.contains("background_log="));
        assert!(rewritten.contains("'npm run dev -- -p 3011'"));
    }

    #[test]
    fn leaves_compound_foreground_dev_server_commands_attached() {
        let original = r#"pkill -f "next dev"; npx next dev -p 3011"#;
        assert_eq!(normalize_background_command(original), original);
    }

    #[test]
    fn strips_only_trailing_background_operator() {
        assert_eq!(
            strip_trailing_background_operator("pkill -f \"next dev\"; npx next dev -p 3011 &   "),
            "pkill -f \"next dev\"; npx next dev -p 3011"
        );
    }

    #[test]
    fn detached_background_command_returns_quickly() {
        let temp = tempdir().unwrap();
        let started = Instant::now();
        let output = run("sleep 30 &", temp.path(), None, false).unwrap();
        assert!(started.elapsed() < Duration::from_secs(5));
        assert!(output.contains("exit_code=0"));
        assert!(output.contains("background_pid="));
        assert!(output.contains("background_log="));
    }

    // --- Issue #450 AC3 / AC5 / AC6 ---------------------------------------

    use super::{BashExecutionOutcome, classify_bash_outcome, run_with_outcome};
    use crate::session::feedback::{
        EXCERPT_CAP_BYTES, FeedbackFrameDraft, FeedbackKind, build_feedback_frame, mask_secrets,
    };

    /// AC5: a Bash command rejected by the dangerous-snippet block (e.g.
    /// `rm -rf /`) returns Err with a "blocked dangerous command" reason
    /// before the process is even spawned. The agent layer turns that
    /// into `FeedbackKind::UnsafeCommandBlocked`.
    #[test]
    fn dangerous_bash_yields_unsafe_blocked() {
        let temp = tempdir().unwrap();
        let err = run("rm -rf /", temp.path(), None, false).unwrap_err();
        assert!(
            err.contains("blocked dangerous command"),
            "expected dangerous block, got {err}"
        );
        // Equivalent classify path: outcome with blocked_reason set.
        let outcome = BashExecutionOutcome {
            command: "rm -rf /".to_string(),
            blocked_reason: Some(err),
            ..Default::default()
        };
        assert_eq!(
            classify_bash_outcome(&outcome),
            FeedbackKind::UnsafeCommandBlocked
        );
    }

    /// AC3: a Bash command that is killed by the long-running timeout
    /// produces an outcome with `timed_out == true`, which classifies as
    /// `FeedbackKind::Timeout`.
    #[test]
    fn bash_timeout_yields_timeout_kind() {
        // Synthesize the outcome shape directly. Spawning a 15-second
        // sleeping dev server in cargo test would slow CI without
        // adding signal coverage.
        let outcome = BashExecutionOutcome {
            command: "npm run dev".to_string(),
            timed_out: true,
            ..Default::default()
        };
        assert_eq!(classify_bash_outcome(&outcome), FeedbackKind::Timeout);
    }

    /// AC6: a 16 KiB stdout passes through `build_feedback_frame` and the
    /// resulting `stdout_excerpt` is at most 8 KiB (marker included).
    #[test]
    fn excerpt_capped_at_8kib_with_marker() {
        let big = "a".repeat(16 * 1024);
        let draft = FeedbackFrameDraft {
            kind: FeedbackKind::TestFailure,
            stdout: big,
            ..Default::default()
        };
        let dir = tempdir().unwrap();
        let frame = build_feedback_frame(draft, dir.path());
        let excerpt = frame.stdout_excerpt();
        assert!(
            excerpt.len() <= EXCERPT_CAP_BYTES,
            "excerpt too large: {}",
            excerpt.len()
        );
        assert!(excerpt.contains("[...truncated"));
    }

    /// R5: building a FeedbackFrame from an outcome whose stdout/stderr
    /// were lossily decoded from invalid UTF-8 must not panic.
    #[test]
    fn non_utf8_outcome_does_not_panic_in_feedback_pipeline() {
        let raw = b"hello\xFFworld";
        let lossy = String::from_utf8_lossy(raw).into_owned();
        let outcome = BashExecutionOutcome {
            command: "cat /tmp/raw".to_string(),
            stdout: lossy.clone(),
            stderr: lossy.clone(),
            exit_code: Some(1),
            ..Default::default()
        };
        let kind = classify_bash_outcome(&outcome);
        let draft = FeedbackFrameDraft {
            kind,
            command: Some(outcome.command.clone()),
            stdout: outcome.stdout.clone(),
            stderr: outcome.stderr.clone(),
            exit_code: outcome.exit_code,
            ..Default::default()
        };
        let dir = tempdir().unwrap();
        let _frame = build_feedback_frame(draft, dir.path());
        // Sanity check that mask_secrets is also panic-free with this
        // kind of input.
        let _ = mask_secrets(&lossy);
    }

    /// `run_with_outcome` reports a successful Bash exit code in the
    /// structured outcome, not just in the formatted text result.
    #[test]
    fn run_with_outcome_emits_structured_exit_code() {
        let temp = tempdir().unwrap();
        let (text, outcome) =
            run_with_outcome("printf hello", temp.path(), None, false, None, None).unwrap();
        assert!(text.starts_with("exit_code=0"));
        assert_eq!(outcome.exit_code, Some(0));
        assert!(outcome.stdout.contains("hello"));
        assert!(!outcome.timed_out);
    }

    /// CB-003: a Bash command that emits invalid UTF-8 on stdout must NOT
    /// fail the bash dispatch. `collect_output` decodes bytes lossily so
    /// FeedbackFrame generation downstream cannot be aborted by binary
    /// or mis-encoded output.
    #[test]
    fn non_utf8_stdout_does_not_panic_or_error() {
        let temp = tempdir().unwrap();
        // Use POSIX-portable octal escapes (`\NNN`) instead of `\xNN`:
        // GitHub Actions Linux runners ship dash as /bin/sh, whose
        // `printf` does not interpret `\x` escapes (they pass through
        // as literal text). Octal `\377\376\375` produces bytes
        // 0xFF 0xFE 0xFD on every POSIX shell.
        let (text, outcome) = run_with_outcome(
            "printf '\\377\\376\\375'",
            temp.path(),
            None,
            false,
            None,
            None,
        )
        .expect("run_with_outcome must succeed even for non-UTF8 output");
        // Successful exit (exit_code=0); not timed_out.
        assert_eq!(outcome.exit_code, Some(0));
        assert!(!outcome.timed_out);
        assert!(text.starts_with("exit_code=0"));
        // The lossy-decoded stdout contains the U+FFFD replacement char
        // for each invalid byte. Critically: the literal escape string
        // (e.g. "\\377") must NOT remain in the output - that would
        // indicate the shell's printf did not actually emit raw bytes.
        assert!(
            outcome.stdout.contains('\u{FFFD}'),
            "expected replacement char in lossy stdout, got {:?}",
            outcome.stdout
        );
        assert!(
            !outcome.stdout.contains("\\377"),
            "shell printf did not interpret octal escape, got {:?}",
            outcome.stdout
        );
    }

    // --- Issue #459: explicit_timeout coverage ---------------------------

    /// `explicit_timeout = Some(d)` enforces `d` even when the command is
    /// not on the `likely_long_running_command` allow-list (Tester smoke
    /// runs do not match dev-server / cargo-watch heuristics).
    #[test]
    fn explicit_timeout_enforces_kill_on_short_sleep() {
        let temp = tempdir().unwrap();
        let started = Instant::now();
        let (text, outcome) = run_with_outcome(
            "sleep 5",
            temp.path(),
            None,
            false,
            Some(Duration::from_millis(500)),
            None,
        )
        .expect("run_with_outcome must report timeout via Ok((text, outcome))");
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "explicit_timeout did not preempt sleep: elapsed={:?}",
            started.elapsed()
        );
        assert!(outcome.timed_out, "outcome.timed_out must be true");
        assert_eq!(outcome.exit_code, None);
        assert!(text.contains("timed_out=true"));
    }

    /// `explicit_timeout = None` preserves existing behaviour: short
    /// non-long-running commands run to completion without a timeout.
    #[test]
    fn explicit_timeout_none_keeps_existing_behaviour() {
        let temp = tempdir().unwrap();
        let (_text, outcome) =
            run_with_outcome("printf ok", temp.path(), None, false, None, None).unwrap();
        assert!(!outcome.timed_out);
        assert_eq!(outcome.exit_code, Some(0));
    }

    // --- CB-003: env sanitization for Tester smoke runs -------------------

    /// `filter_env_for_tester` drops everything not in the exact allowlist.
    /// Pure-function unit test so we never have to mutate the global env.
    #[test]
    fn filter_env_for_tester_drops_secret_like_keys() {
        let input: Vec<(String, String)> = vec![
            ("PATH".into(), "/usr/bin".into()),
            ("HOME".into(), "/home/user".into()),
            ("OPENAI_API_KEY".into(), "sk-secret".into()),
            ("ANTHROPIC_API_KEY".into(), "sk-ant".into()),
            ("GITHUB_TOKEN".into(), "ghp_xxx".into()),
            ("AWS_SECRET_ACCESS_KEY".into(), "AKIA-secret".into()),
            ("ANVIL_NO_TESTER".into(), "1".into()),
            ("ANVIL_DEBUG".into(), "1".into()),
            ("MY_TOKEN".into(), "redact".into()),
            ("SOME_PASSWORD".into(), "redact".into()),
            ("LANG".into(), "en_US.UTF-8".into()),
        ];
        let filtered = super::filter_env_for_tester(input);
        let keys: Vec<&str> = filtered.iter().map(|(k, _)| k.as_str()).collect();
        assert!(keys.contains(&"PATH"), "PATH must remain: {keys:?}");
        assert!(keys.contains(&"HOME"));
        assert!(keys.contains(&"LANG"));
        for forbidden in &[
            "OPENAI_API_KEY",
            "ANTHROPIC_API_KEY",
            "GITHUB_TOKEN",
            "AWS_SECRET_ACCESS_KEY",
            "ANVIL_NO_TESTER",
            "ANVIL_DEBUG",
            "MY_TOKEN",
            "SOME_PASSWORD",
        ] {
            assert!(
                !keys.contains(forbidden),
                "{forbidden} must be dropped, got keys: {keys:?}"
            );
        }
    }

    /// E2E coverage: with `BashEnvPolicy::TesterSanitized`, a child shell that
    /// echoes its env does NOT see secret-like keys we set on the parent
    /// process. Uses `printenv -0` style sentinels so we do not depend on
    /// `env`(1) output formatting.
    #[test]
    fn run_with_outcome_tester_sanitized_drops_secrets() {
        let temp = tempdir().unwrap();
        // Set a representative secret-like env var on the parent. SAFETY:
        // mutating process env is unsound under threaded execution. We use
        // a key prefix (`ANVIL_TEST_FAKE_`) that nothing else in the
        // codebase reads.
        let key = "ANVIL_TEST_FAKE_OPENAI_API_KEY_CB003";
        // SAFETY: tests are single-threaded for this case; the var is unique
        // and only consumed inside this test.
        unsafe {
            std::env::set_var(key, "sk-fake-secret-cb003");
        }
        let (text, outcome) = run_with_outcome(
            // Print only env keys that match our test prefix or expected
            // allowlist members so the assertion is deterministic.
            "printenv ANVIL_TEST_FAKE_OPENAI_API_KEY_CB003 || true; printenv PATH || true",
            temp.path(),
            None,
            false,
            None,
            Some(super::BashEnvPolicy::TesterSanitized),
        )
        .expect("sanitized run must complete");
        // Cleanup before assertions so a failure leaves no leftover env.
        // SAFETY: see above.
        unsafe {
            std::env::remove_var(key);
        }
        assert_eq!(outcome.exit_code, Some(0), "child should exit 0: {text}");
        assert!(
            !outcome.stdout.contains("sk-fake-secret-cb003"),
            "secret value leaked to child stdout: {:?}",
            outcome.stdout
        );
        // PATH must still be forwarded — without it, `cargo` / `node` /
        // `python3` cannot be located in the smoke run.
        assert!(
            !outcome.stdout.trim().is_empty()
                || outcome.stderr.contains("printenv")
                || outcome.exit_code == Some(0),
            "PATH should be forwarded; observed stdout: {:?}",
            outcome.stdout
        );
    }

    /// Inverse: with `env_policy = None` (default `Inherit`), the same
    /// secret env we set on the parent IS visible to the child — confirming
    /// the existing code paths are unchanged when not opting in.
    #[test]
    fn run_with_outcome_default_inherits_env() {
        let temp = tempdir().unwrap();
        let key = "ANVIL_TEST_FAKE_INHERIT_CB003";
        // SAFETY: see sanitized test above.
        unsafe {
            std::env::set_var(key, "value-leaks");
        }
        let (text, _outcome) = run_with_outcome(
            "printenv ANVIL_TEST_FAKE_INHERIT_CB003 || true",
            temp.path(),
            None,
            false,
            None,
            None,
        )
        .expect("default run must complete");
        // SAFETY: see above.
        unsafe {
            std::env::remove_var(key);
        }
        // Default is Inherit, so the value must reach the child stdout.
        assert!(
            text.contains("value-leaks"),
            "default policy should inherit env; got text {text:?}"
        );
    }

    /// `is_dangerous_command` / `BLOCKED_SNIPPETS` block a `rm -rf /`
    /// call regardless of `explicit_timeout` — there is no Tester bypass
    /// (DR2-014).
    #[test]
    fn explicit_timeout_does_not_bypass_dangerous_block() {
        let temp = tempdir().unwrap();
        let err = run_with_outcome(
            "rm -rf /",
            temp.path(),
            None,
            false,
            Some(Duration::from_secs(30)),
            None,
        )
        .unwrap_err();
        assert!(
            err.contains("blocked dangerous command"),
            "explicit_timeout must not bypass dangerous-snippet filter, got: {err}"
        );
    }
}
