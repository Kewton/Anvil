//! Artifact obligation planning helpers for TaskContract construction.
//!
//! This module owns typed obligation merge/shadow behavior and project-profile
//! obligation projection. It deliberately does not inspect raw request text.

use super::task_contract::{
    ArtifactObligation, ArtifactRole, DeliverableKind, DeliverableSchema, ProjectIntent,
    ProjectLanguage, ProjectShape, request_asks_for_data_output_artifact_with_scan,
    request_negates_test_artifacts,
};
use super::task_contract_data_output_context::explicit_path_with_data_extension_with_scan;
use super::task_contract_path_context::{OutputContextScan, contains_any, is_ascii_word_char};

pub(super) fn default_readme_required_sections() -> Vec<String> {
    ["setup", "usage", "test"]
        .into_iter()
        .map(str::to_string)
        .collect()
}

pub(super) fn required_doc_sections_from_request(request: &str) -> Vec<String> {
    if let Some(sections) = explicit_required_sections_list_from_request(request) {
        return sections;
    }
    let lower = request.to_ascii_lowercase();
    let mut sections = Vec::new();
    push_section_if(
        &mut sections,
        contains_any(&lower, &["overview", "summary"]) || contains_any(request, &["概要", "要約"]),
        "overview",
    );
    let mentions_setup = contains_any(&lower, &["setup", "getting started"])
        || contains_any(request, &["セットアップ", "導入"]);
    let mentions_installation = contains_any(&lower, &["install", "installation"])
        || contains_any(request, &["インストール"]);
    push_section_if(
        &mut sections,
        mentions_setup || mentions_installation,
        if mentions_setup {
            "setup"
        } else {
            "installation"
        },
    );
    push_section_if(
        &mut sections,
        contains_any(&lower, &["usage", "how to use", "examples", "example"])
            || contains_any(request, &["使用方法", "使い方", "利用方法", "例"]),
        "usage",
    );
    let test_artifacts_negated = request_negates_test_artifacts(request, &lower);
    let explicit_testing_section = contains_any(
        &lower,
        &[
            "test method",
            "testing section",
            "testing sections",
            "tests section",
            "tests sections",
        ],
    ) || contains_any(request, &["テスト方法"]);
    push_section_if(
        &mut sections,
        explicit_testing_section
            || (!test_artifacts_negated
                && (contains_any(&lower, &["testing", "tests"])
                    || contains_any(request, &["テスト", "検証"]))),
        "testing",
    );
    push_section_if(
        &mut sections,
        contains_any(&lower, &["api", "endpoint", "reference", "configuration"])
            || contains_any(request, &["API", "エンドポイント", "設定"]),
        "reference",
    );
    if sections.is_empty() {
        sections.push("overview".to_string());
    }
    sections
}

fn explicit_required_sections_list_from_request(request: &str) -> Option<Vec<String>> {
    let lower = request.to_ascii_lowercase();
    let marker = ["sections:", "sections："]
        .into_iter()
        .find_map(|marker| lower.find(marker).map(|idx| (idx, marker.len())))?;
    let after = &request[marker.0 + marker.1..];
    let end = after
        .char_indices()
        .find_map(|(idx, ch)| matches!(ch, '.' | '\n' | '\r').then_some(idx))
        .unwrap_or(after.len());
    let list = &after[..end];
    let sections = list
        .split([',', ';', '、', '，'])
        .filter_map(normalize_explicit_section_label)
        .take(12)
        .collect::<Vec<_>>();
    (!sections.is_empty()).then_some(sections)
}

fn normalize_explicit_section_label(raw: &str) -> Option<String> {
    let mut label = raw
        .trim()
        .trim_matches(|ch: char| matches!(ch, '"' | '\'' | '`' | '[' | ']' | '(' | ')' | ':'));
    for prefix in ["and ", "or "] {
        if label
            .get(..prefix.len())
            .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
        {
            label = label[prefix.len()..].trim();
        }
    }
    let normalized = label
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase();
    if normalized.len() < 2
        || normalized.len() > 80
        || normalized.contains("do not")
        || normalized.contains("source code")
        || normalized.contains("package.json")
        || normalized.contains("cargo.toml")
    {
        return None;
    }
    Some(normalized)
}

