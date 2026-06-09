//! Data-output direction and explicit data artifact path inference.
//!
//! This module keeps DataOutput-specific output-context judgement out of
//! `task_contract.rs` while reusing shared `OutputContextScan` and cue
//! vocabulary. It should not grow task-specific filename branches.

use super::task_contract_path_context::{
    DATA_INPUT_EXTRA, INPUT_VERBS_ASCII, JP_OUTPUT_MARKERS, OUTPUT_AFTER_ASCII, OUTPUT_PREP_ASCII,
    OUTPUT_VERB_STEMS_ASCII, OutputContextScan, bounded_context_after, bounded_context_before,
    contains_any, contains_ascii_token, contains_output_verb, normalize_explicit_artifact_path,
    normalize_explicit_user_artifact_path,
};

pub(super) fn explicit_path_with_data_extension(request: &str) -> Option<String> {
    let scan = OutputContextScan::new(request);
    explicit_path_with_data_extension_with_scan(&scan, request)
}

/// Issue #937 (DS3-001): scan-threaded — the per-path `.find()` reuses the
/// single mask instead of re-masking per candidate.
pub(super) fn explicit_path_with_data_extension_with_scan(
    scan: &OutputContextScan,
    request: &str,
) -> Option<String> {
    explicit_data_paths_from_request(request)
        .into_iter()
        .find(|path| data_path_has_output_context_with_scan(scan, path))
}

pub(super) fn explicit_data_paths_from_request(request: &str) -> Vec<String> {
    let mut paths = Vec::new();
    for token in request.split(|ch: char| {
        !(ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | '/' | '\\'))
    }) {
        let Some(path) = normalize_explicit_user_artifact_path(token) else {
            continue;
        };
        if !path_has_data_extension(&path) {
            continue;
        }
        if !paths.iter().any(|existing| existing == &path) {
            paths.push(path);
        }
    }
    paths
}

pub(super) fn request_mentions_protected_data_artifact_path(request: &str) -> bool {
    request
        .split(|ch: char| {
            !(ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | '/' | '\\'))
        })
        .filter_map(normalize_explicit_artifact_path)
        .any(|path| {
            path_has_data_extension(&path)
                && !crate::util::workspace_paths::WorkspacePolicy::default()
                    .admits_artifact_display_path(&path)
        })
}

fn path_has_data_extension(path: &str) -> bool {
    let Some(ext) = std::path::Path::new(path)
        .extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_ascii_lowercase)
    else {
        return false;
    };
    // Issue #921 (P4 / PR-001): this gate drives DataOutput structured_record
    // obligation inference, so it is restricted to the formats the acceptance
    // SSOT (`verifier::assess_structured_data`) can actually parse-check from a
    // text excerpt. `.parquet` is binary/columnar and unverifiable from an
    // excerpt; admitting it here created a structured-data obligation that would
    // "complete" on any non-empty text with no parse-readiness guarantee. It is
    // still recognized as a data file for ownership/telemetry classification
    // (artifact_ledger / project_probe / completion_evidence), just not as a
    // schema-validated DataOutput obligation.
    matches!(ext.as_str(), "csv" | "json" | "jsonl" | "tsv" | "ndjson")
}

#[cfg(test)]
pub(super) fn data_path_has_output_context(request: &str, path: &str) -> bool {
    let scan = OutputContextScan::new(request);
    data_path_has_output_context_with_scan(&scan, path)
}

/// Issue #937 (判断#3, DS3-001): per-occurrence data output detection over the
/// **masked** request. The `file_output_name` stem look-alike is DEMOTED from an
/// unconditional override to a mere auxiliary signal: output is now decided by
/// (a) a masked before-window ASCII output verb/prep (boundary), or (b) an
/// after-window `JP_OUTPUT_MARKERS` substring, or (c) an `OUTPUT_AFTER_ASCII`
/// noun. So `Summarize the trends in output_data.csv` (input reference) no
/// longer fabricates a DataOutput obligation, while
/// `...output.csvを生成してください` stays an obligation via the JP `生成`
/// after-window marker (#921). The `input.jsonl` input-filename drop is
/// preserved.
pub(super) fn data_path_has_output_context_with_scan(scan: &OutputContextScan, path: &str) -> bool {
    let masked = scan.lower_masked.as_str();
    let lower = scan.lower.as_str();
    let path_lower = path.to_ascii_lowercase();
    let file_input_name = file_data_name_looks_like_input(path);
    // Issue #937 (判断#3): the output-looking stem (`output_data.csv`) is
    // DEMOTED from an unconditional override to an AUXILIARY signal — it no
    // longer makes a bare input reference an output (R3/R4/R6), but it DOES
    // still protect a genuine output path from a downstream `from ... input`
    // phrase being read as its input cue (`Generate report output.csv from the
    // input data`). So it survives ONLY as the input-drop guard, never as a
    // standalone positive.
    let file_output_name_aux = data_path_stem_looks_like_output(path);
    lower.match_indices(&path_lower).any(|(idx, _)| {
        let before = bounded_context_before(masked, idx, 48);
        let after_idx = idx + path_lower.len();
        let after = bounded_context_after(masked, after_idx, 32);
        // Input position (incl. the `input.jsonl` input-filename drop) wins,
        // unless the filename itself is output-looking (auxiliary guard).
        let input_context = file_input_name
            || nearest_governing_data_cue_is_input(before)
            || contains_any(after, &[" as input", " input", " sample", " example"]);
        if input_context && !file_output_name_aux {
            return false;
        }
        // Output position: masked before-window verb/prep (boundary),
        // after-window JP markers (substring), or `OUTPUT_AFTER_ASCII` nouns.
        // The output-looking stem is NOT a standalone positive trigger
        // (判断#3 demotion): a bare input reference whose file merely *looks*
        // like output stays neutral.
        contains_output_verb(before, OUTPUT_VERB_STEMS_ASCII)
            || OUTPUT_PREP_ASCII
                .iter()
                .any(|prep| contains_ascii_token(before, prep))
            || contains_any(after, JP_OUTPUT_MARKERS)
            || contains_any(after, OUTPUT_AFTER_ASCII)
    })
}

