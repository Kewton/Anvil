use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RuntimeAcceptanceReport {
    pub passed: bool,
    pub inconclusive: bool,
    pub missing_capabilities: Vec<String>,
    pub missing_evidence: Vec<String>,
    pub weak_evidence: Vec<String>,
    pub inconclusive_reasons: Vec<String>,
    pub primary_reason: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EvidenceKind {
    ImplementationArtifact,
    TestArtifact,
    BoundVerifyCommand,
    NonZeroTestOrAssertionEvidence,
    BuildCommandOrDependencyBoundary,
    InteractiveUiSourceEvidence,
    NonStaticScreenEvidence,
    RequestedContent,
}

impl EvidenceKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::ImplementationArtifact => "implementation_artifact",
            Self::TestArtifact => "test_artifact",
            Self::BoundVerifyCommand => "bound_verify_command",
            Self::NonZeroTestOrAssertionEvidence => "non_zero_test_or_assertion_evidence",
            Self::BuildCommandOrDependencyBoundary => {
                "build_command_or_dependency_missing_boundary"
            }
            Self::InteractiveUiSourceEvidence => "interactive_ui_source_evidence",
            Self::NonStaticScreenEvidence => "non_static_screen_evidence",
            Self::RequestedContent => "requested_content_evidence",
        }
    }
}

#[derive(Debug, Default)]
struct WorkspaceEvidence {
    source_files: Vec<SourceFile>,
    test_files: Vec<SourceFile>,
    package_json: Option<String>,
    cargo_toml: bool,
    readme: bool,
}

#[derive(Debug, Clone)]
struct SourceFile {
    rel: String,
    content: String,
}

pub fn required_evidence_for_capability(capability: &str) -> Vec<String> {
    evidence_kinds_for_capability(capability)
        .into_iter()
        .map(|kind| kind.as_str().to_string())
        .collect()
}

pub fn verify_runtime_acceptance(
    root: &Path,
    required_paths: &[String],
    verify_commands: &[String],
    required_capabilities: &[String],
    required_evidence: &[String],
    deferred_verify_requirements: &[String],
) -> RuntimeAcceptanceReport {
    if required_capabilities.is_empty() && required_evidence.is_empty() {
        return RuntimeAcceptanceReport {
            passed: true,
            primary_reason: "pass".to_string(),
            ..RuntimeAcceptanceReport::default()
        };
    }

    let workspace = collect_workspace_evidence(root);
    let mut required = BTreeSet::new();
    let mut missing_capabilities = Vec::new();
    for capability in required_capabilities {
        let kinds = evidence_kinds_for_capability(capability);
        if kinds.is_empty() {
            missing_capabilities.push(format!("unsupported_required_capability:{capability}"));
        }
        for kind in kinds {
            required.insert(kind.as_str().to_string());
        }
    }
    for evidence in required_evidence {
        let trimmed = evidence.trim();
        if !trimmed.is_empty() {
            required.insert(trimmed.to_string());
        }
    }

    let mut missing_evidence = Vec::new();
    let mut weak_evidence = Vec::new();
    let mut inconclusive_reasons = Vec::new();
    if required_capabilities
        .iter()
        .any(|capability| capability.trim() == "browser_interaction")
    {
        inconclusive_reasons.push("browser_required_but_not_available".to_string());
    }
    for evidence in &required {
        match evidence.as_str() {
            "implementation_artifact" => {
                if !has_implementation_artifact(root, required_paths, &workspace) {
                    missing_evidence.push(evidence.clone());
                }
            }
            "test_artifact" => {
                if !has_test_artifact(&workspace) {
                    missing_evidence.push(evidence.clone());
                }
            }
            "bound_verify_command" => {
                if !has_bound_verify_command(verify_commands, &workspace) {
                    missing_evidence.push(evidence.clone());
                }
            }
            "non_zero_test_or_assertion_evidence" => {
                if !has_assertion_or_test_evidence(&workspace) {
                    missing_evidence.push(evidence.clone());
                }
            }
            "build_command_or_dependency_missing_boundary" => {
                if !has_build_command_or_dependency_boundary(
                    verify_commands,
                    deferred_verify_requirements,
                    &workspace,
                ) {
                    missing_evidence.push(evidence.clone());
                }
            }
            "interactive_ui_source_evidence" => {
                if !has_interactive_ui_source(&workspace) {
                    missing_evidence.push(evidence.clone());
                }
            }
            "non_static_screen_evidence" => {
                if !has_non_static_screen_evidence(&workspace) {
                    missing_evidence.push(evidence.clone());
                }
            }
            "requested_content_evidence" => {
                if !workspace.readme && required_paths.iter().all(|path| !path.ends_with(".md")) {
                    missing_evidence.push(evidence.clone());
                }
            }
            unknown => missing_evidence.push(format!("unsupported_required_evidence:{unknown}")),
        }
    }

    collect_weak_verify_evidence(verify_commands, &workspace, &mut weak_evidence);
    let inconclusive = !inconclusive_reasons.is_empty();
    let passed = missing_capabilities.is_empty() && missing_evidence.is_empty() && !inconclusive;
    let primary_reason = if let Some(reason) = missing_capabilities.first() {
        format!("missing_required_capabilities:{reason}")
    } else if let Some(reason) = missing_evidence.first() {
        format!("missing_required_evidence:{reason}")
    } else if let Some(reason) = inconclusive_reasons.first() {
        format!("inconclusive_acceptance:{reason}")
    } else if let Some(reason) = weak_evidence.first() {
        format!("weak_verification_evidence:{reason}")
    } else {
        "pass".to_string()
    };

    RuntimeAcceptanceReport {
        passed,
        inconclusive,
        missing_capabilities,
        missing_evidence,
        weak_evidence,
        inconclusive_reasons,
        primary_reason,
    }
}

