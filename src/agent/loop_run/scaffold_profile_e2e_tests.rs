//! Issue #1003: table-driven E2E coverage for the `ScaffoldProfile` registry.
//!
//! Every profile is exercised through `materialize` + `verify_bindings`, and the
//! two acceptance bindings (Node test-script/`package.json`, Rust Cargo
//! entrypoint) are pinned directly. `#[cfg(test)]` keeps this out of the
//! production binary (CB-001 / DR3-001).

use std::path::PathBuf;

use super::scaffold_pipeline::{ProjectRuntime, ProjectShape};
use super::scaffold_profile::{
    ALL_PROFILE_IDS, BindingCheck, RuntimeKind, ScaffoldProfile, ScaffoldProfileId, profile,
};

fn paths(files: &[(PathBuf, String)]) -> Vec<String> {
    files
        .iter()
        .map(|(path, _)| path.to_string_lossy().replace('\\', "/"))
        .collect()
}

fn content_of<'a>(files: &'a [(PathBuf, String)], rel: &str) -> &'a str {
    files
        .iter()
        .find(|(path, _)| path.to_string_lossy().replace('\\', "/") == rel)
        .map(|(_, content)| content.as_str())
        .unwrap_or_else(|| panic!("expected materialized file {rel}"))
}

/// Wildcard-free expected runtime — adding a 10th id compile-forces an update.
fn expected_runtime(id: ScaffoldProfileId) -> RuntimeKind {
    match id {
        ScaffoldProfileId::PythonCliPytest => RuntimeKind::Python,
        ScaffoldProfileId::NodeCliNodeTest => RuntimeKind::Node,
        ScaffoldProfileId::NodeLibNodeTest => RuntimeKind::Node,
        ScaffoldProfileId::RustCliCargo => RuntimeKind::Rust,
        ScaffoldProfileId::RustLibCargo => RuntimeKind::Rust,
        ScaffoldProfileId::FastApiPytest => RuntimeKind::Python,
        ScaffoldProfileId::DocsMarkdownCheck => RuntimeKind::Docs,
        ScaffoldProfileId::CsvTransformCheck => RuntimeKind::Data,
        ScaffoldProfileId::ResearchNotesCheck => RuntimeKind::Research,
    }
}

#[test]
fn every_profile_materializes_and_satisfies_its_bindings() {
    for &id in ALL_PROFILE_IDS {
        let prof = profile(id);
        assert_eq!(
            prof.id,
            id,
            "registry entry id mismatch for {}",
            id.as_str()
        );
        assert_eq!(
            prof.runtime,
            expected_runtime(id),
            "runtime mismatch for {}",
            id.as_str()
        );
        assert!(
            !prof.command.program.is_empty(),
            "{} declares no command program",
            id.as_str()
        );

        let files = prof.materialize(None);
        assert!(!files.is_empty(), "{} materialized no files", id.as_str());
        prof.verify_bindings(&files)
            .unwrap_or_else(|err| panic!("{} binding contract: {err}", id.as_str()));
    }
}

#[test]
fn profile_ids_are_total_and_have_distinct_labels() {
    assert_eq!(ALL_PROFILE_IDS.len(), 9, "expected 9 registered profiles");
    let mut slugs: Vec<&str> = ALL_PROFILE_IDS.iter().map(|id| id.as_str()).collect();
    slugs.sort_unstable();
    slugs.dedup();
    assert_eq!(slugs.len(), 9, "profile slugs must be unique");
}

