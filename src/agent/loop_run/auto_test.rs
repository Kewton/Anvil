use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::agent::prompting::load_project_instructions;

const MAX_OUTPUT_BYTES: usize = 12_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct AutoTestPlan {
    pub command: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct AutoTestResult {
    pub command: String,
    pub passed: bool,
    pub output: String,
}

#[derive(Debug, Clone, Default)]
pub(super) struct AutoTestRunner;

impl AutoTestRunner {
    pub(super) fn detect(work_root: &Path, changed_files: &[String]) -> Option<AutoTestPlan> {
        if let Some(plan) = detect_project_instruction_test(work_root, changed_files) {
            return Some(plan);
        }
        if work_root.join("Cargo.toml").is_file() {
            return Some(AutoTestPlan {
                command: "cargo test".to_string(),
                reason: "Cargo.toml detected".to_string(),
            });
        }
        if work_root.join("package.json").is_file() {
            let package = std::fs::read_to_string(work_root.join("package.json")).ok()?;
            if package.contains("\"test\"") {
                return Some(AutoTestPlan {
                    command: "npm test".to_string(),
                    reason: "package.json test script detected".to_string(),
                });
            }
            if package.contains("\"build\"") {
                return Some(AutoTestPlan {
                    command: "npm run build".to_string(),
                    reason: "package.json build script detected".to_string(),
                });
            }
        }
        if has_python_surface(work_root, changed_files) {
            if work_root.join("pytest.ini").is_file()
                || work_root.join("pyproject.toml").is_file()
                || work_root.join("tests").is_dir()
                || changed_files.iter().any(|path| {
                    path.starts_with("tests/")
                        || path.ends_with("_test.py")
                        || path.starts_with("test_")
                })
            {
                return Some(AutoTestPlan {
                    command: "python3 -m pytest".to_string(),
                    reason: "Python tests detected".to_string(),
                });
            }
            if let Some(script) = first_python_script(changed_files) {
                return Some(AutoTestPlan {
                    command: format!("python3 -m py_compile {}", shell_quote(&script)),
                    reason: "Python implementation detected without pytest".to_string(),
                });
            }
        }
        None
    }

    pub(super) fn run(work_root: &Path, plan: &AutoTestPlan) -> Result<AutoTestResult, String> {
        let output = Command::new("sh")
            .arg("-lc")
            .arg(&plan.command)
            .current_dir(work_root)
            .stdin(Stdio::null())
            .output()
            .map_err(|err| format!("failed to run auto test command: {err}"))?;
        let mut combined = String::new();
        combined.push_str(&String::from_utf8_lossy(&output.stdout));
        if !output.stderr.is_empty() {
            if !combined.is_empty() {
                combined.push('\n');
            }
            combined.push_str(&String::from_utf8_lossy(&output.stderr));
        }
        Ok(AutoTestResult {
            command: plan.command.clone(),
            passed: output.status.success(),
            output: truncate(&combined, MAX_OUTPUT_BYTES),
        })
    }
}

fn detect_project_instruction_test(
    work_root: &Path,
    changed_files: &[String],
) -> Option<AutoTestPlan> {
    let instructions = load_project_instructions(work_root, work_root)?;
    let command = extract_safe_preferred_command(&instructions.content, work_root, changed_files)?;
    Some(AutoTestPlan {
        command,
        reason: "ANVIL.md preferred command".to_string(),
    })
}

fn extract_safe_preferred_command(
    text: &str,
    work_root: &Path,
    changed_files: &[String],
) -> Option<String> {
    for command in backtick_commands(text) {
        let normalized = command.trim().to_ascii_lowercase();
        if changed_files.iter().any(|path| path.ends_with(".py"))
            && (normalized.starts_with("python3 ") || normalized.starts_with("python "))
            && command_references_existing_local_file(&command, work_root)
        {
            return Some(command);
        }
        if work_root.join("Cargo.toml").is_file()
            && matches!(
                normalized.as_str(),
                "cargo test" | "cargo clippy --all-targets -- -d warnings" | "cargo check"
            )
        {
            return Some(command);
        }
        if work_root.join("package.json").is_file()
            && matches!(normalized.as_str(), "npm test" | "npm run build")
        {
            return Some(command);
        }
    }
    None
}

fn backtick_commands(text: &str) -> Vec<String> {
    let mut commands = Vec::new();
    let mut rest = text;
    while let Some((_, after_open)) = rest.split_once('`') {
        let Some((candidate, after_close)) = after_open.split_once('`') else {
            break;
        };
        let trimmed = candidate.trim();
        if !trimmed.is_empty() && trimmed.len() <= 200 {
            commands.push(trimmed.to_string());
        }
        rest = after_close;
    }
    commands
}

fn command_references_existing_local_file(command: &str, work_root: &Path) -> bool {
    command
        .split_whitespace()
        .filter(|token| token.ends_with(".py") || token.ends_with(".csv"))
        .map(|token| token.trim_matches(['"', '\'', '`']))
        .all(|token| {
            !token.contains('/')
                && !token.contains('\\')
                && !token.starts_with('.')
                && work_root.join(token).is_file()
        })
}

fn has_python_surface(work_root: &Path, changed_files: &[String]) -> bool {
    changed_files.iter().any(|path| path.ends_with(".py"))
        || work_root.join("pyproject.toml").is_file()
        || work_root.join("requirements.txt").is_file()
}

fn first_python_script(changed_files: &[String]) -> Option<PathBuf> {
    changed_files
        .iter()
        .find(|path| {
            path.ends_with(".py") && !path.starts_with("tests/") && !path.starts_with("test_")
        })
        .map(PathBuf::from)
}

fn shell_quote(path: &Path) -> String {
    let value = path.to_string_lossy();
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn truncate(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_string();
    }
    let mut end = max_bytes;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...", &value[..end])
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn detects_cargo_test_first() {
        let dir = tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname='x'\nversion='0.0.0'\n",
        )
        .expect("write");
        let plan = AutoTestRunner::detect(dir.path(), &[]).expect("plan");
        assert_eq!(plan.command, "cargo test");
    }

    #[test]
    fn detects_python_py_compile_without_tests() {
        let dir = tempdir().expect("tempdir");
        let plan =
            AutoTestRunner::detect(dir.path(), &["scripts/report.py".to_string()]).expect("plan");
        assert_eq!(plan.command, "python3 -m py_compile 'scripts/report.py'");
    }

    #[test]
    fn detects_pytest_when_tests_exist() {
        let dir = tempdir().expect("tempdir");
        std::fs::create_dir(dir.path().join("tests")).expect("tests dir");
        let plan = AutoTestRunner::detect(dir.path(), &["app.py".to_string()]).expect("plan");
        assert_eq!(plan.command, "python3 -m pytest");
    }

    #[test]
    fn detects_safe_anvil_python_command() {
        let dir = tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("ANVIL.md"),
            "Preferred verify: `python3 project_csv_tool.py example.csv`\n",
        )
        .expect("anvil");
        std::fs::write(dir.path().join("project_csv_tool.py"), "print('ok')\n").expect("py");
        std::fs::write(dir.path().join("example.csv"), "Category,Amount\nA,1\n").expect("csv");

        let plan =
            AutoTestRunner::detect(dir.path(), &["project_csv_tool.py".to_string()]).expect("plan");
        assert_eq!(plan.command, "python3 project_csv_tool.py example.csv");
    }

    #[test]
    fn ignores_unsafe_anvil_command() {
        let dir = tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("ANVIL.md"),
            "Preferred verify: `rm -rf .`\n",
        )
        .expect("anvil");
        let plan = AutoTestRunner::detect(dir.path(), &["tool.py".to_string()]);
        assert!(plan.is_some());
        assert_ne!(plan.unwrap().command, "rm -rf .");
    }
}
