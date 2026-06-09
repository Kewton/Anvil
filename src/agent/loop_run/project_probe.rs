//! Generic project completion probe.
//!
//! This module does not decide that a task is done. It only answers whether the
//! current workspace has enough in-scope, current-turn artifacts to justify
//! running the verifier even when the legacy artifact projection is still
//! asking for another edit.

use std::collections::{BTreeSet, HashSet};
use std::path::Path;

use crate::util::file_classify::{
    is_implementation_file, is_setup_file, is_structured_data_file, is_test_file,
};
use crate::util::workspace_paths::is_workspace_artifact_admitted_relative_path;

use super::task_contract::{ArtifactRole, TaskContract};
use super::task_workspace_scope::TaskWorkspaceScope;

const MAX_PROBE_FILES: usize = 512;
const MAX_PROBE_DEPTH: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum CompletionProbeDecision {
    RunVerifier {
        reason: String,
        project_unit: ProjectUnit,
    },
    RejectStackMismatch {
        reason: String,
    },
    KeepArtifactFlow,
}

#[allow(dead_code)]
// ProjectUnit Phase 1 emits ShortUnitTest today; later verifier selection slices construct the remaining bounded classes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProjectUnitTimeoutClass {
    ShortUnitTest,
    BuildCommand,
    DependencySetup,
    Unknown,
}

