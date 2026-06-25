use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use serde::Deserialize;

use crate::config::Config;
use crate::eval_events;
use crate::planner::verify::{VerificationReport, validate_verify_command};
use crate::tools::path_guard::{
    resolve_existing, resolve_optional_existing, validate_workspace_relative,
};

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct CompletionContract {
    #[serde(default)]
    pub required_paths: Vec<String>,
    #[serde(default)]
    pub verify_commands: Vec<String>,
    #[serde(default = "default_verify_repair_cap")]
    pub verify_repair_cap: usize,
}

impl CompletionContract {
    pub fn load_for_config(config: &Config) -> anyhow::Result<Option<Self>> {
        let path = config
            .completion_contract_path
            .clone()
            .or_else(|| std::env::var_os("ANVIL_COMPLETION_CONTRACT").map(PathBuf::from));
        let Some(path) = path else {
            return Ok(None);
        };
        let path = normalize_contract_file_path(&config.workspace_root, &path)?;
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("failed to read completion contract {}", path.display()))?;
        let contract: Self = serde_json::from_str(&text)
            .with_context(|| format!("invalid completion contract JSON {}", path.display()))?;
        Ok(Some(contract.validate(&config.workspace_root)?))
    }

    pub fn validate(mut self, root: &Path) -> anyhow::Result<Self> {
        let mut seen_paths = BTreeSet::new();
        let mut paths = Vec::new();
        for path in self.required_paths {
            validate_contract_path(root, &path)?;
            if seen_paths.insert(path.clone()) {
                paths.push(path);
            }
        }
        let mut seen_commands = BTreeSet::new();
        let mut commands = Vec::new();
        for command in self.verify_commands {
            validate_verify_command(&command)?;
            if seen_commands.insert(command.clone()) {
                commands.push(command);
            }
        }
        self.required_paths = paths;
        self.verify_commands = commands;
        if self.verify_repair_cap == 0 {
            self.verify_repair_cap = default_verify_repair_cap();
        }
        Ok(self)
    }

    pub fn has_verify(&self) -> bool {
        !self.verify_commands.is_empty()
    }

    pub fn dependency_precondition_active(&self, root: &Path) -> bool {
        self.verify_commands.iter().any(|command| {
            requires_next_binary(command) && !root.join("node_modules/.bin/next").is_file()
        })
    }

    pub fn verify(&self, root: &Path) -> VerificationReport {
        let mut report = VerificationReport::pass();
        for path in &self.required_paths {
            if resolve_existing(root, path).is_err() {
                report.push_missing_path(path.clone());
            }
        }
        for command in &self.verify_commands {
            if let Err(err) = validate_verify_command(command) {
                report.push_command_failure(command.clone(), err.to_string());
                continue;
            }
            if requires_next_binary(command) && !root.join("node_modules/.bin/next").is_file() {
                report.push_dependency_missing("node_modules/.bin/next missing for Next.js build");
                continue;
            }
            match crate::tools::bash::run_checked(command, root, false) {
                Ok(output) => {
                    if command.contains("npm") && output.contains("0 tests") {
                        report.push_command_failure(command.clone(), "Node 0 tests rejected");
                    } else if let Some(reason) = classify_python_test_discovery_failure(&output) {
                        report.push_command_failure(command.clone(), reason);
                    }
                }
                Err(err)
                    if err.to_string().contains("not found")
                        || err.to_string().contains("No such file") =>
                {
                    report.push_dependency_missing(command.clone());
                }
                Err(err) => {
                    let reason = err.to_string();
                    if let Some(reason) = classify_python_test_discovery_failure(&reason) {
                        report.push_command_failure(command.clone(), reason);
                    } else {
                        report.push_command_failure(command.clone(), reason);
                    }
                }
            }
        }
        report
    }
}

pub fn format_verify_feedback(report: &VerificationReport) -> String {
    let mut lines = vec![
        "Deterministic completion verification failed. Fix the implementation and retry."
            .to_string(),
    ];
    if !report.missing_paths.is_empty() {
        lines.push(format!(
            "Missing required paths: {}",
            report.missing_paths.join(", ")
        ));
    }
    for reason in &report.dependency_missing {
        lines.push(format!("Dependency missing: {reason}"));
    }
    for failure in &report.command_failures {
        lines.push(format!(
            "Command failed: `{}`\n{}",
            failure.command,
            eval_events::body_snippet(&failure.reason)
        ));
    }
    for failure in &report.profile_failures {
        lines.push(format!("Profile contract failed: {failure}"));
    }
    lines.join("\n")
}

fn validate_contract_path(root: &Path, raw: &str) -> anyhow::Result<()> {
    validate_workspace_relative(raw)?;
    let path = Path::new(raw);
    let blocked = [".anvil", ".git", "target", "node_modules", ".next", ".env"];
    if path.components().any(|component| {
        component
            .as_os_str()
            .to_str()
            .is_some_and(|part| blocked.contains(&part))
    }) {
        bail!("completion contract path is blocked: {raw}");
    }
    resolve_optional_existing(root, raw)
        .with_context(|| format!("completion contract path escapes workspace: {raw}"))?;
    Ok(())
}