fn evidence_kinds_for_capability(capability: &str) -> Vec<EvidenceKind> {
    match capability.trim() {
        "implementation" | "entrypoint" | "input_output_contract" => {
            vec![EvidenceKind::ImplementationArtifact]
        }
        "requested_content" => vec![EvidenceKind::RequestedContent],
        "deterministic_test" => vec![EvidenceKind::TestArtifact, EvidenceKind::BoundVerifyCommand],
        "deterministic_check" => vec![
            EvidenceKind::BoundVerifyCommand,
            EvidenceKind::NonZeroTestOrAssertionEvidence,
        ],
        "buildable" => vec![EvidenceKind::BuildCommandOrDependencyBoundary],
        "browser_interaction"
        | "playable_ui"
        | "stateful_interaction"
        | "start_or_restart_flow"
        | "player_control"
        | "adversary_or_challenge"
        | "progression_or_score"
        | "failure_or_collision_rule"
        | "user_input_or_action"
        | "visible_state_change" => vec![
            EvidenceKind::ImplementationArtifact,
            EvidenceKind::InteractiveUiSourceEvidence,
            EvidenceKind::NonStaticScreenEvidence,
        ],
        _ => Vec::new(),
    }
}

fn collect_workspace_evidence(root: &Path) -> WorkspaceEvidence {
    let mut evidence = WorkspaceEvidence::default();
    for path in collect_candidate_files(root) {
        let Ok(rel) = path.strip_prefix(root) else {
            continue;
        };
        let rel = rel.to_string_lossy().replace('\\', "/");
        if rel == "package.json" {
            evidence.package_json = std::fs::read_to_string(&path).ok();
        }
        if rel == "Cargo.toml" {
            evidence.cargo_toml = true;
        }
        if rel.eq_ignore_ascii_case("README.md") {
            evidence.readme = true;
        }
        if !looks_like_source_or_test(&rel) {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        let file = SourceFile {
            rel: rel.clone(),
            content,
        };
        if looks_like_test_file(&rel) {
            evidence.test_files.push(file.clone());
        }
        evidence.source_files.push(file);
    }
    evidence
}

fn collect_candidate_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            if should_skip_entry(&name) {
                continue;
            }
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_dir() {
                stack.push(path);
            } else if file_type.is_file() {
                out.push(path);
            }
        }
    }
    out
}

