use std::path::{Component, Path, PathBuf};

use super::task_contract::{TaskContract, TaskKind};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct GeneratedTestPreflightDiagnostic {
    pub(super) path: String,
    pub(super) failure_kind: GeneratedTestPreflightFailureKind,
    pub(super) detail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum GeneratedTestPreflightFailureKind {
    MissingFile,
    UnsafePath,
    RacySharedFixture,
    BrittleRustBinaryProbe,
    UnsupportedContractAssertion,
}

impl GeneratedTestPreflightFailureKind {
    #[cfg(test)]
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::MissingFile => "test_bug",
            Self::UnsafePath => "test_bug",
            Self::RacySharedFixture => "test_bug",
            Self::BrittleRustBinaryProbe => "test_bug",
            Self::UnsupportedContractAssertion => "test_bug",
        }
    }
}

pub(super) fn filter_owned_test_artifacts_for_verifier(
    work_root: &Path,
    contract: &TaskContract,
    candidates: &[String],
) -> Vec<String> {
    candidates
        .iter()
        .filter(|path| generated_test_preflight(work_root, contract, path).is_ok())
        .cloned()
        .collect()
}

pub(super) fn generated_test_preflight(
    work_root: &Path,
    contract: &TaskContract,
    relative_path: &str,
) -> Result<(), GeneratedTestPreflightDiagnostic> {
    let Some(path) = safe_relative_path(work_root, relative_path) else {
        return Err(diagnostic(
            relative_path,
            GeneratedTestPreflightFailureKind::UnsafePath,
            "test path is absolute or escapes the workspace",
        ));
    };
    if !path.is_file() {
        return Err(diagnostic(
            relative_path,
            GeneratedTestPreflightFailureKind::MissingFile,
            "owned test artifact does not exist on disk",
        ));
    }
    let Ok(source) = std::fs::read_to_string(&path) else {
        return Ok(());
    };
    if is_racy_shared_fixture_test(&source) {
        return Err(diagnostic(
            relative_path,
            GeneratedTestPreflightFailureKind::RacySharedFixture,
            "test writes a fixed fixture under the project root and can race under parallel execution",
        ));
    }
    if is_brittle_rust_binary_probe(relative_path, &source) {
        return Err(diagnostic(
            relative_path,
            GeneratedTestPreflightFailureKind::BrittleRustBinaryProbe,
            "rust integration test derives a binary path from current_exe instead of Cargo-provided env",
        ));
    }
    if asserts_unsupported_non_ascii_contract(contract, &source) {
        return Err(diagnostic(
            relative_path,
            GeneratedTestPreflightFailureKind::UnsupportedContractAssertion,
            "generated test asserts non-ASCII behavior not present in the task contract",
        ));
    }
    Ok(())
}

fn diagnostic(
    path: &str,
    failure_kind: GeneratedTestPreflightFailureKind,
    detail: &str,
) -> GeneratedTestPreflightDiagnostic {
    GeneratedTestPreflightDiagnostic {
        path: path.to_string(),
        failure_kind,
        detail: detail.to_string(),
    }
}

fn safe_relative_path(work_root: &Path, relative_path: &str) -> Option<PathBuf> {
    let path = Path::new(relative_path);
    if path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::ParentDir | Component::RootDir))
    {
        return None;
    }
    Some(work_root.join(path))
}

fn is_racy_shared_fixture_test(source: &str) -> bool {
    let writes_fixture = source.contains("fs::write")
        || source.contains("std::fs::write")
        || source.contains("File::create")
        || source.contains("writeFileSync")
        || source.contains("write_text(");
    if !writes_fixture {
        return false;
    }
    let project_root_fixture = source.contains("CARGO_MANIFEST_DIR")
        || source.contains("__dirname")
        || source.contains("Path.cwd()")
        || source.contains("process.cwd()");
    let fixed_data_name = [".jsonl", ".ndjson", ".csv", ".tsv", ".txt"]
        .iter()
        .any(|suffix| source.contains(suffix));
    project_root_fixture && fixed_data_name && !source.contains("tempdir")
}

fn is_brittle_rust_binary_probe(relative_path: &str, source: &str) -> bool {
    relative_path.ends_with(".rs")
        && source.contains("current_exe()")
        && (source.matches("parent()").count() >= 2 || source.contains("join(\"target\")"))
        && source.contains("join(\"debug\")")
}

