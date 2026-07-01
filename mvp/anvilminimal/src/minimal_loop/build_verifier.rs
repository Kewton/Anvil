use std::path::Path;

use serde::Serialize;

use crate::eval_events;
use crate::minimal_loop::dependency_setup::{
    self, NodeDependencySetupAuthority, NodeDependencySetupKind, NodeDependencySetupObservation,
    NodeDependencySetupStatus,
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

    pub fn lifecycle_stages(&self) -> Vec<&'static str> {
        let mut stages = vec!["dependency_check"];
        if self.before_setup.status == BuildVerifierStatus::DependencyMissing
            && self.requirement.requires_dependency_setup
        {
            if self
                .setup
                .as_ref()
                .is_some_and(|setup| setup.authority.allows_setup())
            {
                stages.push("setup_authority_selected");
            } else {
                stages.push("setup_authority_missing");
            }
            if self.setup.as_ref().is_some_and(|setup| setup.attempted) {
                stages.push("setup_attempted");
            }
            match self.setup.as_ref().map(|setup| setup.status) {
                Some(NodeDependencySetupStatus::Blocked) => stages.push("setup_blocked"),
                Some(NodeDependencySetupStatus::Attempted) => {}
                Some(NodeDependencySetupStatus::Passed) => stages.push("setup_passed"),
                Some(NodeDependencySetupStatus::Failed) => stages.push("setup_failed"),
                Some(NodeDependencySetupStatus::TimedOut) => stages.push("setup_timed_out"),
                Some(NodeDependencySetupStatus::NotRequired) => stages.push("setup_not_required"),
                None => stages.push("setup_not_requested"),
            }
        }
        if self.after_setup.is_some() {
            stages.push("build_rerun_attempted");
            stages.push("build_rerun");
        }
        stages.push(match self.final_status {
            BuildVerifierStatus::Passed => "verification_passed",
            BuildVerifierStatus::Failed => "verification_failed",
            BuildVerifierStatus::Blocked => "verification_blocked",
            BuildVerifierStatus::DependencyMissing => "verification_dependency_missing",
            BuildVerifierStatus::PolicyRejected => "verification_policy_rejected",
        });
        stages
    }
}

pub fn emit_dependency_build_lifecycle(
    eval_events_path: Option<&Path>,
    mode: &str,
    step_id: Option<&str>,
    lifecycle: &BuildVerifierLifecycleObservation,
) {
    eval_events::emit(
        eval_events_path,
        serde_json::json!({
            "event": "dependency_build_lifecycle",
            "mode": mode,
            "step_id": step_id.unwrap_or(""),
            "lifecycle_stage": "dependency_setup_build",
            "lifecycle_stages": lifecycle.lifecycle_stages(),
            "command": lifecycle.requirement.command,
            "profile": lifecycle.requirement.profile,
            "authority": lifecycle.requirement.authority,
            "required_for_completion": lifecycle.requirement.required_for_completion,
            "requires_dependency_setup": lifecycle.requirement.requires_dependency_setup,
            "before_status": lifecycle.before_setup.status_str(),
            "before_attempted": lifecycle.before_setup.attempted,
            "setup_status": lifecycle.setup_status(),
            "setup_attempted": lifecycle.setup.as_ref().is_some_and(|setup| setup.attempted),
            "setup_authority": lifecycle.setup.as_ref().map(|setup| setup.authority.as_str()).unwrap_or("none"),
            "setup_kind": lifecycle.setup.as_ref().map(|setup| setup.setup_kind.as_str()).unwrap_or("none"),
            "setup_command": lifecycle.setup.as_ref().map(|setup| setup.command.as_str()).unwrap_or(""),
            "setup_changed_paths": lifecycle.setup.as_ref().map(|setup| setup.changed_paths.clone()).unwrap_or_default(),
            "after_status": lifecycle.after_setup.as_ref().map(BuildVerifierObservation::status_str).unwrap_or(""),
            "after_attempted": lifecycle.after_setup.as_ref().is_some_and(|observation| observation.attempted),
            "build_rerun_attempted": lifecycle.after_setup.as_ref().is_some_and(|observation| observation.attempted),
            "final_status": lifecycle.final_status.as_str(),
            "final_reason": eval_events::body_snippet(&lifecycle.final_reason),
        }),
    );
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
            primary_reason: dependency_missing_reason(root, &requirement.command),
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
    observe_requirement_lifecycle_with_setup_program(
        root,
        requirement,
        setup_authority,
        Path::new("npm"),
    )
}

