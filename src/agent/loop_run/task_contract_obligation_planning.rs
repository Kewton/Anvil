//! Artifact obligation planning helpers for TaskContract construction.
//!
//! This module owns the path from request/profile signals to typed artifact
//! obligations: explicit path projection, inferred data/doc/ops/research
//! obligations, merge/shadow behavior, and project-profile projection.

use super::completion_evidence::classify_repo_edit_path;
use super::contract_request_signals::negated_artifact_list_contains;
use super::task_contract::{
    ArtifactObligation, ArtifactRole, DeliverableKind, DeliverableSchema, ProjectIntent,
    ProjectLanguage, ProjectShape, StructuredColumnPolicy, TaskKind,
    request_asks_for_data_output_artifact_with_scan, request_asks_for_ops_task,
    request_negates_test_artifacts, role_from_repo_edit,
};
use super::task_contract_data_output_context::{
    data_path_has_output_context_with_scan, explicit_path_with_data_extension_with_scan,
};
use super::task_contract_path_context::{
    DOCS_OUTPUT_AFTER_JP, INPUT_VERBS_ASCII, OUTPUT_AFTER_ASCII, OUTPUT_PREP_ASCII,
    OUTPUT_VERB_STEMS_ASCII, OutputContextScan, bounded_context_after, bounded_context_before,
    contains_any, contains_ascii_token, contains_output_verb, is_ascii_word_char,
    normalize_explicit_user_artifact_path,
};

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

pub(super) fn inferred_docs_obligations_from_request(
    request: &str,
    lower: &str,
    required_artifacts: &[ArtifactRole],
) -> Vec<ArtifactObligation> {
    if !required_artifacts.contains(&ArtifactRole::UsageDocs)
        || !lower.contains("readme")
        || !lower.contains("section")
        || negated_artifact_list_contains(lower, &["readme", "readme.md", "docs", "documentation"])
    {
        return Vec::new();
    }
    let sections = required_doc_sections_from_request(request);
    if sections.is_empty() {
        return Vec::new();
    }
    vec![ArtifactObligation::readme("README.md", sections)]
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

/// Issue #923 (P6): the OpsRunbook obligation bridge. Without this, an Ops
/// request produces only a `TaskDeliverable` (no obligation), so the production
/// obligation diagnostic never runs `ops_runbook_pass` and loosening the
/// predicate would be a no-op (Codex DR3-001).
///
/// Only genuine runbook/deploy/operation requests get an obligation. A pure
/// setup/install request reaches `TaskKind::Ops` via `asks_for_setup` (not the
/// ops keywords), so gating on `request_asks_for_ops_task` leaves setup-only
/// completion to the Setup evidence / SetupBootstrap path (DR3-003). Path falls
/// back to a literal `runbook.md` (validated in the ctor, DR4-001).
pub(super) fn inferred_ops_obligations_from_request(
    request: &str,
    lower: &str,
    task_kind: TaskKind,
) -> Vec<ArtifactObligation> {
    if task_kind != TaskKind::Ops {
        return Vec::new();
    }
    let required_sections = required_ops_sections_from_request(request);
    let command_observations = required_command_observations_from_request(request);
    let explicit_docs = explicit_artifact_obligations_from_request(request)
        .into_iter()
        .filter(|identity| identity.role == ArtifactRole::UsageDocs)
        .collect::<Vec<_>>();
    if !command_observations.is_empty() && !explicit_docs.is_empty() {
        return explicit_docs
            .into_iter()
            .map(|identity| {
                ArtifactObligation::command_output(
                    identity.path,
                    required_sections.clone(),
                    command_observations.clone(),
                )
            })
            .collect();
    }
    if request_asks_for_ops_task(request, lower) {
        return vec![ArtifactObligation::ops_runbook(
            default_ops_runbook_path_from_request(request),
            required_sections,
        )];
    }
    explicit_docs
        .into_iter()
        .map(|identity| {
            ArtifactObligation::command_output(
                identity.path,
                required_sections.clone(),
                command_observations.clone(),
            )
        })
        .collect()
}

fn required_command_observations_from_request(request: &str) -> Vec<String> {
    let mut commands = backtick_command_observations_from_request(request);
    if commands.is_empty() {
        commands.extend(run_clause_command_observations_from_request(request));
    }
    commands.sort();
    commands.dedup();
    commands
}

fn backtick_command_observations_from_request(request: &str) -> Vec<String> {
    let mut commands = Vec::new();
    let mut in_backtick = false;
    let mut start = 0usize;
    for (idx, ch) in request.char_indices() {
        if ch != '`' {
            continue;
        }
        if in_backtick {
            if let Some(command) = normalize_required_command_observation(&request[start..idx]) {
                commands.push(command);
            }
            in_backtick = false;
        } else {
            in_backtick = true;
            start = idx + ch.len_utf8();
        }
    }
    commands
}

fn run_clause_command_observations_from_request(request: &str) -> Vec<String> {
    let lower = request.to_ascii_lowercase();
    let Some(run_idx) = lower.find("run ") else {
        return Vec::new();
    };
    let after_start = run_idx + "run ".len();
    let after = &request[after_start..];
    let after_lower = &lower[after_start..];
    let end = [
        ", then",
        " then ",
        " and write ",
        " and create ",
        " and document ",
        " and report ",
        ". ",
        "\n",
    ]
    .into_iter()
    .filter_map(|marker| after_lower.find(marker))
    .min()
    .unwrap_or(after.len());
    after[..end]
        .split([',', ';'])
        .flat_map(|chunk| chunk.split(" and "))
        .filter_map(normalize_required_command_observation)
        .collect()
}

fn normalize_required_command_observation(raw: &str) -> Option<String> {
    let command = raw
        .trim()
        .trim_matches(|ch: char| matches!(ch, '"' | '\'' | '`' | ':'))
        .trim_end_matches('.');
    if command.is_empty()
        || command.len() > 80
        || command.chars().any(|ch| {
            matches!(
                ch,
                '|' | '&' | ';' | '<' | '>' | '$' | '(' | ')' | '\n' | '\r'
            )
        })
    {
        return None;
    }
    let first = command.split_whitespace().next().unwrap_or_default();
    if first.is_empty()
        || !first
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | '/'))
    {
        return None;
    }
    if matches!(
        first.to_ascii_lowercase().as_str(),
        "create" | "write" | "edit" | "add" | "make" | "produce" | "document"
    ) {
        return None;
    }
    Some(command.split_whitespace().collect::<Vec<_>>().join(" "))
}

