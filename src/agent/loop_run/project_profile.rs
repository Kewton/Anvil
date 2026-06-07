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

pub(super) const PROJECT_PROFILE_CONFIRM_TIMEOUT_SECS: u64 = 10;
pub(super) const PROJECT_PROFILE_CONFIRM_CONFIDENCE_THRESHOLD: f32 = 0.70;

pub(super) fn confirmation_is_authoritative(profile: &ProjectProfileConfirmation) -> bool {
    profile.confidence >= PROJECT_PROFILE_CONFIRM_CONFIDENCE_THRESHOLD
}

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

#[derive(Debug, Clone, PartialEq)]
pub(super) struct ProjectProfileConfirmation {
    pub(super) language: Option<ProjectLanguage>,
    pub(super) shape: Option<ProjectShape>,
    pub(super) deliverable_kind: Option<ProfileDeliverableKind>,
    pub(super) primary_artifacts: Vec<String>,
    pub(super) forbidden_artifacts: Vec<ForbiddenArtifact>,
    pub(super) evidence_kind: Option<ProfileEvidenceKind>,
    pub(super) needs_environment_setup: Option<bool>,
    pub(super) preferred_runner: Option<String>,
    pub(super) confidence: f32,
    pub(super) reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ProfileDeliverableKind {
    Code,
    Document,
    Data,
    ResearchReport,
    CommandObservation,
    None,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ProfileEvidenceKind {
    TestRun,
    ContentCheck,
    SchemaCheck,
    CommandObservation,
    SourceFetch,
    None,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ForbiddenArtifact {
    SourceCode,
    Tests,
    Setup,
    Unknown,
}

pub(super) fn build_project_profile_confirm_prompt(
    request: &str,
    language: ProjectLanguage,
    shape: ProjectShape,
    first_pass_summary: &str,
) -> String {
    format!(
        concat!(
            "Classify the user's objective profile for a local-first agent.\n",
            "Return exactly one JSON object with keys: language, shape, deliverable_kind, ",
            "primary_artifacts, forbidden_artifacts, evidence_kind, needs_environment_setup, ",
            "preferred_runner, confidence, reason.\n",
            "The user request is authoritative. The first pass is only a low-priority hint and may be wrong.\n",
            "If first-pass language, shape, task kind, setup, or coding signals conflict with the user request, ignore the first pass.\n",
            "Allowed language values: rust, node, python, docs, unknown.\n",
            "Allowed shape values: cli, library, api, web_app, documentation, unknown.\n",
            "Allowed deliverable_kind values: code, document, data, research_report, command_observation, none, unknown.\n",
            "Allowed forbidden_artifacts values: source_code, tests, setup. Use [] when none.\n",
            "Allowed evidence_kind values: test_run, content_check, schema_check, command_observation, source_fetch, none, unknown.\n",
            "confidence must be a number from 0.0 to 1.0, not a word.\n",
            "All enum values must be quoted JSON strings. primary_artifacts must be an array of path strings, not objects.\n",
            "JSON, CSV, TSV, and other structured output files are deliverable_kind=data, not code.\n",
            "Do not infer rust/library/api from the first pass when the user explicitly says no source code, tests, or scripts.\n",
            "primary_artifacts must contain only output deliverables the agent should create or modify; never include files the user asks to read, inspect, summarize, or use as input.\n",
            "When both input and output files are mentioned, list only the output file(s) in primary_artifacts.\n",
            "Use preferred_runner only when evidence should be produced by a command, otherwise null.\n",
            "If the user forbids source code or tests, put that in forbidden_artifacts.\n",
            "If setup is a document section rather than environment work, set needs_environment_setup=false.\n",
            "Do not write prose outside JSON.\n\n",
            "First pass: language={:?}, shape={:?}\n",
            "First pass contract summary: {}\n",
            "User request:\n{}"
        ),
        language, shape, first_pass_summary, request
    )
}

pub(super) fn parse_project_profile_confirmation(raw: &str) -> Option<ProjectProfileConfirmation> {
    let stripped = strip_think_tags(raw);
    let json = extract_first_json_object(&stripped)?;
    let decoded = decode_project_profile_confirmation_wire(json)?;
    Some(ProjectProfileConfirmation {
        language: decoded.language.as_deref().and_then(parse_language),
        shape: decoded.shape.as_deref().and_then(parse_shape),
        deliverable_kind: decoded
            .deliverable_kind
            .as_deref()
            .and_then(parse_deliverable_kind),
        primary_artifacts: sanitize_artifact_paths(decoded.primary_artifacts.unwrap_or_default()),
        forbidden_artifacts: decoded
            .forbidden_artifacts
            .unwrap_or_default()
            .iter()
            .filter_map(|value| parse_forbidden_artifact(value))
            .collect(),
        evidence_kind: decoded
            .evidence_kind
            .as_deref()
            .and_then(parse_evidence_kind),
        needs_environment_setup: decoded.needs_environment_setup,
        preferred_runner: decoded
            .preferred_runner
            .filter(|runner| runner.len() <= 128),
        confidence: decoded.confidence.unwrap_or(0.0).clamp(0.0, 1.0),
        reason: decoded.reason.filter(|reason| reason.len() <= 256),
    })
}

fn decode_project_profile_confirmation_wire(json: &str) -> Option<ProjectProfileConfirmationWire> {
    serde_json::from_str(json).ok().or_else(|| {
        let normalized = normalize_jsonish_constants(json);
        serde_json::from_str(&normalized).ok()
    })
}

pub(super) fn project_profile_confirm_disabled<F>(getenv: F) -> bool
where
    F: Fn(&str) -> Result<String, std::env::VarError>,
{
    matches!(
        getenv("ANVIL_NO_PROJECT_PROFILE_CONFIRM")
            .ok()
            .as_deref()
            .map(str::trim),
        Some("1" | "true" | "TRUE" | "yes" | "YES")
    )
}

#[derive(Debug, Deserialize)]
struct ProjectProfileConfirmationWire {
    #[serde(default, deserialize_with = "deserialize_optional_string_scalar")]
    language: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_string_scalar")]
    shape: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_string_scalar")]
    deliverable_kind: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_string_vec")]
    primary_artifacts: Option<Vec<String>>,
    #[serde(default, deserialize_with = "deserialize_optional_string_vec")]
    forbidden_artifacts: Option<Vec<String>>,
    #[serde(default, deserialize_with = "deserialize_optional_string_scalar")]
    evidence_kind: Option<String>,
    needs_environment_setup: Option<bool>,
    #[serde(default, deserialize_with = "deserialize_optional_string_scalar")]
    preferred_runner: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_confidence")]
    confidence: Option<f32>,
    #[serde(default, deserialize_with = "deserialize_optional_string_scalar")]
    reason: Option<String>,
}

fn deserialize_optional_string_scalar<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    Ok(value.and_then(string_scalar_from_value))
}

fn string_scalar_from_value(value: serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::String(value) => Some(value),
        serde_json::Value::Array(items) => items
            .into_iter()
            .find_map(|item| item.as_str().map(str::to_string)),
        _ => None,
    }
}

fn deserialize_optional_string_vec<'de, D>(deserializer: D) -> Result<Option<Vec<String>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    Ok(value.map(string_vec_from_value))
}

