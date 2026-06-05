use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

use super::task_contract::{ArtifactRole, TaskContract};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct GeneratedTestPreflightDiagnostic {
    pub(super) path: String,
    pub(super) failure_kind: GeneratedTestPreflightFailureKind,
    pub(super) detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct GeneratedTestPreflightReport {
    pub(super) admitted: Vec<String>,
    pub(super) rejected: Vec<GeneratedTestPreflightDiagnostic>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum GeneratedTestPreflightFailureKind {
    MissingFile,
    UnsafePath,
    InvalidManifest,
    MissingBinaryPath,
    UnrunnableCommand,
    RacySharedFixture,
    BrittleRustBinaryProbe,
    UnsupportedContractAssertion,
    MissingContractCoverage,
}

impl GeneratedTestPreflightFailureKind {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::MissingFile => "test_bug",
            Self::UnsafePath => "test_bug",
            Self::InvalidManifest => "test_bug",
            Self::MissingBinaryPath => "test_bug",
            Self::UnrunnableCommand => "test_bug",
            Self::RacySharedFixture => "test_bug",
            Self::BrittleRustBinaryProbe => "test_bug",
            Self::UnsupportedContractAssertion => "test_bug",
            Self::MissingContractCoverage => "test_bug",
        }
    }

    pub(super) fn reason_code(self) -> &'static str {
        match self {
            Self::MissingFile => "missing_file",
            Self::UnsafePath => "unsafe_path",
            Self::InvalidManifest => "invalid_manifest",
            Self::MissingBinaryPath => "missing_binary_path",
            Self::UnrunnableCommand => "unrunnable_command",
            Self::RacySharedFixture => "racy_shared_fixture",
            Self::BrittleRustBinaryProbe => "brittle_rust_binary_probe",
            Self::UnsupportedContractAssertion => "unsupported_contract_assertion",
            Self::MissingContractCoverage => "missing_contract_coverage",
        }
    }
}

#[cfg(test)]
pub(super) fn filter_owned_test_artifacts_for_verifier(
    work_root: &Path,
    contract: &TaskContract,
    candidates: &[String],
) -> Vec<String> {
    preflight_owned_test_artifacts_for_verifier(work_root, contract, candidates).admitted
}

pub(super) fn preflight_owned_test_artifacts_for_verifier(
    work_root: &Path,
    contract: &TaskContract,
    candidates: &[String],
) -> GeneratedTestPreflightReport {
    let mut admitted = Vec::new();
    let mut rejected = Vec::new();
    let mut admitted_sources = Vec::new();
    for path in candidates {
        match generated_test_preflight_with_source(work_root, contract, path) {
            Ok(source) => {
                admitted.push(path.clone());
                if let Some(source) = source {
                    admitted_sources.push((path.clone(), source));
                }
            }
            Err(diagnostic) => rejected.push(diagnostic),
        }
    }
    if !admitted.is_empty()
        && let Some(detail) = generated_suite_contract_coverage_gap(contract, &admitted_sources)
    {
        rejected.extend(admitted.iter().map(|path| {
            diagnostic(
                path,
                GeneratedTestPreflightFailureKind::MissingContractCoverage,
                &detail,
            )
        }));
        admitted.clear();
    }
    GeneratedTestPreflightReport { admitted, rejected }
}

#[cfg(test)]
pub(super) fn generated_test_preflight(
    work_root: &Path,
    contract: &TaskContract,
    relative_path: &str,
) -> Result<(), GeneratedTestPreflightDiagnostic> {
    let source = generated_test_preflight_with_source(work_root, contract, relative_path)?;
    if let Some(source) = source
        && let Some(detail) =
            generated_suite_contract_coverage_gap(contract, &[(relative_path.to_string(), source)])
    {
        return Err(diagnostic(
            relative_path,
            GeneratedTestPreflightFailureKind::MissingContractCoverage,
            &detail,
        ));
    }
    Ok(())
}