#[test]
fn node_cli_binds_test_file_to_package_json_test_script() {
    // Acceptance: Node CSV-style fixture — the test file and a package.json with a
    // test script must materialize together (the v0.6.5 missing_verification hole).
    let prof = profile(ScaffoldProfileId::NodeCliNodeTest);
    let files = prof.materialize(None);
    let names = paths(&files);

    assert!(names.contains(&"package.json".to_string()));
    assert!(names.contains(&"tests/index.test.js".to_string()));
    let package_json = content_of(&files, "package.json");
    assert!(
        package_json.contains(r#""test": "node --test""#),
        "package.json must declare the node --test script"
    );
    assert!(
        prof.binding_checks
            .contains(&BindingCheck::NodeManifestDeclaresTestScript)
    );
    prof.verify_bindings(&files).expect("node cli bindings");
}

#[test]
fn rust_cli_and_library_follow_entrypoint_and_test_shape() {
    let cli = profile(ScaffoldProfileId::RustCliCargo);
    let cli_files = cli.materialize(None);
    let cli_paths = paths(&cli_files);
    assert!(cli_paths.contains(&"Cargo.toml".to_string()));
    assert!(cli_paths.contains(&"src/main.rs".to_string()));
    assert!(cli_paths.contains(&"tests/cli.rs".to_string()));
    assert!(content_of(&cli_files, "Cargo.toml").contains("[[bin]]"));
    cli.verify_bindings(&cli_files).expect("rust cli bindings");

    let lib = profile(ScaffoldProfileId::RustLibCargo);
    let lib_files = lib.materialize(None);
    let lib_paths = paths(&lib_files);
    assert!(lib_paths.contains(&"Cargo.toml".to_string()));
    assert!(lib_paths.contains(&"src/lib.rs".to_string()));
    assert!(lib_paths.contains(&"tests/integration.rs".to_string()));
    assert!(content_of(&lib_files, "Cargo.toml").contains("[lib]"));
    lib.verify_bindings(&lib_files).expect("rust lib bindings");
}

#[test]
fn rust_cli_honors_requested_entrypoint() {
    let prof = profile(ScaffoldProfileId::RustCliCargo);
    let entrypoint = PathBuf::from("tools/word_count.rs");
    let files = prof.materialize(Some(&entrypoint));
    let names = paths(&files);

    assert!(names.contains(&"tools/word_count.rs".to_string()));
    assert!(!names.contains(&"src/main.rs".to_string()));
    assert!(content_of(&files, "Cargo.toml").contains(r#"path = "tools/word_count.rs""#));
    prof.verify_bindings(&files)
        .expect("requested-entrypoint bindings");
}

#[test]
fn docs_data_research_share_the_same_registry() {
    for (id, runtime) in [
        (ScaffoldProfileId::DocsMarkdownCheck, RuntimeKind::Docs),
        (ScaffoldProfileId::CsvTransformCheck, RuntimeKind::Data),
        (ScaffoldProfileId::ResearchNotesCheck, RuntimeKind::Research),
    ] {
        let prof = profile(id);
        assert_eq!(prof.runtime, runtime, "{} runtime", id.as_str());
        let files = prof.materialize(None);
        assert!(!files.is_empty(), "{} materialized no files", id.as_str());
        prof.verify_bindings(&files)
            .unwrap_or_else(|err| panic!("{} bindings: {err}", id.as_str()));
    }

    // Data profile carries a CSV transform script + sample input.
    let data_files = profile(ScaffoldProfileId::CsvTransformCheck).materialize(None);
    let data_paths = paths(&data_files);
    assert!(data_paths.contains(&"transform.py".to_string()));
    assert!(data_paths.contains(&"sample.csv".to_string()));

    // Research profile carries notes.
    let research_files = profile(ScaffoldProfileId::ResearchNotesCheck).materialize(None);
    assert!(paths(&research_files).contains(&"research-notes.md".to_string()));
}

#[test]
fn for_runtime_shape_routes_coding_profiles() {
    let cases = [
        (
            ProjectRuntime::Rust,
            ProjectShape::Cli,
            ScaffoldProfileId::RustCliCargo,
        ),
        (
            ProjectRuntime::Rust,
            ProjectShape::Library,
            ScaffoldProfileId::RustLibCargo,
        ),
        (
            ProjectRuntime::Node,
            ProjectShape::Cli,
            ScaffoldProfileId::NodeCliNodeTest,
        ),
        (
            ProjectRuntime::Node,
            ProjectShape::Library,
            ScaffoldProfileId::NodeLibNodeTest,
        ),
    ];
    for (runtime, shape, expected) in cases {
        assert_eq!(
            ScaffoldProfile::for_runtime_shape(runtime, shape).id,
            expected
        );
    }
}

#[test]
fn node_binding_check_rejects_missing_test_script() {
    let prof = profile(ScaffoldProfileId::NodeCliNodeTest);

    // Required roles satisfied, but package.json lacks a test script while a test
    // file is present -> NodeManifestDeclaresTestScript must fire.
    let without_script: Vec<(PathBuf, String)> = vec![
        (
            PathBuf::from("package.json"),
            r#"{"name":"app"}"#.to_string(),
        ),
        (
            PathBuf::from("src/index.js"),
            "export const x = 1;\n".to_string(),
        ),
        (
            PathBuf::from("tests/index.test.js"),
            "import test from \"node:test\";\n".to_string(),
        ),
        (PathBuf::from("README.md"), "# App\n".to_string()),
    ];
    assert!(
        prof.verify_bindings(&without_script).is_err(),
        "missing test script should violate the node binding"
    );

    let with_script: Vec<(PathBuf, String)> = vec![
        (
            PathBuf::from("package.json"),
            r#"{"scripts":{"test":"node --test"}}"#.to_string(),
        ),
        (
            PathBuf::from("src/index.js"),
            "export const x = 1;\n".to_string(),
        ),
        (
            PathBuf::from("tests/index.test.js"),
            "import test from \"node:test\";\n".to_string(),
        ),
        (PathBuf::from("README.md"), "# App\n".to_string()),
    ];
    prof.verify_bindings(&with_script)
        .expect("declared test script satisfies the node binding");
}

#[test]
fn cargo_binding_check_rejects_unbound_entrypoint() {
    let prof = profile(ScaffoldProfileId::RustCliCargo);

    // Cargo.toml declares an entrypoint path that no materialized file provides.
    let files: Vec<(PathBuf, String)> = vec![
        (
            PathBuf::from("Cargo.toml"),
            "[package]\nname = \"app\"\n\n[[bin]]\nname = \"app\"\npath = \"src/other.rs\"\n"
                .to_string(),
        ),
        (PathBuf::from("src/main.rs"), "fn main() {}\n".to_string()),
        (
            PathBuf::from("tests/cli.rs"),
            "#[test]\nfn t() {}\n".to_string(),
        ),
        (PathBuf::from("README.md"), "# App\n".to_string()),
    ];
    assert!(
        prof.verify_bindings(&files).is_err(),
        "unbound Cargo entrypoint should violate the rust binding"
    );
}