fn should_skip_entry(name: &str) -> bool {
    matches!(
        name,
        ".git" | ".anvil" | "target" | "node_modules" | ".next" | "dist" | "build"
    )
}

fn looks_like_source_or_test(rel: &str) -> bool {
    let lower = rel.to_ascii_lowercase();
    lower.ends_with(".js")
        || lower.ends_with(".jsx")
        || lower.ends_with(".mjs")
        || lower.ends_with(".cjs")
        || lower.ends_with(".ts")
        || lower.ends_with(".tsx")
        || lower.ends_with(".py")
        || lower.ends_with(".rs")
        || lower.ends_with(".md")
}

fn looks_like_test_file(rel: &str) -> bool {
    let lower = rel.to_ascii_lowercase();
    lower.contains("/tests/")
        || lower.starts_with("tests/")
        || lower.contains("/test/")
        || lower.starts_with("test/")
        || lower.contains(".test.")
        || lower.contains(".spec.")
        || lower.starts_with("test_")
        || lower.ends_with("_test.py")
        || lower.ends_with("_test.rs")
}

fn has_implementation_artifact(
    root: &Path,
    required_paths: &[String],
    workspace: &WorkspaceEvidence,
) -> bool {
    required_paths.iter().any(|path| root.join(path).is_file())
        || workspace.source_files.iter().any(|file| {
            !looks_like_test_file(&file.rel)
                && !file.rel.eq_ignore_ascii_case("README.md")
                && !file.content.trim().is_empty()
        })
}

fn has_test_artifact(workspace: &WorkspaceEvidence) -> bool {
    !workspace.test_files.is_empty()
        || workspace
            .source_files
            .iter()
            .any(|file| has_inline_test_or_self_test(file))
}

fn has_assertion_or_test_evidence(workspace: &WorkspaceEvidence) -> bool {
    workspace.source_files.iter().any(|file| {
        has_inline_test_or_self_test(file)
            || contains_assertion(&file.content)
            || file.content.contains("#[test]")
    })
}

fn has_inline_test_or_self_test(file: &SourceFile) -> bool {
    let content = file.content.as_str();
    if file.rel.ends_with(".rs") {
        return content.contains("#[test]") || content.contains("#[cfg(test)]");
    }
    if file.rel.ends_with(".py") {
        return (content.contains("def test_") || content.contains("unittest.TestCase"))
            && contains_assertion(content);
    }
    if file.rel.ends_with(".js")
        || file.rel.ends_with(".mjs")
        || file.rel.ends_with(".cjs")
        || file.rel.ends_with(".ts")
        || file.rel.ends_with(".tsx")
        || file.rel.ends_with(".jsx")
    {
        return (content.contains("node:assert")
            || content.contains("require(\"assert\")")
            || content.contains("require('assert')")
            || content.contains("assert."))
            && contains_assertion(content);
    }
    false
}

fn contains_assertion(content: &str) -> bool {
    content.contains("assert")
        || content.contains("expect(")
        || content.contains("should")
        || content.contains("assert_eq!")
        || content.contains("assert_ne!")
}

fn has_bound_verify_command(verify_commands: &[String], workspace: &WorkspaceEvidence) -> bool {
    verify_commands
        .iter()
        .any(|command| verify_command_kind(command, workspace).is_strong_for_capability())
}

fn has_build_command_or_dependency_boundary(
    verify_commands: &[String],
    deferred_verify_requirements: &[String],
    workspace: &WorkspaceEvidence,
) -> bool {
    verify_commands
        .iter()
        .any(|command| verify_command_kind(command, workspace).is_build())
        || deferred_verify_requirements
            .iter()
            .any(|command| verify_command_kind(command, workspace).is_build())
}

