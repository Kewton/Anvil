use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::eval_events;

const SETUP_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PackageManagerKind {
    Npm,
    Pnpm,
    Yarn,
    Unknown,
    Mixed,
}

impl PackageManagerKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Npm => "npm",
            Self::Pnpm => "pnpm",
            Self::Yarn => "yarn",
            Self::Unknown => "unknown",
            Self::Mixed => "mixed",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeDependencySetupAuthority {
    PlanSetupStep,
    CompletionContract,
    EvalExplicit,
    TuiConfirmed,
    None,
}

impl NodeDependencySetupAuthority {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PlanSetupStep => "plan_setup_step",
            Self::CompletionContract => "completion_contract",
            Self::EvalExplicit => "eval_explicit",
            Self::TuiConfirmed => "tui_confirmed",
            Self::None => "none",
        }
    }

    pub fn allows_setup(self) -> bool {
        !matches!(self, Self::None)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NodeDependencySetupRequirement {
    pub profile: Option<String>,
    pub package_manager: PackageManagerKind,
    pub project_root: String,
    pub reason: String,
    pub required_binary: String,
    pub setup_authority: NodeDependencySetupAuthority,
    pub allowed: bool,
    pub blocked_reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeDependencySetupStatus {
    NotRequired,
    Blocked,
    Attempted,
    Passed,
    Failed,
    TimedOut,
}

impl NodeDependencySetupStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotRequired => "not_required",
            Self::Blocked => "blocked",
            Self::Attempted => "attempted",
            Self::Passed => "passed",
            Self::Failed => "failed",
            Self::TimedOut => "timed_out",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NodeDependencySetupObservation {
    pub status: NodeDependencySetupStatus,
    pub package_manager: PackageManagerKind,
    pub authority: NodeDependencySetupAuthority,
    pub attempted: bool,
    pub command: String,
    pub primary_reason: String,
    pub output_snippet: String,
    pub changed_paths: Vec<String>,
}

impl NodeDependencySetupObservation {
    pub fn not_required(package_manager: PackageManagerKind) -> Self {
        Self {
            status: NodeDependencySetupStatus::NotRequired,
            package_manager,
            authority: NodeDependencySetupAuthority::None,
            attempted: false,
            command: String::new(),
            primary_reason: "dependency setup not required".to_string(),
            output_snippet: String::new(),
            changed_paths: Vec::new(),
        }
    }

    pub fn blocked(
        package_manager: PackageManagerKind,
        authority: NodeDependencySetupAuthority,
        reason: impl Into<String>,
    ) -> Self {
        Self {
            status: NodeDependencySetupStatus::Blocked,
            package_manager,
            authority,
            attempted: false,
            command: String::new(),
            primary_reason: reason.into(),
            output_snippet: String::new(),
            changed_paths: Vec::new(),
        }
    }
}

pub fn package_manager_for_root(root: &Path) -> PackageManagerKind {
    let has_npm = root.join("package-lock.json").is_file();
    let has_pnpm = root.join("pnpm-lock.yaml").is_file();
    let has_yarn = root.join("yarn.lock").is_file();
    let count = [has_npm, has_pnpm, has_yarn]
        .into_iter()
        .filter(|value| *value)
        .count();
    match (count, has_npm, has_pnpm, has_yarn) {
        (0, _, _, _) => PackageManagerKind::Unknown,
        (1, true, _, _) => PackageManagerKind::Npm,
        (1, _, true, _) => PackageManagerKind::Pnpm,
        (1, _, _, true) => PackageManagerKind::Yarn,
        _ => PackageManagerKind::Mixed,
    }
}

pub fn package_json_declares_dependencies(root: &Path) -> bool {
    let Ok(raw) = std::fs::read_to_string(root.join("package.json")) else {
        return false;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return false;
    };
    let Some(object) = value.as_object() else {
        return false;
    };
    [
        "dependencies",
        "devDependencies",
        "optionalDependencies",
        "peerDependencies",
    ]
    .iter()
    .any(|key| {
        object
            .get(*key)
            .and_then(serde_json::Value::as_object)
            .is_some_and(|deps| !deps.is_empty())
    })
}

pub fn next_binary_ready(root: &Path) -> bool {
    root.join("node_modules/.bin/next").is_file()
}

pub fn requirement_for_next_build(
    root: &Path,
    profile: Option<&str>,
    reason: &str,
    authority: NodeDependencySetupAuthority,
) -> NodeDependencySetupRequirement {
    let package_manager = package_manager_for_root(root);
    let mut blocked_reason = None;
    if !root.join("package.json").is_file() {
        blocked_reason = Some("package.json missing".to_string());
    } else if !package_json_declares_dependencies(root) {
        blocked_reason = Some("package.json has no dependency table".to_string());
    } else if next_binary_ready(root) {
        blocked_reason = Some("node_modules/.bin/next already present".to_string());
    } else if !authority.allows_setup() {
        blocked_reason = Some("dependency setup authority missing".to_string());
    } else if matches!(
        package_manager,
        PackageManagerKind::Pnpm | PackageManagerKind::Yarn | PackageManagerKind::Mixed
    ) {
        blocked_reason = Some(format!(
            "package manager {} is not supported by initial npm-only setup bridge",
            package_manager.as_str()
        ));
    }
    let allowed = blocked_reason.is_none();
    NodeDependencySetupRequirement {
        profile: profile.map(str::to_string),
        package_manager,
        project_root: ".".to_string(),
        reason: reason.to_string(),
        required_binary: "node_modules/.bin/next".to_string(),
        setup_authority: authority,
        allowed,
        blocked_reason,
    }
}

pub fn run_node_dependency_setup(
    root: &Path,
    requirement: &NodeDependencySetupRequirement,
) -> NodeDependencySetupObservation {
    run_node_dependency_setup_with_program(root, requirement, Path::new("npm"))
}

pub(crate) fn run_node_dependency_setup_with_program(
    root: &Path,
    requirement: &NodeDependencySetupRequirement,
    npm_program: &Path,
) -> NodeDependencySetupObservation {
    if !requirement.allowed {
        return NodeDependencySetupObservation::blocked(
            requirement.package_manager,
            requirement.setup_authority,
            requirement
                .blocked_reason
                .clone()
                .unwrap_or_else(|| "dependency setup blocked".to_string()),
        );
    }
    if requirement.package_manager == PackageManagerKind::Pnpm
        || requirement.package_manager == PackageManagerKind::Yarn
        || requirement.package_manager == PackageManagerKind::Mixed
    {
        return NodeDependencySetupObservation::blocked(
            requirement.package_manager,
            requirement.setup_authority,
            "non-npm package manager setup is deferred",
        );
    }
    let before_lock = root.join("package-lock.json").exists();
    let before_next = next_binary_ready(root);
    let started = Instant::now();
    let mut child = match Command::new(npm_program)
        .args(["install", "--ignore-scripts"])
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(err) => {
            return NodeDependencySetupObservation {
                status: NodeDependencySetupStatus::Failed,
                package_manager: requirement.package_manager,
                authority: requirement.setup_authority,
                attempted: true,
                command: "npm install --ignore-scripts".to_string(),
                primary_reason: format!("failed to spawn npm: {err}"),
                output_snippet: String::new(),
                changed_paths: Vec::new(),
            };
        }
    };
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let output = child.wait_with_output().ok();
                let stdout = output
                    .as_ref()
                    .map(|out| String::from_utf8_lossy(&out.stdout).to_string())
                    .unwrap_or_default();
                let stderr = output
                    .as_ref()
                    .map(|out| String::from_utf8_lossy(&out.stderr).to_string())
                    .unwrap_or_default();
                let combined = format!("{stderr}\n{stdout}");
                let mut changed_paths = Vec::new();
                if !before_lock && root.join("package-lock.json").exists() {
                    changed_paths.push("package-lock.json".to_string());
                }
                if !before_next && next_binary_ready(root) {
                    changed_paths.push("node_modules/.bin/next".to_string());
                }
                let status_kind = if status.success() {
                    NodeDependencySetupStatus::Passed
                } else {
                    NodeDependencySetupStatus::Failed
                };
                return NodeDependencySetupObservation {
                    status: status_kind,
                    package_manager: requirement.package_manager,
                    authority: requirement.setup_authority,
                    attempted: true,
                    command: "npm install --ignore-scripts".to_string(),
                    primary_reason: if status.success() {
                        "dependency setup passed".to_string()
                    } else {
                        format!("dependency setup failed: {status}")
                    },
                    output_snippet: eval_events::body_snippet(&combined),
                    changed_paths,
                };
            }
            Ok(None) => {}
            Err(err) => {
                return NodeDependencySetupObservation {
                    status: NodeDependencySetupStatus::Failed,
                    package_manager: requirement.package_manager,
                    authority: requirement.setup_authority,
                    attempted: true,
                    command: "npm install --ignore-scripts".to_string(),
                    primary_reason: format!("dependency setup wait failed: {err}"),
                    output_snippet: String::new(),
                    changed_paths: Vec::new(),
                };
            }
        }
        if started.elapsed() >= SETUP_TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            return NodeDependencySetupObservation {
                status: NodeDependencySetupStatus::TimedOut,
                package_manager: requirement.package_manager,
                authority: requirement.setup_authority,
                attempted: true,
                command: "npm install --ignore-scripts".to_string(),
                primary_reason: format!(
                    "dependency setup timed out after {} ms",
                    SETUP_TIMEOUT.as_millis()
                ),
                output_snippet: String::new(),
                changed_paths: Vec::new(),
            };
        }
        thread::sleep(Duration::from_millis(20));
    }
}

