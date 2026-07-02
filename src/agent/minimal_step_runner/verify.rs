use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::{PlanStep, VerifyExpectedResult};

const VERIFY_FAILURE_HEAD_LINES: usize = 6;
const VERIFY_FAILURE_TAIL_LINES: usize = 20;
const VERIFY_FAILURE_CONTEXT_BEFORE: usize = 3;
const VERIFY_FAILURE_CONTEXT_AFTER: usize = 8;
const VERIFY_FAILURE_SOURCE_CONTEXT: usize = 3;
const VERIFY_FAILURE_MAX_CHARS: usize = 6_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FailureKind {
    DependencyMissing,
    VerifierUnavailable,
}

impl FailureKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::DependencyMissing => "dependency_missing",
            Self::VerifierUnavailable => "verifier_unavailable",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct VerificationReport {
    pub(super) success: bool,
    pub(super) failures: Vec<String>,
}

pub(super) fn verify_step(work_root: &Path, step: &PlanStep) -> VerificationReport {
    let mut failures = Vec::new();
    for path in missing_expected_paths(work_root, step) {
        failures.push(format!("missing expected path: {path}"));
    }
    for command in &step.verify {
        if let Some(precondition_failure) = verifier_precondition_failure(work_root, command) {
            failures.push(precondition_failure);
            continue;
        }
        match (step.expected_result, run_verify_command(work_root, command)) {
            (VerifyExpectedResult::Pass, Ok(())) => {}
            (VerifyExpectedResult::Pass, Err(err)) => {
                failures.push(format!("verify failed `{command}`: {err}"));
            }
            (VerifyExpectedResult::Fail, Ok(())) => {
                failures.push(format!("verify unexpectedly passed `{command}`"));
            }
            (VerifyExpectedResult::Fail, Err(_)) => {}
        }
    }
    VerificationReport {
        success: failures.is_empty(),
        failures,
    }
}

fn verifier_precondition_failure(work_root: &Path, command: &str) -> Option<String> {
    if command == "npm run build" && nextjs_build_requires_missing_next_binary(work_root) {
        return Some(format!(
            "{}: {}: npm run build requires node_modules/.bin/next, but it is missing. Install dependencies with npm install/npm ci when allowed, or stop as dependency_missing; do not change scripts.build away from next build to fake success.",
            FailureKind::DependencyMissing.as_str(),
            FailureKind::VerifierUnavailable.as_str()
        ));
    }
    None
}

fn nextjs_build_requires_missing_next_binary(work_root: &Path) -> bool {
    let package_path = work_root.join("package.json");
    let Ok(raw) = std::fs::read_to_string(package_path) else {
        return false;
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return false;
    };
    let build = json
        .pointer("/scripts/build")
        .and_then(|value| value.as_str())
        .unwrap_or("");
    if build != "next build" {
        return false;
    }
    let has_next_dependency = ["dependencies", "devDependencies"].iter().any(|section| {
        json.get(section)
            .and_then(|value| value.get("next"))
            .is_some()
    });
    has_next_dependency && !work_root.join("node_modules/.bin/next").is_file()
}

pub(super) fn missing_expected_paths(work_root: &Path, step: &PlanStep) -> Vec<String> {
    let mut missing = Vec::new();
    for path in &step.expected_paths {
        let Some(normalized) = normalize_relative_path(Path::new(path)) else {
            missing.push(path.clone());
            continue;
        };
        if !work_root.join(normalized).exists() {
            missing.push(path.clone());
        }
    }
    missing
}

pub(super) fn early_success_paths_for_step(work_root: &Path, step: &PlanStep) -> Vec<String> {
    if missing_expected_paths(work_root, step).is_empty() {
        Vec::new()
    } else {
        step.expected_paths.clone()
    }
}

