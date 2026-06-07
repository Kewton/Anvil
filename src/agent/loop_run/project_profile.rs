//! Project-profile inference boundary.
//!
//! `TaskContract` owns the objective contract, but project language / shape
//! inference is a separate concern. Keeping this boundary small lets the
//! controller accept an LLM-derived profile later without spreading more string
//! rules through the contract builder.

use serde::Deserialize;

use super::task_contract::{ProjectLanguage, ProjectShape};
use crate::agent::loop_run::lifecycle::extract_first_json_object;
use crate::ollama::xml_fallback::strip_think_tags;

#[derive(Debug, Clone, Copy)]
struct LanguageProfile {
    language: ProjectLanguage,
    lower_markers: &'static [&'static str],
    case_markers: &'static [&'static str],
}

#[derive(Debug, Clone, Copy)]
struct ShapeProfile {
    shape: ProjectShape,
    lower_markers: &'static [&'static str],
    ascii_tokens: &'static [&'static str],
    case_markers: &'static [&'static str],
}

const LANGUAGE_PROFILES: &[LanguageProfile] = &[
    LanguageProfile {
        language: ProjectLanguage::Rust,
        lower_markers: &["rust", "cargo", "crate", "cargo.toml", ".rs", "rustc"],
        case_markers: &["Rust"],
    },
    LanguageProfile {
        language: ProjectLanguage::Node,
        lower_markers: &[
            "node",
            "node.js",
            "nodejs",
            "npm",
            "package.json",
            "javascript",
            "typescript",
            ".js",
            ".ts",
            "tsx",
            "jsx",
        ],
        case_markers: &[],
    },
    LanguageProfile {
        language: ProjectLanguage::Python,
        lower_markers: &[
            "python",
            "python3",
            "pytest",
            "pip",
            "fastapi",
            "flask",
            "django",
            ".py",
            "requirements.txt",
        ],
        case_markers: &["Python"],
    },
    LanguageProfile {
        language: ProjectLanguage::Docs,
        lower_markers: &[
            "readme",
            "markdown",
            ".md",
            "docs/",
            "documentation",
            "manual",
        ],
        case_markers: &["README", "ドキュメント", "仕様書", "設計書", "手順書"],
    },
];

const SHAPE_PROFILES: &[ShapeProfile] = &[
    ShapeProfile {
        shape: ProjectShape::Cli,
        lower_markers: &["cli", "command", "stdin", "stdout"],
        ascii_tokens: &[],
        case_markers: &["標準入力", "コマンド"],
    },
    ShapeProfile {
        shape: ProjectShape::Library,
        lower_markers: &["library", "crate", "package", "module"],
        ascii_tokens: &[],
        case_markers: &["ライブラリ", "クレート", "パッケージ", "モジュール"],
    },
    ShapeProfile {
        shape: ProjectShape::Api,
        lower_markers: &[
            "crud", "endpoint", "server", "backend", "fastapi", "flask", "django",
        ],
        ascii_tokens: &["api"],
        case_markers: &["エンドポイント", "サーバ", "バックエンド"],
    },
    ShapeProfile {
        shape: ProjectShape::WebApp,
        lower_markers: &[
            "web app",
            "browser app",
            "frontend",
            "front-end",
            "next.js",
            "nextjs",
            "react",
            "vue",
            "nuxt",
            "svelte",
        ],
        ascii_tokens: &[],
        case_markers: &["アプリ", "フロントエンド", "画面"],
    },
    ShapeProfile {
        shape: ProjectShape::Documentation,
        lower_markers: &[
            "readme",
            "markdown",
            ".md",
            "docs/",
            "documentation",
            "manual",
        ],
        ascii_tokens: &[],
        case_markers: &["README", "ドキュメント", "仕様書", "設計書", "手順書"],
    },
];

pub(super) fn infer_language(request: &str, lower: &str) -> ProjectLanguage {
    LANGUAGE_PROFILES
        .iter()
        .find(|profile| {
            contains_any(lower, profile.lower_markers)
                || contains_any(request, profile.case_markers)
        })
        .map(|profile| profile.language)
        .unwrap_or(ProjectLanguage::Unknown)
}

pub(super) fn infer_shape(request: &str, lower: &str) -> ProjectShape {
    let explicit_entrypoint_shape = explicit_entrypoint_shape(lower);
    SHAPE_PROFILES
        .iter()
        .find(|profile| {
            matches!(
                (profile.shape, explicit_entrypoint_shape),
                (ProjectShape::Cli, Some(ProjectShape::Cli))
                    | (ProjectShape::Library, Some(ProjectShape::Library))
            ) || contains_any(lower, profile.lower_markers)
                || profile
                    .ascii_tokens
                    .iter()
                    .any(|token| contains_ascii_token(lower, token))
                || contains_any(request, profile.case_markers)
        })
        .map(|profile| profile.shape)
        .unwrap_or(ProjectShape::Unknown)
}

fn explicit_entrypoint_shape(lower: &str) -> Option<ProjectShape> {
    if contains_any(lower, &["src/main.rs", "main.py"]) {
        return Some(ProjectShape::Cli);
    }
    if contains_any(lower, &["src/lib.rs", "lib.rs"]) {
        return Some(ProjectShape::Library);
    }
    None
}

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq)]
pub(super) struct ProjectProfileConfirmation {
    pub(super) language: Option<ProjectLanguage>,
    pub(super) shape: Option<ProjectShape>,
    pub(super) preferred_runner: Option<String>,
    pub(super) confidence: f32,
    pub(super) reason: Option<String>,
}

