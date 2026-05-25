//! Generic project completion probe.
//!
//! This module does not decide that a task is done. It only answers whether the
//! current workspace has enough in-scope, current-turn artifacts to justify
//! running the verifier even when the legacy artifact projection is still
//! asking for another edit.

use std::collections::{BTreeSet, HashSet};
use std::path::Path;

use crate::util::file_classify::{is_implementation_file, is_setup_file, is_test_file};

use super::task_contract::{ArtifactRole, TaskContract};
use super::task_workspace_scope::TaskWorkspaceScope;

const MAX_PROBE_FILES: usize = 512;
const MAX_PROBE_DEPTH: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum CompletionProbeDecision {
    RunVerifier { reason: String },
    RejectStackMismatch { reason: String },
    KeepArtifactFlow,
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
        if !role_has_current_artifact(work_root, &facts, *role) {
            return CompletionProbeDecision::KeepArtifactFlow;
        }
    }

    if !has_verifier_candidate(work_root, &facts) {
        return CompletionProbeDecision::KeepArtifactFlow;
    }

    CompletionProbeDecision::RunVerifier {
        reason: "current-turn implementation, test, docs/setup artifacts and a safe verifier candidate are present".to_string(),
    }
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
        if crate::util::workspace_paths::is_ignored_workspace_relative_path(relative) {
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

fn has_verifier_candidate(work_root: &Path, facts: &WorkspaceFacts) -> bool {
    has_rust_verifier(work_root, facts)
        || has_python_verifier(facts)
        || has_node_verifier(work_root, facts)
}

fn has_rust_verifier(work_root: &Path, facts: &WorkspaceFacts) -> bool {
    work_root.join("Cargo.toml").is_file()
        && facts.files.iter().any(|path| {
            facts.edited_files.contains(path)
                && is_test_file(Path::new(path))
                && path.ends_with(".rs")
        })
}

fn has_python_verifier(facts: &WorkspaceFacts) -> bool {
    facts.files.iter().any(|path| {
        facts.edited_files.contains(path) && is_test_file(Path::new(path)) && path.ends_with(".py")
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
        assert!(matches!(
            decision,
            CompletionProbeDecision::RunVerifier { .. }
        ));
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
}