impl ProjectUnitTimeoutClass {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::ShortUnitTest => "short_unit_test",
            Self::BuildCommand => "build_command",
            Self::DependencySetup => "dependency_setup",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProjectUnitConfidence {
    High,
    Medium,
    Low,
}

impl ProjectUnitConfidence {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::High => "high",
            Self::Medium => "medium",
            Self::Low => "low",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProjectUnitVerifierCandidate {
    pub(super) command_preview: String,
    pub(super) source: &'static str,
    pub(super) timeout_class: ProjectUnitTimeoutClass,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProjectUnit {
    pub(super) root: String,
    pub(super) manifests: Vec<String>,
    pub(super) artifact_roles: BTreeSet<ArtifactRole>,
    pub(super) verifier_candidates: Vec<ProjectUnitVerifierCandidate>,
    pub(super) observed_stacks: Vec<&'static str>,
    pub(super) confidence: ProjectUnitConfidence,
}

impl ProjectUnit {
    pub(super) fn summary(&self) -> String {
        let roles = self
            .artifact_roles
            .iter()
            .map(|role| role.label())
            .collect::<Vec<_>>()
            .join(",");
        let manifests = if self.manifests.is_empty() {
            "none".to_string()
        } else {
            self.manifests.join(",")
        };
        let verifiers = self
            .verifier_candidates
            .iter()
            .map(|candidate| {
                format!(
                    "{}:{}:{}",
                    candidate.source,
                    candidate.timeout_class.as_str(),
                    candidate.command_preview
                )
            })
            .collect::<Vec<_>>()
            .join("|");
        let base = format!(
            "project_unit root={} stacks={} roles={} manifests={} verifiers={}",
            self.root,
            self.observed_stacks.join(","),
            roles,
            manifests,
            verifiers
        );
        format!("{base} confidence={}", self.confidence.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum StackKind {
    Python,
    Rust,
    Node,
    TypeScript,
    Unknown,
}

#[derive(Debug, Clone)]
struct WorkspaceFacts {
    files: Vec<String>,
    edited_files: HashSet<String>,
    observed_stacks: BTreeSet<StackKind>,
}

pub(super) fn probe_completion(
    work_root: &Path,
    request: &str,
    contract: &TaskContract,
    scope: &TaskWorkspaceScope,
    edited_files: &HashSet<String>,
) -> CompletionProbeDecision {
    let facts = WorkspaceFacts {
        files: collect_scoped_files(work_root, scope),
        edited_files: edited_files.clone(),
        observed_stacks: observed_stacks(work_root, edited_files),
    };
    if facts.files.is_empty() {
        return CompletionProbeDecision::KeepArtifactFlow;
    }

    if let Some(reason) = stack_mismatch_reason(request, &facts) {
        return CompletionProbeDecision::RejectStackMismatch { reason };
    }

    for role in &contract.required_artifacts {
        if !contract_required_role_has_current_artifact(work_root, &facts, contract, *role) {
            return CompletionProbeDecision::KeepArtifactFlow;
        }
    }

    let Some(project_unit) = build_project_unit(work_root, &facts) else {
        return CompletionProbeDecision::KeepArtifactFlow;
    };

    if project_unit.verifier_candidates.is_empty() {
        return CompletionProbeDecision::KeepArtifactFlow;
    }

    CompletionProbeDecision::RunVerifier {
        reason: format!(
            "current-turn implementation, test, docs/setup artifacts and a safe verifier candidate are present; {}",
            project_unit.summary()
        ),
        project_unit,
    }
}

pub(super) fn probe_project_unit(
    work_root: &Path,
    scope: &TaskWorkspaceScope,
    edited_files: &HashSet<String>,
) -> Option<ProjectUnit> {
    let facts = WorkspaceFacts {
        files: collect_scoped_files(work_root, scope),
        edited_files: edited_files.clone(),
        observed_stacks: observed_stacks(work_root, edited_files),
    };
    if facts.files.is_empty() {
        return None;
    }
    build_project_unit(work_root, &facts)
}

pub(super) fn probe_project_unit_for_request(
    work_root: &Path,
    request: &str,
    scope: &TaskWorkspaceScope,
    edited_files: &HashSet<String>,
) -> Option<ProjectUnit> {
    probe_project_unit_for_request_with_evidence_hint(work_root, request, scope, edited_files, None)
}

pub(super) fn probe_project_unit_for_request_with_evidence_hint(
    work_root: &Path,
    request: &str,
    scope: &TaskWorkspaceScope,
    edited_files: &HashSet<String>,
    evidence_command_hint: Option<&str>,
) -> Option<ProjectUnit> {
    let facts = WorkspaceFacts {
        files: collect_scoped_files(work_root, scope),
        edited_files: edited_files.clone(),
        observed_stacks: observed_stacks(work_root, edited_files),
    };
    if facts.files.is_empty() {
        return None;
    }
    if stack_mismatch_reason(request, &facts).is_some() {
        return None;
    }
    let mut unit = build_project_unit(work_root, &facts)?;
    constrain_project_unit_to_request(request, &mut unit)?;
    apply_evidence_command_preference(evidence_command_hint, &mut unit);
    Some(unit)
}

fn collect_scoped_files(work_root: &Path, scope: &TaskWorkspaceScope) -> Vec<String> {
    let root_canon = std::fs::canonicalize(work_root).unwrap_or_else(|_| work_root.to_path_buf());
    let mut out = Vec::new();
    collect_files_inner(work_root, work_root, &root_canon, scope, 0, &mut out);
    out.sort();
    out.dedup();
    out
}

fn collect_files_inner(
    work_root: &Path,
    dir: &Path,
    root_canon: &Path,
    scope: &TaskWorkspaceScope,
    depth: usize,
    out: &mut Vec<String>,
) {
    if depth > MAX_PROBE_DEPTH || out.len() >= MAX_PROBE_FILES {
        return;
    }
    let Ok(read_dir) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in read_dir.flatten() {
        if out.len() >= MAX_PROBE_FILES {
            return;
        }
        let path = entry.path();
        let Ok(relative) = path.strip_prefix(work_root) else {
            continue;
        };
        if !is_workspace_artifact_admitted_relative_path(relative) {
            continue;
        }
        let rel = relative.to_string_lossy().replace('\\', "/");
        if !scope.contains(&rel) {
            continue;
        }
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            collect_files_inner(work_root, &path, root_canon, scope, depth + 1, out);
            continue;
        }
        if !file_type.is_file() {
            continue;
        }
        let Ok(canon) = std::fs::canonicalize(&path) else {
            continue;
        };
        if canon.strip_prefix(root_canon).is_err() {
            continue;
        }
        out.push(rel);
    }
}

fn role_has_current_artifact(work_root: &Path, facts: &WorkspaceFacts, role: ArtifactRole) -> bool {
    facts
        .files
        .iter()
        .any(|path| facts.edited_files.contains(path) && file_matches_role(work_root, path, role))
}

fn contract_required_role_has_current_artifact(
    work_root: &Path,
    facts: &WorkspaceFacts,
    contract: &TaskContract,
    role: ArtifactRole,
) -> bool {
    let identities = contract.required_identities_for_role(role);
    if identities.is_empty() {
        return role_has_current_artifact(work_root, facts, role);
    }
    identities.iter().all(|identity| {
        facts.files.iter().any(|path| {
            (role == ArtifactRole::Setup || facts.edited_files.contains(path))
                && super::task_contract::normalized_artifact_path_eq(path, &identity.path)
                && file_matches_role(work_root, path, role)
        })
    })
}

fn file_matches_role(work_root: &Path, relative_path: &str, role: ArtifactRole) -> bool {
    let p = Path::new(relative_path);
    match role {
        ArtifactRole::Implementation => {
            is_implementation_file(p) && !is_test_file(p) && !is_setup_file(p)
        }
        ArtifactRole::Test => is_test_file(p),
        ArtifactRole::UsageDocs => is_usage_doc_path(p),
        ArtifactRole::Setup => {
            is_setup_file(p)
                || relative_path == "Cargo.toml"
                || (work_root.join(relative_path).is_file()
                    && matches!(
                        p.file_name().and_then(|name| name.to_str()),
                        Some("go.mod" | "pom.xml" | "Gemfile" | "composer.json")
                    ))
        }
        ArtifactRole::DataOutput => is_structured_data_file(p),
    }
}

fn is_usage_doc_path(path: &Path) -> bool {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    file_name == "readme.md"
        || matches!(
            path.extension().and_then(|ext| ext.to_str()),
            Some("md" | "mdx" | "txt" | "rst")
        )
}

fn has_rust_verifier(work_root: &Path, facts: &WorkspaceFacts) -> bool {
    work_root.join("Cargo.toml").is_file()
        && facts.files.iter().any(|path| {
            facts.edited_files.contains(path)
                && is_test_file(Path::new(path))
                && path.ends_with(".rs")
        })
}

fn python_verifier_command(work_root: &Path, facts: &WorkspaceFacts) -> Option<&'static str> {
    let mut has_python_test = false;
    for path in facts.files.iter().filter(|path| {
        facts.edited_files.contains(*path) && is_test_file(Path::new(path)) && path.ends_with(".py")
    }) {
        has_python_test = true;
        if python_test_uses_unittest(work_root, path) {
            return Some("python3 -m unittest discover -s tests");
        }
    }
    has_python_test.then_some("python3 -B -m pytest -p no:cacheprovider")
}

fn python_test_uses_unittest(work_root: &Path, relative_path: &str) -> bool {
    let Ok(content) = std::fs::read_to_string(work_root.join(relative_path)) else {
        return false;
    };
    content.lines().take(120).any(|line| {
        let line = line.trim_start();
        line.starts_with("import unittest")
            || line.starts_with("from unittest")
            || line.contains("unittest.TestCase")
    })
}

fn has_node_verifier(work_root: &Path, facts: &WorkspaceFacts) -> bool {
    package_json_has_test_script(work_root)
        && facts.files.iter().any(|path| {
            facts.edited_files.contains(path)
                && is_test_file(Path::new(path))
                && matches!(
                    Path::new(path).extension().and_then(|ext| ext.to_str()),
                    Some("js" | "jsx" | "ts" | "tsx" | "mjs" | "cjs")
                )
        })
}

fn build_project_unit(work_root: &Path, facts: &WorkspaceFacts) -> Option<ProjectUnit> {
    let mut artifact_roles = BTreeSet::new();
    for role in [
        ArtifactRole::Implementation,
        ArtifactRole::Test,
        ArtifactRole::UsageDocs,
        ArtifactRole::Setup,
    ] {
        if role_has_current_artifact(work_root, facts, role) {
            artifact_roles.insert(role);
        }
    }
    if artifact_roles.is_empty() {
        return None;
    }
    let verifier_candidates = verifier_candidates(work_root, facts);
    let confidence = project_unit_confidence(&artifact_roles, &verifier_candidates);
    Some(ProjectUnit {
        root: ".".to_string(),
        manifests: project_manifests(work_root, facts),
        artifact_roles,
        verifier_candidates,
        observed_stacks: facts
            .observed_stacks
            .iter()
            .copied()
            .filter(|stack| *stack != StackKind::Unknown)
            .map(stack_label)
            .collect(),
        confidence,
    })
}

fn project_unit_confidence(
    artifact_roles: &BTreeSet<ArtifactRole>,
    verifier_candidates: &[ProjectUnitVerifierCandidate],
) -> ProjectUnitConfidence {
    let has_impl = artifact_roles.contains(&ArtifactRole::Implementation);
    let has_test = artifact_roles.contains(&ArtifactRole::Test);
    let has_docs = artifact_roles.contains(&ArtifactRole::UsageDocs);
    let has_setup = artifact_roles.contains(&ArtifactRole::Setup);
    let has_verifier = !verifier_candidates.is_empty();
    if has_impl && has_test && has_verifier {
        ProjectUnitConfidence::High
    } else if has_verifier || ((has_impl || has_test) && (has_docs || has_setup)) {
        ProjectUnitConfidence::Medium
    } else {
        ProjectUnitConfidence::Low
    }
}

fn project_manifests(work_root: &Path, facts: &WorkspaceFacts) -> Vec<String> {
    let known = [
        "Cargo.toml",
        "pyproject.toml",
        "requirements.txt",
        "package.json",
        "tsconfig.json",
        "go.mod",
        "pom.xml",
        "Gemfile",
        "composer.json",
    ];
    known
        .into_iter()
        .filter(|path| work_root.join(path).is_file() || facts.files.iter().any(|p| p == path))
        .map(str::to_string)
        .collect()
}

fn verifier_candidates(
    work_root: &Path,
    facts: &WorkspaceFacts,
) -> Vec<ProjectUnitVerifierCandidate> {
    let mut out = Vec::new();
    if has_rust_verifier(work_root, facts) {
        out.push(ProjectUnitVerifierCandidate {
            command_preview: "cargo test".to_string(),
            source: "cargo_manifest",
            timeout_class: ProjectUnitTimeoutClass::ShortUnitTest,
        });
    }
    if let Some(command_preview) = python_verifier_command(work_root, facts) {
        out.push(ProjectUnitVerifierCandidate {
            command_preview: command_preview.to_string(),
            source: "python_tests",
            timeout_class: ProjectUnitTimeoutClass::ShortUnitTest,
        });
    }
    if has_node_verifier(work_root, facts) {
        out.push(ProjectUnitVerifierCandidate {
            command_preview: "npm test".to_string(),
            source: "package_json_scripts",
            timeout_class: ProjectUnitTimeoutClass::ShortUnitTest,
        });
    }
    out
}

fn apply_evidence_command_preference(evidence_command_hint: Option<&str>, unit: &mut ProjectUnit) {
    let Some(command_preview) =
        super::verifier_command_policy::canonical_project_unit_evidence_command(
            evidence_command_hint,
        )
    else {
        return;
    };
    for candidate in &mut unit.verifier_candidates {
        if candidate.source == "python_tests" {
            candidate.command_preview = command_preview.to_string();
        }
    }
}

fn constrain_project_unit_to_request(request: &str, unit: &mut ProjectUnit) -> Option<()> {
    let Some(expected) = expected_stack(request) else {
        return Some(());
    };
    let verifier_count_before = unit.verifier_candidates.len();
    unit.verifier_candidates
        .retain(|candidate| verifier_candidate_matches_expected_stack(candidate, expected));
    if !unit.verifier_candidates.is_empty()
        || project_unit_has_expected_stack_signal(unit, expected)
        || verifier_count_before == 0
    {
        return Some(());
    }
    None
}

fn verifier_candidate_matches_expected_stack(
    candidate: &ProjectUnitVerifierCandidate,
    expected: StackKind,
) -> bool {
    matches!(
        (candidate.source, expected),
        ("cargo_manifest", StackKind::Rust)
            | ("python_tests", StackKind::Python)
            | (
                "package_json_scripts",
                StackKind::Node | StackKind::TypeScript
            )
    )
}

fn project_unit_has_expected_stack_signal(unit: &ProjectUnit, expected: StackKind) -> bool {
    let expected_label = stack_label(expected);
    unit.observed_stacks.contains(&expected_label)
        || unit
            .manifests
            .iter()
            .any(|manifest| manifest_matches_stack(manifest, expected))
}

fn manifest_matches_stack(manifest: &str, expected: StackKind) -> bool {
    matches!(
        (manifest, expected),
        ("Cargo.toml", StackKind::Rust)
            | ("pyproject.toml" | "requirements.txt", StackKind::Python)
            | ("package.json", StackKind::Node | StackKind::TypeScript)
            | ("tsconfig.json", StackKind::TypeScript)
    )
}

fn package_json_has_test_script(work_root: &Path) -> bool {
    let Ok(raw) = std::fs::read_to_string(work_root.join("package.json")) else {
        return false;
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return false;
    };
    json.get("scripts")
        .and_then(serde_json::Value::as_object)
        .and_then(|scripts| scripts.get("test"))
        .and_then(serde_json::Value::as_str)
        .is_some_and(|script| !script.trim().is_empty())
}

fn stack_mismatch_reason(request: &str, facts: &WorkspaceFacts) -> Option<String> {
    let expected = expected_stack(request)?;
    if facts.observed_stacks.contains(&expected) {
        return None;
    }
    let mut observed = facts
        .observed_stacks
        .iter()
        .copied()
        .filter(|stack| *stack != StackKind::Unknown)
        .collect::<Vec<_>>();
    observed.sort();
    observed.dedup();
    if observed.is_empty() {
        return None;
    }
    Some(format!(
        "request expects {:?}, but current-turn implementation artifacts look like {:?}",
        expected, observed
    ))
}

fn expected_stack(request: &str) -> Option<StackKind> {
    let lower = request.to_ascii_lowercase();
    if lower.contains("rust") || request.contains("Rust") {
        Some(StackKind::Rust)
    } else if lower.contains("typescript") || lower.contains("type script") {
        Some(StackKind::TypeScript)
    } else if lower.contains("node") || lower.contains("javascript") {
        Some(StackKind::Node)
    } else if lower.contains("python") {
        Some(StackKind::Python)
    } else {
        None
    }
}

fn observed_stacks(work_root: &Path, edited_files: &HashSet<String>) -> BTreeSet<StackKind> {
    let mut out = BTreeSet::new();
    for path in edited_files {
        let p = Path::new(path);
        if is_test_file(p) || is_setup_file(p) || is_usage_doc_path(p) {
            continue;
        }
        let stack = match p.extension().and_then(|ext| ext.to_str()) {
            Some("rs") => StackKind::Rust,
            Some("py") => StackKind::Python,
            Some("ts" | "tsx") => StackKind::TypeScript,
            Some("js" | "jsx" | "mjs" | "cjs") => StackKind::Node,
            _ => StackKind::Unknown,
        };
        out.insert(stack);
    }
    if work_root.join("Cargo.toml").is_file() {
        out.insert(StackKind::Rust);
    }
    if work_root.join("pyproject.toml").is_file() {
        out.insert(StackKind::Python);
    }
    if work_root.join("tsconfig.json").is_file() {
        out.insert(StackKind::TypeScript);
    }
    if work_root.join("package.json").is_file() {
        out.insert(StackKind::Node);
    }
    out
}

fn stack_label(stack: StackKind) -> &'static str {
    match stack {
        StackKind::Python => "python",
        StackKind::Rust => "rust",
        StackKind::Node => "node",
        StackKind::TypeScript => "typescript",
        StackKind::Unknown => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope(root: &Path, request: &str) -> TaskWorkspaceScope {
        TaskWorkspaceScope::detect(root, request)
    }

    fn contract(request: &str) -> TaskContract {
        TaskContract::from_request(request)
    }

    fn edited(paths: &[&str]) -> HashSet<String> {
        paths.iter().map(|path| path.to_string()).collect()
    }

    #[test]
    fn rust_current_artifacts_allow_verifier() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path().join("src")).expect("src");
        std::fs::create_dir_all(dir.path().join("tests")).expect("tests");
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname='x'\nversion='0.0.0'\n",
        )
        .expect("manifest");
        std::fs::write(
            dir.path().join("src/lib.rs"),
            "pub fn add(a:i32,b:i32)->i32{a+b}",
        )
        .expect("lib");
        std::fs::write(dir.path().join("tests/add.rs"), "#[test] fn t(){}").expect("test");
        std::fs::write(dir.path().join("README.md"), "# Usage\ncargo test\n").expect("readme");

        let request = "Rustでライブラリを実装し、READMEとテストを書いてください";
        let decision = probe_completion(
            dir.path(),
            request,
            &contract(request),
            &scope(dir.path(), request),
            &edited(&["src/lib.rs", "tests/add.rs", "README.md"]),
        );
        let CompletionProbeDecision::RunVerifier {
            project_unit,
            reason,
        } = decision
        else {
            panic!("expected RunVerifier");
        };
        assert!(reason.contains("project_unit"));
        assert_eq!(project_unit.root, ".");
        assert_eq!(project_unit.manifests, vec!["Cargo.toml"]);
        assert!(
            project_unit
                .artifact_roles
                .contains(&ArtifactRole::Implementation)
        );
        assert!(project_unit.artifact_roles.contains(&ArtifactRole::Test));
        assert!(
            project_unit
                .artifact_roles
                .contains(&ArtifactRole::UsageDocs)
        );
        assert_eq!(project_unit.verifier_candidates.len(), 1);
        assert_eq!(project_unit.verifier_candidates[0].source, "cargo_manifest");
        assert_eq!(
            project_unit.verifier_candidates[0].timeout_class,
            ProjectUnitTimeoutClass::ShortUnitTest
        );
    }