pub(super) fn required_research_sections_from_request(request: &str) -> Vec<String> {
    let lower = request.to_ascii_lowercase();
    let mut sections = vec!["findings".to_string(), "sources".to_string()];
    push_section_if(
        &mut sections,
        contains_any(&lower, &["recommend", "compare", "tradeoff"])
            || contains_any(request, &["比較", "推奨", "トレードオフ"]),
        "recommendation",
    );
    sections
}

/// Issue #923 (P6): the explicit-override carrier for Ops runbooks. Emits only
/// canonical `OpsSection` labels, so the values are safe to store on the
/// obligation and surface in diagnostics / repair packets.
pub(super) fn required_ops_sections_from_request(request: &str) -> Vec<String> {
    let lower = request.to_ascii_lowercase();
    let mut sections = Vec::new();
    use super::verifier::OpsSection;
    push_section_if(
        &mut sections,
        contains_any(
            &lower,
            &["runbook", "procedure", "checklist", "deploy", "deployment"],
        ) || contains_any(request, &["手順", "チェックリスト", "デプロイ"]),
        OpsSection::Checklist.label(),
    );
    push_section_if(
        &mut sections,
        contains_any(&lower, &["validate", "validation", "verify"])
            || contains_any(request, &["確認", "検証"]),
        OpsSection::Validation.label(),
    );
    push_section_if(
        &mut sections,
        contains_any(&lower, &["rollback", "restore", "roll back", "revert"])
            || contains_any(request, &["ロールバック", "切り戻し"]),
        OpsSection::Rollback.label(),
    );
    push_section_if(
        &mut sections,
        contains_any(&lower, &["risk", "impact"]) || contains_any(request, &["リスク", "注意"]),
        OpsSection::Risk.label(),
    );
    if sections.is_empty() {
        sections.push(OpsSection::Checklist.label().to_string());
    }
    sections
}

fn push_section_if(sections: &mut Vec<String>, condition: bool, section: &str) {
    if condition && !sections.iter().any(|existing| existing == section) {
        sections.push(section.to_string());
    }
}

/// Issue #937 (DS3-001): scan-threaded variant called from `from_request`.
pub(super) fn inferred_data_obligations_from_request_with_scan(
    scan: &OutputContextScan,
    request: &str,
) -> Vec<ArtifactObligation> {
    if !request_asks_for_data_output_artifact_with_scan(scan, request) {
        return Vec::new();
    }
    let lower = scan.lower.as_str();
    let explicit_output = explicit_path_with_data_extension_with_scan(scan, request);
    let path = explicit_output.unwrap_or_else(|| {
        if lower.contains("tsv") {
            "output.tsv".to_string()
        } else if lower.contains("jsonl") || lower.contains("ndjson") {
            "output.jsonl".to_string()
        } else {
            "output.csv".to_string()
        }
    });
    let columns = extract_required_columns_from_request(request);
    let expected_rows = extract_expected_rows_from_request(request, &columns);
    vec![ArtifactObligation::structured_record_with_expected_rows(
        path,
        columns,
        expected_rows,
    )]
}

fn extract_required_columns_from_request(request: &str) -> Vec<String> {
    let Some(start) = request.to_ascii_lowercase().find("column") else {
        return Vec::new();
    };
    let tail = &request[start..];
    let window = tail
        .split(['.', '\n', ';'])
        .next()
        .unwrap_or(tail)
        .replace(['`', '"', '\''], " ");
    let mut columns = Vec::new();
    let tokens = window
        .split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_' || ch == '-'))
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .collect::<Vec<_>>();
    for (index, token) in tokens.iter().enumerate() {
        let lower = token.to_ascii_lowercase();
        if structured_row_count_token_before_row_marker(&tokens, index) {
            break;
        }
        if matches!(
            lower.as_str(),
            "column"
                | "columns"
                | "with"
                | "and"
                | "or"
                | "as"
                | "the"
                | "a"
                | "an"
                | "exactly"
                | "only"
        ) {
            continue;
        }
        if matches!(
            lower.as_str(),
            "from" | "for" | "in" | "into" | "row" | "rows" | "record" | "records"
        ) {
            break;
        }
        if columns.iter().any(|existing| existing == token) {
            continue;
        }
        columns.push((*token).to_string());
    }
    columns
}