/// Issue #923 (CB-002 / DR4-001): honor an explicit markdown path the user named
/// (e.g. `deployment-runbook.md`) so the obligation matches the artifact the
/// agent will actually write, instead of always demanding a literal `runbook.md`.
/// Explicit paths come from `explicit_artifact_obligations_from_request` (already
/// `validated_obligation_path`-sanitized); the fallback is the literal
/// `runbook.md`. Mirrors `default_docs_path_from_request`.
pub(super) fn default_ops_runbook_path_from_request(request: &str) -> String {
    explicit_artifact_obligations_from_request(request)
        .into_iter()
        .find(|identity| identity.role == ArtifactRole::UsageDocs)
        .map(|identity| identity.path)
        .unwrap_or_else(|| "runbook.md".to_string())
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
    let column_policy = extract_column_policy_from_request(request);
    vec![ArtifactObligation::structured_record_with_column_policy(
        path,
        columns,
        expected_rows,
        column_policy,
    )]
}

fn extract_column_policy_from_request(request: &str) -> StructuredColumnPolicy {
    let lower = request.to_ascii_lowercase();
    let Some(column_index) = lower.find("column") else {
        return StructuredColumnPolicy::RequiredOnly;
    };
    let start = column_index.saturating_sub(48);
    let end = lower.len().min(column_index + 96);
    let window = &lower[start..end];
    if contains_any(
        window,
        &[
            "exactly the same column",
            "exactly same column",
            "same columns",
            "same column",
            "exactly columns",
            "exactly column",
            "only columns",
            "only column",
            "no extra column",
        ],
    ) {
        StructuredColumnPolicy::Exact
    } else {
        StructuredColumnPolicy::RequiredOnly
    }
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
        if structured_row_context_token_before_row_marker(&tokens, index) {
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

fn structured_row_context_token_before_row_marker(tokens: &[&str], index: usize) -> bool {
    if !matches!(tokens[index].to_ascii_lowercase().as_str(), "same" | "data") {
        return false;
    }
    tokens
        .iter()
        .skip(index + 1)
        .take(4)
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

pub(super) fn explicit_artifact_obligations_from_request(request: &str) -> Vec<ArtifactObligation> {
    let scan = OutputContextScan::new(request);
    explicit_artifact_obligations_from_request_with_scan(&scan, request)
}

/// Issue #937 (DS3-001): scan-threaded variant. The DataOutput identity gate
/// reuses the single mask for `data_path_has_output_context` per path candidate.
pub(super) fn explicit_artifact_obligations_from_request_with_scan(
    scan: &OutputContextScan,
    request: &str,
) -> Vec<ArtifactObligation> {
    let mut obligations = Vec::new();
    for token in request.split(|ch: char| {
        !(ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | '/' | '\\'))
    }) {
        let Some(path) = normalize_explicit_user_artifact_path(token) else {
            continue;
        };
        let category = classify_repo_edit_path(std::path::Path::new(&path));
        let Some(role) = role_from_repo_edit(category) else {
            continue;
        };
        if role == ArtifactRole::DataOutput && !data_path_has_output_context_with_scan(scan, &path)
        {
            continue;
        }
        if !obligations
            .iter()
            .any(|existing: &ArtifactObligation| existing.role == role && existing.path == path)
        {
            obligations.push(ArtifactObligation::file(role, path));
        }
    }
    // Issue #919 (CB2-001): mirror the `DataOutput` source/output discriminator
    // for `UsageDocs` paths so a translation/authoring *source* (input) is not
    // modeled as a required deliverable. Unlike the unconditional `DataOutput`
    // filter, this is scoped so it can never leave an authoring request with
    // zero deliverables: a docs path is dropped from `required` only when it
    // carries a clear *source/input* cue AND a *distinct* docs path that is not
    // itself a source/input (a real output) is also present. In-place authoring
    // (`rewrite docs/intro.md ...`) keeps its single path; true multi-output
    // (`write intro.md and faq.md`) keeps both (neither is a source).
    retain_docs_outputs_when_distinct_source(scan, &mut obligations);
    obligations.sort_by(|a, b| (a.role, a.path.as_str()).cmp(&(b.role, b.path.as_str())));
    obligations
}

/// Issue #919 (CB2-001): remove `UsageDocs` obligations that are clearly a
/// *source/input* of an authoring/translation request, but only when a distinct
/// `UsageDocs` *output* obligation also survives — guaranteeing the request is
/// never left with zero docs deliverables (fail-open to "everything required").
///
/// "Source/input" is keyed on the SAME before/after preposition+verb cues the
/// `DataOutput` discriminator (`data_path_has_output_context`) already uses,
/// extended minimally with translation cues (`translate` / `翻訳`) and a
/// language-stamped filename hint (`README.ja.md`). It deliberately does NOT
/// invent new output heuristics: an output is simply "any docs path that is not
/// classified as a source/input".
fn retain_docs_outputs_when_distinct_source(
    scan: &OutputContextScan,
    obligations: &mut Vec<ArtifactObligation>,
) {
    let docs_sources: Vec<String> = obligations
        .iter()
        .filter(|o| o.role == ArtifactRole::UsageDocs)
        .filter(|o| docs_path_is_clearly_source_input_with_scan(scan, &o.path))
        .map(|o| o.path.clone())
        .collect();
    if docs_sources.is_empty() {
        return;
    }
    // A distinct output exists iff some UsageDocs obligation is NOT a source.
    let has_distinct_output = obligations
        .iter()
        .any(|o| o.role == ArtifactRole::UsageDocs && !docs_sources.contains(&o.path));
    if !has_distinct_output {
        return;
    }
    obligations.retain(|o| !(o.role == ArtifactRole::UsageDocs && docs_sources.contains(&o.path)));
}

/// Issue #919 (CB2-001): true iff `path` (a recognized docs path) is referenced
/// in `request` with a clear source/input cue and never with an output cue —
/// mirroring the `input_context && !output` branch of
/// [`data_path_has_output_context`], extended for translation/authoring.
#[cfg(test)]
pub(super) fn docs_path_is_clearly_source_input(request: &str, path: &str) -> bool {
    let scan = OutputContextScan::new(request);
    docs_path_is_clearly_source_input_with_scan(&scan, path)
}

/// Issue #937 (判断#5, DS3-001): the docs/authoring source discriminator, hardened
/// for masking + word-boundary + a JP output marker. Polarity is PRESERVED:
/// `true` = DROP as source, requiring `saw_occurrence && every(source) &&
/// !any(output)`. The before/after windows run on the **masked** text so an
/// adjacent path cannot supply a false cue; ASCII verbs are word-boundary
/// matched; the new `DOCS_OUTPUT_AFTER_JP` after-window (`に書いて`/`に出力`/
/// `として保存`) lets `...README.mdに書いてください` count `README.md` as an
/// OUTPUT (so it is kept, not pruned as source) — required to keep the polarity
/// correct (N8).
fn docs_path_is_clearly_source_input_with_scan(scan: &OutputContextScan, path: &str) -> bool {
    let lower = scan.lower.as_str();
    let masked = scan.lower_masked.as_str();
    let path_lower = path.to_ascii_lowercase();
    let filename_source = docs_path_file_name_looks_like_source(path);
    let mut saw_occurrence = false;
    let mut every_occurrence_is_source = true;
    for (idx, _) in lower.match_indices(&path_lower) {
        saw_occurrence = true;
        let before = bounded_context_before(masked, idx, 48);
        let after_idx = idx + path_lower.len();
        let after = bounded_context_after(masked, after_idx, 32);
        // Output cues take precedence: a written/saved/into position makes this a
        // deliverable, not a source. ASCII output verbs/preps (boundary) +
        // `OUTPUT_AFTER_ASCII` nouns + JP output markers (judgement #5).
        let output_context = contains_output_verb(before, OUTPUT_VERB_STEMS_ASCII)
            || OUTPUT_PREP_ASCII
                .iter()
                .any(|prep| contains_ascii_token(before, prep))
            || contains_any(after, OUTPUT_AFTER_ASCII)
            || contains_any(after, DOCS_OUTPUT_AFTER_JP);
        let input_context = INPUT_VERBS_ASCII
            .iter()
            .any(|cue| contains_ascii_token(before, cue))
            || contains_ascii_token(before, "original")
            || contains_ascii_token(before, "translate")
            || contains_ascii_token(before, "translates")
            || contains_ascii_token(before, "translating")
            || contains_any(before, &["translation of", "翻訳", "英訳"])
            || contains_any(
                after,
                &[" as input", " input", " 翻訳", " を英訳", " を翻訳"],
            );
        let occurrence_is_source = (filename_source || input_context) && !output_context;
        if !occurrence_is_source {
            every_occurrence_is_source = false;
        }
    }
    saw_occurrence && every_occurrence_is_source
}

/// Issue #919 (CB2-001): a docs filename that itself signals a translation
/// *source* via a language stamp (`README.ja.md`, `intro.fr.mdx`) — i.e. a
/// non-English language tag immediately before the extension. The English tag
/// (`.en.`) is treated as a likely *output* (translation target), so it is not
/// a source hint.
fn docs_path_file_name_looks_like_source(path: &str) -> bool {
    let Some(stem) = std::path::Path::new(path)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .map(str::to_ascii_lowercase)
    else {
        return false;
    };
    // The file_stem of `README.ja.md` is `README.ja`; its inner extension is the
    // language tag.
    let Some(lang) = std::path::Path::new(&stem)
        .extension()
        .and_then(|ext| ext.to_str())
    else {
        return false;
    };
    matches!(
        lang,
        "ja" | "fr" | "de" | "es" | "it" | "pt" | "zh" | "ko" | "ru" | "nl"
    )
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

#[cfg(test)]
mod tests {
    use super::super::task_contract::TaskContract;
    use super::*;

    #[test]
    fn docs_source_discriminator_en_n7() {
        let req = "Translate README.ja.md and write README.md";
        assert!(
            docs_path_is_clearly_source_input(req, "README.ja.md"),
            "language-stamped translation source must be a clear source"
        );
        assert!(
            !docs_path_is_clearly_source_input(req, "README.md"),
            "the written output README.md must not be classified as source"
        );
    }

    #[test]
    fn docs_source_discriminator_jp_n8() {
        let req = "README.ja.mdを翻訳してREADME.mdに書いてください";
        assert!(
            docs_path_is_clearly_source_input(req, "README.ja.md"),
            "JP translation source must be a clear source"
        );
        assert!(
            !docs_path_is_clearly_source_input(req, "README.md"),
            "the `に書いて` output target README.md must not be classified as source"
        );
        let contract = TaskContract::from_request(req);
        let docs: Vec<&str> = contract
            .required_artifact_identities
            .iter()
            .filter(|o| o.role == ArtifactRole::UsageDocs)
            .map(|o| o.path.as_str())
            .collect();
        assert_eq!(
            docs,
            vec!["README.md"],
            "JP translation must prune the source and keep only README.md"
        );
    }
}