#[allow(dead_code)]
pub(super) fn build_project_profile_confirm_prompt(
    request: &str,
    language: ProjectLanguage,
    shape: ProjectShape,
) -> String {
    format!(
        concat!(
            "Classify the user's objective profile for a local-first agent.\n",
            "Return exactly one JSON object with keys: language, shape, preferred_runner, confidence, reason.\n",
            "Allowed language values: rust, node, python, docs, unknown.\n",
            "Allowed shape values: cli, library, api, web_app, documentation, unknown.\n",
            "Use preferred_runner only when evidence should be produced by a command, otherwise null.\n",
            "Do not write prose outside JSON.\n\n",
            "First pass: language={:?}, shape={:?}\n",
            "User request:\n{}"
        ),
        language, shape, request
    )
}

#[allow(dead_code)]
pub(super) fn parse_project_profile_confirmation(raw: &str) -> Option<ProjectProfileConfirmation> {
    let stripped = strip_think_tags(raw);
    let json = extract_first_json_object(&stripped)?;
    let decoded: ProjectProfileConfirmationWire = serde_json::from_str(json).ok()?;
    Some(ProjectProfileConfirmation {
        language: decoded.language.as_deref().and_then(parse_language),
        shape: decoded.shape.as_deref().and_then(parse_shape),
        preferred_runner: decoded
            .preferred_runner
            .filter(|runner| runner.len() <= 128),
        confidence: decoded.confidence.unwrap_or(0.0).clamp(0.0, 1.0),
        reason: decoded.reason.filter(|reason| reason.len() <= 256),
    })
}

#[derive(Debug, Deserialize)]
struct ProjectProfileConfirmationWire {
    language: Option<String>,
    shape: Option<String>,
    preferred_runner: Option<String>,
    confidence: Option<f32>,
    reason: Option<String>,
}

fn parse_language(value: &str) -> Option<ProjectLanguage> {
    match value.trim().to_ascii_lowercase().as_str() {
        "rust" => Some(ProjectLanguage::Rust),
        "node" | "javascript" | "typescript" => Some(ProjectLanguage::Node),
        "python" => Some(ProjectLanguage::Python),
        "docs" | "documentation" => Some(ProjectLanguage::Docs),
        "unknown" => Some(ProjectLanguage::Unknown),
        _ => None,
    }
}

fn parse_shape(value: &str) -> Option<ProjectShape> {
    match value.trim().to_ascii_lowercase().as_str() {
        "cli" => Some(ProjectShape::Cli),
        "library" | "lib" => Some(ProjectShape::Library),
        "api" => Some(ProjectShape::Api),
        "web_app" | "webapp" | "web app" => Some(ProjectShape::WebApp),
        "documentation" | "docs" => Some(ProjectShape::Documentation),
        "unknown" => Some(ProjectShape::Unknown),
        _ => None,
    }
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}

fn contains_ascii_token(haystack: &str, needle: &str) -> bool {
    haystack
        .split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_' || ch == '-'))
        .any(|token| token == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_inference_preserves_entrypoint_shape_over_generic_library_words() {
        let request = "Create src/main.rs as a Rust package with tests.";
        let lower = request.to_ascii_lowercase();

        assert_eq!(infer_language(request, &lower), ProjectLanguage::Rust);
        assert_eq!(infer_shape(request, &lower), ProjectShape::Cli);
    }

    #[test]
    fn profile_inference_preserves_legacy_cli_priority_for_ambiguous_entrypoint_text() {
        let request = "Create src/lib.rs as a command module.";
        let lower = request.to_ascii_lowercase();

        assert_eq!(infer_shape(request, &lower), ProjectShape::Cli);
    }

    #[test]
    fn profile_inference_supports_non_coding_documentation() {
        let request = "READMEに手順書を追記してください";
        let lower = request.to_ascii_lowercase();

        assert_eq!(infer_language(request, &lower), ProjectLanguage::Docs);
        assert_eq!(infer_shape(request, &lower), ProjectShape::Documentation);
    }

    #[test]
    fn llm_profile_confirmation_parser_accepts_think_wrapped_json() {
        let raw = r#"<think>draft</think>{"language":"python","shape":"api","preferred_runner":"pytest","confidence":0.82,"reason":"FastAPI request"}"#;

        let parsed = parse_project_profile_confirmation(raw).expect("parse profile");

        assert_eq!(parsed.language, Some(ProjectLanguage::Python));
        assert_eq!(parsed.shape, Some(ProjectShape::Api));
        assert_eq!(parsed.preferred_runner.as_deref(), Some("pytest"));
        assert_eq!(parsed.confidence, 0.82);
    }

    #[test]
    fn llm_profile_confirmation_parser_rejects_unknown_labels_without_rule_growth() {
        let parsed = parse_project_profile_confirmation(
            r#"{"language":"spreadsheet","shape":"analysis","confidence":4.2}"#,
        )
        .expect("json is syntactically valid");

        assert_eq!(parsed.language, None);
        assert_eq!(parsed.shape, None);
        assert_eq!(parsed.confidence, 1.0);
    }
}