/// Data-path direction is governed by the nearest local cue before the path.
/// This keeps `read input.csv and create summary.json` from letting the earlier
/// `read` consume the later JSON output, while preserving read/input references.
fn nearest_governing_data_cue_is_input(before: &str) -> bool {
    for token in before
        .rsplit(|ch: char| !ch.is_ascii_alphanumeric())
        .filter(|tok| !tok.is_empty())
    {
        if contains_output_verb(token, OUTPUT_VERB_STEMS_ASCII)
            || OUTPUT_PREP_ASCII.contains(&token)
        {
            return false;
        }
        if INPUT_VERBS_ASCII.contains(&token) || DATA_INPUT_EXTRA.contains(&token) {
            return true;
        }
    }
    false
}

/// Issue #937 (判断#3): auxiliary "the filename stem looks like an output"
/// signal, used ONLY to protect a genuine output path from a downstream input
/// phrase (never as a standalone output trigger). Mirrors the historical
/// `data_path_file_name_looks_like_output` stems.
fn data_path_stem_looks_like_output(path: &str) -> bool {
    data_path_file_stem(path).is_some_and(|stem| {
        stem.starts_with("output")
            || stem.starts_with("summary")
            || stem.starts_with("result")
            || stem.starts_with("report")
            || stem.starts_with("export")
            || stem.starts_with("cleaned")
    })
}

#[cfg(test)]
pub(super) fn request_explicitly_requests_standalone_data_artifact(
    request: &str,
    lower: &str,
) -> bool {
    let scan = OutputContextScan::new(request);
    debug_assert_eq!(scan.lower, lower, "scan.lower must equal request lowercase");
    request_explicitly_requests_standalone_data_artifact_with_scan(&scan, request)
}

/// Issue #937 (判断#6, DS1-004): `output_action` is the gate that closes R4. It
/// is evaluated over the **masked** request, so a filename `output_data.csv`
/// (`output` substring) can no longer satisfy it; a real output verb still does.
pub(super) fn request_explicitly_requests_standalone_data_artifact_with_scan(
    scan: &OutputContextScan,
    request: &str,
) -> bool {
    let masked = scan.lower_masked.as_str();
    let output_action = contains_output_verb(masked, OUTPUT_VERB_STEMS_ASCII)
        || contains_any(request, &["出力", "生成", "作成", "書き出"]);
    let artifact_noun = contains_any(
        masked,
        &[
            "csv file",
            "tsv file",
            "jsonl file",
            "ndjson file",
            "data file",
            "data artifact",
            "structured output",
            "structured data",
        ],
    ) || contains_any(
        request,
        &[
            "CSVファイル",
            "JSONLファイル",
            "データファイル",
            "構造化データ",
        ],
    );
    output_action && artifact_noun
}

// Issue #937 (判断#3): `data_path_file_name_looks_like_output` was removed — the
// filename stem look-alike is no longer an output trigger (it used to be an
// unconditional override that fabricated false DataOutput obligations from an
// input reference like `Summarize the trends in output_data.csv`). Output is now
// decided by the masked before/after windows + JP markers. The *input* filename
// drop (`input.jsonl`) is retained below as a genuine input signal.
fn file_data_name_looks_like_input(path: &str) -> bool {
    data_path_file_stem(path).is_some_and(|stem| {
        stem.starts_with("input")
            || stem.starts_with("sample")
            || stem.starts_with("example")
            || stem.starts_with("fixture")
            || stem.starts_with("source")
    })
}

fn data_path_file_stem(path: &str) -> Option<String> {
    let file_name = std::path::Path::new(path)
        .file_stem()
        .and_then(|stem| stem.to_str())?;
    Some(file_name.to_ascii_lowercase())
}