fn asserts_unsupported_non_ascii_contract(contract: &TaskContract, source: &str) -> bool {
    if contract.task_kind != TaskKind::Coding || contract_mentions_non_ascii(contract) {
        return false;
    }
    source
        .split('"')
        .skip(1)
        .step_by(2)
        .any(|literal| !literal.is_ascii())
}

fn contract_mentions_non_ascii(contract: &TaskContract) -> bool {
    contract
        .required_behavior
        .domain_terms
        .as_ref()
        .is_some_and(|terms| terms.iter().any(|term| !term.is_ascii()))
        || contract
            .required_behavior
            .behavior_goal
            .as_ref()
            .and_then(|goal| goal.excerpt.as_ref())
            .is_some_and(|excerpt| !excerpt.is_ascii())
        || contract
            .required_behavior
            .non_goals
            .as_ref()
            .is_some_and(|items| {
                items
                    .iter()
                    .filter_map(|item| item.excerpt.as_ref())
                    .any(|excerpt| !excerpt.is_ascii())
            })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_test_file(root: &Path, relative: &str, contents: &str) {
        let path = root.join(relative);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("create parent");
        std::fs::write(path, contents).expect("write fixture");
    }

    #[test]
    fn preflight_rejects_missing_test_file_as_test_bug() {
        let root = tempfile::tempdir().expect("tempdir");
        let contract = TaskContract::from_request("Implement a Rust CLI and add tests");
        let err = generated_test_preflight(root.path(), &contract, "tests/cli.rs")
            .expect_err("missing test rejected");

        assert_eq!(err.failure_kind.as_str(), "test_bug");
        assert_eq!(
            err.failure_kind,
            GeneratedTestPreflightFailureKind::MissingFile
        );
    }

    #[test]
    fn preflight_rejects_rust_current_exe_binary_probe() {
        let root = tempfile::tempdir().expect("tempdir");
        write_test_file(
            root.path(),
            "tests/cli.rs",
            r#"#[test]
fn runs_cli() {
    let bin = std::env::current_exe().unwrap().parent().unwrap().parent().unwrap().join("debug").join("wordcount");
    assert!(bin.exists());
}
"#,
        );
        let contract =
            TaskContract::from_request("Implement a Rust word counter CLI and add tests");
        let err = generated_test_preflight(root.path(), &contract, "tests/cli.rs")
            .expect_err("brittle binary path rejected");

        assert_eq!(
            err.failure_kind,
            GeneratedTestPreflightFailureKind::BrittleRustBinaryProbe
        );
    }

    #[test]
    fn preflight_rejects_project_root_shared_fixture_writes() {
        let root = tempfile::tempdir().expect("tempdir");
        write_test_file(
            root.path(),
            "tests/cli.rs",
            r#"#[test]
fn reads_file() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("test_input.jsonl");
    std::fs::write(&path, "{\"x\":1}\n").unwrap();
}
"#,
        );
        let contract = TaskContract::from_request("Implement a Rust NDJSON CLI and add tests");
        let err = generated_test_preflight(root.path(), &contract, "tests/cli.rs")
            .expect_err("racy shared fixture rejected");

        assert_eq!(
            err.failure_kind,
            GeneratedTestPreflightFailureKind::RacySharedFixture
        );
    }

    #[test]
    fn preflight_rejects_unsupported_non_ascii_slug_assertions() {
        let root = tempfile::tempdir().expect("tempdir");
        write_test_file(
            root.path(),
            "tests/lib.rs",
            r#"#[test]
fn slugifies_ascii_only() {
    assert_eq!(slugify("Café Résumé"), "caf-rsum");
}
"#,
        );
        let contract = TaskContract::from_request("Implement a Rust slugify helper and add tests");
        let err = generated_test_preflight(root.path(), &contract, "tests/lib.rs")
            .expect_err("unsupported non-ascii assertion rejected");

        assert_eq!(
            err.failure_kind,
            GeneratedTestPreflightFailureKind::UnsupportedContractAssertion
        );
    }

    #[test]
    fn filter_keeps_healthy_owned_tests() {
        let root = tempfile::tempdir().expect("tempdir");
        write_test_file(
            root.path(),
            "tests/lib.rs",
            r#"#[test]
fn slugifies_ascii() {
    assert_eq!(slugify("Hello World"), "hello-world");
}
"#,
        );
        let contract = TaskContract::from_request("Implement a Rust slugify helper and add tests");
        let admitted = filter_owned_test_artifacts_for_verifier(
            root.path(),
            &contract,
            &["tests/lib.rs".to_string()],
        );

        assert_eq!(admitted, vec!["tests/lib.rs".to_string()]);
    }
}