fn structured_row_count_token_before_row_marker(tokens: &[&str], index: usize) -> bool {
    if !structured_count_token(tokens[index]) {
        return false;
    }
    tokens
        .iter()
        .skip(index + 1)
        .take(3)
        .map(|token| token.to_ascii_lowercase())
        .any(|token| matches!(token.as_str(), "row" | "rows" | "record" | "records"))
}

fn structured_count_token(token: &str) -> bool {
    token.chars().all(|ch| ch.is_ascii_digit())
        || matches!(
            token.to_ascii_lowercase().as_str(),
            "one" | "two" | "three" | "four" | "five" | "six" | "seven" | "eight" | "nine" | "ten"
        )
}

const EXPECTED_STRUCTURED_ROWS_MAX: usize = 8;
const EXPECTED_STRUCTURED_ROW_CELLS_MAX: usize = 12;

fn extract_expected_rows_from_request(request: &str, columns: &[String]) -> Vec<Vec<String>> {
    if columns.is_empty() || columns.len() > EXPECTED_STRUCTURED_ROW_CELLS_MAX {
        return Vec::new();
    }
    let lower = request.to_ascii_lowercase();
    let Some(after_marker) = data_rows_marker_end(&lower) else {
        return Vec::new();
    };
    let tail = &request[after_marker..];
    let window = tail
        .split(['.', '\n'])
        .next()
        .unwrap_or(tail)
        .replace(['`', '"', '\'', '[', ']', '(', ')'], " ")
        .replace(';', "\n");
    let mut rows = Vec::new();
    for segment in window
        .split('\n')
        .flat_map(|part| part.split(" and "))
        .take(EXPECTED_STRUCTURED_ROWS_MAX * 2)
    {
        let cells = segment
            .split(',')
            .map(clean_expected_row_cell)
            .filter(|cell| !cell.is_empty())
            .collect::<Vec<_>>();
        if cells.len() == columns.len() && !rows.iter().any(|existing| existing == &cells) {
            rows.push(cells);
            if rows.len() >= EXPECTED_STRUCTURED_ROWS_MAX {
                break;
            }
        }
    }
    rows
}

fn data_rows_marker_end(lower: &str) -> Option<usize> {
    ["records", "record", "rows", "row"]
        .iter()
        .filter_map(|marker| {
            lower.find(marker).and_then(|index| {
                let before = lower[..index].chars().next_back();
                let after = lower[index + marker.len()..].chars().next();
                (!before.is_some_and(is_ascii_word_char) && !after.is_some_and(is_ascii_word_char))
                    .then_some(index + marker.len())
            })
        })
        .min()
}

fn clean_expected_row_cell(raw: &str) -> String {
    raw.trim()
        .trim_matches(|ch: char| {
            ch.is_whitespace() || matches!(ch, ':' | '=' | '-' | '>' | '[' | ']' | '(' | ')')
        })
        .trim()
        .to_string()
}