fn normalize_contract_file_path(root: &Path, raw: &Path) -> anyhow::Result<PathBuf> {
    if raw
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name == ".env")
    {
        bail!("completion contract file may not be .env");
    }
    let path = if raw.is_absolute() {
        raw.to_path_buf()
    } else {
        root.join(raw)
    };
    let canonical = path.canonicalize().with_context(|| {
        format!(
            "failed to canonicalize completion contract {}",
            path.display()
        )
    })?;
    let mut allowed_roots = vec![root.canonicalize()?];
    for candidate in [
        std::env::temp_dir(),
        PathBuf::from("/tmp"),
        PathBuf::from("/private/tmp"),
    ] {
        if let Ok(temp) = candidate.canonicalize()
            && !allowed_roots.contains(&temp)
        {
            allowed_roots.push(temp);
        }
    }
    if allowed_roots
        .iter()
        .any(|allowed| canonical.starts_with(allowed))
    {
        Ok(canonical)
    } else {
        bail!(
            "completion contract file must be under workspace or temp directory: {}",
            canonical.display()
        );
    }
}

fn requires_next_binary(command: &str) -> bool {
    let lower = command.to_ascii_lowercase();
    lower == "npm run build"
        || lower.starts_with("npm run build ")
        || lower == "pnpm build"
        || lower.starts_with("pnpm build ")
        || lower == "yarn build"
        || lower.starts_with("yarn build ")
}

fn classify_python_test_discovery_failure(output: &str) -> Option<String> {
    let lower = output.to_ascii_lowercase();
    if lower.contains("no tests ran") || lower.contains("ran 0 tests") {
        Some("test_discovery_failure:no_tests_ran".to_string())
    } else {
        None
    }
}

fn default_verify_repair_cap() -> usize {
    2
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn structured_contract_deduplicates_and_accepts_safe_verify() {
        let dir = tempfile::tempdir().unwrap();
        let contract = CompletionContract {
            required_paths: vec!["src/main.rs".to_string(), "src/main.rs".to_string()],
            verify_commands: vec!["cargo test".to_string(), "cargo test".to_string()],
            verify_repair_cap: 0,
        }
        .validate(dir.path())
        .unwrap();
        assert_eq!(contract.required_paths, vec!["src/main.rs"]);
        assert_eq!(contract.verify_commands, vec!["cargo test"]);
        assert_eq!(contract.verify_repair_cap, 2);
    }

    #[test]
    fn contract_rejects_escape_and_secret_paths() {
        let dir = tempfile::tempdir().unwrap();
        for path in [
            "../x",
            "/tmp/x",
            ".env",
            ".anvil/session.json",
            "target/debug/app",
        ] {
            let err = CompletionContract {
                required_paths: vec![path.to_string()],
                verify_commands: Vec::new(),
                verify_repair_cap: 2,
            }
            .validate(dir.path())
            .unwrap_err()
            .to_string();
            assert!(!err.is_empty(), "{path}");
        }
    }

    #[test]
    #[cfg(unix)]
    fn contract_rejects_symlink_escape() {
        let dir = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink("/tmp", dir.path().join("out")).unwrap();
        let err = CompletionContract {
            required_paths: vec!["out/file.txt".to_string()],
            verify_commands: Vec::new(),
            verify_repair_cap: 2,
        }
        .validate(dir.path())
        .unwrap_err()
        .to_string();
        assert!(err.contains("escapes workspace") || err.contains("path"));
    }

    #[test]
    fn contract_rejects_setup_and_shell_control_verify_commands() {
        let dir = tempfile::tempdir().unwrap();
        for command in [
            "npm install",
            "npm test && npm run build",
            "next dev -p 3011",
        ] {
            let err = CompletionContract {
                required_paths: Vec::new(),
                verify_commands: vec![command.to_string()],
                verify_repair_cap: 2,
            }
            .validate(dir.path())
            .unwrap_err()
            .to_string();
            assert!(!err.is_empty(), "{command}");
        }
    }

    #[test]
    fn contract_file_path_rejects_env_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".env"), "OPENAI_API_KEY=x").unwrap();
        let err = normalize_contract_file_path(dir.path(), Path::new(".env"))
            .unwrap_err()
            .to_string();
        assert!(err.contains(".env"));
    }

    #[test]
    fn python_no_tests_ran_is_test_discovery_failure() {
        assert_eq!(
            classify_python_test_discovery_failure("Ran 0 tests in 0.000s\n\nOK").as_deref(),
            Some("test_discovery_failure:no_tests_ran")
        );
        assert_eq!(
            classify_python_test_discovery_failure("NO TESTS RAN").as_deref(),
            Some("test_discovery_failure:no_tests_ran")
        );
    }

    #[test]
    fn unittest_zero_tests_is_not_verify_pass() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("test_repair_report.py"),
            "def test_free():\n    pass\n",
        )
        .unwrap();
        let report = CompletionContract {
            required_paths: vec!["test_repair_report.py".to_string()],
            verify_commands: vec!["python3 -m unittest test_repair_report.py".to_string()],
            verify_repair_cap: 2,
        }
        .validate(dir.path())
        .unwrap()
        .verify(dir.path());
        assert!(!report.is_pass());
        assert!(
            report
                .primary_reason()
                .contains("test_discovery_failure:no_tests_ran")
        );
    }
}