fn generated_test_preflight_with_source(
    work_root: &Path,
    contract: &TaskContract,
    relative_path: &str,
) -> Result<Option<String>, GeneratedTestPreflightDiagnostic> {
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
        return Ok(None);
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
    if rust_test_references_undeclared_cargo_binary(work_root, relative_path, &source) {
        return Err(diagnostic(
            relative_path,
            GeneratedTestPreflightFailureKind::MissingBinaryPath,
            "rust generated test references a Cargo binary target not declared by Cargo.toml",
        ));
    }
    if let Some(binary_path) = missing_literal_binary_path(work_root, &source) {
        return Err(diagnostic(
            relative_path,
            GeneratedTestPreflightFailureKind::MissingBinaryPath,
            &format!("generated test invokes non-existent binary path: {binary_path}"),
        ));
    }
    if let Some(program) = unrunnable_command_literal(&source) {
        return Err(diagnostic(
            relative_path,
            GeneratedTestPreflightFailureKind::UnrunnableCommand,
            &format!(
                "generated test passes an unrunnable program literal to Command::new: {program}"
            ),
        ));
    }
    if asserts_unsupported_non_ascii_contract(contract, &source) {
        return Err(diagnostic(
            relative_path,
            GeneratedTestPreflightFailureKind::UnsupportedContractAssertion,
            "generated test asserts non-ASCII behavior not present in the task contract",
        ));
    }
    if rust_test_requires_cargo_manifest(relative_path, &source)
        && let Some(detail) = invalid_cargo_manifest_detail(work_root)
    {
        return Err(diagnostic(
            relative_path,
            GeneratedTestPreflightFailureKind::InvalidManifest,
            &detail,
        ));
    }
    if javascript_test_requires_package_manifest(relative_path)
        && let Some(detail) = invalid_package_manifest_detail(work_root)
    {
        return Err(diagnostic(
            relative_path,
            GeneratedTestPreflightFailureKind::InvalidManifest,
            &detail,
        ));
    }
    Ok(Some(source))
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

fn rust_test_requires_cargo_manifest(relative_path: &str, source: &str) -> bool {
    relative_path.ends_with(".rs")
        && (relative_path.starts_with("tests/")
            || source.contains("CARGO_MANIFEST_DIR")
            || source.contains("cargo")
            || source.contains("Command::cargo_bin"))
}

fn invalid_cargo_manifest_detail(work_root: &Path) -> Option<String> {
    let manifest = work_root.join("Cargo.toml");
    let source = match std::fs::read_to_string(&manifest) {
        Ok(source) => source,
        Err(_) => {
            return Some("Cargo.toml is missing for a Rust generated test".to_string());
        }
    };
    let mut in_package = false;
    let mut saw_package = false;
    let mut saw_name = false;
    for raw_line in source.lines() {
        let line = strip_toml_comment(raw_line).trim();
        if line.starts_with('[') {
            if !line.ends_with(']') {
                return Some("Cargo.toml contains a malformed table header".to_string());
            }
            in_package = line == "[package]";
            saw_package |= in_package;
            continue;
        }
        if in_package
            && let Some((key, value)) = line.split_once('=')
            && key.trim() == "name"
        {
            let Some(value) = parse_toml_string_value(value.trim()) else {
                return Some("Cargo.toml [package] name is malformed".to_string());
            };
            saw_name = !value.is_empty();
        }
    }
    if !saw_package {
        return Some("Cargo.toml does not contain a [package] section".to_string());
    }
    if !saw_name {
        return Some("Cargo.toml [package] does not declare a non-empty name".to_string());
    }
    None
}

fn rust_test_references_undeclared_cargo_binary(
    work_root: &Path,
    relative_path: &str,
    source: &str,
) -> bool {
    if !relative_path.ends_with(".rs") {
        return false;
    }
    let referenced = referenced_cargo_binary_names(source);
    if referenced.is_empty() {
        return false;
    }
    let manifest = declared_cargo_binary_names(work_root);
    referenced.iter().any(|name| !manifest.contains(name))
}

fn referenced_cargo_binary_names(source: &str) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for rest in source.split("CARGO_BIN_EXE_").skip(1) {
        let name = rest
            .chars()
            .take_while(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
            .collect::<String>();
        if !name.is_empty() {
            names.insert(name);
        }
    }
    for marker in ["cargo_bin(\"", "cargo_bin!(\""] {
        let mut tail = source;
        while let Some(idx) = tail.find(marker) {
            let after = &tail[idx + marker.len()..];
            let Some(end) = after.find('"') else {
                break;
            };
            let name = &after[..end];
            if !name.is_empty()
                && name
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
            {
                names.insert(name.to_string());
            }
            tail = &after[end + 1..];
        }
    }
    names
}

fn declared_cargo_binary_names(work_root: &Path) -> BTreeSet<String> {
    let Ok(source) = std::fs::read_to_string(work_root.join("Cargo.toml")) else {
        return BTreeSet::new();
    };
    let mut names = BTreeSet::new();
    let mut section = CargoManifestSection::Other;
    for raw_line in source.lines() {
        let line = strip_toml_comment(raw_line).trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            section = match line {
                "[package]" => CargoManifestSection::Package,
                "[[bin]]" => CargoManifestSection::Bin,
                _ => CargoManifestSection::Other,
            };
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key.trim() != "name" {
            continue;
        }
        let Some(name) = parse_toml_string_value(value.trim()) else {
            continue;
        };
        if matches!(
            section,
            CargoManifestSection::Package | CargoManifestSection::Bin
        ) {
            names.insert(name);
        }
    }
    names
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CargoManifestSection {
    Package,
    Bin,
    Other,
}

// Issue #989: shared with `evidence_binding.rs` (Cargo manifest binding plan).
// Pure TOML line helpers; `pub(super)` so the binding adapter reuses them
// instead of re-implementing ad hoc string mutation (DRY). Behavior unchanged.
pub(super) fn strip_toml_comment(line: &str) -> &str {
    let mut in_string = false;
    let mut escaped = false;
    for (idx, ch) in line.char_indices() {
        if ch == '"' && !escaped {
            in_string = !in_string;
        }
        if ch == '#' && !in_string {
            return &line[..idx];
        }
        escaped = ch == '\\' && !escaped;
        if ch != '\\' {
            escaped = false;
        }
    }
    line
}

pub(super) fn parse_toml_string_value(value: &str) -> Option<String> {
    let quoted = value.trim().strip_prefix('"')?;
    let end = quoted.find('"')?;
    Some(quoted[..end].to_string())
}

fn javascript_test_requires_package_manifest(relative_path: &str) -> bool {
    matches!(
        Path::new(relative_path)
            .extension()
            .and_then(|ext| ext.to_str()),
        Some("js" | "mjs" | "cjs" | "ts" | "tsx")
    )
}

fn invalid_package_manifest_detail(work_root: &Path) -> Option<String> {
    let manifest = work_root.join("package.json");
    if !manifest.is_file() {
        return None;
    }
    let source = std::fs::read_to_string(manifest).ok()?;
    serde_json::from_str::<serde_json::Value>(&source)
        .is_err()
        .then(|| "package.json is not valid JSON".to_string())
}

fn is_racy_shared_fixture_test(source: &str) -> bool {
    let writes_fixture = source.contains("fs::write")
        || source.contains("std::fs::write")
        || source.contains("File::create")
        || source.contains("OpenOptions::new")
        || source.contains("remove_file")
        || source.contains("writeFileSync")
        || source.contains("write_text(")
        || source.contains("open(");
    if !writes_fixture {
        return false;
    }
    let shared_location = source.contains("CARGO_MANIFEST_DIR")
        || source.contains("__dirname")
        || source.contains("Path.cwd()")
        || source.contains("process.cwd()")
        || source.contains("current_dir()")
        || source.contains("env::temp_dir()")
        || source.contains("std::env::temp_dir()");
    let fixed_data_name = double_quoted_literals(source).any(looks_like_fixed_fixture_name);
    let isolated_fixture = [
        "tempdir",
        "TempDir",
        "NamedTempFile",
        "mkdtemp",
        "tmp_path",
        "TemporaryDirectory",
    ]
    .iter()
    .any(|marker| source.contains(marker));
    (shared_location || writes_relative_fixed_fixture(source))
        && fixed_data_name
        && !isolated_fixture
}

fn writes_relative_fixed_fixture(source: &str) -> bool {
    double_quoted_literals(source).any(|literal| {
        let path = Path::new(literal);
        !path.is_absolute()
            && !literal.contains('/')
            && !literal.contains('\\')
            && looks_like_fixed_fixture_name(literal)
    })
}

fn looks_like_fixed_fixture_name(literal: &str) -> bool {
    let normalized = literal.replace('\\', "/");
    let name = normalized.rsplit('/').next().unwrap_or(normalized.as_str());
    let lower = name.to_ascii_lowercase();
    let fixed_prefix = lower.starts_with("test_")
        || lower.starts_with("tmp_")
        || lower.starts_with("fixture")
        || lower.starts_with("input")
        || lower.starts_with("output");
    let data_suffix = [
        ".jsonl", ".ndjson", ".json", ".csv", ".tsv", ".txt", ".log", ".db", ".sqlite",
    ]
    .iter()
    .any(|suffix| lower.ends_with(suffix));
    fixed_prefix && data_suffix
}

fn is_brittle_rust_binary_probe(relative_path: &str, source: &str) -> bool {
    relative_path.ends_with(".rs")
        && source.contains("current_exe()")
        && (source.matches("parent()").count() >= 2 || source.contains("join(\"target\")"))
        && source.contains("join(\"debug\")")
}

fn missing_literal_binary_path(work_root: &Path, source: &str) -> Option<String> {
    for literal in double_quoted_literals(source) {
        let normalized = literal.strip_prefix("./").unwrap_or(literal);
        let normalized = normalized.replace('\\', "/");
        if !(normalized.starts_with("target/debug/") || normalized.starts_with("target/release/")) {
            continue;
        }
        let candidate = work_root.join(&normalized);
        if !candidate.is_file() {
            return Some(normalized);
        }
    }
    None
}

fn unrunnable_command_literal(source: &str) -> Option<String> {
    command_new_literals(source).into_iter().find(|program| {
        program.split_whitespace().count() > 1
            || super::completion_evidence::contains_evidence_poisoning_shell_control(program)
    })
}

fn command_new_literals(source: &str) -> Vec<String> {
    let mut out = Vec::new();
    for marker in ["Command::new(", "process::Command::new("] {
        let mut tail = source;
        while let Some(idx) = tail.find(marker) {
            let after = &tail[idx + marker.len()..];
            let Some(start) = after.find('"') else {
                tail = after;
                continue;
            };
            let after_quote = &after[start + 1..];
            let Some(end) = after_quote.find('"') else {
                break;
            };
            out.push(after_quote[..end].to_string());
            tail = &after_quote[end + 1..];
        }
    }
    out
}

fn asserts_unsupported_non_ascii_contract(contract: &TaskContract, source: &str) -> bool {
    // Issue #918 (P1): route the coding-only gate through the capability spine
    // instead of a hardcoded `task_kind == Coding` comparison. `allows_process_exec()`
    // is the canonical "this is the Coding kind (the only kind with executable
    // verification tooling)" predicate, so this is a 1:1 behavior-preserving swap.
    if !super::verifier::capability_for(contract.task_kind).allows_process_exec()
        || contract_mentions_non_ascii(contract)
    {
        return false;
    }
    double_quoted_literals(source).any(|literal| !literal.is_ascii())
}

fn contract_mentions_non_ascii(contract: &TaskContract) -> bool {
    if contract
        .required_behavior
        .domain_terms
        .as_ref()
        .is_some_and(|terms| terms.iter().any(|term| !term.is_ascii()))
    {
        return true;
    }

    let mut text = String::new();
    if let Some(goal) = contract.required_behavior.behavior_goal.as_ref() {
        text.push_str(&goal.label);
        if let Some(excerpt) = goal.excerpt.as_ref() {
            text.push(' ');
            text.push_str(excerpt);
        }
    }
    for item in contract
        .required_behavior
        .required_capabilities
        .as_deref()
        .unwrap_or_default()
        .iter()
        .chain(
            contract
                .required_behavior
                .verification_expectations
                .as_deref()
                .unwrap_or_default()
                .iter(),
        )
    {
        text.push(' ');
        text.push_str(&item.label);
        if let Some(excerpt) = item.excerpt.as_ref() {
            text.push(' ');
            text.push_str(excerpt);
        }
    }

    let lower = text.to_ascii_lowercase();
    contains_any(
        &lower,
        &[
            "non-ascii",
            "non ascii",
            "unicode",
            "utf-8",
            "utf8",
            "accent",
            "diacritic",
            "international",
        ],
    ) || contains_any(
        &text,
        &[
            "非ASCII",
            "ユニコード",
            "アクセント",
            "全角",
            "マルチバイト",
        ],
    )
}

fn generated_suite_contract_coverage_gap(
    contract: &TaskContract,
    sources: &[(String, String)],
) -> Option<String> {
    // Issue #918 (P1): coding-only gate via the capability spine (see
    // `asserts_unsupported_non_ascii_contract`). `allows_process_exec()` ⟺ Coding.
    if !super::verifier::capability_for(contract.task_kind).allows_process_exec()
        || !contract.completion_policy.test_execution_required()
        || sources.is_empty()
    {
        return None;
    }
    let needles = contract_coverage_needles(contract);
    if needles.is_empty() {
        return None;
    }
    let combined = sources
        .iter()
        .map(|(_, source)| source.as_str())
        .collect::<Vec<_>>()
        .join("\n")
        .to_ascii_lowercase();

    let io_needles = explicit_io_coverage_needles(contract);
    if !io_needles.is_empty()
        && io_needles
            .iter()
            .any(|needle| !source_covers_required_term(&combined, needle))
    {
        return Some(
            "generated test suite does not cover an explicit task-contract I/O requirement"
                .to_string(),
        );
    }

    if needles
        .iter()
        .any(|needle| source_covers_required_term(&combined, needle))
    {
        return None;
    }
    Some(format!(
        "generated test suite does not cover any task-contract signal: {}",
        needles.join(",")
    ))
}

fn contract_coverage_needles(contract: &TaskContract) -> Vec<String> {
    let mut needles = Vec::new();
    needles.extend(
        explicit_io_coverage_needles(contract)
            .into_iter()
            .map(str::to_string),
    );
    // Operation labels are intentionally not used here: requests like
    // "add tests" can legitimately produce tests whose source never says
    // "add" or "create". Domain terms and explicit criteria are safer
    // static signals for generated-test coverage.
    if let Some(terms) = contract.required_behavior.domain_terms.as_ref() {
        needles.extend(
            terms
                .iter()
                .map(|term| term.to_ascii_lowercase())
                .filter(|term| coverage_term_is_informative(term)),
        );
    }
    for criterion in contract
        .required_artifact_identities
        .iter()
        .filter(|identity| identity.role == ArtifactRole::Test)
        .flat_map(|identity| identity.acceptance_criteria.iter())
    {
        needles.extend(
            criterion
                .split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_')
                .map(str::to_ascii_lowercase)
                .filter(|term| coverage_term_is_informative(term)),
        );
    }
    needles.sort();
    needles.dedup();
    needles
}

fn explicit_io_coverage_needles(contract: &TaskContract) -> Vec<&'static str> {
    let lower = contract_coverage_text(contract).to_ascii_lowercase();
    let mut out = Vec::new();
    if lower.contains("stdin") || lower.contains("standard input") {
        out.push("stdin");
    }
    if lower.contains("stdout") || lower.contains("standard output") {
        out.push("stdout");
    }
    if lower.contains("stderr") || lower.contains("standard error") {
        out.push("stderr");
    }
    out
}

fn contract_coverage_text(contract: &TaskContract) -> String {
    let mut text = String::new();
    if let Some(goal) = contract.required_behavior.behavior_goal.as_ref() {
        text.push_str(&goal.label);
        if let Some(excerpt) = goal.excerpt.as_ref() {
            text.push(' ');
            text.push_str(excerpt);
        }
    }
    if let Some(terms) = contract.required_behavior.domain_terms.as_ref() {
        for term in terms {
            text.push(' ');
            text.push_str(term);
        }
    }
    for identity in &contract.required_artifact_identities {
        for criterion in &identity.acceptance_criteria {
            text.push(' ');
            text.push_str(criterion);
        }
    }
    text
}

fn source_covers_required_term(source_lower: &str, needle: &str) -> bool {
    match needle {
        "stdin" => contains_any(source_lower, &["stdin", "write_stdin", "standard input"]),
        "stdout" => contains_any(source_lower, &["stdout", "standard output", "get_output"]),
        "stderr" => contains_any(source_lower, &["stderr", "standard error"]),
        other => {
            let lower = other.to_ascii_lowercase();
            source_lower.contains(&lower)
                || normalize_coverage_token(source_lower)
                    .contains(&normalize_coverage_token(&lower))
        }
    }
}

fn normalize_coverage_token(s: &str) -> String {
    s.chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .collect::<String>()
}

fn coverage_term_is_informative(term: &str) -> bool {
    if term.len() < 4 {
        return false;
    }
    !matches!(
        term,
        "test"
            | "tests"
            | "unit"
            | "spec"
            | "rust"
            | "node"
            | "python"
            | "code"
            | "file"
            | "files"
            | "main"
            | "src"
            | "impl"
            | "implementation"
            | "helper"
            | "feature"
            | "library"
            | "package"
            | "cargo"
            | "manifest"
            | "add"
            | "build"
            | "create"
            | "read"
            | "update"
            | "delete"
            | "implement"
            | "support"
    )
}

fn double_quoted_literals(source: &str) -> impl Iterator<Item = &str> {
    source.split('"').skip(1).step_by(2)
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_valid_cargo_manifest(root: &Path) {
        write_test_file(
            root,
            "Cargo.toml",
            r#"[package]
name = "generated-test-guard-fixture"
version = "0.1.0"
edition = "2021"
"#,
        );
    }

    fn write_valid_package_manifest(root: &Path) {
        write_test_file(
            root,
            "package.json",
            r#"{"scripts":{"test":"node --test"}}"#,
        );
    }

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
        write_valid_cargo_manifest(root.path());
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
        write_valid_cargo_manifest(root.path());
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
    fn preflight_rejects_relative_shared_fixture_writes() {
        let root = tempfile::tempdir().expect("tempdir");
        write_valid_cargo_manifest(root.path());
        write_test_file(
            root.path(),
            "tests/cli.rs",
            r#"#[test]
fn reads_file() {
    std::fs::write("test_input.jsonl", "{\"x\":1}\n").unwrap();
}
"#,
        );
        let contract = TaskContract::from_request("Implement a Rust NDJSON CLI and add tests");
        let err = generated_test_preflight(root.path(), &contract, "tests/cli.rs")
            .expect_err("relative racy shared fixture rejected");

        assert_eq!(
            err.failure_kind,
            GeneratedTestPreflightFailureKind::RacySharedFixture
        );
        assert_eq!(err.failure_kind.as_str(), "test_bug");
    }

    #[test]
    fn preflight_rejects_unsupported_non_ascii_slug_assertions() {
        let root = tempfile::tempdir().expect("tempdir");
        write_valid_cargo_manifest(root.path());
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
    fn preflight_allows_non_ascii_when_contract_requires_unicode() {
        let root = tempfile::tempdir().expect("tempdir");
        write_valid_cargo_manifest(root.path());
        write_test_file(
            root.path(),
            "tests/lib.rs",
            r#"#[test]
fn slugifies_unicode() {
    assert_eq!(slugify("Café Résumé"), "cafe-resume");
}
"#,
        );
        let contract = TaskContract::from_request(
            "Implement a Rust slugify helper with Unicode support and add tests",
        );

        generated_test_preflight(root.path(), &contract, "tests/lib.rs")
            .expect("explicit Unicode requirement should admit non-ASCII assertions");
    }

    #[test]
    fn preflight_rejects_invalid_rust_manifest() {
        let root = tempfile::tempdir().expect("tempdir");
        write_test_file(
            root.path(),
            "Cargo.toml",
            r#"[workspace]
members = []
"#,
        );
        write_test_file(
            root.path(),
            "tests/cli.rs",
            r#"#[test]
fn runs_cli() {
    assert!(env!("CARGO_MANIFEST_DIR").contains("fixture"));
}
"#,
        );
        let contract =
            TaskContract::from_request("Implement a Rust word counter CLI and add tests");
        let err = generated_test_preflight(root.path(), &contract, "tests/cli.rs")
            .expect_err("invalid manifest rejected");

        assert_eq!(
            err.failure_kind,
            GeneratedTestPreflightFailureKind::InvalidManifest
        );
        assert_eq!(err.failure_kind.as_str(), "test_bug");
    }

    #[test]
    fn preflight_rejects_invalid_package_json_manifest() {
        let root = tempfile::tempdir().expect("tempdir");
        write_test_file(root.path(), "package.json", r#"{"scripts":"#);
        write_test_file(
            root.path(),
            "tests/index.test.js",
            "import test from 'node:test';\ntest('smoke', () => {});\n",
        );
        let contract = TaskContract::from_request("Implement a Node CLI and add tests");
        let err = generated_test_preflight(root.path(), &contract, "tests/index.test.js")
            .expect_err("invalid package.json rejected");

        assert_eq!(
            err.failure_kind,
            GeneratedTestPreflightFailureKind::InvalidManifest
        );
        assert_eq!(err.failure_kind.as_str(), "test_bug");
    }

    #[test]
    fn preflight_allows_valid_package_json_manifest() {
        let root = tempfile::tempdir().expect("tempdir");
        write_valid_package_manifest(root.path());
        write_test_file(
            root.path(),
            "tests/index.test.js",
            "import test from 'node:test';\ntest('smoke', () => {});\n",
        );
        let contract = TaskContract::from_request("Implement a Node CLI and add tests");

        generated_test_preflight(root.path(), &contract, "tests/index.test.js")
            .expect("valid package.json should not block JS tests");
    }

    #[test]
    fn preflight_rejects_undeclared_cargo_binary_env() {
        let root = tempfile::tempdir().expect("tempdir");
        write_valid_cargo_manifest(root.path());
        write_test_file(
            root.path(),
            "tests/cli.rs",
            r#"#[test]
fn runs_cli() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_wordcount")).output().unwrap();
    assert!(output.status.success());
}
"#,
        );
        let contract =
            TaskContract::from_request("Implement a Rust word-counter CLI and add tests");
        let err = generated_test_preflight(root.path(), &contract, "tests/cli.rs")
            .expect_err("undeclared cargo binary rejected");

        assert_eq!(
            err.failure_kind,
            GeneratedTestPreflightFailureKind::MissingBinaryPath
        );
        assert_eq!(err.failure_kind.as_str(), "test_bug");
    }

    #[test]
    fn preflight_rejects_nonexistent_literal_binary_path() {
        let root = tempfile::tempdir().expect("tempdir");
        write_valid_cargo_manifest(root.path());
        write_test_file(
            root.path(),
            "tests/cli.rs",
            r#"#[test]
fn runs_cli() {
    let output = std::process::Command::new("target/debug/word-counter").output().unwrap();
    assert!(output.status.success());
}
"#,
        );
        let contract =
            TaskContract::from_request("Implement a Rust word counter CLI and add tests");
        let err = generated_test_preflight(root.path(), &contract, "tests/cli.rs")
            .expect_err("missing binary path rejected");

        assert_eq!(
            err.failure_kind,
            GeneratedTestPreflightFailureKind::MissingBinaryPath
        );
    }

    #[test]
    fn preflight_rejects_unrunnable_command_new_program_literal() {
        let root = tempfile::tempdir().expect("tempdir");
        write_valid_cargo_manifest(root.path());
        write_test_file(
            root.path(),
            "tests/cli.rs",
            r#"#[test]
fn runs_cli() {
    let output = std::process::Command::new("cargo test").output().unwrap();
    assert!(output.status.success());
}
"#,
        );
        let contract =
            TaskContract::from_request("Implement a Rust word counter CLI and add tests");
        let err = generated_test_preflight(root.path(), &contract, "tests/cli.rs")
            .expect_err("unrunnable Command::new literal rejected");

        assert_eq!(
            err.failure_kind,
            GeneratedTestPreflightFailureKind::UnrunnableCommand
        );
    }

    #[test]
    fn report_rejects_generated_suite_without_contract_coverage() {
        let root = tempfile::tempdir().expect("tempdir");
        write_valid_cargo_manifest(root.path());
        write_test_file(
            root.path(),
            "tests/repo.rs",
            r#"#[test]
fn unrelated_math() {
    assert_eq!(2 + 2, 4);
}
"#,
        );
        let contract = TaskContract::from_request(
            "Implement create and read support for Task records. Add tests.",
        );
        let report = preflight_owned_test_artifacts_for_verifier(
            root.path(),
            &contract,
            &["tests/repo.rs".to_string()],
        );

        assert!(report.admitted.is_empty());
        assert_eq!(report.rejected.len(), 1);
        assert_eq!(
            report.rejected[0].failure_kind,
            GeneratedTestPreflightFailureKind::MissingContractCoverage
        );
        assert_eq!(report.rejected[0].failure_kind.as_str(), "test_bug");
    }

    #[test]
    fn report_rejects_generated_suite_missing_stdin_coverage() {
        let root = tempfile::tempdir().expect("tempdir");
        write_valid_cargo_manifest(root.path());
        write_test_file(
            root.path(),
            "tests/cli.rs",
            r#"#[test]
fn runs_cli_with_file_arg() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_generated-test-guard-fixture"))
        .arg("input.txt")
        .output()
        .unwrap();
    assert!(output.status.success());
}
"#,
        );
        let contract = TaskContract::from_request(
            "Implement a Rust word-counter CLI that reads stdin and add integration tests",
        );
        let report = preflight_owned_test_artifacts_for_verifier(
            root.path(),
            &contract,
            &["tests/cli.rs".to_string()],
        );

        assert!(report.admitted.is_empty());
        assert_eq!(
            report.rejected[0].failure_kind,
            GeneratedTestPreflightFailureKind::MissingContractCoverage
        );
        assert_eq!(report.rejected[0].failure_kind.as_str(), "test_bug");
    }

    #[test]
    fn report_accepts_generated_suite_with_stdin_coverage() {
        let root = tempfile::tempdir().expect("tempdir");
        write_valid_cargo_manifest(root.path());
        write_test_file(
            root.path(),
            "tests/cli.rs",
            r#"#[test]
fn runs_cli_with_stdin() {
    let _program = env!("CARGO_BIN_EXE_generated-test-guard-fixture");
    let _case = "stdin";
}
"#,
        );
        let contract = TaskContract::from_request(
            "Implement a Rust word-counter CLI that reads stdin and add integration tests",
        );
        let report = preflight_owned_test_artifacts_for_verifier(
            root.path(),
            &contract,
            &["tests/cli.rs".to_string()],
        );

        assert_eq!(report.admitted, vec!["tests/cli.rs".to_string()]);
        assert!(report.rejected.is_empty());
    }

    #[test]
    fn filter_keeps_healthy_owned_tests() {
        let root = tempfile::tempdir().expect("tempdir");
        write_valid_cargo_manifest(root.path());
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