fn collect_weak_verify_evidence(
    verify_commands: &[String],
    workspace: &WorkspaceEvidence,
    weak: &mut Vec<String>,
) {
    for command in verify_commands {
        match verify_command_kind(command, workspace) {
            VerifyCommandKind::Weak(reason) => weak.push(reason),
            VerifyCommandKind::ArtifactOnly => weak.push(format!("artifact_only_verify:{command}")),
            _ => {}
        }
    }
    weak.sort();
    weak.dedup();
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum VerifyCommandKind {
    Test,
    Build,
    StaticSyntax,
    ArtifactOnly,
    Weak(String),
    Other,
}

impl VerifyCommandKind {
    fn is_strong_for_capability(&self) -> bool {
        matches!(self, Self::Test | Self::Build | Self::StaticSyntax)
    }

    fn is_build(&self) -> bool {
        matches!(self, Self::Build)
    }
}

fn verify_command_kind(command: &str, workspace: &WorkspaceEvidence) -> VerifyCommandKind {
    let lower = command.trim().to_ascii_lowercase();
    if lower.starts_with("test -f ") || lower.starts_with("cat ") {
        return VerifyCommandKind::ArtifactOnly;
    }
    if lower == "npm run build"
        || lower == "pnpm build"
        || lower == "yarn build"
        || lower == "cargo build"
        || lower.starts_with("cargo build ")
    {
        return VerifyCommandKind::Build;
    }
    if lower.starts_with("python3 -m py_compile ") || lower.starts_with("python -m py_compile ") {
        return VerifyCommandKind::StaticSyntax;
    }
    if lower == "cargo test" || lower.starts_with("cargo test ") {
        if has_assertion_or_test_evidence(workspace) {
            return VerifyCommandKind::Test;
        }
        return VerifyCommandKind::Weak("cargo_test_without_test_evidence".to_string());
    }
    if lower == "npm test"
        || lower == "npm run test"
        || lower == "pnpm test"
        || lower == "yarn test"
    {
        if has_test_artifact(workspace) {
            return VerifyCommandKind::Test;
        }
        return VerifyCommandKind::Weak("node_test_without_test_artifact".to_string());
    }
    if lower.starts_with("python3 -m unittest") || lower.starts_with("python -m unittest") {
        if has_test_artifact(workspace) {
            return VerifyCommandKind::Test;
        }
        return VerifyCommandKind::Weak("unittest_without_test_artifact".to_string());
    }
    if lower.starts_with("node ") {
        if has_assertion_or_test_evidence(workspace) {
            return VerifyCommandKind::Test;
        }
        return VerifyCommandKind::Weak("node_smoke_without_assertion".to_string());
    }
    VerifyCommandKind::Other
}

fn has_interactive_ui_source(workspace: &WorkspaceEvidence) -> bool {
    workspace.source_files.iter().any(|file| {
        let content = file.content.as_str();
        let lower = content.to_ascii_lowercase();
        (content.contains("useState")
            || content.contains("useReducer")
            || content.contains("addEventListener")
            || content.contains("onKeyDown")
            || content.contains("onClick")
            || content.contains("requestAnimationFrame")
            || lower.contains("<canvas"))
            && (lower.contains("keydown")
                || lower.contains("arrow")
                || lower.contains("click")
                || lower.contains("pointer")
                || lower.contains("touch")
                || lower.contains("canvas"))
    })
}

fn has_non_static_screen_evidence(workspace: &WorkspaceEvidence) -> bool {
    workspace.source_files.iter().any(|file| {
        let lower = file.content.to_ascii_lowercase();
        (lower.contains("score")
            || lower.contains("level")
            || lower.contains("life")
            || lower.contains("lives")
            || lower.contains("enemy")
            || lower.contains("invader")
            || lower.contains("collision")
            || lower.contains("bullet")
            || lower.contains("shot")
            || lower.contains("state"))
            && (lower.contains("setinterval")
                || lower.contains("requestanimationframe")
                || lower.contains("usestate")
                || lower.contains("usereducer")
                || lower.contains("canvas"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn js_exports_only_missing_deterministic_test() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("date-helper.js"),
            "exports.formatDate = (d) => String(d);\n",
        )
        .unwrap();
        let report = verify_runtime_acceptance(
            dir.path(),
            &["date-helper.js".to_string()],
            &["node date-helper.js".to_string()],
            &[
                "implementation".to_string(),
                "deterministic_test".to_string(),
            ],
            &[],
            &[],
        );
        assert!(!report.passed);
        assert!(
            report
                .missing_evidence
                .contains(&"test_artifact".to_string())
        );
        assert!(
            report
                .missing_evidence
                .contains(&"bound_verify_command".to_string())
        );
        assert!(
            report
                .weak_evidence
                .contains(&"node_smoke_without_assertion".to_string())
        );
    }

    #[test]
    fn js_self_test_satisfies_deterministic_test() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("date-helper.js"),
            "const assert = require('assert');\nexports.addDays = () => 1;\nassert.equal(exports.addDays(), 1);\n",
        )
        .unwrap();
        let report = verify_runtime_acceptance(
            dir.path(),
            &["date-helper.js".to_string()],
            &["node date-helper.js".to_string()],
            &[
                "implementation".to_string(),
                "deterministic_test".to_string(),
            ],
            &[],
            &[],
        );
        assert!(report.passed, "{report:?}");
    }

    #[test]
    fn rust_hello_world_missing_deterministic_check() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname=\"x\"\nversion=\"0.1.0\"\nedition=\"2021\"\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("src/main.rs"),
            "fn main(){println!(\"Hello, world!\");}\n",
        )
        .unwrap();
        let report = verify_runtime_acceptance(
            dir.path(),
            &["Cargo.toml".to_string(), "src/main.rs".to_string()],
            &["cargo test".to_string()],
            &["entrypoint".to_string(), "deterministic_check".to_string()],
            &[],
            &[],
        );
        assert!(!report.passed);
        assert!(
            report
                .missing_evidence
                .contains(&"non_zero_test_or_assertion_evidence".to_string())
        );
    }

    #[test]
    fn rust_inline_test_satisfies_deterministic_check() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname=\"x\"\nversion=\"0.1.0\"\nedition=\"2021\"\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("src/main.rs"),
            "fn main(){}\n#[cfg(test)] mod tests { #[test] fn it_works(){ assert_eq!(2, 2); } }\n",
        )
        .unwrap();
        let report = verify_runtime_acceptance(
            dir.path(),
            &["Cargo.toml".to_string(), "src/main.rs".to_string()],
            &["cargo test".to_string()],
            &["entrypoint".to_string(), "deterministic_check".to_string()],
            &[],
            &[],
        );
        assert!(report.passed, "{report:?}");
    }

    #[test]
    fn interactive_game_requires_dynamic_source() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src/app")).unwrap();
        std::fs::write(
            dir.path().join("src/app/page.tsx"),
            "export default function Page(){ return <main>Press any key to start</main>; }\n",
        )
        .unwrap();
        let report = verify_runtime_acceptance(
            dir.path(),
            &["src/app/page.tsx".to_string()],
            &[],
            &[
                "player_control".to_string(),
                "progression_or_score".to_string(),
            ],
            &[],
            &["npm run build".to_string()],
        );
        assert!(!report.passed);
        assert!(
            report
                .missing_evidence
                .contains(&"interactive_ui_source_evidence".to_string())
        );
    }

    #[test]
    fn browser_interaction_is_inconclusive_without_browser_oracle() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src/app")).unwrap();
        std::fs::write(
            dir.path().join("src/app/page.tsx"),
            r#""use client";
export default function Page(){ return <button onClick={() => alert("ok")}>Go</button>; }
"#,
        )
        .unwrap();
        let report = verify_runtime_acceptance(
            dir.path(),
            &["src/app/page.tsx".to_string()],
            &[],
            &["browser_interaction".to_string()],
            &[],
            &[],
        );
        assert!(!report.passed);
        assert!(report.inconclusive);
        assert!(
            report
                .inconclusive_reasons
                .contains(&"browser_required_but_not_available".to_string())
        );
    }
}