pub(crate) fn observe_requirement_lifecycle_with_setup_program(
    root: &Path,
    requirement: &BuildVerifierRequirement,
    setup_authority: NodeDependencySetupAuthority,
    npm_program: &Path,
) -> BuildVerifierLifecycleObservation {
    let before_setup = observe_requirement(root, requirement);
    let mut setup = None;
    let mut after_setup = None;
    if before_setup.status == BuildVerifierStatus::DependencyMissing
        && requirement.requires_dependency_setup
    {
        let setup_requirement = dependency_setup_requirement(root, requirement, setup_authority);
        let setup_observation = if setup_requirement.allowed {
            dependency_setup::run_node_dependency_setup_with_program(
                root,
                &setup_requirement,
                npm_program,
            )
        } else {
            NodeDependencySetupObservation::blocked(
                setup_requirement.setup_kind,
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
        || normalized == "npm test"
        || normalized == "npm run test"
        || normalized == "pnpm build"
        || normalized == "pnpm test"
        || normalized == "yarn build"
        || normalized == "yarn test"
        || normalized.starts_with("npm run build ")
        || normalized.starts_with("npm test ")
        || normalized.starts_with("npm run test ")
        || normalized.starts_with("pnpm build ")
        || normalized.starts_with("pnpm test ")
        || normalized.starts_with("yarn build ")
        || normalized.starts_with("yarn test ")
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
    requires_next_binary(command) || requires_node_test_runner(command)
}

fn dependency_ready(root: &Path, command: &str) -> bool {
    if requires_next_binary(command) {
        if requires_package_manifest(command) && !root.join("package.json").is_file() {
            return false;
        }
        return dependency_setup::next_build_dependencies_ready(root);
    }
    if requires_node_test_runner(command) {
        return dependency_setup::node_test_runner_bindable(root);
    }
    true
}

fn requires_package_manifest(command: &str) -> bool {
    let normalized = command.to_ascii_lowercase();
    normalized.starts_with("npm ")
        || normalized.starts_with("pnpm ")
        || normalized.starts_with("yarn ")
}

fn dependency_missing_reason(root: &Path, command: &str) -> String {
    if requires_next_binary(command) {
        dependency_setup::next_build_missing_dependency_reason(root)
    } else if requires_node_test_runner(command) {
        "package.json scripts.test missing before Node test verifier".to_string()
    } else {
        format!("dependency setup missing before `{command}`")
    }
}

fn requires_node_test_runner(command: &str) -> bool {
    let normalized = command.trim().to_ascii_lowercase();
    normalized == "npm test"
        || normalized == "npm run test"
        || normalized == "pnpm test"
        || normalized == "yarn test"
        || normalized.starts_with("npm test ")
        || normalized.starts_with("npm run test ")
        || normalized.starts_with("pnpm test ")
        || normalized.starts_with("yarn test ")
}

fn dependency_setup_requirement(
    root: &Path,
    requirement: &BuildVerifierRequirement,
    setup_authority: NodeDependencySetupAuthority,
) -> dependency_setup::NodeDependencySetupRequirement {
    if requires_next_binary(&requirement.command) {
        return dependency_setup::requirement_for_next_build(
            root,
            requirement.profile.as_deref(),
            &requirement.reason,
            setup_authority,
        );
    }
    if requires_node_test_runner(&requirement.command) {
        return dependency_setup::requirement_for_node_test_runner(
            root,
            requirement.profile.as_deref(),
            &requirement.reason,
            setup_authority,
        );
    }
    dependency_setup::NodeDependencySetupRequirement {
        profile: requirement.profile.clone(),
        setup_kind: NodeDependencySetupKind::NextBuildDependencies,
        package_manager: dependency_setup::package_manager_for_root(root),
        project_root: ".".to_string(),
        reason: requirement.reason.clone(),
        required_binary: "unknown".to_string(),
        setup_authority,
        allowed: false,
        blocked_reason: Some("unsupported dependency setup requirement".to_string()),
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
    fn dependency_missing_then_setup_blocked_records_lifecycle_taxonomy() {
        let dir = TempDir::new().unwrap();
        let requirement = requirement_from_deferred(
            "node_modules/.bin/next build",
            Some("nextjs"),
            "final build check",
            "profile:nextjs",
            "pending",
        )
        .unwrap();
        let lifecycle = observe_requirement_lifecycle(
            dir.path(),
            &requirement,
            NodeDependencySetupAuthority::None,
        );
        assert_eq!(
            lifecycle.before_setup.status,
            BuildVerifierStatus::DependencyMissing
        );
        assert_eq!(lifecycle.setup_status(), "blocked");
        assert_eq!(
            lifecycle.lifecycle_stages(),
            vec![
                "dependency_check",
                "setup_authority_missing",
                "setup_blocked",
                "verification_dependency_missing"
            ]
        );
    }

    #[test]
    fn next_build_with_tailwind_requires_tailwind_node_modules_not_only_next_binary() {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("src/app")).unwrap();
        std::fs::create_dir_all(dir.path().join("node_modules/.bin")).unwrap();
        std::fs::write(dir.path().join("node_modules/.bin/next"), "").unwrap();
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"dependencies":{"next":"^14.2.0","react":"^18.3.0","react-dom":"^18.3.0"},"devDependencies":{"tailwindcss":"^3.4.19","postcss":"^8.5.15","autoprefixer":"^10.4.20"},"scripts":{"build":"next build"}}"#,
        )
        .unwrap();
        std::fs::write(
            dir.path().join("src/app/globals.css"),
            "@tailwind base;\n@tailwind components;\n@tailwind utilities;\n",
        )
        .unwrap();
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
        assert!(
            observation
                .primary_reason
                .contains("node_modules/tailwindcss"),
            "{observation:?}"
        );
    }

    #[test]
    fn dependency_missing_setup_allowed_then_build_rerun_records_lifecycle() {
        let dir = TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"dependencies":{"next":"^14.2.0","react":"^18.3.0","react-dom":"^18.3.0"},"scripts":{"build":"next build"}}"#,
        )
        .unwrap();
        let fake_npm = dir.path().join("fake-npm.sh");
        std::fs::write(
            &fake_npm,
            "#!/bin/sh\nmkdir -p node_modules/.bin\ncat > node_modules/.bin/next <<'EOF'\n#!/bin/sh\nexit 0\nEOF\nchmod +x node_modules/.bin/next\ntouch package-lock.json\nexit 0\n",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&fake_npm).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&fake_npm, perms).unwrap();
        }
        let requirement = requirement_from_deferred(
            "node_modules/.bin/next build",
            Some("nextjs"),
            "final build check",
            "profile:nextjs",
            "pending",
        )
        .unwrap();
        let lifecycle = observe_requirement_lifecycle_with_setup_program(
            dir.path(),
            &requirement,
            NodeDependencySetupAuthority::EvalExplicit,
            &fake_npm,
        );
        assert_eq!(lifecycle.setup_status(), "passed");
        assert_eq!(lifecycle.final_status, BuildVerifierStatus::Passed);
        assert!(lifecycle.after_setup.is_some());
        assert!(
            lifecycle
                .lifecycle_stages()
                .contains(&"setup_authority_selected")
        );
        assert!(lifecycle.lifecycle_stages().contains(&"setup_attempted"));
        assert!(lifecycle.lifecycle_stages().contains(&"setup_passed"));
        assert!(
            lifecycle
                .lifecycle_stages()
                .contains(&"build_rerun_attempted")
        );
        assert!(lifecycle.lifecycle_stages().contains(&"build_rerun"));
        assert!(
            lifecycle
                .lifecycle_stages()
                .contains(&"verification_passed")
        );
    }

    #[test]
    fn dependency_missing_setup_failed_records_attempted_and_failed_stages() {
        let dir = TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"dependencies":{"next":"^14.2.0","react":"^18.3.0","react-dom":"^18.3.0"},"scripts":{"build":"next build"}}"#,
        )
        .unwrap();
        let fake_npm = dir.path().join("fake-npm-fail.sh");
        std::fs::write(&fake_npm, "#!/bin/sh\necho install failed >&2\nexit 1\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&fake_npm).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&fake_npm, perms).unwrap();
        }
        let requirement = requirement_from_deferred(
            "npm run build",
            Some("nextjs"),
            "final build check",
            "profile:nextjs",
            "pending",
        )
        .unwrap();
        let lifecycle = observe_requirement_lifecycle_with_setup_program(
            dir.path(),
            &requirement,
            NodeDependencySetupAuthority::EvalExplicit,
            &fake_npm,
        );
        assert_eq!(lifecycle.setup_status(), "failed");
        assert_eq!(
            lifecycle.final_status,
            BuildVerifierStatus::DependencyMissing
        );
        assert!(lifecycle.after_setup.is_none());
        assert!(
            lifecycle
                .lifecycle_stages()
                .contains(&"setup_authority_selected")
        );
        assert!(lifecycle.lifecycle_stages().contains(&"setup_attempted"));
        assert!(lifecycle.lifecycle_stages().contains(&"setup_failed"));
        assert!(
            lifecycle
                .lifecycle_stages()
                .contains(&"verification_dependency_missing")
        );
    }

    #[test]
    fn node_test_runner_missing_manifest_setup_blocked_records_lifecycle() {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("tests")).unwrap();
        std::fs::write(
            dir.path().join("tests").join("main.test.js"),
            "import test from 'node:test';\n",
        )
        .unwrap();
        let requirement = requirement_from_deferred(
            "npm test",
            Some("js"),
            "node test verifier",
            "profile:js",
            "required",
        )
        .unwrap();
        let lifecycle = observe_requirement_lifecycle(
            dir.path(),
            &requirement,
            NodeDependencySetupAuthority::None,
        );
        assert_eq!(
            lifecycle.before_setup.status,
            BuildVerifierStatus::DependencyMissing
        );
        assert_eq!(lifecycle.setup_status(), "blocked");
        assert_eq!(
            lifecycle
                .setup
                .as_ref()
                .map(|setup| setup.setup_kind.as_str()),
            Some("node_test_runner_manifest")
        );
        assert_eq!(
            lifecycle.lifecycle_stages(),
            vec![
                "dependency_check",
                "setup_authority_missing",
                "setup_blocked",
                "verification_dependency_missing"
            ]
        );
    }

    #[test]
    fn node_test_runner_setup_allowed_then_test_rerun_records_lifecycle() {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("tests")).unwrap();
        std::fs::write(
            dir.path().join("tests").join("main.test.js"),
            "import test from 'node:test';\nimport assert from 'node:assert/strict';\ntest('ok', () => assert.equal(1, 1));\n",
        )
        .unwrap();
        let requirement = requirement_from_deferred(
            "npm test",
            Some("js"),
            "node test verifier",
            "profile:js",
            "required",
        )
        .unwrap();
        let lifecycle = observe_requirement_lifecycle(
            dir.path(),
            &requirement,
            NodeDependencySetupAuthority::CompletionContract,
        );
        assert_eq!(lifecycle.setup_status(), "passed");
        assert_eq!(lifecycle.final_status, BuildVerifierStatus::Passed);
        assert!(lifecycle.after_setup.is_some());
        assert!(dir.path().join("package.json").is_file());
        assert!(
            lifecycle
                .lifecycle_stages()
                .contains(&"setup_authority_selected")
        );
        assert!(lifecycle.lifecycle_stages().contains(&"setup_attempted"));
        assert!(lifecycle.lifecycle_stages().contains(&"setup_passed"));
        assert!(
            lifecycle
                .lifecycle_stages()
                .contains(&"build_rerun_attempted")
        );
        assert!(lifecycle.lifecycle_stages().contains(&"build_rerun"));
        assert!(
            lifecycle
                .lifecycle_stages()
                .contains(&"verification_passed")
        );
    }

    #[test]
    fn dependency_build_lifecycle_event_uses_same_taxonomy_for_modes() {
        let dir = TempDir::new().unwrap();
        let events = dir.path().join("events.jsonl");
        let requirement = requirement_from_deferred(
            "npm run build",
            Some("nextjs"),
            "final build check",
            "profile:nextjs",
            "pending",
        )
        .unwrap();
        let lifecycle = observe_requirement_lifecycle(
            dir.path(),
            &requirement,
            NodeDependencySetupAuthority::None,
        );
        for mode in ["minimal-loop", "plan-run", "ultra-plan-run"] {
            emit_dependency_build_lifecycle(Some(&events), mode, Some("step"), &lifecycle);
        }
        let text = std::fs::read_to_string(events).unwrap();
        assert_eq!(
            text.matches("\"event\":\"dependency_build_lifecycle\"")
                .count(),
            3
        );
        assert!(text.contains("\"mode\":\"minimal-loop\""));
        assert!(text.contains("\"mode\":\"plan-run\""));
        assert!(text.contains("\"mode\":\"ultra-plan-run\""));
        assert!(text.contains("\"lifecycle_stage\":\"dependency_setup_build\""));
        assert!(text.contains("setup_blocked"));
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