pub fn nextjs_build_fallback_command(root: &Path) -> Option<String> {
    if !root.join("package.json").is_file() {
        return None;
    }
    let raw = std::fs::read_to_string(root.join("package.json")).ok()?;
    let value = serde_json::from_str::<serde_json::Value>(&raw).ok()?;
    let has_next = ["dependencies", "devDependencies"]
        .iter()
        .filter_map(|key| value.get(*key).and_then(serde_json::Value::as_object))
        .any(|deps| deps.contains_key("next"));
    let has_build_script = value
        .get("scripts")
        .and_then(serde_json::Value::as_object)
        .and_then(|scripts| scripts.get("build"))
        .and_then(serde_json::Value::as_str)
        .is_some();
    if has_next && !has_build_script {
        Some("npm exec -- next build".to_string())
    } else {
        None
    }
}

pub fn changed_setup_paths(observation: &NodeDependencySetupObservation) -> Vec<String> {
    observation
        .changed_paths
        .iter()
        .filter(|path| path.as_str() != "node_modules/.bin/next")
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn write_package(root: &Path, raw: &str) {
        fs::write(root.join("package.json"), raw).unwrap();
    }

    #[test]
    fn package_manager_detects_npm_and_blocks_mixed() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("package-lock.json"), "{}").unwrap();
        assert_eq!(
            package_manager_for_root(dir.path()),
            PackageManagerKind::Npm
        );
        fs::write(dir.path().join("yarn.lock"), "").unwrap();
        assert_eq!(
            package_manager_for_root(dir.path()),
            PackageManagerKind::Mixed
        );
    }

    #[test]
    fn yes_alone_is_not_authority() {
        let dir = TempDir::new().unwrap();
        write_package(
            dir.path(),
            r#"{"dependencies":{"next":"^14.2.0","react":"^18.3.0","react-dom":"^18.3.0"}}"#,
        );
        let requirement = requirement_for_next_build(
            dir.path(),
            Some("nextjs"),
            "build",
            NodeDependencySetupAuthority::None,
        );
        assert!(!requirement.allowed);
        assert_eq!(
            requirement.blocked_reason.as_deref(),
            Some("dependency setup authority missing")
        );
    }

    #[test]
    fn npm_package_with_authority_is_allowed_candidate() {
        let dir = TempDir::new().unwrap();
        write_package(
            dir.path(),
            r#"{"dependencies":{"next":"^14.2.0","react":"^18.3.0","react-dom":"^18.3.0"}}"#,
        );
        fs::write(dir.path().join("package-lock.json"), "{}").unwrap();
        let requirement = requirement_for_next_build(
            dir.path(),
            Some("nextjs"),
            "build",
            NodeDependencySetupAuthority::CompletionContract,
        );
        assert!(requirement.allowed);
        assert_eq!(requirement.package_manager, PackageManagerKind::Npm);
    }

    #[test]
    fn pnpm_lock_blocks_initial_setup_bridge() {
        let dir = TempDir::new().unwrap();
        write_package(
            dir.path(),
            r#"{"dependencies":{"next":"^14.2.0","react":"^18.3.0","react-dom":"^18.3.0"}}"#,
        );
        fs::write(dir.path().join("pnpm-lock.yaml"), "").unwrap();
        let requirement = requirement_for_next_build(
            dir.path(),
            Some("nextjs"),
            "build",
            NodeDependencySetupAuthority::CompletionContract,
        );
        assert!(!requirement.allowed);
        assert!(requirement.blocked_reason.unwrap().contains("pnpm"));
    }

    #[test]
    fn next_build_fallback_requires_next_without_script() {
        let dir = TempDir::new().unwrap();
        write_package(
            dir.path(),
            r#"{"dependencies":{"next":"^14.2.0","react":"^18.3.0","react-dom":"^18.3.0"}}"#,
        );
        assert_eq!(
            nextjs_build_fallback_command(dir.path()).as_deref(),
            Some("npm exec -- next build")
        );
    }

    #[test]
    fn blocked_requirement_does_not_execute_fake_npm() {
        let dir = TempDir::new().unwrap();
        let requirement = NodeDependencySetupRequirement {
            profile: Some("nextjs".to_string()),
            package_manager: PackageManagerKind::Npm,
            project_root: ".".to_string(),
            reason: "test".to_string(),
            required_binary: "node_modules/.bin/next".to_string(),
            setup_authority: NodeDependencySetupAuthority::None,
            allowed: false,
            blocked_reason: Some("blocked for test".to_string()),
        };
        let observation =
            run_node_dependency_setup_with_program(dir.path(), &requirement, Path::new("missing"));
        assert_eq!(observation.status, NodeDependencySetupStatus::Blocked);
        assert!(!observation.attempted);
    }
}
