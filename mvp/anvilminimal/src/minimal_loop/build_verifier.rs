use std::path::Path;

use serde::Serialize;

use crate::eval_events;
use crate::minimal_loop::dependency_setup::{
    self, NodeDependencySetupAuthority, NodeDependencySetupObservation, NodeDependencySetupStatus,
};
use crate::planner::verify::validate_verify_command;
use crate::tools::bash;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BuildVerifierRequirement {
    pub command: String,
    pub profile: Option<String>,
    pub reason: String,
    pub authority: String,
    pub status: String,
    pub requires_dependency_setup: bool,
    pub required_for_completion: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BuildVerifierStatus {
    PolicyRejected,
    DependencyMissing,
    Blocked,
    Passed,
    Failed,
}

impl BuildVerifierStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PolicyRejected => "policy_rejected",
            Self::DependencyMissing => "dependency_missing",
            Self::Blocked => "blocked",
            Self::Passed => "passed",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BuildVerifierObservation {
    pub command: String,
    pub profile: Option<String>,
    pub authority: String,
    pub required_for_completion: bool,
    pub requires_dependency_setup: bool,
    pub dependency_ready: bool,
    pub attempted: bool,
    pub status: BuildVerifierStatus,
    pub primary_reason: String,
    pub output_snippet: String,
}

impl BuildVerifierObservation {
    pub fn status_str(&self) -> &'static str {
        self.status.as_str()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BuildVerifierLifecycleObservation {
    pub requirement: BuildVerifierRequirement,
    pub before_setup: BuildVerifierObservation,
    pub setup: Option<NodeDependencySetupObservation>,
    pub after_setup: Option<BuildVerifierObservation>,
    pub final_status: BuildVerifierStatus,
    pub final_reason: String,
}

impl BuildVerifierLifecycleObservation {
    pub fn final_observation(&self) -> &BuildVerifierObservation {
        self.after_setup.as_ref().unwrap_or(&self.before_setup)
    }

    pub fn setup_status(&self) -> &'static str {
        self.setup
            .as_ref()
            .map(|setup| setup.status.as_str())
            .unwrap_or("not_required")
    }
}

pub fn requirement_from_deferred(
    command: &str,
    profile: Option<&str>,
    reason: &str,
    authority: &str,
    status: &str,
) -> Option<BuildVerifierRequirement> {
    if !is_build_verifier_command(command) {
        return None;
    }
    Some(BuildVerifierRequirement {
        command: command.to_string(),
        profile: profile.map(str::to_string),
        reason: reason.to_string(),
        authority: authority.to_string(),
        status: status.to_string(),
        requires_dependency_setup: requires_dependency_setup(command),
        required_for_completion: status != "optional",
    })
}

pub fn observe_requirement(
    root: &Path,
    requirement: &BuildVerifierRequirement,
) -> BuildVerifierObservation {
    let dependency_ready =
        !requirement.requires_dependency_setup || dependency_ready(root, &requirement.command);
    if let Err(err) = validate_verify_command(&requirement.command) {
        return BuildVerifierObservation {
            command: requirement.command.clone(),
            profile: requirement.profile.clone(),
            authority: requirement.authority.clone(),
            required_for_completion: requirement.required_for_completion,
            requires_dependency_setup: requirement.requires_dependency_setup,
            dependency_ready,
            attempted: false,
            status: BuildVerifierStatus::PolicyRejected,
            primary_reason: err.to_string(),
            output_snippet: String::new(),
        };
    }
    if !dependency_ready {
        return BuildVerifierObservation {
            command: requirement.command.clone(),
            profile: requirement.profile.clone(),
            authority: requirement.authority.clone(),
            required_for_completion: requirement.required_for_completion,
            requires_dependency_setup: requirement.requires_dependency_setup,
            dependency_ready,
            attempted: false,
            status: BuildVerifierStatus::DependencyMissing,
            primary_reason: dependency_missing_reason(&requirement.command),
            output_snippet: String::new(),
        };
    }
    match bash::run_checked(&requirement.command, root, false) {
        Ok(output) => BuildVerifierObservation {
            command: requirement.command.clone(),
            profile: requirement.profile.clone(),
            authority: requirement.authority.clone(),
            required_for_completion: requirement.required_for_completion,
            requires_dependency_setup: requirement.requires_dependency_setup,
            dependency_ready,
            attempted: true,
            status: BuildVerifierStatus::Passed,
            primary_reason: "build verifier passed".to_string(),
            output_snippet: eval_events::body_snippet(&output),
        },
        Err(err) => {
            let reason = err.to_string();
            let status = if is_dependency_missing_output(&reason) {
                BuildVerifierStatus::DependencyMissing
            } else if reason.contains("blocked") {
                BuildVerifierStatus::Blocked
            } else {
                BuildVerifierStatus::Failed
            };
            BuildVerifierObservation {
                command: requirement.command.clone(),
                profile: requirement.profile.clone(),
                authority: requirement.authority.clone(),
                required_for_completion: requirement.required_for_completion,
                requires_dependency_setup: requirement.requires_dependency_setup,
                dependency_ready,
                attempted: true,
                status,
                primary_reason: eval_events::body_snippet(&reason),
                output_snippet: eval_events::body_snippet(&reason),
            }
        }
    }
}

pub fn observe_requirement_lifecycle(
    root: &Path,
    requirement: &BuildVerifierRequirement,
    setup_authority: NodeDependencySetupAuthority,
) -> BuildVerifierLifecycleObservation {
    let before_setup = observe_requirement(root, requirement);
    let mut setup = None;
    let mut after_setup = None;
    if before_setup.status == BuildVerifierStatus::DependencyMissing
        && requirement.requires_dependency_setup
        && requires_next_binary(&requirement.command)
    {
        let setup_requirement = dependency_setup::requirement_for_next_build(
            root,
            requirement.profile.as_deref(),
            &requirement.reason,
            setup_authority,
        );
        let setup_observation = if setup_requirement.allowed {
            dependency_setup::run_node_dependency_setup(root, &setup_requirement)
        } else {
            NodeDependencySetupObservation::blocked(
                setup_requirement.package_manager,
                setup_requirement.setup_authority,
                setup_requirement
                    .blocked_reason
                    .clone()
                    .unwrap_or_else(|| "dependency setup blocked".to_string()),
            )
        };
        if setup_observation.status == NodeDependencySetupStatus::Passed {
            after_setup = Some(observe_requirement(root, requirement));
        }
        setup = Some(setup_observation);
    }
    let final_status = after_setup.as_ref().unwrap_or(&before_setup).status;
    let final_reason = after_setup
        .as_ref()
        .unwrap_or(&before_setup)
        .primary_reason
        .clone();
    BuildVerifierLifecycleObservation {
        requirement: requirement.clone(),
        before_setup,
        setup,
        after_setup,
        final_status,
        final_reason,
    }
}

pub fn is_build_verifier_command(command: &str) -> bool {
    let normalized = command.to_ascii_lowercase();
    normalized == "npm run build"
        || normalized == "pnpm build"
        || normalized == "yarn build"
        || normalized.starts_with("npm run build ")
        || normalized.starts_with("pnpm build ")
        || normalized.starts_with("yarn build ")
        || normalized.contains("next build")
        || normalized.contains("cargo build")
}

pub fn requires_next_binary(command: &str) -> bool {
    let normalized = command.to_ascii_lowercase();
    normalized.contains("next build")
        || normalized.contains("npm run build")
        || normalized.contains("pnpm build")
        || normalized.contains("yarn build")
}

fn requires_dependency_setup(command: &str) -> bool {
    requires_next_binary(command)
}

fn dependency_ready(root: &Path, command: &str) -> bool {
    if requires_next_binary(command) {
        return root.join("node_modules/.bin/next").is_file();
    }
    true
}

fn dependency_missing_reason(command: &str) -> String {
    if requires_next_binary(command) {
        "node_modules/.bin/next missing for Next.js build".to_string()
    } else {
        format!("dependency setup missing before `{command}`")
    }
}

pub fn is_dependency_missing_output(output: &str) -> bool {
    let lower = output.to_ascii_lowercase();
    lower.contains("command not found")
        || lower.contains("not found")
        || lower.contains("cannot find module")
        || lower.contains("module not found")
        || lower.contains("no such file or directory")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn deferred_next_build_becomes_required_build_verifier() {
        let requirement = requirement_from_deferred(
            "npm run build",
            Some("nextjs"),
            "final build check",
            "profile:nextjs",
            "pending",
        )
        .unwrap();
        assert!(requirement.required_for_completion);
        assert!(requirement.requires_dependency_setup);
    }

    #[test]
    fn static_profile_coverage_status_does_not_disable_build_verifier() {
        let requirement = requirement_from_deferred(
            "npm run build",
            Some("nextjs"),
            "legacy static coverage marker",
            "profile:nextjs",
            "covered_by_static_profile_check",
        )
        .unwrap();
        assert!(requirement.required_for_completion);
    }

    #[test]
    fn next_build_reports_dependency_missing_before_execution() {
        let dir = TempDir::new().unwrap();
        let requirement = requirement_from_deferred(
            "npm run build",
            Some("nextjs"),
            "final build check",
            "profile:nextjs",
            "pending",
        )
        .unwrap();
        let observation = observe_requirement(dir.path(), &requirement);
        assert_eq!(observation.status, BuildVerifierStatus::DependencyMissing);
        assert!(!observation.attempted);
    }

    #[test]
    fn shell_control_syntax_is_policy_rejected() {
        let dir = TempDir::new().unwrap();
        let requirement = requirement_from_deferred(
            "npm run build && npm test",
            Some("nextjs"),
            "bad command",
            "test",
            "pending",
        )
        .unwrap();
        let observation = observe_requirement(dir.path(), &requirement);
        assert_eq!(observation.status, BuildVerifierStatus::PolicyRejected);
    }
}