fn string_vec_from_value(value: serde_json::Value) -> Vec<String> {
    match value {
        serde_json::Value::Array(items) => items
            .into_iter()
            .filter_map(string_from_artifact_value)
            .collect(),
        serde_json::Value::String(value) => value
            .split([',', '|'])
            .map(str::trim)
            .filter(|item| !item.is_empty())
            .map(str::to_string)
            .collect(),
        _ => Vec::new(),
    }
}

fn string_from_artifact_value(value: serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::String(value) => Some(value),
        serde_json::Value::Object(object) => ["path", "name", "file"]
            .into_iter()
            .find_map(|key| object.get(key).and_then(|value| value.as_str()))
            .map(str::to_string),
        _ => None,
    }
}

fn normalize_jsonish_constants(input: &str) -> String {
    let mut normalized = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    let mut in_string = false;
    let mut escaped = false;
    let mut expecting_value = false;

    while let Some(ch) = chars.next() {
        if in_string {
            normalized.push(ch);
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }

        if ch == '"' {
            in_string = true;
            normalized.push(ch);
            continue;
        }

        if ch == ':' {
            expecting_value = true;
            normalized.push(ch);
            continue;
        }

        if ch == '[' || ch == ',' {
            expecting_value = true;
            normalized.push(ch);
            continue;
        }

        if expecting_value && ch.is_ascii_whitespace() {
            normalized.push(ch);
            continue;
        }

        if ch.is_ascii_alphabetic() {
            let mut token = String::from(ch);
            while chars
                .peek()
                .is_some_and(|next| next.is_ascii_alphanumeric() || *next == '_' || *next == '-')
            {
                token.push(chars.next().expect("peeked"));
            }
            match token.as_str() {
                "None" => normalized.push_str("null"),
                "True" => normalized.push_str("true"),
                "False" => normalized.push_str("false"),
                "null" | "true" | "false" => normalized.push_str(&token),
                _ if expecting_value => {
                    normalized.push('"');
                    normalized.push_str(&token);
                    normalized.push('"');
                }
                _ => normalized.push_str(&token),
            }
            expecting_value = false;
        } else {
            if !ch.is_ascii_whitespace() {
                expecting_value = false;
            }
            normalized.push(ch);
        }
    }

    normalized
}