pub(super) fn validate_verify_command(command: &str) -> Result<(), String> {
    if command.trim().is_empty() {
        return Err("verify command must not be empty".to_string());
    }
    if command.len() > 240 {
        return Err(format!("verify command is too long: {command}"));
    }
    let forbidden = ["&&", "||", ";", "|", ">", "<", "`", "$(", "\n", "\r"];
    if forbidden.iter().any(|needle| command.contains(needle)) {
        return Err(format!(
            "verify command contains shell control syntax: {command}"
        ));
    }
    let parts = command.split_whitespace().collect::<Vec<_>>();
    let allowed = match parts.as_slice() {
        ["npm", "run", "build"] | ["npm", "run", "test"] | ["npm", "test"] => true,
        ["cargo", "check"] | ["cargo", "test"] => true,
        ["cargo", "check", rest @ ..] | ["cargo", "test", rest @ ..] => rest
            .iter()
            .all(|part| safe_command_argument(part) && !part.eq_ignore_ascii_case("--release")),
        ["python", "-m", "py_compile", rest @ ..] | ["python3", "-m", "py_compile", rest @ ..] => {
            !rest.is_empty() && rest.iter().all(|part| safe_command_argument(part))
        }
        ["python", "-m", "pytest", rest @ ..] | ["python3", "-m", "pytest", rest @ ..] => {
            rest.iter().all(|part| safe_command_argument(part))
        }
        ["python", script, rest @ ..] | ["python3", script, rest @ ..] => {
            safe_python_script_path(script) && rest.iter().all(|part| safe_command_argument(part))
        }
        ["pytest", rest @ ..] => rest.iter().all(|part| safe_command_argument(part)),
        ["node", "--check", rest @ ..] => {
            !rest.is_empty() && rest.iter().all(|part| safe_node_check_path(part))
        }
        ["cat", rest @ ..] => {
            !rest.is_empty() && rest.iter().all(|part| safe_command_argument(part))
        }
        _ => false,
    };
    if allowed {
        Ok(())
    } else {
        Err(format!(
            "verify command is not in the safe allowlist: {command}"
        ))
    }
}

pub(super) fn normalize_relative_path(path: &Path) -> Option<PathBuf> {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::Normal(part) => normalized.push(part),
            std::path::Component::ParentDir => {
                if !normalized.pop() {
                    return None;
                }
            }
            std::path::Component::RootDir | std::path::Component::Prefix(_) => return None,
        }
    }
    Some(normalized)
}

fn run_verify_command(work_root: &Path, command: &str) -> Result<(), String> {
    validate_verify_command(command)?;
    let output = Command::new("sh")
        .arg("-lc")
        .arg(command)
        .current_dir(work_root)
        .output()
        .map_err(|err| format!("failed to run: {err}"))?;
    if output.status.success() {
        return Ok(());
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{stdout}{stderr}");
    Err(verifier_failure_excerpt(work_root, &combined))
}

fn safe_python_script_path(value: &str) -> bool {
    safe_command_argument(value)
        && value.ends_with(".py")
        && !value.starts_with('-')
        && !Path::new(value).is_absolute()
}

fn safe_node_check_path(value: &str) -> bool {
    safe_command_argument(value)
        && !value.starts_with('-')
        && !Path::new(value).is_absolute()
        && (value.ends_with(".js") || value.ends_with(".mjs") || value.ends_with(".cjs"))
}

fn safe_command_argument(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '/' | '_' | '-' | '=' | ':'))
        && !value.contains("..")
}

fn verifier_failure_excerpt(work_root: &Path, value: &str) -> String {
    if value.trim().is_empty() {
        "command exited non-zero with no output".to_string()
    } else {
        let lines = value.lines().collect::<Vec<_>>();
        let selected = selected_failure_lines(&lines);
        let rendered = render_selected_lines(&lines, &selected);
        let source_excerpt = source_excerpts_for_output(work_root, &lines);
        let mut excerpt = String::from("diagnostic excerpt:\n");
        excerpt.push_str(&rendered);
        if !source_excerpt.is_empty() {
            excerpt.push_str("\n\nrelated source excerpt:\n");
            excerpt.push_str(&source_excerpt);
        }
        truncate_chars(&excerpt, VERIFY_FAILURE_MAX_CHARS)
    }
}

fn selected_failure_lines(lines: &[&str]) -> BTreeSet<usize> {
    let mut selected = BTreeSet::new();
    for index in 0..lines.len().min(VERIFY_FAILURE_HEAD_LINES) {
        selected.insert(index);
    }
    let tail_start = lines.len().saturating_sub(VERIFY_FAILURE_TAIL_LINES);
    for index in tail_start..lines.len() {
        selected.insert(index);
    }
    for (index, line) in lines.iter().enumerate() {
        if is_diagnostic_line(line) || parse_source_location(line).is_some() {
            let start = index.saturating_sub(VERIFY_FAILURE_CONTEXT_BEFORE);
            let end = (index + VERIFY_FAILURE_CONTEXT_AFTER).min(lines.len().saturating_sub(1));
            for selected_index in start..=end {
                selected.insert(selected_index);
            }
        }
    }
    selected
}

