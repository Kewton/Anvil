//! Artifact-intent helpers for TaskContract request interpretation.
//!
//! This module decides whether request text points at an output artifact versus
//! an input/reference artifact. It does not evaluate completion or build repair
//! jobs.

use super::task_contract::{ArtifactRole, TaskIntent};
use super::task_contract_obligation_planning::{
    explicit_artifact_obligations_from_request,
    explicit_artifact_obligations_from_request_with_scan,
};
use super::task_contract_path_context::{
    AUTHORING_KEYWORD_NEEDLES_ASCII, INPUT_REFERENCE_VERBS_ASCII, INPUT_VERBS_ASCII,
    OUTPUT_AFTER_ASCII, OUTPUT_PREP_ASCII, OUTPUT_VERB_STEMS_ASCII, OutputContextScan,
    RESEARCH_OUTPUT_AFTER_JP, bounded_context_after, bounded_context_before, contains_any,
    contains_ascii_token, contains_output_verb, normalize_explicit_user_artifact_path,
};

pub(super) fn default_docs_path_from_request(request: &str) -> String {
    explicit_artifact_obligations_from_request(request)
        .into_iter()
        .find(|identity| identity.role == ArtifactRole::UsageDocs)
        .map(|identity| identity.path)
        .unwrap_or_else(|| "README.md".to_string())
}

/// Issue #922 (P5 / DD4 / PR-003): a research request *intends a written report
/// artifact* (vs. a genuine answer-only Q&A) when EITHER (a) a doc-like path is
/// used as an output target (output verb / preposition directing content to it,
/// not a read-only input reference), or (b) an output verb co-occurs with a
/// report noun. Conservative on purpose: "summarize this for me" and a bare
/// input reference like "summarize notes.txt for me" stay answer-only and are
/// NOT routed into a file-edit obligation (regression guard / S7-001 / PR-003).
#[cfg(test)]
pub(super) fn research_report_artifact_intended(request: &str, lower: &str) -> bool {
    let scan = OutputContextScan::new(request);
    debug_assert_eq!(scan.lower, lower, "scan.lower must equal request lowercase");
    research_report_artifact_intended_with_scan(&scan, request)
}

/// Issue #937 (mode 2, DS3-001): the no-path research entry path scanned over the
/// **filename-stripped** request. A filename-internal substring (`draft` inside
/// `draft_report.md`, `report` inside `output_report.md`) is masked, so the
/// `output_verb && report_noun` co-occurrence can no longer be satisfied by a
/// file NAME. A genuine no-path request (`Research ... and draft a report`,
/// where `draft`/`report` are real words) is unmasked and still fires.
pub(super) fn research_report_artifact_intended_with_scan(
    scan: &OutputContextScan,
    request: &str,
) -> bool {
    // Issue #922 (PR2-001): an explicit "do not edit / read-only" instruction
    // must never be turned into a file-edit report obligation, even if the
    // request also asks for a "report". Fail closed -> stays answer-only, and the
    // WorkMode AnswerOnly->Docs override (which keys off `report_intended_research`)
    // also stays closed.
    if crate::modes::plan_act::request_has_explicit_no_edit(request) {
        return false;
    }
    if research_report_output_path_from_request_with_scan(scan, request).is_some() {
        return true;
    }
    // Issue #937 (judgement #2 only-loosens): evaluate over the masked text so a
    // filename can never supply the verb/noun; the verb scan also gains the
    // plural matcher (`generates`/`creates`) which is strictly more correct.
    let masked = scan.lower_masked.as_str();
    let output_verb = contains_output_verb(masked, OUTPUT_VERB_STEMS_ASCII)
        || contains_any(request, &["作成", "まとめ", "書いて", "出力"]);
    let report_noun = contains_any(masked, &["report", "write-up", "writeup"])
        || contains_any(request, &["レポート", "報告書"]);
    output_verb && report_noun
}

/// Issue #922 (PR-003 / Codex-High): a doc-like path counts as a research
/// *report output target* only in an **output context** — there must be an
/// explicit intent to WRITE content to it. A bare read / comparison / input
/// reference is `false`, EVEN for an output-looking file name (`report.md`,
/// `summary.md`): "Compare report.md and summary.md" and "Review findings.md"
/// are answer-only and must NOT create an obligation. Mirrors
/// `data_path_has_output_context`.
#[cfg(test)]
pub(super) fn report_path_in_output_context(request: &str, path: &str) -> bool {
    let scan = OutputContextScan::new(request);
    report_path_in_output_context_with_scan(&scan, path)
}