fn deserialize_optional_confidence<'de, D>(deserializer: D) -> Result<Option<f32>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    Ok(value.and_then(parse_confidence_value))
}

fn parse_confidence_value(value: serde_json::Value) -> Option<f32> {
    match value {
        serde_json::Value::Number(number) => number.as_f64().map(|value| value as f32),
        serde_json::Value::String(label) => parse_confidence_label(&label),
        _ => None,
    }
}

fn parse_confidence_label(label: &str) -> Option<f32> {
    let normalized = label.trim().to_ascii_lowercase();
    if let Ok(value) = normalized.parse::<f32>() {
        return Some(value);
    }
    match normalized.as_str() {
        "high" => Some(0.85),
        "medium" | "moderate" => Some(0.50),
        "low" => Some(0.25),
        _ => None,
    }
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

fn parse_deliverable_kind(value: &str) -> Option<ProfileDeliverableKind> {
    match value.trim().to_ascii_lowercase().as_str() {
        "code" | "source_code" | "implementation" => Some(ProfileDeliverableKind::Code),
        "document" | "docs" | "documentation" => Some(ProfileDeliverableKind::Document),
        "data" | "data_output" => Some(ProfileDeliverableKind::Data),
        "research_report" | "report" => Some(ProfileDeliverableKind::ResearchReport),
        "command_observation" | "ops" | "shell" => Some(ProfileDeliverableKind::CommandObservation),
        "none" | "answer_only" => Some(ProfileDeliverableKind::None),
        "unknown" => Some(ProfileDeliverableKind::Unknown),
        _ => None,
    }
}

fn parse_evidence_kind(value: &str) -> Option<ProfileEvidenceKind> {
    match value.trim().to_ascii_lowercase().as_str() {
        "test_run" | "tests" | "cargo test" | "npm test" | "pytest" => {
            Some(ProfileEvidenceKind::TestRun)
        }
        "content_check" | "document_check" => Some(ProfileEvidenceKind::ContentCheck),
        "schema_check" | "data_schema" => Some(ProfileEvidenceKind::SchemaCheck),
        "command_observation" | "shell_observation" => {
            Some(ProfileEvidenceKind::CommandObservation)
        }
        "source_fetch" | "source_check" => Some(ProfileEvidenceKind::SourceFetch),
        "none" => Some(ProfileEvidenceKind::None),
        "unknown" => Some(ProfileEvidenceKind::Unknown),
        _ => None,
    }
}

fn parse_forbidden_artifact(value: &str) -> Option<ForbiddenArtifact> {
    match value.trim().to_ascii_lowercase().as_str() {
        "source_code" | "code" | "implementation" => Some(ForbiddenArtifact::SourceCode),
        "tests" | "test" => Some(ForbiddenArtifact::Tests),
        "setup" | "environment_setup" => Some(ForbiddenArtifact::Setup),
        "unknown" => Some(ForbiddenArtifact::Unknown),
        _ => None,
    }
}

fn sanitize_artifact_paths(paths: Vec<String>) -> Vec<String> {
    let mut sanitized = Vec::new();
    for path in paths.into_iter().take(8) {
        let trimmed = path.trim();
        if trimmed.is_empty()
            || trimmed.len() > 160
            || trimmed.starts_with('/')
            || trimmed.contains("..")
            || trimmed.contains('\0')
            || trimmed.chars().any(char::is_control)
        {
            continue;
        }
        if !sanitized.iter().any(|existing| existing == trimmed) {
            sanitized.push(trimmed.to_string());
        }
    }
    sanitized
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
        let raw = r#"<think>draft</think>{"language":"python","shape":"api","deliverable_kind":"code","primary_artifacts":["app/main.py"],"forbidden_artifacts":[],"evidence_kind":"test_run","needs_environment_setup":true,"preferred_runner":"pytest","confidence":0.82,"reason":"FastAPI request"}"#;

        let parsed = parse_project_profile_confirmation(raw).expect("parse profile");

        assert_eq!(parsed.language, Some(ProjectLanguage::Python));
        assert_eq!(parsed.shape, Some(ProjectShape::Api));
        assert_eq!(parsed.deliverable_kind, Some(ProfileDeliverableKind::Code));
        assert_eq!(parsed.primary_artifacts, vec!["app/main.py"]);
        assert_eq!(parsed.evidence_kind, Some(ProfileEvidenceKind::TestRun));
        assert_eq!(parsed.needs_environment_setup, Some(true));
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

    #[test]
    fn llm_profile_confirmation_parser_accepts_label_confidence() {
        let parsed = parse_project_profile_confirmation(
            r#"{"language":"docs","shape":"documentation","deliverable_kind":"document","primary_artifacts":["README.md"],"evidence_kind":"content_check","confidence":"high"}"#,
        )
        .expect("parse profile");

        assert_eq!(parsed.confidence, 0.85);
        assert_eq!(
            parsed.deliverable_kind,
            Some(ProfileDeliverableKind::Document)
        );
    }

    #[test]
    fn llm_profile_confirmation_parser_accepts_singleton_enum_arrays() {
        let parsed = parse_project_profile_confirmation(
            r#"{"language":["docs"],"shape":["documentation"],"deliverable_kind":["document"],"primary_artifacts":["README.md"],"forbidden_artifacts":["source_code"],"evidence_kind":["content_check"],"needs_environment_setup":false,"confidence":0.92}"#,
        )
        .expect("parse profile");

        assert_eq!(parsed.language, Some(ProjectLanguage::Docs));
        assert_eq!(parsed.shape, Some(ProjectShape::Documentation));
        assert_eq!(
            parsed.deliverable_kind,
            Some(ProfileDeliverableKind::Document)
        );
        assert_eq!(
            parsed.evidence_kind,
            Some(ProfileEvidenceKind::ContentCheck)
        );
        assert_eq!(parsed.confidence, 0.92);
    }

    #[test]
    fn llm_profile_confirmation_parser_accepts_artifact_object_arrays() {
        let parsed = parse_project_profile_confirmation(
            r#"{"language":"docs","shape":"unknown","deliverable_kind":"data","primary_artifacts":[{"name":"summary.json","action":"create"}],"forbidden_artifacts":[],"evidence_kind":"schema_check","confidence":0.9}"#,
        )
        .expect("parse profile");

        assert_eq!(parsed.primary_artifacts, vec!["summary.json"]);
        assert_eq!(parsed.deliverable_kind, Some(ProfileDeliverableKind::Data));
        assert_eq!(parsed.evidence_kind, Some(ProfileEvidenceKind::SchemaCheck));
    }

    #[test]
    fn llm_profile_confirmation_parser_accepts_unquoted_enum_values() {
        let parsed = parse_project_profile_confirmation(
            r#"{
                "language": null,
                "shape": unknown,
                "deliverable_kind": data,
                "primary_artifacts": ["/inventory.csv", "/summary.json"],
                "forbidden_artifacts": [],
                "evidence_kind": content_check,
                "needs_environment_setup": false,
                "preferred_runner": null,
                "confidence": 0.95,
                "reason": "data processing"
            }"#,
        )
        .expect("parse profile");

        assert_eq!(parsed.shape, Some(ProjectShape::Unknown));
        assert_eq!(parsed.deliverable_kind, Some(ProfileDeliverableKind::Data));
        assert_eq!(
            parsed.evidence_kind,
            Some(ProfileEvidenceKind::ContentCheck)
        );
        assert!(parsed.primary_artifacts.is_empty());
        assert_eq!(parsed.confidence, 0.95);
    }

    #[test]
    fn project_profile_prompt_marks_primary_artifacts_as_outputs_only() {
        let prompt = build_project_profile_confirm_prompt(
            "Read inventory.csv and create summary.json.",
            ProjectLanguage::Unknown,
            ProjectShape::Unknown,
            "task_kind=data",
        );

        assert!(prompt.contains("only output deliverables"));
        assert!(prompt.contains("never include files the user asks to read"));
        assert!(prompt.contains("list only the output file"));
        assert!(prompt.contains("structured output files are deliverable_kind=data"));
        assert!(prompt.contains("array of path strings"));
        assert!(prompt.contains("first pass is only a low-priority hint"));
        assert!(prompt.contains("user request, ignore the first pass"));
    }

    #[test]
    fn llm_profile_confirmation_parser_accepts_jsonish_none_array() {
        let parsed = parse_project_profile_confirmation(
            r#"{"language":"docs","shape":"cli","deliverable_kind":"document","primary_artifacts":["README.md"],"forbidden_artifacts":[None],"evidence_kind":"none","needs_environment_setup":False,"confidence":0.85}"#,
        )
        .expect("parse profile");

        assert_eq!(parsed.primary_artifacts, vec!["README.md"]);
        assert!(parsed.forbidden_artifacts.is_empty());
        assert_eq!(parsed.needs_environment_setup, Some(false));
        assert_eq!(parsed.confidence, 0.85);
    }

    #[test]
    fn llm_profile_confirmation_parser_captures_non_goals_and_document_setup() {
        let parsed = parse_project_profile_confirmation(
            r#"{"language":"docs","shape":"documentation","deliverable_kind":"document","primary_artifacts":["README.md","../escape.md"],"forbidden_artifacts":["source_code","tests"],"evidence_kind":"content_check","needs_environment_setup":false,"confidence":0.91}"#,
        )
        .expect("parse profile");

        assert_eq!(
            parsed.deliverable_kind,
            Some(ProfileDeliverableKind::Document)
        );
        assert_eq!(parsed.primary_artifacts, vec!["README.md"]);
        assert_eq!(
            parsed.forbidden_artifacts,
            vec![ForbiddenArtifact::SourceCode, ForbiddenArtifact::Tests]
        );
        assert_eq!(
            parsed.evidence_kind,
            Some(ProfileEvidenceKind::ContentCheck)
        );
        assert_eq!(parsed.needs_environment_setup, Some(false));
    }

    #[test]
    fn project_profile_confirm_disable_env_accepts_boolean_values() {
        assert!(project_profile_confirm_disabled(|_| Ok("true".to_string())));
        assert!(project_profile_confirm_disabled(|_| Ok("1".to_string())));
        assert!(!project_profile_confirm_disabled(|_| Ok("0".to_string())));
        assert!(!project_profile_confirm_disabled(|_| Err(
            std::env::VarError::NotPresent
        )));
    }
}
