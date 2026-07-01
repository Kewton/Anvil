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
pub enum NodeDependencySetupKind {
    NextBuildDependencies,
    NodeTestRunnerManifest,
}

impl NodeDependencySetupKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NextBuildDependencies => "next_build_dependencies",
            Self::NodeTestRunnerManifest => "node_test_runner_manifest",
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
    pub setup_kind: NodeDependencySetupKind,
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
    pub setup_kind: NodeDependencySetupKind,
    pub package_manager: PackageManagerKind,
    pub authority: NodeDependencySetupAuthority,
    pub attempted: bool,
    pub command: String,
    pub primary_reason: String,
    pub output_snippet: String,
    pub changed_paths: Vec<String>,
}

impl NodeDependencySetupObservation {
    pub fn not_required(
        setup_kind: NodeDependencySetupKind,
        package_manager: PackageManagerKind,
    ) -> Self {
        Self {
            status: NodeDependencySetupStatus::NotRequired,
            setup_kind,
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
        setup_kind: NodeDependencySetupKind,
        package_manager: PackageManagerKind,
        authority: NodeDependencySetupAuthority,
        reason: impl Into<String>,
    ) -> Self {
        Self {
            status: NodeDependencySetupStatus::Blocked,
            setup_kind,
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

pub fn next_build_dependencies_ready(root: &Path) -> bool {
    next_build_missing_dependency_labels(root).is_empty()
}

pub fn next_build_missing_dependency_reason(root: &Path) -> String {
    let missing = next_build_missing_dependency_labels(root);
    if missing.is_empty() {
        "Next.js build dependencies are ready".to_string()
    } else {
        format!(
            "Next.js build dependency setup missing: {}",
            missing.join(", ")
        )
    }
}

fn next_build_missing_dependency_labels(root: &Path) -> Vec<String> {
    let mut missing = Vec::new();
    if !next_binary_ready(root) {
        missing.push("node_modules/.bin/next".to_string());
    }
    if workspace_requires_tailwind_toolchain(root) {
        for package in ["tailwindcss", "postcss", "autoprefixer"] {
            if !node_package_installed(root, package) {
                missing.push(format!("node_modules/{package}"));
            }
        }
    }
    missing
}

fn node_package_installed(root: &Path, package: &str) -> bool {
    let package_dir = root.join("node_modules").join(package);
    package_dir.join("package.json").is_file() || package_dir.is_dir()
}

fn workspace_requires_tailwind_toolchain(root: &Path) -> bool {
    package_json_has_dependency(root, "tailwindcss")
        || !tailwind_directive_files(root).is_empty()
        || has_any_file(
            root,
            &[
                "tailwind.config.js",
                "tailwind.config.cjs",
                "tailwind.config.mjs",
                "tailwind.config.ts",
            ],
        )
        || postcss_config_references_tailwind(root)
}

fn tailwind_package_contract_missing(root: &Path) -> Option<String> {
    if !workspace_requires_tailwind_toolchain(root) {
        return None;
    }
    let missing = ["tailwindcss", "postcss", "autoprefixer"]
        .into_iter()
        .filter(|package| !package_json_has_dependency(root, package))
        .collect::<Vec<_>>();
    if missing.is_empty() {
        None
    } else {
        Some(format!(
            "Tailwind toolchain package dependency missing: {}",
            missing.join(", ")
        ))
    }
}

fn package_json_has_dependency(root: &Path, name: &str) -> bool {
    let Ok(raw) = std::fs::read_to_string(root.join("package.json")) else {
        return false;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return false;
    };
    [
        "dependencies",
        "devDependencies",
        "optionalDependencies",
        "peerDependencies",
    ]
    .iter()
    .filter_map(|section| value.get(*section).and_then(serde_json::Value::as_object))
    .any(|deps| deps.contains_key(name))
}

fn tailwind_directive_files(root: &Path) -> Vec<String> {
    [
        "src/app/globals.css",
        "src/app/global.css",
        "app/globals.css",
        "app/global.css",
        "src/styles/globals.css",
        "styles/globals.css",
    ]
    .iter()
    .filter_map(|rel| {
        std::fs::read_to_string(root.join(rel))
            .ok()
            .filter(|content| content.contains("@tailwind"))
            .map(|_| (*rel).to_string())
    })
    .collect()
}

fn postcss_config_references_tailwind(root: &Path) -> bool {
    [
        "postcss.config.js",
        "postcss.config.cjs",
        "postcss.config.mjs",
    ]
    .iter()
    .any(|rel| {
        std::fs::read_to_string(root.join(rel))
            .is_ok_and(|content| content.to_ascii_lowercase().contains("tailwind"))
    })
}

fn has_any_file(root: &Path, paths: &[&str]) -> bool {
    paths.iter().any(|path| root.join(path).is_file())
}

pub fn requirement_for_next_build(
    root: &Path,
    profile: Option<&str>,
    reason: &str,
    authority: NodeDependencySetupAuthority,
) -> NodeDependencySetupRequirement {
    let package_manager = package_manager_for_root(root);
    let mut blocked_reason = None;
    let missing_dependencies = next_build_missing_dependency_labels(root);
    if !root.join("package.json").is_file() {
        blocked_reason = Some("package.json missing".to_string());
    } else if !package_json_declares_dependencies(root) {
        blocked_reason = Some("package.json has no dependency table".to_string());
    } else if let Some(reason) = tailwind_package_contract_missing(root) {
        blocked_reason = Some(reason);
    } else if missing_dependencies.is_empty() {
        blocked_reason = Some("Next.js build dependencies already present".to_string());
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
        setup_kind: NodeDependencySetupKind::NextBuildDependencies,
        project_root: ".".to_string(),
        reason: reason.to_string(),
        required_binary: missing_dependencies
            .first()
            .cloned()
            .unwrap_or_else(|| "node_modules/.bin/next".to_string()),
        setup_authority: authority,
        allowed,
        blocked_reason,
    }
}

pub fn requirement_for_node_test_runner(
    root: &Path,
    profile: Option<&str>,
    reason: &str,
    authority: NodeDependencySetupAuthority,
) -> NodeDependencySetupRequirement {
    let package_manager = package_manager_for_root(root);
    let mut blocked_reason = None;
    if !workspace_has_node_test_file(root) {
        blocked_reason = Some("node test artifact missing".to_string());
    } else if node_test_runner_bindable(root) {
        blocked_reason = Some("node test runner already bindable".to_string());
    } else if !authority.allows_setup() {
        blocked_reason = Some("dependency setup authority missing".to_string());
    } else if matches!(
        package_manager,
        PackageManagerKind::Pnpm | PackageManagerKind::Yarn | PackageManagerKind::Mixed
    ) {
        blocked_reason = Some(format!(
            "package manager {} is not supported by initial npm-only test runner setup bridge",
            package_manager.as_str()
        ));
    }
    let allowed = blocked_reason.is_none();
    NodeDependencySetupRequirement {
        profile: profile.map(str::to_string),
        setup_kind: NodeDependencySetupKind::NodeTestRunnerManifest,
        package_manager,
        project_root: ".".to_string(),
        reason: reason.to_string(),
        required_binary: "package.json:scripts.test".to_string(),
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
            requirement.setup_kind,
            requirement.package_manager,
            requirement.setup_authority,
            requirement
                .blocked_reason
                .clone()
                .unwrap_or_else(|| "dependency setup blocked".to_string()),
        );
    }
    if requirement.setup_kind == NodeDependencySetupKind::NodeTestRunnerManifest {
        return run_node_test_runner_manifest_setup(root, requirement);
    }
    if requirement.package_manager == PackageManagerKind::Pnpm
        || requirement.package_manager == PackageManagerKind::Yarn
        || requirement.package_manager == PackageManagerKind::Mixed
    {
        return NodeDependencySetupObservation::blocked(
            requirement.setup_kind,
            requirement.package_manager,
            requirement.setup_authority,
            "non-npm package manager setup is deferred",
        );
    }
    let before_lock = root.join("package-lock.json").exists();
    let before_missing = next_build_missing_dependency_labels(root);
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
                setup_kind: requirement.setup_kind,
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
                for path in before_missing {
                    if path == "node_modules/.bin/next" {
                        if next_binary_ready(root) {
                            changed_paths.push(path);
                        }
                    } else if let Some(package) = path.strip_prefix("node_modules/")
                        && node_package_installed(root, package)
                    {
                        changed_paths.push(path);
                    }
                }
                let required_ready = next_build_dependencies_ready(root);
                let status_kind = if status.success() && required_ready {
                    NodeDependencySetupStatus::Passed
                } else {
                    NodeDependencySetupStatus::Failed
                };
                return NodeDependencySetupObservation {
                    status: status_kind,
                    setup_kind: requirement.setup_kind,
                    package_manager: requirement.package_manager,
                    authority: requirement.setup_authority,
                    attempted: true,
                    command: "npm install --ignore-scripts".to_string(),
                    primary_reason: if status.success() && required_ready {
                        "dependency setup passed".to_string()
                    } else if status.success() {
                        format!(
                            "dependency setup completed but required dependencies are still missing: {}",
                            next_build_missing_dependency_labels(root).join(", ")
                        )
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
                    setup_kind: requirement.setup_kind,
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
                setup_kind: requirement.setup_kind,
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

fn run_node_test_runner_manifest_setup(
    root: &Path,
    requirement: &NodeDependencySetupRequirement,
) -> NodeDependencySetupObservation {
    let existing = std::fs::read_to_string(root.join("package.json")).ok();
    let Some(completion) = complete_node_test_runner_manifest(existing.as_deref()) else {
        return NodeDependencySetupObservation::blocked(
            requirement.setup_kind,
            requirement.package_manager,
            requirement.setup_authority,
            "node test runner manifest cannot be completed safely",
        );
    };
    match std::fs::write(root.join("package.json"), completion.contents.as_bytes()) {
        Ok(()) => NodeDependencySetupObservation {
            status: NodeDependencySetupStatus::Passed,
            setup_kind: requirement.setup_kind,
            package_manager: requirement.package_manager,
            authority: requirement.setup_authority,
            attempted: true,
            command: format!("deterministic package.json {}", completion.action.as_str()),
            primary_reason: format!("node test runner manifest {}", completion.action.as_str()),
            output_snippet: String::new(),
            changed_paths: vec!["package.json".to_string()],
        },
        Err(err) => NodeDependencySetupObservation {
            status: NodeDependencySetupStatus::Failed,
            setup_kind: requirement.setup_kind,
            package_manager: requirement.package_manager,
            authority: requirement.setup_authority,
            attempted: true,
            command: format!("deterministic package.json {}", completion.action.as_str()),
            primary_reason: format!("failed to write package.json: {err}"),
            output_snippet: String::new(),
            changed_paths: Vec::new(),
        },
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NodeManifestAction {
    CreateManifest,
    AddTestScript,
}

impl NodeManifestAction {
    fn as_str(self) -> &'static str {
        match self {
            Self::CreateManifest => "create_manifest",
            Self::AddTestScript => "add_test_script",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NodeManifestCompletion {
    action: NodeManifestAction,
    contents: String,
}

fn complete_node_test_runner_manifest(existing: Option<&str>) -> Option<NodeManifestCompletion> {
    let Some(raw) = existing else {
        return Some(NodeManifestCompletion {
            action: NodeManifestAction::CreateManifest,
            contents: created_node_test_manifest_contents(),
        });
    };
    let value = serde_json::from_str::<serde_json::Value>(raw).ok()?;
    let serde_json::Value::Object(mut object) = value else {
        return None;
    };
    if manifest_has_script(&object, "test") {
        return None;
    }
    let scripts = object
        .entry("scripts".to_string())
        .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
    let serde_json::Value::Object(scripts) = scripts else {
        return None;
    };
    scripts.insert(
        "test".to_string(),
        serde_json::Value::String("node --test".to_string()),
    );
    let mut contents = serde_json::to_string_pretty(&serde_json::Value::Object(object)).ok()?;
    contents.push('\n');
    Some(NodeManifestCompletion {
        action: NodeManifestAction::AddTestScript,
        contents,
    })
}

fn created_node_test_manifest_contents() -> String {
    r#"{
  "name": "app",
  "version": "0.1.0",
  "private": true,
  "type": "module",
  "scripts": {
    "test": "node --test"
  }
}
"#
    .to_string()
}

pub fn node_test_runner_bindable(root: &Path) -> bool {
    let Ok(raw) = std::fs::read_to_string(root.join("package.json")) else {
        return false;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return false;
    };
    let Some(object) = value.as_object() else {
        return false;
    };
    manifest_has_script(object, "test")
}

fn manifest_has_script(object: &serde_json::Map<String, serde_json::Value>, script: &str) -> bool {
    object
        .get("scripts")
        .and_then(serde_json::Value::as_object)
        .is_some_and(|scripts| {
            scripts.iter().any(|(name, value)| {
                name.eq_ignore_ascii_case(script)
                    && value
                        .as_str()
                        .is_some_and(|command| !command.trim().is_empty())
            })
        })
}

const NODE_TEST_FILE_SUFFIXES: &[&str] = &[
    ".test.js",
    ".test.mjs",
    ".test.cjs",
    ".test.jsx",
    ".test.ts",
    ".test.mts",
    ".test.cts",
    ".test.tsx",
    ".spec.js",
    ".spec.mjs",
    ".spec.cjs",
    ".spec.jsx",
    ".spec.ts",
    ".spec.mts",
    ".spec.cts",
    ".spec.tsx",
];

pub fn workspace_has_node_test_file(root: &Path) -> bool {
    dir_has_node_test_file(root)
        || ["tests", "test", "__tests__"]
            .iter()
            .any(|subdir| dir_has_node_test_file(&root.join(subdir)))
}

fn dir_has_node_test_file(dir: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    entries.flatten().any(|entry| {
        entry.path().is_file()
            && entry
                .file_name()
                .to_str()
                .is_some_and(is_node_test_filename)
    })
}

fn is_node_test_filename(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    NODE_TEST_FILE_SUFFIXES
        .iter()
        .any(|suffix| lower.ends_with(suffix))
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
    fn tailwind_directive_requires_package_contract_before_network_setup() {
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("src/app")).unwrap();
        write_package(
            dir.path(),
            r#"{"dependencies":{"next":"^14.2.0","react":"^18.3.0","react-dom":"^18.3.0"}}"#,
        );
        fs::write(
            dir.path().join("src/app/globals.css"),
            "@tailwind base;\n@tailwind components;\n@tailwind utilities;\n",
        )
        .unwrap();
        let requirement = requirement_for_next_build(
            dir.path(),
            Some("nextjs"),
            "build",
            NodeDependencySetupAuthority::CompletionContract,
        );
        assert!(!requirement.allowed);
        assert!(
            requirement
                .blocked_reason
                .as_deref()
                .unwrap_or("")
                .contains("Tailwind toolchain package dependency missing"),
            "{requirement:?}"
        );
    }

    #[test]
    fn tailwind_declared_but_not_installed_is_setup_candidate_even_when_next_exists() {
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("src/app")).unwrap();
        fs::create_dir_all(dir.path().join("node_modules/.bin")).unwrap();
        fs::write(dir.path().join("node_modules/.bin/next"), "").unwrap();
        write_package(
            dir.path(),
            r#"{"dependencies":{"next":"^14.2.0","react":"^18.3.0","react-dom":"^18.3.0"},"devDependencies":{"tailwindcss":"^3.4.19","postcss":"^8.5.15","autoprefixer":"^10.4.20"}}"#,
        );
        fs::write(
            dir.path().join("src/app/globals.css"),
            "@tailwind base;\n@tailwind components;\n@tailwind utilities;\n",
        )
        .unwrap();
        let requirement = requirement_for_next_build(
            dir.path(),
            Some("nextjs"),
            "build",
            NodeDependencySetupAuthority::CompletionContract,
        );
        assert!(requirement.allowed, "{requirement:?}");
        assert_eq!(requirement.required_binary, "node_modules/tailwindcss");
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
            setup_kind: NodeDependencySetupKind::NextBuildDependencies,
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

    #[test]
    fn node_test_runner_manifest_setup_is_blocked_without_authority() {
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("tests")).unwrap();
        fs::write(
            dir.path().join("tests").join("main.test.js"),
            "import 'node:test';\n",
        )
        .unwrap();
        let requirement = requirement_for_node_test_runner(
            dir.path(),
            Some("js"),
            "node test verifier",
            NodeDependencySetupAuthority::None,
        );
        assert!(!requirement.allowed);
        assert_eq!(
            requirement.setup_kind,
            NodeDependencySetupKind::NodeTestRunnerManifest
        );
        assert_eq!(
            requirement.blocked_reason.as_deref(),
            Some("dependency setup authority missing")
        );
    }

    #[test]
    fn node_test_runner_manifest_setup_creates_package_manifest_without_network() {
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("tests")).unwrap();
        fs::write(
            dir.path().join("tests").join("main.test.js"),
            "import test from 'node:test';\n",
        )
        .unwrap();
        let requirement = requirement_for_node_test_runner(
            dir.path(),
            Some("js"),
            "node test verifier",
            NodeDependencySetupAuthority::CompletionContract,
        );
        assert!(requirement.allowed);
        let observation =
            run_node_dependency_setup_with_program(dir.path(), &requirement, Path::new("missing"));
        assert_eq!(observation.status, NodeDependencySetupStatus::Passed);
        assert_eq!(
            observation.setup_kind,
            NodeDependencySetupKind::NodeTestRunnerManifest
        );
        assert!(observation.command.contains("create_manifest"));
        assert!(node_test_runner_bindable(dir.path()));
        assert!(
            observation
                .changed_paths
                .contains(&"package.json".to_string())
        );
    }

    #[test]
    fn node_test_runner_manifest_setup_adds_script_without_clobbering_manifest() {
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("test")).unwrap();
        fs::write(
            dir.path().join("test").join("main.spec.js"),
            "test('x',()=>{});\n",
        )
        .unwrap();
        write_package(
            dir.path(),
            r#"{"name":"keep","scripts":{"build":"node build.js"}}"#,
        );
        let requirement = requirement_for_node_test_runner(
            dir.path(),
            Some("js"),
            "node test verifier",
            NodeDependencySetupAuthority::CompletionContract,
        );
        let observation =
            run_node_dependency_setup_with_program(dir.path(), &requirement, Path::new("missing"));
        assert_eq!(observation.status, NodeDependencySetupStatus::Passed);
        let package = fs::read_to_string(dir.path().join("package.json")).unwrap();
        assert!(package.contains("\"name\": \"keep\""));
        assert!(package.contains("\"build\": \"node build.js\""));
        assert!(package.contains("\"test\": \"node --test\""));
    }
}
