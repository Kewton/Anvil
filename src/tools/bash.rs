use std::io::Read;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use crate::tools::registry::truncate_output;

const BLOCKED_SNIPPETS: &[&str] = &[
    "rm -rf /",
    "rm -rf ~",
    "mkfs",
    "dd if=",
    ":(){",
    "rm -rf .anvil",
];
const LONG_RUNNING_TIMEOUT: Duration = Duration::from_secs(15);
const TERMINATE_GRACE: Duration = Duration::from_secs(2);
const POLL_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BashCommandClass {
    ReadOnly,
    BuildTest,
    General,
}

pub fn run(
    command: &str,
    cwd: &std::path::Path,
    cancel_flag: Option<&Arc<AtomicBool>>,
    offline: bool,
) -> Result<String, String> {
    let command = normalize_background_command(&normalize_noninteractive_scaffold_command(command));
    for snippet in BLOCKED_SNIPPETS {
        if command.contains(snippet) {
            return Err(format!("blocked dangerous command fragment: {snippet}"));
        }
    }
    let class = classify_command(&command);
    enforce_offline_policy(&command, class, offline)?;

    let mut cmd = Command::new("sh");
    cmd.args(["-lc", &command])
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if is_noninteractive_scaffold_command(&command) {
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
    let timeout = likely_long_running_command(&command).then_some(LONG_RUNNING_TIMEOUT);
    let started = Instant::now();

    let status = loop {
        if cancel_flag.is_some_and(|flag| flag.load(Ordering::SeqCst)) {
            terminate_child(&mut child);
            let output = collect_output(&mut child)?;
            let combined = render_combined_output(output.stdout, output.stderr);
            return Ok(format!(
                "exit_code=-1\ninterrupted=true\n{}",
                truncate_output(&combined, 20_000)
            ));
        }

        if let Some(limit) = timeout
            && started.elapsed() >= limit
        {
            terminate_child(&mut child);
            let output = collect_output(&mut child)?;
            let combined = render_combined_output(output.stdout, output.stderr);
            return Ok(format!(
                "exit_code=-1\ntimed_out=true\ntimeout_secs={}\n{}",
                limit.as_secs(),
                truncate_output(&combined, 20_000)
            ));
        }

        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => thread::sleep(POLL_INTERVAL),
            Err(err) => return Err(format!("failed to wait on shell command: {err}")),
        }
    };

    let output = collect_output(&mut child)?;
    let combined = render_combined_output(output.stdout, output.stderr);
    Ok(format!(
        "exit_code={}\n{}",
        status.code().unwrap_or(-1),
        truncate_output(&combined, 20_000)
    ))
}

pub fn classify_command(command: &str) -> BashCommandClass {
    let normalized = command.trim().to_ascii_lowercase();
    if is_read_only_command(&normalized) {
        BashCommandClass::ReadOnly
    } else if is_build_test_command(&normalized) {
        BashCommandClass::BuildTest
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
    let mut stdout = String::new();
    let mut stderr = String::new();
    if let Some(mut out) = child.stdout.take() {
        out.read_to_string(&mut stdout)
            .map_err(|err| format!("failed to read command stdout: {err}"))?;
    }
    if let Some(mut err_out) = child.stderr.take() {
        err_out
            .read_to_string(&mut stderr)
            .map_err(|err| format!("failed to read command stderr: {err}"))?;
    }
    let _ = child.wait();
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
    if matches!(class, BashCommandClass::General) {
        return Err(format!(
            "offline mode only allows read-only or build-test shell commands: {}",
            command.trim()
        ));
    }
    Ok(())
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
            classify_command("echo hello > output.txt"),
            BashCommandClass::General
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
}
