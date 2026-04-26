use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

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
}