    #[test]
    fn preexisting_artifacts_without_current_edits_do_not_allow_verifier() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path().join("src")).expect("src");
        std::fs::create_dir_all(dir.path().join("tests")).expect("tests");
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname='x'\nversion='0.0.0'\n",
        )
        .expect("manifest");
        std::fs::write(dir.path().join("src/lib.rs"), "").expect("lib");
        std::fs::write(dir.path().join("tests/add.rs"), "").expect("test");
        std::fs::write(dir.path().join("README.md"), "").expect("readme");

        let request = "Rustで実装し、READMEとテストを書いてください";
        let decision = probe_completion(
            dir.path(),
            request,
            &contract(request),
            &scope(dir.path(), request),
            &edited(&[]),
        );
        assert_eq!(decision, CompletionProbeDecision::KeepArtifactFlow);
    }

    #[test]
    fn explicit_impl_filename_identity_prevents_wrong_path_probe_promotion() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path().join("tests")).expect("tests");
        std::fs::write(dir.path().join("main.py"), "def put(): pass\n").expect("impl");
        std::fs::write(
            dir.path().join("tests/test_lru_cache.py"),
            "def test_cache(): pass\n",
        )
        .expect("test");
        std::fs::write(dir.path().join("README.md"), "pytest\n").expect("readme");

        let request = "Create lru_cache.py with tests and README.";
        let decision = probe_completion(
            dir.path(),
            request,
            &contract(request),
            &scope(dir.path(), request),
            &edited(&["main.py", "tests/test_lru_cache.py", "README.md"]),
        );
        assert_eq!(decision, CompletionProbeDecision::KeepArtifactFlow);
    }

    #[test]
    fn node_test_script_with_current_artifacts_allows_verifier() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path().join("tests")).expect("tests");
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"scripts":{"test":"node --test tests/*.test.js"}}"#,
        )
        .expect("package");
        std::fs::write(dir.path().join("index.js"), "module.exports = {}").expect("impl");
        std::fs::write(dir.path().join("tests/index.test.js"), "test('x',()=>{})").expect("test");
        std::fs::write(dir.path().join("README.md"), "npm test").expect("readme");

        let request = "Node.jsで小さなツールを実装し、READMEとテストを書いてください";
        let decision = probe_completion(
            dir.path(),
            request,
            &contract(request),
            &scope(dir.path(), request),
            &edited(&["index.js", "tests/index.test.js", "README.md"]),
        );
        assert!(matches!(
            decision,
            CompletionProbeDecision::RunVerifier { .. }
        ));
    }

    #[test]
    fn multi_directory_python_project_unit_preserves_artifact_roles() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path().join("backend/app")).expect("app");
        std::fs::create_dir_all(dir.path().join("backend/tests")).expect("tests");
        std::fs::write(
            dir.path().join("pyproject.toml"),
            "[project]\nname='x'\nversion='0.0.0'\n",
        )
        .expect("pyproject");
        std::fs::write(
            dir.path().join("backend/app/main.py"),
            "def run(): return 1",
        )
        .expect("impl");
        std::fs::write(
            dir.path().join("backend/tests/test_main.py"),
            "def test_run(): pass",
        )
        .expect("test");
        std::fs::write(dir.path().join("backend/README.md"), "pytest").expect("readme");

        let request =
            "backend ディレクトリで Python のツールを実装し、READMEとテストを書いてください";
        let decision = probe_completion(
            dir.path(),
            request,
            &contract(request),
            &scope(dir.path(), request),
            &edited(&[
                "backend/app/main.py",
                "backend/tests/test_main.py",
                "backend/README.md",
            ]),
        );

        let CompletionProbeDecision::RunVerifier { project_unit, .. } = decision else {
            panic!("expected RunVerifier");
        };
        assert_eq!(project_unit.root, ".");
        assert_eq!(project_unit.manifests, vec!["pyproject.toml"]);
        assert!(project_unit.observed_stacks.contains(&"python"));
        assert!(
            project_unit
                .artifact_roles
                .contains(&ArtifactRole::Implementation)
        );
        assert!(project_unit.artifact_roles.contains(&ArtifactRole::Test));
        assert_eq!(project_unit.verifier_candidates.len(), 1);
        assert_eq!(project_unit.verifier_candidates[0].source, "python_tests");
    }

    #[test]
    fn requested_stack_mismatch_is_rejected() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path().join("tests")).expect("tests");
        std::fs::write(dir.path().join("main.py"), "print('x')").expect("impl");
        std::fs::write(dir.path().join("tests/test_main.py"), "def test_x(): pass").expect("test");
        std::fs::write(dir.path().join("README.md"), "pytest").expect("readme");

        let request = "TypeScriptで実装し、READMEとテストを書いてください";
        let decision = probe_completion(
            dir.path(),
            request,
            &contract(request),
            &scope(dir.path(), request),
            &edited(&["main.py", "tests/test_main.py", "README.md"]),
        );
        assert!(matches!(
            decision,
            CompletionProbeDecision::RejectStackMismatch { .. }
        ));
    }

    #[test]
    fn ignored_generated_dirs_do_not_count() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path().join(".anvil-state/tests")).expect("ignored");
        std::fs::write(
            dir.path().join(".anvil-state/tests/test_main.py"),
            "def test_x(): pass",
        )
        .expect("test");
        std::fs::write(dir.path().join("main.py"), "print('x')").expect("impl");
        std::fs::write(dir.path().join("README.md"), "pytest").expect("readme");

        let request = "Pythonで実装し、READMEとテストを書いてください";
        let decision = probe_completion(
            dir.path(),
            request,
            &contract(request),
            &scope(dir.path(), request),
            &edited(&["main.py", ".anvil-state/tests/test_main.py", "README.md"]),
        );
        assert_eq!(decision, CompletionProbeDecision::KeepArtifactFlow);
    }

    #[test]
    fn controller_state_manifest_does_not_create_project_unit() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path().join(".anvil-state/tests")).expect("ignored");
        std::fs::write(
            dir.path().join(".anvil-state/Cargo.toml"),
            "[package]\nname='ignored'\nversion='0.0.0'\n",
        )
        .expect("ignored manifest");
        std::fs::write(
            dir.path().join(".anvil-state/tests/generated.rs"),
            "#[test] fn generated(){}",
        )
        .expect("ignored test");
        std::fs::write(dir.path().join("README.md"), "docs").expect("readme");

        let request = "Rustで実装し、READMEとテストを書いてください";
        let decision = probe_completion(
            dir.path(),
            request,
            &contract(request),
            &scope(dir.path(), request),
            &edited(&[
                ".anvil-state/Cargo.toml",
                ".anvil-state/tests/generated.rs",
                "README.md",
            ]),
        );
        assert_eq!(decision, CompletionProbeDecision::KeepArtifactFlow);
    }

    #[test]
    fn docs_only_project_unit_has_no_verifier_and_low_confidence() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("README.md"), "# Notes\n").expect("readme");
        let request = "READMEだけを更新してください";
        let scope = scope(dir.path(), request);
        let edited = edited(&["README.md"]);

        let unit = probe_project_unit(dir.path(), &scope, &edited).expect("docs unit");

        assert!(unit.artifact_roles.contains(&ArtifactRole::UsageDocs));
        assert!(unit.verifier_candidates.is_empty());
        assert_eq!(unit.confidence, ProjectUnitConfidence::Low);
        assert!(unit.summary().contains("confidence=low"));
    }

    #[test]
    fn rust_request_does_not_accept_python_only_project_unit() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path().join("tests")).expect("tests");
        std::fs::write(
            dir.path().join("tests/test_main.py"),
            "def test_slug(): assert True\n",
        )
        .expect("python test");
        std::fs::write(dir.path().join("README.md"), "# Slug\npytest\n").expect("readme");

        let request = "文字列スラッグ生成用のRustライブラリを開発してください。README.mdに使用方法を書き、cargo testで動くテストコードも実装してください。";
        let scope = scope(dir.path(), request);
        let edited = edited(&["tests/test_main.py", "README.md"]);

        let generic_unit = probe_project_unit(dir.path(), &scope, &edited).expect("generic unit");
        assert_eq!(generic_unit.verifier_candidates[0].source, "python_tests");

        let request_unit = probe_project_unit_for_request(dir.path(), request, &scope, &edited);
        assert_eq!(request_unit, None);
    }

    #[test]
    fn rust_request_filters_unrelated_python_verifier_when_rust_manifest_exists() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path().join("src")).expect("src");
        std::fs::create_dir_all(dir.path().join("tests")).expect("tests");
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname='slug'\nversion='0.0.0'\nedition='2021'\n",
        )
        .expect("manifest");
        std::fs::write(
            dir.path().join("src/lib.rs"),
            "pub fn slug(s:&str)->String{s.into()}",
        )
        .expect("lib");
        std::fs::write(
            dir.path().join("tests/test_main.py"),
            "def test_slug(): assert True\n",
        )
        .expect("python test");
        std::fs::write(dir.path().join("README.md"), "# Slug\ncargo test\n").expect("readme");

        let request =
            "Rustでslugライブラリを実装し、READMEとcargo testで動くテストを書いてください";
        let scope = scope(dir.path(), request);
        let edited = edited(&["src/lib.rs", "tests/test_main.py", "README.md"]);

        let unit =
            probe_project_unit_for_request(dir.path(), request, &scope, &edited).expect("unit");
        assert!(unit.verifier_candidates.is_empty());
        assert!(unit.observed_stacks.contains(&"rust"));
    }

    #[test]
    fn typescript_request_can_use_package_json_verifier() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path().join("src")).expect("src");
        std::fs::create_dir_all(dir.path().join("tests")).expect("tests");
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"scripts":{"test":"node --test tests/*.test.ts"}}"#,
        )
        .expect("package");
        std::fs::write(dir.path().join("src/index.ts"), "export const x = 1;").expect("impl");
        std::fs::write(dir.path().join("tests/index.test.ts"), "test('x',()=>{})").expect("test");
        std::fs::write(dir.path().join("README.md"), "npm test").expect("readme");

        let request = "TypeScriptで小さなライブラリを実装し、READMEとテストを書いてください";
        let scope = scope(dir.path(), request);
        let edited = edited(&["src/index.ts", "tests/index.test.ts", "README.md"]);

        let unit =
            probe_project_unit_for_request(dir.path(), request, &scope, &edited).expect("unit");
        assert_eq!(unit.verifier_candidates.len(), 1);
        assert_eq!(unit.verifier_candidates[0].source, "package_json_scripts");
    }

    #[test]
    fn evidence_command_hint_prefers_unittest_project_unit_verifier() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path().join("tests")).expect("tests");
        std::fs::write(
            dir.path().join("math_utils.py"),
            "def clamp(v, lo, hi): return v",
        )
        .expect("impl");
        std::fs::write(
            dir.path().join("tests/test_math_utils.py"),
            "import unittest\n\nclass TestClamp(unittest.TestCase):\n    def test_smoke(self):\n        self.assertTrue(True)\n",
        )
        .expect("test");

        let request = "Create math_utils.py and tests/test_math_utils.py.";
        let scope = scope(dir.path(), request);
        let edited = edited(&["math_utils.py", "tests/test_math_utils.py"]);

        let unit = probe_project_unit_for_request_with_evidence_hint(
            dir.path(),
            request,
            &scope,
            &edited,
            Some("python -m unittest discover -s tests"),
        )
        .expect("unit");

        assert_eq!(unit.verifier_candidates.len(), 1);
        assert_eq!(unit.verifier_candidates[0].source, "python_tests");
        assert_eq!(
            unit.verifier_candidates[0].command_preview,
            "python3 -m unittest discover -s tests"
        );
    }

    #[test]
    fn python_unittest_artifact_selects_stdlib_unittest_verifier() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path().join("tests")).expect("tests");
        std::fs::write(
            dir.path().join("math_utils.py"),
            "def clamp(value, minimum, maximum):\n    return max(minimum, min(maximum, value))\n",
        )
        .expect("impl");
        std::fs::write(
            dir.path().join("tests/test_math_utils.py"),
            "import unittest\n\nfrom math_utils import clamp\n\nclass ClampTests(unittest.TestCase):\n    def test_clamps_low(self):\n        self.assertEqual(clamp(-1, 0, 5), 0)\n",
        )
        .expect("test");

        let request = "Create math_utils.py and tests/test_math_utils.py.";
        let scope = scope(dir.path(), request);
        let edited = edited(&["math_utils.py", "tests/test_math_utils.py"]);

        let unit = probe_project_unit_for_request(dir.path(), request, &scope, &edited)
            .expect("project unit");

        assert_eq!(unit.verifier_candidates.len(), 1);
        assert_eq!(unit.verifier_candidates[0].source, "python_tests");
        assert_eq!(
            unit.verifier_candidates[0].command_preview,
            "python3 -m unittest discover -s tests"
        );
    }

    #[test]
    fn project_unit_timeout_class_labels_are_stable() {
        assert_eq!(
            ProjectUnitTimeoutClass::ShortUnitTest.as_str(),
            "short_unit_test"
        );
        assert_eq!(
            ProjectUnitTimeoutClass::BuildCommand.as_str(),
            "build_command"
        );
        assert_eq!(
            ProjectUnitTimeoutClass::DependencySetup.as_str(),
            "dependency_setup"
        );
        assert_eq!(ProjectUnitTimeoutClass::Unknown.as_str(), "unknown");
        assert_eq!(ProjectUnitConfidence::High.as_str(), "high");
        assert_eq!(ProjectUnitConfidence::Medium.as_str(), "medium");
        assert_eq!(ProjectUnitConfidence::Low.as_str(), "low");
    }
}
