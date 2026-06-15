use std::path::{Path, PathBuf};
use std::process::Command;

use super::{PlanStep, VerifyExpectedResult};

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
    Err(first_lines(&combined, 12))
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

fn first_lines(value: &str, max_lines: usize) -> String {
    let lines = value.lines().take(max_lines).collect::<Vec<_>>().join("\n");
    if lines.is_empty() {
        "command exited non-zero with no output".to_string()
    } else {
        lines
    }
}