fn render_selected_lines(lines: &[&str], selected: &BTreeSet<usize>) -> String {
    let mut rendered = Vec::new();
    let mut previous = None;
    for &index in selected {
        if index >= lines.len() {
            continue;
        }
        if let Some(previous_index) = previous {
            if index > previous_index + 1 {
                rendered.push(format!(
                    "... omitted {} lines ...",
                    index - previous_index - 1
                ));
            }
        }
        rendered.push(lines[index].to_string());
        previous = Some(index);
    }
    rendered.join("\n")
}

fn is_diagnostic_line(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    lower.contains("type error:")
        || lower.contains("syntaxerror")
        || lower.contains("referenceerror")
        || lower.contains("assertionerror")
        || lower.contains("modulenotfounderror")
        || lower.contains("module not found")
        || lower.contains("cannot find module")
        || lower.contains("failed to compile")
        || lower.contains("traceback")
        || lower.contains("panicked at")
        || lower.starts_with("error:")
        || lower.contains("error[")
        || lower == "failures:"
        || lower.contains(" failed")
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct SourceLocation {
    path: PathBuf,
    line: usize,
}

fn source_excerpts_for_output(work_root: &Path, lines: &[&str]) -> String {
    let mut locations = BTreeSet::new();
    for line in lines {
        if let Some(location) = parse_source_location(line) {
            if work_root.join(&location.path).is_file() {
                locations.insert(location);
            }
        }
        if locations.len() >= 3 {
            break;
        }
    }
    locations
        .into_iter()
        .filter_map(|location| render_source_excerpt(work_root, &location))
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn render_source_excerpt(work_root: &Path, location: &SourceLocation) -> Option<String> {
    let raw = std::fs::read_to_string(work_root.join(&location.path)).ok()?;
    let lines = raw.lines().collect::<Vec<_>>();
    if lines.is_empty() || location.line == 0 {
        return None;
    }
    let start = location
        .line
        .saturating_sub(VERIFY_FAILURE_SOURCE_CONTEXT + 1);
    let end = (location.line + VERIFY_FAILURE_SOURCE_CONTEXT).min(lines.len());
    let mut rendered = vec![format!("{}:{}-{}", location.path.display(), start + 1, end)];
    for index in start..end {
        let marker = if index + 1 == location.line { ">" } else { " " };
        rendered.push(format!("{marker} {:>4} | {}", index + 1, lines[index]));
    }
    Some(rendered.join("\n"))
}

fn parse_source_location(line: &str) -> Option<SourceLocation> {
    line.split(|ch: char| ch.is_whitespace() || matches!(ch, '`' | '"' | '\'' | '(' | ')' | ','))
        .find_map(parse_source_location_token)
}

fn parse_source_location_token(token: &str) -> Option<SourceLocation> {
    let token = token.trim_matches(|ch: char| matches!(ch, ':' | ';'));
    let parts = token.rsplitn(3, ':').collect::<Vec<_>>();
    if parts.len() < 2 {
        return None;
    }
    let last = parts[0];
    let second = parts[1];
    let (line, path_part) = if let Ok(line) = second.parse::<usize>() {
        let path = parts.get(2).copied()?;
        (line, path)
    } else if let Ok(line) = last.parse::<usize>() {
        (line, second)
    } else {
        return None;
    };
    if line == 0 || !looks_like_source_path(path_part) {
        return None;
    }
    let path_part = path_part.strip_prefix("./").unwrap_or(path_part);
    let normalized = normalize_relative_path(Path::new(path_part))?;
    Some(SourceLocation {
        path: normalized,
        line,
    })
}

fn looks_like_source_path(path: &str) -> bool {
    !path.is_empty()
        && !Path::new(path).is_absolute()
        && [
            ".rs", ".py", ".js", ".jsx", ".mjs", ".cjs", ".ts", ".tsx", ".mts", ".cts",
        ]
        .iter()
        .any(|ext| path.ends_with(ext))
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        value.to_string()
    } else {
        let mut out = value.chars().take(max_chars).collect::<String>();
        out.push_str("\n...[truncated]");
        out
    }
}