/// Issue #937 (mode 1, DS1-001 / DS2-003): per-occurrence **directional** output
/// detection. The immediate prev_word input/output guards keep their fast
/// decisions; the former *global* `has_output_verb` neutral-preposition fallback
/// is replaced by a **masked, bounded (<=48B) before-window** ASCII output
/// verb/prep scan, so a filename anywhere in the request (and any adjacent path)
/// can no longer attribute output intent to a neutral input reference. The
/// after-window carries JP markers + `OUTPUT_AFTER_ASCII` nouns ONLY — never an
/// ASCII output VERB — so `...source_report.md and produce findings.md` cannot
/// leak `produce` backward onto `source_report.md` (R5). Issue #937 CB-001: the
/// JP markers are likewise after-window-only, so the function reads exclusively
/// from `scan` (masked + lower) and no longer needs the raw `request`.
fn report_path_in_output_context_with_scan(scan: &OutputContextScan, path: &str) -> bool {
    let lower = scan.lower.as_str();
    let masked = scan.lower_masked.as_str();
    let path_lower = path.to_ascii_lowercase();
    lower.match_indices(&path_lower).any(|(idx, _)| {
        let prev_word = masked[..idx]
            .rsplit(|ch: char| !ch.is_ascii_alphanumeric())
            .find(|word| !word.is_empty())
            .unwrap_or("");
        let after_idx = idx + path_lower.len();
        let after = bounded_context_after(masked, after_idx, 24);

        if matches!(
            prev_word,
            "input"
                | "source"
                | "from"
                | "read"
                | "reads"
                | "load"
                | "loads"
                | "of"
                | "summarize"
                | "summarise"
                | "analyze"
                | "analyse"
                | "sample"
                | "fixture"
                | "compare"
                | "compares"
                | "review"
                | "reviews"
                | "and"
                | "or"
                | "vs"
                | "versus"
                | "between"
        ) {
            return false;
        }

        if matches!(
            prev_word,
            "to" | "into"
                | "output"
                | "write"
                | "writes"
                | "produce"
                | "produces"
                | "generate"
                | "generates"
                | "export"
                | "exports"
                | "save"
                | "saves"
                | "create"
                | "creates"
                | "emit"
                | "emits"
        ) {
            return true;
        }

        let before = bounded_context_before(masked, idx, 48);
        contains_output_verb(before, OUTPUT_VERB_STEMS_ASCII)
            || OUTPUT_PREP_ASCII
                .iter()
                .any(|prep| contains_ascii_token(before, prep))
            || contains_any(after, RESEARCH_OUTPUT_AFTER_JP)
            || contains_any(after, OUTPUT_AFTER_ASCII)
    })
}

/// Issue #922 (PR-003 / DD3 / DR4-001): the first doc-like path used as an
/// output target, admitted via the SSOT `normalize_explicit_user_artifact_path`.
/// Issue #937 (DS3-001): scan-threaded variant — masks once, reuses for every
/// candidate path so the per-path `.find()` loop never re-masks.
fn research_report_output_path_from_request_with_scan(
    scan: &OutputContextScan,
    request: &str,
) -> Option<String> {
    request
        .split(|ch: char| {
            !(ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | '/' | '\\'))
        })
        .filter_map(normalize_explicit_user_artifact_path)
        .filter(|path| {
            let lower = path.to_ascii_lowercase();
            lower.ends_with(".md")
                || lower.ends_with(".markdown")
                || lower.ends_with(".rst")
                || lower.ends_with(".txt")
        })
        .find(|path| report_path_in_output_context_with_scan(scan, path))
}

/// Issue #922 (P5 / DD3 / DR4-001): the report artifact path for a research
/// obligation — the output-context path if present, else the `report.md`
/// default. Never stores a raw/unadmitted path.
pub(super) fn research_report_path_from_request_with_scan(
    scan: &OutputContextScan,
    request: &str,
) -> String {
    research_report_output_path_from_request_with_scan(scan, request)
        .unwrap_or_else(|| "report.md".to_string())
}

/// Issue #919 / #937: true iff the request names an explicit `UsageDocs` path
/// that is an output target and not a pure input reference.
pub(super) fn request_names_explicit_output_docs(scan: &OutputContextScan, request: &str) -> bool {
    explicit_artifact_obligations_from_request_with_scan(scan, request)
        .iter()
        .filter(|identity| identity.role == ArtifactRole::UsageDocs)
        .any(|identity| !docs_path_is_input_reference_with_scan(scan, &identity.path))
}

fn docs_path_is_input_reference_with_scan(scan: &OutputContextScan, path: &str) -> bool {
    if report_path_in_output_context_with_scan(scan, path) {
        return false;
    }
    let path_lower = path.to_ascii_lowercase();
    scan.lower.match_indices(&path_lower).any(|(idx, _)| {
        let before = bounded_context_before(&scan.lower_masked, idx, 64);
        nearest_governing_cue_is_input_reference(before)
    })
}

#[cfg_attr(test, allow(dead_code))]
pub(super) fn nearest_governing_cue_is_input_reference(before: &str) -> bool {
    for token in before
        .rsplit(|ch: char| !ch.is_ascii_alphanumeric())
        .filter(|tok| !tok.is_empty())
    {
        if contains_output_verb(token, OUTPUT_VERB_STEMS_ASCII)
            || AUTHORING_KEYWORD_NEEDLES_ASCII
                .iter()
                .any(|stem| token.starts_with(stem))
        {
            return false;
        }
        if INPUT_REFERENCE_VERBS_ASCII.contains(&token) || INPUT_VERBS_ASCII.contains(&token) {
            return true;
        }
    }
    false
}

pub(super) fn request_matches_authoring_keyword(scan: &OutputContextScan, request: &str) -> bool {
    contains_any(&scan.lower_masked, AUTHORING_KEYWORD_NEEDLES_ASCII)
        || contains_any(
            request,
            &["翻訳", "書き直", "言い換え", "校正", "清書", "推敲"],
        )
}

pub(super) fn prose_output_shaped(intent: TaskIntent, keyword: bool) -> bool {
    matches!(intent, TaskIntent::Explain) || keyword
}
