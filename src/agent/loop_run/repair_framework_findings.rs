use std::path::{Path, PathBuf};

use super::package_manifest_summary::{PackageModuleType, parse_package_manifest_summary};
use super::repair_assertion_analysis::pytest_output_suggests_shared_state_leak;
use super::repair_python_test_analysis::{
    excerpt_has_disconnected_fixture_state_assertion, line_mentions_identifier,
};
use super::task_contract::ArtifactRole;
use crate::util::workspace_paths::is_workspace_artifact_admitted_relative_path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct VerifierDiagnosticFileExcerpt {
    pub(crate) path: String,
    pub(crate) role: ArtifactRole,
    pub(crate) excerpt: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum VerifierDiagnosticFrameworkFindingKind {
    UnittestLifecycleMismatch,
    SetupNameError,
    StatefulClientMissingIsolation,
    ImportedStateRebindMismatch,
    TestOnlyMissingLocalModuleImport,
    DisconnectedFixtureStateAssertion,
    TestOnlyMissingImportSymbol,
    DisconnectedSetupStateAssignment,
    RustIntegrationTestCrateImportMismatch,
    NodeModuleSyntaxMismatch,
    LocalProviderMissingImportSymbol,
}

impl VerifierDiagnosticFrameworkFindingKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::UnittestLifecycleMismatch => "pytest_unittest_lifecycle_mismatch",
            Self::SetupNameError => "pytest_setup_name_error",
            Self::StatefulClientMissingIsolation => "pytest_stateful_client_missing_isolation",
            Self::ImportedStateRebindMismatch => "pytest_imported_state_rebind_mismatch",
            Self::TestOnlyMissingLocalModuleImport => {
                "pytest_test_only_missing_local_module_import"
            }
            Self::DisconnectedFixtureStateAssertion => {
                "pytest_disconnected_fixture_state_assertion"
            }
            Self::TestOnlyMissingImportSymbol => "pytest_test_only_missing_import_symbol",
            Self::DisconnectedSetupStateAssignment => "pytest_disconnected_setup_state_assignment",
            Self::RustIntegrationTestCrateImportMismatch => {
                "rust_integration_test_crate_import_mismatch"
            }
            Self::NodeModuleSyntaxMismatch => "node_module_syntax_mismatch",
            Self::LocalProviderMissingImportSymbol => "python_local_provider_missing_import_symbol",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct VerifierDiagnosticFrameworkFinding {
    pub(crate) kind: VerifierDiagnosticFrameworkFindingKind,
    pub(crate) path: String,
    pub(crate) role: ArtifactRole,
    pub(crate) summary: String,
}

pub(crate) fn findings_for_diagnostic(
    work_root: &Path,
    command: &str,
    output_excerpt: &str,
    excerpts: &[VerifierDiagnosticFileExcerpt],
) -> Vec<VerifierDiagnosticFrameworkFinding> {
    let looks_like_pytest = output_or_command_looks_like_pytest(command, output_excerpt);
    let looks_like_cargo = output_or_command_looks_like_cargo(command, output_excerpt);
    let looks_like_node = output_or_command_looks_like_node(command, output_excerpt);
    if !looks_like_pytest && !looks_like_cargo && !looks_like_node {
        return Vec::new();
    }
    let mut findings = Vec::new();
    for excerpt in excerpts {
        if findings.len() >= 4 {
            break;
        }
        if excerpt.role != ArtifactRole::Test {
            continue;
        }
        if looks_like_node
            && node_test_module_syntax_mismatch(work_root, &excerpt.path, &excerpt.excerpt)
        {
            findings.push(VerifierDiagnosticFrameworkFinding {
                kind: VerifierDiagnosticFrameworkFindingKind::NodeModuleSyntaxMismatch,
                path: excerpt.path.clone(),
                role: excerpt.role,
                summary: "package.json module settings make this generated Node test run with a module format that conflicts with the test source import/require API; repair the test module syntax before changing implementation behavior".to_string(),
            });
        }
        if looks_like_cargo
            && excerpt.path.ends_with(".rs")
            && cargo_output_has_test_only_unresolved_crate_import(
                work_root,
                output_excerpt,
                &excerpt.path,
                &excerpt.excerpt,
            )
        {
            findings.push(VerifierDiagnosticFrameworkFinding {
                kind: VerifierDiagnosticFrameworkFindingKind::RustIntegrationTestCrateImportMismatch,
                path: excerpt.path.clone(),
                role: excerpt.role,
                summary: "cargo reports an unresolved crate import from this generated integration test, and that crate name is not declared by the local manifest or implementation artifacts; repair the test import/setup instead of repeatedly editing implementation logic".to_string(),
            });
        }
        if !looks_like_pytest || !excerpt.path.ends_with(".py") {
            continue;
        }
        if python_excerpt_has_plain_pytest_unittest_lifecycle_mismatch(&excerpt.excerpt) {
            findings.push(VerifierDiagnosticFrameworkFinding {
                kind: VerifierDiagnosticFrameworkFindingKind::UnittestLifecycleMismatch,
                path: excerpt.path.clone(),
                role: excerpt.role,
                summary: "pytest will not run setUp/tearDown on a plain test class; use setup_method/teardown_method, unittest.TestCase, or a pytest fixture for isolation".to_string(),
            });
        }
        if pytest_output_has_setup_name_error_for_path(output_excerpt, &excerpt.path) {
            findings.push(VerifierDiagnosticFrameworkFinding {
                kind: VerifierDiagnosticFrameworkFindingKind::SetupNameError,
                path: excerpt.path.clone(),
                role: excerpt.role,
                summary: "verifier reports NameError during pytest setup for this test artifact; repair the test setup/imports before changing implementation behavior".to_string(),
            });
        }
        if python_excerpt_has_stateful_client_without_pytest_isolation(&excerpt.excerpt)
            && pytest_output_suggests_shared_state_leak(output_excerpt)
        {
            findings.push(VerifierDiagnosticFrameworkFinding {
                kind: VerifierDiagnosticFrameworkFindingKind::StatefulClientMissingIsolation,
                path: excerpt.path.clone(),
                role: excerpt.role,
                summary: "pytest test artifact uses a shared stateful client with mutating requests but no visible isolation fixture or setup hook; add isolation instead of relaxing assertions".to_string(),
            });
        }
        if python_excerpt_has_imported_state_rebind_in_pytest_isolation(&excerpt.excerpt) {
            findings.push(VerifierDiagnosticFrameworkFinding {
                kind: VerifierDiagnosticFrameworkFindingKind::ImportedStateRebindMismatch,
                path: excerpt.path.clone(),
                role: excerpt.role,
                summary: "pytest isolation rebinds a directly imported symbol; this changes only the test module binding and does not reset provider module state".to_string(),
            });
        }
        if python_excerpt_has_disconnected_setup_state_assignment(work_root, &excerpt.excerpt) {
            findings.push(VerifierDiagnosticFrameworkFinding {
                kind: VerifierDiagnosticFrameworkFindingKind::DisconnectedSetupStateAssignment,
                path: excerpt.path.clone(),
                role: excerpt.role,
                summary: "pytest setup assigns state on an imported object, but the provider module does not read that state path; repair the test isolation to reset the actual provider state or assert independent public behavior".to_string(),
            });
        }
        if pytest_output_has_test_only_missing_local_module_import(
            work_root,
            output_excerpt,
            &excerpt.path,
            &excerpt.excerpt,
        ) {
            findings.push(VerifierDiagnosticFrameworkFinding {
                kind: VerifierDiagnosticFrameworkFindingKind::TestOnlyMissingLocalModuleImport,
                path: excerpt.path.clone(),
                role: excerpt.role,
                summary: "pytest setup imports a missing local module that implementation artifacts do not import; repair the generated test setup/imports before creating a provider module".to_string(),
            });
        }
        if excerpt_has_disconnected_fixture_state_assertion(&excerpt.excerpt) {
            findings.push(VerifierDiagnosticFrameworkFinding {
                kind: VerifierDiagnosticFrameworkFindingKind::DisconnectedFixtureStateAssertion,
                path: excerpt.path.clone(),
                role: excerpt.role,
                summary: "pytest assertions observe a test-local mutable fixture that is not connected to the system under test; replace that local-state observation with public behavior assertions instead of weakening coverage".to_string(),
            });
        }
        if pytest_output_has_test_only_missing_import_symbol(
            work_root,
            output_excerpt,
            &excerpt.path,
            &excerpt.excerpt,
        ) {
            findings.push(VerifierDiagnosticFrameworkFinding {
                kind: VerifierDiagnosticFrameworkFindingKind::TestOnlyMissingImportSymbol,
                path: excerpt.path.clone(),
                role: excerpt.role,
                summary: "pytest collection fails because the generated test imports a non-existent symbol from a local implementation module; repair the test import/setup or replace the internal-helper check with public behavior assertions".to_string(),
            });
        }
        if let Some((provider_path, module, name)) =
            pytest_output_local_provider_missing_import_symbol(
                work_root,
                output_excerpt,
                &excerpt.path,
                &excerpt.excerpt,
            )
        {
            findings.push(VerifierDiagnosticFrameworkFinding {
                kind: VerifierDiagnosticFrameworkFindingKind::LocalProviderMissingImportSymbol,
                path: provider_path,
                role: ArtifactRole::Implementation,
                summary: format!(
                    "pytest collection imports `{name}` from local module `{module}`, but the provider source does not define that exported name; repair the provider export unless higher-authority objective evidence says the test imported the wrong internal helper",
                ),
            });
        }
    }
    findings.truncate(4);
    findings
}

pub(crate) fn output_or_command_looks_like_pytest(command: &str, output_excerpt: &str) -> bool {
    let signal = format!("{command}\n{output_excerpt}").to_ascii_lowercase();
    signal.contains("pytest")
        || signal.contains("test session starts")
        || signal.contains("collected ")
}

pub(crate) fn output_or_command_looks_like_cargo(command: &str, output_excerpt: &str) -> bool {
    let signal = format!("{command}\n{output_excerpt}").to_ascii_lowercase();
    signal.starts_with("cargo ")
        || signal.contains("\ncargo ")
        || signal.contains("error[e")
        || signal.contains("could not compile")
}

pub(crate) fn output_or_command_looks_like_node(command: &str, output_excerpt: &str) -> bool {
    let signal = format!("{command}\n{output_excerpt}").to_ascii_lowercase();
    signal.starts_with("npm ")
        || signal.contains("\nnpm ")
        || signal.starts_with("node ")
        || signal.contains("\nnode ")
        || signal.contains("node.js v")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NodeModuleFormat {
    EsModule,
    CommonJs,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct JavascriptModuleSyntax {
    uses_static_esm: bool,
    uses_commonjs_require: bool,
    uses_create_require: bool,
}

fn node_test_module_syntax_mismatch(work_root: &Path, relative_path: &str, excerpt: &str) -> bool {
    let Some(format) = node_module_format_for_path(
        Path::new(relative_path),
        package_manifest_module_type(work_root),
    ) else {
        return false;
    };
    let source = read_capped_workspace_artifact(work_root, relative_path)
        .unwrap_or_else(|| excerpt.to_string());
    let syntax = javascript_module_syntax(&source);
    match format {
        NodeModuleFormat::EsModule => syntax.uses_commonjs_require && !syntax.uses_create_require,
        NodeModuleFormat::CommonJs => syntax.uses_static_esm,
    }
}

fn node_module_format_for_path(
    relative_path: &Path,
    package_module_type: Option<PackageModuleType>,
) -> Option<NodeModuleFormat> {
    let extension = relative_path
        .extension()
        .and_then(|ext| ext.to_str())?
        .to_ascii_lowercase();
    match extension.as_str() {
        "mjs" => Some(NodeModuleFormat::EsModule),
        "cjs" => Some(NodeModuleFormat::CommonJs),
        "js" => match package_module_type {
            Some(PackageModuleType::EsModule) => Some(NodeModuleFormat::EsModule),
            Some(PackageModuleType::CommonJs) | None => Some(NodeModuleFormat::CommonJs),
        },
        _ => None,
    }
}

fn package_manifest_module_type(work_root: &Path) -> Option<PackageModuleType> {
    let raw = read_capped_workspace_artifact(work_root, "package.json")?;
    parse_package_manifest_summary(&raw).ok()?.module_type
}

fn javascript_module_syntax(source: &str) -> JavascriptModuleSyntax {
    let mut syntax = JavascriptModuleSyntax::default();
    for line in source.lines() {
        let line = strip_javascript_line_comment(line);
        let trimmed = line.trim_start();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.starts_with("import ") || trimmed.starts_with("export ") {
            syntax.uses_static_esm = true;
        }
        if trimmed.contains("createRequire(") {
            syntax.uses_create_require = true;
        }
        if javascript_line_uses_commonjs_require(trimmed) {
            syntax.uses_commonjs_require = true;
        }
    }
    syntax
}

fn javascript_line_uses_commonjs_require(line: &str) -> bool {
    line.match_indices("require(")
        .any(|(index, _)| !line[..index].ends_with("create"))
}

fn strip_javascript_line_comment(line: &str) -> &str {
    line.split_once("//").map(|(head, _)| head).unwrap_or(line)
}

fn read_capped_workspace_artifact(work_root: &Path, relative_path: &str) -> Option<String> {
    const MAX_DIAGNOSTIC_SOURCE_BYTES: u64 = 256 * 1024;

    let relative = Path::new(relative_path);
    if relative.is_absolute() || !is_workspace_artifact_admitted_relative_path(relative) {
        return None;
    }
    let path = work_root.join(relative);
    if std::fs::metadata(&path).ok()?.len() > MAX_DIAGNOSTIC_SOURCE_BYTES {
        return None;
    }
    std::fs::read_to_string(path).ok()
}

pub(crate) fn missing_python_module_name_from_output(output: &str) -> Option<String> {
    for quote in ["'", "\""] {
        let marker = format!("No module named {quote}");
        let Some(start) = output.find(&marker) else {
            continue;
        };
        let rest = &output[start + marker.len()..];
        let Some(end) = rest.find(quote) else {
            continue;
        };
        let module = rest[..end].trim();
        if module.split('.').count() < 2 {
            continue;
        }
        if module.split('.').all(|part| {
            !part.is_empty()
                && part
                    .chars()
                    .all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
                && !part.chars().next().is_some_and(|ch| ch.is_ascii_digit())
        }) {
            return Some(module.to_string());
        }
    }
    None
}

pub(crate) fn workspace_implementation_imports_python_module(
    work_root: &Path,
    module: &str,
) -> bool {
    let Some(files) = meaningful_workspace_files(work_root, 128) else {
        return false;
    };
    files.into_iter().any(|relative| {
        if relative.extension().and_then(|ext| ext.to_str()) != Some("py") {
            return false;
        }
        if super::completion_evidence::classify_repo_edit_path(&relative)
            != super::completion_evidence::RepoEditCategory::Impl
        {
            return false;
        }
        let path = work_root.join(&relative);
        if std::fs::metadata(&path)
            .ok()
            .is_none_or(|metadata| metadata.len() > 65_536)
        {
            return false;
        }
        std::fs::read_to_string(path)
            .ok()
            .is_some_and(|source| python_source_imports_module(&source, module))
    })
}

fn python_excerpt_has_plain_pytest_unittest_lifecycle_mismatch(excerpt: &str) -> bool {
    let has_unittest_lifecycle =
        excerpt.contains("def setUp(") || excerpt.contains("def tearDown(");
    if !has_unittest_lifecycle {
        return false;
    }
    let lower = excerpt.to_ascii_lowercase();
    if lower.contains("import unittest") || lower.contains("from unittest") {
        return false;
    }
    excerpt.contains("class ")
        && !excerpt.contains("TestCase")
        && !excerpt.contains("unittest.TestCase")
}

fn pytest_output_has_setup_name_error_for_path(output_excerpt: &str, path: &str) -> bool {
    let lower = output_excerpt.to_ascii_lowercase();
    if !lower.contains("nameerror") || !output_excerpt.replace('\\', "/").contains(path) {
        return false;
    }
    lower.contains("error at setup")
        || lower.contains("error collecting")
        || lower.contains("errors during collection")
        || lower.contains(" in <module>")
}

fn python_excerpt_has_stateful_client_without_pytest_isolation(excerpt: &str) -> bool {
    let lower = excerpt.to_ascii_lowercase();
    let has_shared_client = lower.contains("testclient(") || lower.contains("client =");
    let mutating_calls = [".post(", ".put(", ".patch(", ".delete("]
        .iter()
        .filter(|needle| lower.contains(**needle))
        .count();
    let has_multiple_tests =
        lower.matches("def test_").count() >= 2 || lower.contains("class test");
    let has_isolation = lower.contains("@pytest.fixture")
        || lower.contains("setup_method")
        || lower.contains("teardown_method")
        || lower.contains("setup_function")
        || lower.contains("teardown_function")
        || lower.contains("autouse=true");
    has_shared_client && mutating_calls > 0 && has_multiple_tests && !has_isolation
}

fn python_excerpt_has_imported_state_rebind_in_pytest_isolation(excerpt: &str) -> bool {
    let imported_names = python_from_imported_names(excerpt);
    if imported_names.is_empty() {
        return false;
    }
    let lower = excerpt.to_ascii_lowercase();
    let has_isolation = lower.contains("@pytest.fixture")
        || lower.contains("setup_method")
        || lower.contains("setup_function")
        || lower.contains("def setup(")
        || lower.contains("def setUp(");
    if !has_isolation {
        return false;
    }

    for line in excerpt.lines() {
        let stripped = strip_python_inline_comment(line);
        let trimmed = stripped.trim();
        let globals = python_global_names(trimmed);
        for name in &imported_names {
            if globals.iter().any(|global| global == name)
                || python_line_rebinds_name(trimmed, name)
            {
                return true;
            }
        }
    }
    false
}

fn python_excerpt_has_disconnected_setup_state_assignment(work_root: &Path, excerpt: &str) -> bool {
    let lower = excerpt.to_ascii_lowercase();
    let has_isolation = lower.contains("@pytest.fixture")
        || lower.contains("setup_method")
        || lower.contains("setup_function")
        || lower.contains("def setup(")
        || lower.contains("def setUp(");
    if !has_isolation {
        return false;
    }
    let imported_modules = python_from_imported_name_modules(excerpt);
    if imported_modules.is_empty() {
        return false;
    }
    for line in excerpt.lines() {
        let stripped = strip_python_inline_comment(line);
        let Some((lhs, _)) = stripped.split_once('=') else {
            continue;
        };
        let lhs = lhs.trim();
        for (name, module) in &imported_modules {
            let Some(rest) = lhs.strip_prefix(&format!("{name}.")) else {
                continue;
            };
            let state_path = rest.trim();
            if let Some((container_path, key_name)) = python_subscript_assignment_path(state_path) {
                if !python_state_assignment_path_looks_like_test_isolation(container_path) {
                    continue;
                }
                if imported_modules.get(key_name) != Some(module) {
                    continue;
                }
                if !python_module_source_uses_name_outside_definition(work_root, module, key_name) {
                    return true;
                }
                continue;
            }
            if !python_attr_path_is_safe(state_path) {
                continue;
            }
            if !python_state_assignment_path_looks_like_test_isolation(state_path) {
                continue;
            }
            if !python_module_source_mentions_attr_path(work_root, module, name, state_path) {
                return true;
            }
        }
    }
    false
}

fn python_subscript_assignment_path(path: &str) -> Option<(&str, &str)> {
    let (container_path, rest) = path.split_once('[')?;
    let key = rest.split_once(']')?.0.trim();
    if !python_attr_path_is_safe(container_path) || !python_identifier_is_safe(key) {
        return None;
    }
    Some((container_path.trim(), key))
}

fn python_from_imported_name_modules(excerpt: &str) -> std::collections::HashMap<String, String> {
    let mut names = std::collections::HashMap::new();
    for line in excerpt.lines() {
        let stripped = strip_python_inline_comment(line);
        let trimmed = stripped.trim();
        let Some(rest) = trimmed.strip_prefix("from ") else {
            continue;
        };
        let Some((module, imports)) = rest.split_once(" import ") else {
            continue;
        };
        let module = module.trim();
        if !python_module_name_for_import_validation_is_safe(module) {
            continue;
        }
        if imports.trim().starts_with('*') {
            continue;
        }
        for raw in imports.split(',') {
            let imported = raw
                .split_whitespace()
                .next()
                .unwrap_or_default()
                .trim_matches(|ch: char| ch == '(' || ch == ')' || ch == '\\');
            if python_identifier_is_safe(imported) {
                names.insert(imported.to_string(), module.to_string());
            }
        }
    }
    names
}

fn python_module_name_for_import_validation_is_safe(module: &str) -> bool {
    !module.is_empty()
        && !module.starts_with('.')
        && module.split('.').all(python_identifier_is_safe)
}

fn python_top_level_assignment_defines_name(line: &str, name: &str) -> bool {
    let Some((assignment_head, _)) = line.split_once('=') else {
        return false;
    };
    let binding_head = assignment_head
        .split_once(':')
        .map(|(head, _)| head)
        .unwrap_or(assignment_head)
        .trim();
    binding_head == name
}

fn python_attr_path_is_safe(path: &str) -> bool {
    path.split('.')
        .all(|part| python_identifier_is_safe(part.trim()))
}

fn python_state_assignment_path_looks_like_test_isolation(path: &str) -> bool {
    let mut parts = path.split('.');
    let Some(first) = parts.next() else {
        return false;
    };
    if first == "state" && parts.next().is_some() {
        return true;
    }
    let lower = path.to_ascii_lowercase();
    lower.contains("override") || lower.contains("patch") || lower.contains("mock")
}

fn python_module_source_mentions_attr_path(
    work_root: &Path,
    module: &str,
    imported_name: &str,
    attr_path: &str,
) -> bool {
    let relative = format!("{}.py", module.replace('.', "/"));
    let path = work_root.join(relative);
    if std::fs::metadata(&path)
        .ok()
        .is_none_or(|metadata| metadata.len() > 65_536)
    {
        return false;
    }
    let Ok(source) = std::fs::read_to_string(path) else {
        return false;
    };
    source.contains(&format!("{imported_name}.{attr_path}"))
        || source.contains(&format!(".{attr_path}"))
}

fn python_module_source_uses_name_outside_definition(
    work_root: &Path,
    module: &str,
    name: &str,
) -> bool {
    let relative = format!("{}.py", module.replace('.', "/"));
    let path = work_root.join(relative);
    if std::fs::metadata(&path)
        .ok()
        .is_none_or(|metadata| metadata.len() > 65_536)
    {
        return false;
    }
    let Ok(source) = std::fs::read_to_string(path) else {
        return false;
    };
    source.lines().any(|line| {
        let stripped = strip_python_inline_comment(line);
        let trimmed = stripped.trim();
        if trimmed.is_empty() {
            return false;
        }
        if trimmed.starts_with(&format!("def {name}("))
            || trimmed.starts_with(&format!("async def {name}("))
            || trimmed.starts_with(&format!("class {name}("))
            || trimmed.starts_with(&format!("class {name}:"))
            || python_top_level_assignment_defines_name(trimmed, name)
        {
            return false;
        }
        line_mentions_identifier(trimmed, name)
    })
}

fn python_from_imported_names(excerpt: &str) -> std::collections::HashSet<String> {
    let mut names = std::collections::HashSet::new();
    for line in excerpt.lines() {
        let stripped = strip_python_inline_comment(line);
        let trimmed = stripped.trim();
        let Some(rest) = trimmed.strip_prefix("from ") else {
            continue;
        };
        let Some((_, imports)) = rest.split_once(" import ") else {
            continue;
        };
        if imports.trim().starts_with('*') {
            continue;
        }
        for raw in imports.split(',') {
            let name = raw
                .split_whitespace()
                .next()
                .unwrap_or_default()
                .trim_matches(|ch: char| ch == '(' || ch == ')' || ch == '\\');
            if python_identifier_is_safe(name) {
                names.insert(name.to_string());
            }
        }
    }
    names
}

fn python_global_names(line: &str) -> Vec<String> {
    let Some(rest) = line.trim_start().strip_prefix("global ") else {
        return Vec::new();
    };
    rest.split(',')
        .filter_map(|name| {
            let name = name.trim();
            python_identifier_is_safe(name).then_some(name.to_string())
        })
        .collect()
}

fn python_line_rebinds_name(line: &str, name: &str) -> bool {
    let trimmed = line.trim_start();
    if !trimmed.starts_with(name) {
        return false;
    }
    let rest = &trimmed[name.len()..];
    rest.starts_with(" =") || rest.starts_with("=") || rest.starts_with(":")
}

fn strip_python_inline_comment(line: &str) -> String {
    line.split_once('#')
        .map(|(head, _)| head)
        .unwrap_or(line)
        .to_string()
}

fn python_identifier_is_safe(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first == '_' || first.is_ascii_alphabetic())
        && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

fn pytest_output_has_test_only_missing_local_module_import(
    work_root: &Path,
    output_excerpt: &str,
    test_path: &str,
    test_excerpt: &str,
) -> bool {
    let Some(module) = missing_python_module_name_from_output(output_excerpt) else {
        return false;
    };
    if !output_excerpt.replace('\\', "/").contains(test_path) {
        return false;
    }
    if !python_source_imports_module(test_excerpt, &module) {
        return false;
    }
    !workspace_implementation_imports_python_module(work_root, &module)
}

fn pytest_output_has_test_only_missing_import_symbol(
    work_root: &Path,
    output_excerpt: &str,
    test_path: &str,
    test_excerpt: &str,
) -> bool {
    let Some((name, module)) = missing_python_import_name_from_output(output_excerpt) else {
        return false;
    };
    if !output_excerpt.replace('\\', "/").contains(test_path) {
        return false;
    }
    if !python_source_imports_name_from_module(test_excerpt, &module, &name) {
        return false;
    }
    python_local_module_file_exists(work_root, &module)
}

fn pytest_output_local_provider_missing_import_symbol(
    work_root: &Path,
    output_excerpt: &str,
    test_path: &str,
    test_excerpt: &str,
) -> Option<(String, String, String)> {
    let (name, module) = missing_python_import_name_from_output(output_excerpt)?;
    if !output_excerpt.replace('\\', "/").contains(test_path) {
        return None;
    }
    if !python_source_imports_name_from_module(test_excerpt, &module, &name) {
        return None;
    }
    let provider_path = python_local_module_relative_path(work_root, &module)?;
    let provider_source = read_capped_workspace_artifact(work_root, &provider_path)?;
    if python_source_defines_top_level_name(&provider_source, &name) {
        return None;
    }
    Some((provider_path, module, name))
}

fn cargo_output_has_test_only_unresolved_crate_import(
    work_root: &Path,
    output_excerpt: &str,
    test_path: &str,
    test_excerpt: &str,
) -> bool {
    let Some(crate_name) = missing_rust_crate_name_from_output(output_excerpt) else {
        return false;
    };
    let normalized_output = output_excerpt.replace('\\', "/");
    if !normalized_output.contains(test_path) {
        return false;
    }
    if !rust_source_imports_crate(test_excerpt, &crate_name) {
        return false;
    }
    if cargo_manifest_defines_rust_crate_name(work_root, &crate_name) {
        return false;
    }
    !workspace_rust_implementation_imports_crate(work_root, &crate_name)
}

fn missing_rust_crate_name_from_output(output: &str) -> Option<String> {
    let candidates = [
        "unresolved import `",
        "unresolved module or unlinked crate `",
        "use of unresolved module or unlinked crate `",
    ];
    for marker in candidates {
        let Some(start) = output.find(marker) else {
            continue;
        };
        let rest = &output[start + marker.len()..];
        let name = rest.split('`').next().unwrap_or_default();
        let first_segment = name.split("::").next().unwrap_or_default();
        if rust_identifier_is_safe(first_segment) {
            return Some(first_segment.to_string());
        }
    }
    None
}

fn rust_source_imports_crate(source: &str, crate_name: &str) -> bool {
    source.lines().any(|line| {
        let trimmed = line.trim_start();
        trimmed == format!("extern crate {crate_name};")
            || trimmed.strip_prefix("use ").is_some_and(|rest| {
                rest == crate_name || rest.starts_with(&format!("{crate_name}::"))
            })
    })
}

fn cargo_manifest_defines_rust_crate_name(work_root: &Path, crate_name: &str) -> bool {
    let Ok(raw) = std::fs::read_to_string(work_root.join("Cargo.toml")) else {
        return false;
    };
    let normalized_crate = crate_name.replace('-', "_");
    raw.lines().any(|line| {
        let trimmed = line.trim();
        if trimmed.starts_with('#') {
            return false;
        }
        let Some((key, value)) = trimmed.split_once('=') else {
            return false;
        };
        let key = key.trim();
        if key != "name" && key != crate_name {
            return false;
        }
        let value = value.trim().trim_matches('"').replace('-', "_");
        value == normalized_crate || key == crate_name
    })
}

fn workspace_rust_implementation_imports_crate(work_root: &Path, crate_name: &str) -> bool {
    let mut stack = vec![work_root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if super::task_workspace_scope::is_workspace_ignored_dir(&name) {
                continue;
            }
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path.extension().and_then(|ext| ext.to_str()) != Some("rs") {
                continue;
            }
            let Ok(relative) = path.strip_prefix(work_root) else {
                continue;
            };
            let rel = relative.to_string_lossy().replace('\\', "/");
            if rel.starts_with("tests/") {
                continue;
            }
            if let Ok(raw) = std::fs::read_to_string(&path)
                && rust_source_imports_crate(&raw, crate_name)
            {
                return true;
            }
        }
    }
    false
}

fn rust_identifier_is_safe(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first == '_' || first.is_ascii_alphabetic())
        && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

fn missing_python_import_name_from_output(output: &str) -> Option<(String, String)> {
    for quote in ["'", "\""] {
        let marker = format!("ImportError: cannot import name {quote}");
        let Some(start) = output.find(&marker) else {
            continue;
        };
        let rest = &output[start + marker.len()..];
        let Some(name_end) = rest.find(quote) else {
            continue;
        };
        let name = rest[..name_end].trim();
        let from_marker = format!(" from {quote}");
        let rest = &rest[name_end + quote.len()..];
        let Some(module_start) = rest.find(&from_marker) else {
            continue;
        };
        let rest = &rest[module_start + from_marker.len()..];
        let Some(module_end) = rest.find(quote) else {
            continue;
        };
        let module = rest[..module_end].trim();
        if python_identifier_is_safe(name)
            && python_module_name_for_import_validation_is_safe(module)
        {
            return Some((name.to_string(), module.to_string()));
        }
    }
    None
}

fn python_local_module_file_exists(work_root: &Path, module: &str) -> bool {
    python_local_module_relative_path(work_root, module).is_some()
}

fn python_local_module_relative_path(work_root: &Path, module: &str) -> Option<String> {
    if !python_module_name_for_import_validation_is_safe(module) {
        return None;
    }
    let relative = module.replace('.', "/");
    let file = format!("{relative}.py");
    if workspace_python_artifact_exists(work_root, &file) {
        return Some(file);
    }
    let init = format!("{relative}/__init__.py");
    workspace_python_artifact_exists(work_root, &init).then_some(init)
}

fn workspace_python_artifact_exists(work_root: &Path, relative: &str) -> bool {
    let relative_path = Path::new(relative);
    is_workspace_artifact_admitted_relative_path(relative_path)
        && work_root.join(relative_path).is_file()
}

fn python_source_defines_top_level_name(source: &str, name: &str) -> bool {
    source.lines().any(|line| {
        if line.chars().next().is_some_and(char::is_whitespace) {
            return false;
        }
        let stripped = strip_python_inline_comment(line);
        let trimmed = stripped.trim();
        trimmed.starts_with(&format!("def {name}("))
            || trimmed.starts_with(&format!("async def {name}("))
            || trimmed.starts_with(&format!("class {name}("))
            || trimmed.starts_with(&format!("class {name}:"))
            || python_top_level_assignment_defines_name(trimmed, name)
            || python_top_level_import_defines_name(trimmed, name)
    })
}

fn python_top_level_import_defines_name(line: &str, name: &str) -> bool {
    if let Some(rest) = line.strip_prefix("import ") {
        return rest.split(',').any(|raw| {
            let parts = raw.split_whitespace().collect::<Vec<_>>();
            match parts.as_slice() {
                [module, "as", alias] => {
                    *alias == name && python_module_name_for_import_validation_is_safe(module)
                }
                [module] => module.split('.').next().is_some_and(|root| {
                    root == name && python_module_name_for_import_validation_is_safe(module)
                }),
                _ => false,
            }
        });
    }
    if let Some(rest) = line.strip_prefix("from ") {
        let Some((_module, imports)) = rest.split_once(" import ") else {
            return false;
        };
        return imports.split(',').any(|raw| {
            let parts = raw.split_whitespace().collect::<Vec<_>>();
            match parts.as_slice() {
                [imported, "as", alias] => *alias == name && python_identifier_is_safe(imported),
                [imported] => *imported == name && python_identifier_is_safe(imported),
                _ => false,
            }
        });
    }
    false
}

fn python_source_imports_module(source: &str, module: &str) -> bool {
    let (parent, leaf) = module.rsplit_once('.').unwrap_or(("", module));
    for line in source.lines() {
        let stripped = strip_python_inline_comment(line);
        let trimmed = stripped.trim();
        if let Some(rest) = trimmed.strip_prefix("import ")
            && python_import_list_contains_module(rest, module)
        {
            return true;
        }
        if let Some(rest) = trimmed.strip_prefix("from ")
            && let Some((from_module, imports)) = rest.split_once(" import ")
        {
            let from_module = from_module.trim();
            if from_module == module {
                return true;
            }
            if !parent.is_empty()
                && from_module == parent
                && python_import_list_contains_name(imports, leaf)
            {
                return true;
            }
        }
    }
    false
}

fn python_source_imports_name_from_module(source: &str, module: &str, name: &str) -> bool {
    source.lines().any(|line| {
        let stripped = strip_python_inline_comment(line);
        let trimmed = stripped.trim();
        let Some(rest) = trimmed.strip_prefix("from ") else {
            return false;
        };
        let Some((from_module, imports)) = rest.split_once(" import ") else {
            return false;
        };
        from_module.trim() == module && python_import_list_contains_name(imports, name)
    })
}

fn python_import_list_contains_module(imports: &str, module: &str) -> bool {
    imports.split(',').any(|entry| {
        let imported = entry.split_whitespace().next().unwrap_or_default();
        imported == module || imported.starts_with(&format!("{module}."))
    })
}

fn python_import_list_contains_name(imports: &str, name: &str) -> bool {
    imports.split(',').any(|entry| {
        let imported = entry
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .trim_matches(|ch: char| ch == '(' || ch == ')' || ch == '\\');
        imported == name
    })
}

fn meaningful_workspace_files(work_root: &Path, limit: usize) -> Option<Vec<PathBuf>> {
    if !work_root.is_dir() {
        return None;
    }
    let mut files = Vec::new();
    collect_meaningful_workspace_files(work_root, work_root, limit, &mut files);
    (!files.is_empty()).then_some(files)
}

fn collect_meaningful_workspace_files(
    work_root: &Path,
    dir: &Path,
    limit: usize,
    files: &mut Vec<PathBuf>,
) {
    if files.len() >= limit {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if files.len() >= limit {
            break;
        }
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if super::task_workspace_scope::is_workspace_ignored_dir(&name) {
            continue;
        }
        if path.is_dir() {
            collect_meaningful_workspace_files(work_root, &path, limit, files);
            continue;
        }
        if !path.is_file() {
            continue;
        }
        let Ok(relative) = path.strip_prefix(work_root) else {
            continue;
        };
        if !is_workspace_artifact_admitted_relative_path(relative) {
            continue;
        }
        files.push(relative.to_path_buf());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_python_module_parser_extracts_dotted_module() {
        let parsed = missing_python_module_name_from_output(
            "ModuleNotFoundError: No module named 'app.models'",
        );

        assert_eq!(parsed, Some("app.models".to_string()));
    }

    #[test]
    fn cargo_signal_detection_uses_command_and_output() {
        assert!(output_or_command_looks_like_cargo("cargo test", ""));
        assert!(output_or_command_looks_like_cargo(
            "",
            "error[E0432]: unresolved import"
        ));
    }

    #[test]
    fn node_signal_detection_uses_command_and_output() {
        assert!(output_or_command_looks_like_node("npm test", ""));
        assert!(output_or_command_looks_like_node(
            "node tests/cli.test.js",
            ""
        ));
        assert!(output_or_command_looks_like_node("", "Node.js v24.0.0"));
    }

    #[test]
    fn node_framework_finding_flags_esm_test_using_commonjs_require() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join("tests")).unwrap();
        std::fs::write(temp.path().join("package.json"), r#"{"type":"module"}"#).unwrap();
        let test_source = r#"import test from "node:test";
import assert from "node:assert/strict";
const fs = require("node:fs");

test("reads a fixture", () => {
  assert.equal(typeof fs.readFileSync, "function");
});
"#;
        std::fs::write(temp.path().join("tests/cli.test.js"), test_source).unwrap();

        let findings = findings_for_diagnostic(
            temp.path(),
            "npm test",
            "ReferenceError: require is not defined in ES module scope",
            &[VerifierDiagnosticFileExcerpt {
                path: "tests/cli.test.js".to_string(),
                role: ArtifactRole::Test,
                excerpt: test_source.to_string(),
            }],
        );

        assert_eq!(findings.len(), 1);
        assert_eq!(
            findings[0].kind,
            VerifierDiagnosticFrameworkFindingKind::NodeModuleSyntaxMismatch
        );
        assert_eq!(findings[0].path, "tests/cli.test.js");
    }

    #[test]
    fn node_framework_finding_flags_commonjs_test_using_static_import() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join("tests")).unwrap();
        std::fs::write(temp.path().join("package.json"), r#"{"name":"app"}"#).unwrap();
        let test_source = r#"import test from "node:test";
import assert from "node:assert/strict";

test("works", () => assert.equal(1, 1));
"#;
        std::fs::write(temp.path().join("tests/cli.test.js"), test_source).unwrap();

        let findings = findings_for_diagnostic(
            temp.path(),
            "node tests/cli.test.js",
            "SyntaxError: Cannot use import statement outside a module",
            &[VerifierDiagnosticFileExcerpt {
                path: "tests/cli.test.js".to_string(),
                role: ArtifactRole::Test,
                excerpt: test_source.to_string(),
            }],
        );

        assert_eq!(findings.len(), 1);
        assert_eq!(
            findings[0].kind,
            VerifierDiagnosticFrameworkFindingKind::NodeModuleSyntaxMismatch
        );
    }

    #[test]
    fn node_framework_finding_allows_create_require_in_esm_test() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join("tests")).unwrap();
        std::fs::write(temp.path().join("package.json"), r#"{"type":"module"}"#).unwrap();
        let test_source = r#"import test from "node:test";
import { createRequire } from "node:module";
const require = createRequire(import.meta.url);
const fs = require("node:fs");

test("reads a fixture", () => {
  if (!fs.readFileSync) throw new Error("missing fs");
});
"#;
        std::fs::write(temp.path().join("tests/cli.test.js"), test_source).unwrap();

        let findings = findings_for_diagnostic(
            temp.path(),
            "npm test",
            "",
            &[VerifierDiagnosticFileExcerpt {
                path: "tests/cli.test.js".to_string(),
                role: ArtifactRole::Test,
                excerpt: test_source.to_string(),
            }],
        );

        assert!(findings.is_empty(), "{findings:?}");
    }

    #[test]
    fn pytest_single_file_provider_missing_import_symbol_is_framework_finding() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join("tests")).unwrap();
        std::fs::write(
            temp.path().join("main.py"),
            "def cli_main():\n    return 0\n",
        )
        .unwrap();
        let test_source = "from main import merge_toml_files\n\n\
def test_merge_toml_files():\n    assert callable(merge_toml_files)\n";

        let findings = findings_for_diagnostic(
            temp.path(),
            "python3 -B -m pytest -p no:cacheprovider tests/test_main.py",
            "ERROR collecting tests/test_main.py\n\
tests/test_main.py:1: in <module>\n\
    from main import merge_toml_files\n\
E   ImportError: cannot import name 'merge_toml_files' from 'main' (/tmp/work/main.py)\n",
            &[VerifierDiagnosticFileExcerpt {
                path: "tests/test_main.py".to_string(),
                role: ArtifactRole::Test,
                excerpt: test_source.to_string(),
            }],
        );

        let finding = findings
            .iter()
            .find(|finding| {
                finding.kind
                    == VerifierDiagnosticFrameworkFindingKind::LocalProviderMissingImportSymbol
            })
            .expect("single-file provider import mismatch finding");
        assert_eq!(finding.path, "main.py");
        assert_eq!(finding.role, ArtifactRole::Implementation);
        assert!(finding.summary.contains("merge_toml_files"));
    }

    #[test]
    fn pytest_single_file_provider_defined_symbol_is_not_framework_finding() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join("tests")).unwrap();
        std::fs::write(
            temp.path().join("main.py"),
            "def merge_toml_files():\n    return {}\n",
        )
        .unwrap();
        let test_source = "from main import merge_toml_files\n\n\
def test_merge_toml_files():\n    assert callable(merge_toml_files)\n";

        let findings = findings_for_diagnostic(
            temp.path(),
            "python3 -B -m pytest -p no:cacheprovider tests/test_main.py",
            "ERROR collecting tests/test_main.py\n\
tests/test_main.py:1: in <module>\n\
    from main import merge_toml_files\n\
E   ImportError: cannot import name 'merge_toml_files' from 'main' (/tmp/work/main.py)\n",
            &[VerifierDiagnosticFileExcerpt {
                path: "tests/test_main.py".to_string(),
                role: ArtifactRole::Test,
                excerpt: test_source.to_string(),
            }],
        );

        assert!(
            !findings.iter().any(|finding| {
                finding.kind
                    == VerifierDiagnosticFrameworkFindingKind::LocalProviderMissingImportSymbol
            }),
            "provider already defines the symbol, got {findings:?}"
        );
    }

    #[test]
    fn meaningful_workspace_files_excludes_metadata_exploration_candidates() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("prompt.md"), "task\n").unwrap();
        std::fs::write(temp.path().join("cmd.txt"), "cargo test\n").unwrap();
        std::fs::write(temp.path().join("anvil.out"), "stdout\n").unwrap();
        std::fs::write(temp.path().join("postcheck.err"), "stderr\n").unwrap();
        std::fs::create_dir_all(temp.path().join("logs")).unwrap();
        std::fs::write(temp.path().join("logs/llm-io.jsonl"), "{}\n").unwrap();
        std::fs::create_dir_all(temp.path().join(".anvil")).unwrap();
        std::fs::write(temp.path().join(".anvil/session.json"), "{}\n").unwrap();
        std::fs::create_dir_all(temp.path().join("src")).unwrap();
        std::fs::write(temp.path().join("src/main.py"), "import app.models\n").unwrap();

        let files = meaningful_workspace_files(temp.path(), 16).unwrap();

        assert_eq!(files, vec![PathBuf::from("src/main.py")]);
    }
}