pub(super) fn inferred_artifact_obligations_from_project_intent(
    project_intent: &ProjectIntent,
    required_artifacts: &[ArtifactRole],
) -> Vec<ArtifactObligation> {
    if !required_artifacts.contains(&ArtifactRole::Implementation) {
        return Vec::new();
    }
    let shape = project_intent.shape.unwrap_or(ProjectShape::Unknown);
    if !matches!(shape, ProjectShape::Cli | ProjectShape::Library) {
        return Vec::new();
    }
    let mut obligations = Vec::new();
    match project_intent.language.unwrap_or(ProjectLanguage::Unknown) {
        ProjectLanguage::Rust => {
            obligations.push(ArtifactObligation::file(ArtifactRole::Setup, "Cargo.toml"));
            if matches!(shape, ProjectShape::Cli) {
                obligations.push(ArtifactObligation::file(
                    ArtifactRole::Implementation,
                    "src/main.rs",
                ));
                if required_artifacts.contains(&ArtifactRole::Test) {
                    obligations.push(ArtifactObligation::file(ArtifactRole::Test, "tests/cli.rs"));
                }
                if required_artifacts.contains(&ArtifactRole::UsageDocs) {
                    obligations.push(ArtifactObligation::readme(
                        "README.md",
                        default_readme_required_sections(),
                    ));
                }
            } else if matches!(shape, ProjectShape::Library) {
                obligations.push(ArtifactObligation::file(
                    ArtifactRole::Implementation,
                    "src/lib.rs",
                ));
                if required_artifacts.contains(&ArtifactRole::Test) {
                    obligations.push(ArtifactObligation::file(ArtifactRole::Test, "tests/lib.rs"));
                }
                if required_artifacts.contains(&ArtifactRole::UsageDocs) {
                    obligations.push(ArtifactObligation::readme(
                        "README.md",
                        default_readme_required_sections(),
                    ));
                }
            }
        }
        ProjectLanguage::Node => {
            obligations.push(ArtifactObligation::file(
                ArtifactRole::Setup,
                "package.json",
            ));
            if matches!(shape, ProjectShape::Cli) {
                obligations.push(ArtifactObligation::json_field(
                    ArtifactRole::Setup,
                    "package.json",
                    "bin",
                    "package.json declares a bin entry for the CLI",
                ));
                obligations.push(ArtifactObligation::file(
                    ArtifactRole::Implementation,
                    "src/index.js",
                ));
                if required_artifacts.contains(&ArtifactRole::Test) {
                    obligations.push(ArtifactObligation::file(
                        ArtifactRole::Test,
                        "tests/index.test.js",
                    ));
                }
                if required_artifacts.contains(&ArtifactRole::UsageDocs) {
                    obligations.push(ArtifactObligation::readme(
                        "README.md",
                        default_readme_required_sections(),
                    ));
                }
            }
        }
        ProjectLanguage::Python => {
            if matches!(shape, ProjectShape::Cli) {
                obligations.push(ArtifactObligation::file(
                    ArtifactRole::Implementation,
                    "main.py",
                ));
                if required_artifacts.contains(&ArtifactRole::Test) {
                    obligations.push(ArtifactObligation::file(
                        ArtifactRole::Test,
                        "tests/test_main.py",
                    ));
                }
                if required_artifacts.contains(&ArtifactRole::UsageDocs) {
                    obligations.push(ArtifactObligation::readme(
                        "README.md",
                        default_readme_required_sections(),
                    ));
                }
            }
        }
        ProjectLanguage::Docs | ProjectLanguage::Unknown => {}
    }
    obligations
}

pub(super) fn push_or_merge_artifact_obligation(
    obligations: &mut Vec<ArtifactObligation>,
    incoming: ArtifactObligation,
) {
    let Some(existing) = obligations
        .iter_mut()
        .find(|existing| should_merge_artifact_obligations(existing, &incoming))
    else {
        obligations.push(incoming);
        return;
    };
    if existing.kind == DeliverableKind::File && incoming.kind != DeliverableKind::File {
        existing.kind = incoming.kind;
    }
    if existing.required_sections.is_empty() && !incoming.required_sections.is_empty() {
        existing.required_sections = incoming.required_sections;
    }
    if existing.acceptance_criteria.is_empty() && !incoming.acceptance_criteria.is_empty() {
        existing.acceptance_criteria = incoming.acceptance_criteria;
    }
    if existing.structured_record_schema.is_none() {
        existing.structured_record_schema = incoming.structured_record_schema;
    }
    if existing.schema.is_none() {
        existing.schema = incoming.schema;
    }
}

pub(super) fn inferred_obligation_shadowed_by_explicit_identity(
    existing: &[ArtifactObligation],
    incoming: &ArtifactObligation,
) -> bool {
    matches!(
        incoming.role,
        ArtifactRole::Implementation | ArtifactRole::Test
    ) && existing
        .iter()
        .any(|identity| identity.role == incoming.role && identity.path != incoming.path)
}

pub(super) fn profile_obligation_shadowed_by_prior_identity(
    existing: &[ArtifactObligation],
    incoming: &ArtifactObligation,
) -> bool {
    incoming.role == ArtifactRole::DataOutput
        && existing
            .iter()
            .any(|identity| identity.role == incoming.role && identity.path != incoming.path)
}

fn should_merge_artifact_obligations(
    existing: &ArtifactObligation,
    incoming: &ArtifactObligation,
) -> bool {
    if existing.role != incoming.role || existing.path != incoming.path {
        return false;
    }
    if existing.role == ArtifactRole::Setup
        && existing.path == "package.json"
        && (matches!(
            existing.schema.as_ref(),
            Some(DeliverableSchema::JsonFields(_))
        ) || matches!(
            incoming.schema.as_ref(),
            Some(DeliverableSchema::JsonFields(_))
        ))
    {
        return false;
    }
    true
}
