use std::path::Path;

use crate::planner::step_plan::PlanStep;
use crate::tools::path_guard::{resolve_existing, validate_workspace_relative};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerifyStatus {
    Pass,
    MissingPath(String),
    CommandFailed(String),
    DependencyMissing(String),
    ProfileContractFailed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerificationReport {
    pub status: VerifyStatus,
}

impl VerificationReport {
    pub fn pass() -> Self {
        Self {
            status: VerifyStatus::Pass,
        }
    }

    pub fn is_pass(&self) -> bool {
        self.status == VerifyStatus::Pass
    }
}

pub fn verify_step(root: &Path, step: &PlanStep) -> VerificationReport {
    for path in &step.expected_paths {
        if resolve_existing(root, path).is_err() {
            return VerificationReport {
                status: VerifyStatus::MissingPath(path.clone()),
            };
        }
    }
    for command in &step.verify {
        if let Err(err) = validate_verify_command(command) {
            return VerificationReport {
                status: VerifyStatus::CommandFailed(err.to_string()),
            };
        }
        match crate::tools::bash::run_checked(command, root, false) {
            Ok(output) if command.contains("npm") && output.contains("0 tests") => {
                return VerificationReport {
                    status: VerifyStatus::CommandFailed("Node 0 tests rejected".to_string()),
                };
            }
            Ok(_) => {}
            Err(err)
                if err.to_string().contains("not found")
                    || err.to_string().contains("No such file") =>
            {
                return VerificationReport {
                    status: VerifyStatus::DependencyMissing(command.clone()),
                };
            }
            Err(err) => {
                return VerificationReport {
                    status: VerifyStatus::CommandFailed(err.to_string()),
                };
            }
        }
    }
    VerificationReport::pass()
}

pub fn validate_verify_command(command: &str) -> anyhow::Result<()> {
    let trimmed = command.trim();
    if trimmed.is_empty() {
        anyhow::bail!("verify command is empty");
    }
    if crate::tools::bash::blocked_reason(trimmed, false).is_some() {
        anyhow::bail!("verify command is blocked");
    }
    if let Some(path) = manifest_path_arg(trimmed) {
        validate_workspace_relative(path)?;
    }
    Ok(())
}

fn manifest_path_arg(command: &str) -> Option<&str> {
    let parts: Vec<&str> = command.split_whitespace().collect();
    parts
        .windows(2)
        .find(|pair| pair[0] == "--manifest-path")
        .map(|pair| pair[1])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planner::step_plan::PlanStep;

    #[test]
    fn missing_path_before_evidence() {
        let dir = tempfile::tempdir().unwrap();
        let step = PlanStep {
            id: "s".to_string(),
            kind: "work".to_string(),
            instruction: "x".to_string(),
            expected_paths: vec!["missing.txt".to_string()],
            verify: vec!["false".to_string()],
        };
        assert!(matches!(
            verify_step(dir.path(), &step).status,
            VerifyStatus::MissingPath(_)
        ));
    }

    #[test]
    fn rust_manifest_path_escape_rejected() {
        assert!(validate_verify_command("cargo test --manifest-path ../Cargo.toml").is_err());
    }

    #[test]
    fn verify_command_nonzero_fails() {
        let dir = tempfile::tempdir().unwrap();
        let step = PlanStep {
            id: "s".to_string(),
            kind: "work".to_string(),
            instruction: "x".to_string(),
            expected_paths: Vec::new(),
            verify: vec!["false".to_string()],
        };
        assert!(matches!(
            verify_step(dir.path(), &step).status,
            VerifyStatus::CommandFailed(_)
        ));
    }
}
