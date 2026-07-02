//! Request-inference primitives for TaskContract construction.
//!
//! This module is an extraction boundary for existing deterministic request
//! parsing. It should not grow new benchmark-specific branches; longer-term
//! semantic interpretation belongs in LLM-produced candidates plus deterministic
//! admission.

use super::contract_request_signals::{
    contains_callable_signature_hint, contains_dotted_callable_change_action,
    contains_implementation_file_hint, negated_artifact_list_contains,
};
use super::task_contract::{
    ArtifactRole, ProjectLanguage, ProjectShape, TaskIntent, VerificationRequirement,
    request_asks_for_setup,
};
use super::task_contract_path_context::{contains_any, contains_ascii_token};

/// Issue #664 (CB-002): English setup-marker substring set. Each marker is
/// matched with a **token boundary** check (`contains_setup_token_ascii`)
/// plus a **negation-prefix guard** (`negation_prefix_within_window`) so
/// negated phrasings ("uninstall dependencies", "do not install", "no setup",
/// "without dependencies", "disable setup", etc.) do NOT trip the
/// `required_artifacts::Setup` Bash job policy.
///
/// Pinned to ASCII lowercase needles only — Japanese markers live in
/// `SETUP_MARKER_NEEDLES_JP` and use a separate negation-suffix guard.
pub(super) const SETUP_MARKER_NEEDLES_ASCII: &[&str] = &[
    "install",
    "dependency",
    "dependencies",
    "requirements",
    "package.json",
    "setup",
];

/// Issue #664 (CB-002): Japanese setup-marker substrings. Matched verbatim
/// (no token-boundary equivalent in JP), but a trailing-suffix negation
/// guard (`否定しない`, `不要`, `無し`, `しない`) prevents false positives.
pub(super) const SETUP_MARKER_NEEDLES_JP: &[&str] = &["依存", "インストール", "セットアップ"];

/// Issue #664 (CB-002): English negation-prefix tokens that, when present
/// in a window before the matched needle, suppress the setup-intent signal.
/// Each entry is lowercase and is checked against the haystack window with
/// `ends_with` after lowercasing.
const SETUP_NEGATION_PREFIXES_ASCII: &[&str] = &[
    "un",       // "uninstall ..."
    "do not ",  // "do not install ..."
    "don't ",   // "don't install ..."
    "no ",      // "no dependencies"
    "without ", // "without dependencies"
    "disable ", // "disable setup"
    "remove ",  // "remove dependencies" (uninstall semantics)
    "skip ",    // "skip setup"
    "avoid ",   // "avoid install"
];

/// Issue #664 (CB-002): pure-fn token-boundary match for ASCII setup markers
/// with English negation-prefix suppression. Returns `true` iff `lower`
/// contains `needle` as a word-bounded token AND the lookback window of
/// up to [`SETUP_NEGATION_LOOKBACK_BYTES`] characters preceding the match
/// neither
///   - **ends with** any multi-character prefix in
///     [`SETUP_NEGATION_PREFIXES_ASCII`] (e.g. `"do not "`,
///     `"don't "`, `"without "`, `"disable "`, …), nor
///   - **contains** any documented negation phrase anywhere in the
///     lookback window — Issue #664 iteration-3 (CB2-002) phrase-span
///     extension: `"do not install dependencies"` would otherwise match
///     the `dependencies` marker because the lookback ends with
///     `"install "` (not `"do not "`). The phrase-span scan catches
///     `"do not "` anywhere in the 24-byte window so any marker carried
///     downstream of a negation in the same phrase is suppressed, nor
///   - **carries** an immediately-preceding token that **starts with**
///     `"un"` (covering `"uninstall"`, `"unset"`, `"undo"`, etc.) — the
///     `un` prefix in the negation list is interpreted as a leading
///     morpheme of the preceding word rather than a free-standing token.
///
/// Issue #664 iteration-3 (CB2-002) suffix-compound extension: a marker
/// followed by `-free` / `less` (e.g. `dependency-free`, `dependencyless`)
/// or any of [`SETUP_NEGATION_SUFFIXES_COMPOUND_ASCII`] is treated as a
/// suffix-form negation and suppressed at the right boundary.
///
/// Pre-condition: `needle` is already lowercase ASCII; `lower` is the
/// caller's pre-computed lowercase form of the request.
pub(super) fn lower_contains_setup_token_unnegated(lower: &str, needle: &str) -> bool {
    lower.match_indices(needle).any(|(idx, _)| {
        // 1. Token boundary on the right (after the needle).
        let after_idx = idx + needle.len();
        let after_rest = &lower[after_idx..];
        let after_ok = after_rest
            .chars()
            .next()
            .is_none_or(|ch| !ch.is_ascii_alphanumeric());
        if !after_ok {
            return false;
        }
        // CB2-002: suffix-compound negation. `dependency-free`,
        // `dependencyless`, `setup-less`, etc. — the marker is followed
        // by a negation morpheme that the original prefix-only guard
        // missed. Check the byte-slice immediately after the needle
        // against the documented suffix set.
        if SETUP_NEGATION_SUFFIXES_COMPOUND_ASCII
            .iter()
            .any(|suffix| after_rest.starts_with(suffix))
        {
            return false;
        }
        // 2. Token boundary on the left + negation-prefix lookback. The
        // window is a small ASCII byte-count anchored to the left edge.
        let lookback_start = idx.saturating_sub(SETUP_NEGATION_LOOKBACK_BYTES);
        // Walk forward to a char boundary; the haystack is `lower`
        // (pre-lowered), so we operate on byte offsets but
        // `is_char_boundary` keeps UTF-8 safety.
        let mut window_start = lookback_start;
        while window_start < idx && !lower.is_char_boundary(window_start) {
            window_start += 1;
        }
        let window = &lower[window_start..idx];
        // Token boundary on the left: the char immediately before `idx`
        // (if any) must NOT be alphanumeric. "uninstall" → "install"
        // starts directly after "un" which IS alphanumeric → fails the
        // token-boundary check here.
        let left_token_ok = window
            .chars()
            .next_back()
            .is_none_or(|ch| !ch.is_ascii_alphanumeric());
        if !left_token_ok {
            return false;
        }
        // 3a. Negation-prefix lookback (immediately-preceding match):
        // if any documented multi-char prefix (e.g. "do not ") occurs
        // at the tail of the window, the signal is negated.
        let multi_char_negated = SETUP_NEGATION_PREFIXES_ASCII
            .iter()
            .filter(|p| p.len() > 2) // skip the bare "un" — handled below
            .any(|prefix| window.ends_with(prefix));
        if multi_char_negated {
            return false;
        }
        // 3b. CB2-002 phrase-span negation: scan the entire lookback
        // window for any documented negation phrase. This catches
        // cases like `"do not install dependencies"` where the
        // `dependencies` marker is at byte offset N and the
        // immediately-preceding token is `install ` (which is not in
        // the negation prefix set), but `"do not "` sits earlier in
        // the same window. By scanning the window with `contains`
        // (token-bounded at both ends of the phrase against
        // whitespace / window start), the marker downstream of any
        // documented negation is suppressed.
        if phrase_span_window_contains_negation(window) {
            return false;
        }
        // 3c. Detect "un"-prefixed preceding token by walking back from
        // `idx` to the nearest non-alphanumeric byte (or window start)
        // and checking the resulting prev-word slice. Whitespace /
        // punctuation breaks the search; embedded numerals are treated
        // as part of the word for symmetry with the boundary check.
        if previous_word_starts_with_un_prefix(window) {
            return false;
        }
        true
    })
}

/// Issue #664 iteration-3 (CB2-002) helper: scan the lookback window
/// for a documented negation phrase appearing anywhere in the window,
/// not just at its tail. Each phrase is matched with a left token
/// boundary (start of window OR preceded by whitespace / punctuation)
/// so substrings inside larger tokens (`"undo "`-inside-some-word) do
/// not falsely suppress positive markers.
///
/// Multi-character phrases (length > 2) are tested via this scan; the
/// bare `"un"` is handled separately by
/// `previous_word_starts_with_un_prefix` because it requires
/// preceding-token semantics, not free-standing whitespace boundary.
fn phrase_span_window_contains_negation(window: &str) -> bool {
    let bytes = window.as_bytes();
    SETUP_NEGATION_PREFIXES_ASCII
        .iter()
        .filter(|p| p.len() > 2)
        .any(|phrase| {
            let phrase: &str = phrase;
            // Find every occurrence and check left token boundary.
            window.match_indices(phrase).any(|(idx, _)| {
                if idx == 0 {
                    return true;
                }
                let prev = bytes[idx - 1];
                // Left boundary: whitespace, punctuation, or any non-
                // alphanumeric ASCII byte. Avoid matching inside a
                // larger alphabetic token (`"random-do not "` would
                // already split on `-`; this guard catches contiguous
                // letters like `"redo not "` accidentally matching).
                !prev.is_ascii_alphanumeric()
            })
        })
}

/// Issue #664 iteration-3 (CB2-002) suffix-compound negation morphemes.
/// Each entry is matched against the byte-slice **immediately after**
/// the setup marker. The morphemes are intentionally minimal and only
/// cover the documented suffix-form patterns (`-free` / `less`); future
/// additions go here and stay covered by
/// `request_asks_for_setup_dependency_free_compound_suffix`.
const SETUP_NEGATION_SUFFIXES_COMPOUND_ASCII: &[&str] = &["-free", "-less", "less"];

/// CB-002 helper: returns `true` iff the last (rightmost) ASCII-token
/// in `window` starts with the negation morpheme `"un"`. Whitespace and
/// non-alphanumeric characters split tokens. Used to suppress markers
/// like `"uninstall dependencies"` where the prior token is `"uninstall"`
/// (treated as a negation of `"install"` and adjacent markers).
fn previous_word_starts_with_un_prefix(window: &str) -> bool {
    // Walk back to find the rightmost token: skip trailing non-alnum
    // separators, then collect contiguous alnum chars.
    let bytes = window.as_bytes();
    let mut end = bytes.len();
    while end > 0 && !bytes[end - 1].is_ascii_alphanumeric() {
        end -= 1;
    }
    if end == 0 {
        return false;
    }
    let mut start = end;
    while start > 0 && bytes[start - 1].is_ascii_alphanumeric() {
        start -= 1;
    }
    let token = &window[start..end];
    token.starts_with("un")
}

/// Issue #664 (CB-002): byte window for ASCII negation-prefix lookback.
/// 24 bytes covers all documented prefixes plus typical preceding
/// whitespace / punctuation; keep it tight to avoid matching distant
/// negations.
const SETUP_NEGATION_LOOKBACK_BYTES: usize = 24;

/// Issue #664 (CB-002): Japanese negation-suffix tokens that, when they
/// appear in a short trailing window after a JP setup marker, suppress
/// the setup-intent signal.
const SETUP_NEGATION_SUFFIXES_JP: &[&str] =
    &["しない", "禁止", "不要", "無し", "なし", "せず", "無効"];

/// Issue #664 (CB-002): characters (bytes) examined after a JP marker.
const SETUP_NEGATION_LOOKAHEAD_BYTES_JP: usize = 32;

/// Issue #664 (CB-002): pure-fn negation-aware check for the JP marker set.
pub(super) fn request_contains_jp_setup_marker_unnegated(request: &str, needle: &str) -> bool {
    request.match_indices(needle).any(|(idx, _)| {
        let after_idx = idx + needle.len();
        let lookahead_end = (after_idx + SETUP_NEGATION_LOOKAHEAD_BYTES_JP).min(request.len());
        let mut window_end = lookahead_end;
        while window_end > after_idx && !request.is_char_boundary(window_end) {
            window_end -= 1;
        }
        let window = &request[after_idx..window_end];
        let negated = SETUP_NEGATION_SUFFIXES_JP
            .iter()
            .any(|suffix| window.contains(suffix));
        !negated
    })
}

pub(super) fn infer_intent(request: &str, lower: &str) -> TaskIntent {
    if request_asks_for_setup(request, lower) && !request_asks_for_code_work(request, lower) {
        return TaskIntent::Install;
    }
    if contains_any(
        lower,
        &[
            "explain",
            "summarize",
            "tell me",
            "analyze",
            "review",
            "説明",
            "要約",
            "教えて",
            "調査",
        ],
    ) && !request_asks_for_code_work(request, lower)
    {
        return TaskIntent::Explain;
    }
    if contains_any(lower, &["fix", "repair", "bug", "修正", "直して"]) {
        return TaskIntent::Fix;
    }
    if contains_any(
        lower,
        &[
            "update", "modify", "edit", "refactor", "変更", "更新", "編集",
        ],
    ) {
        return TaskIntent::Modify;
    }
    TaskIntent::Build
}

pub(super) fn infer_project_language(request: &str, lower: &str) -> ProjectLanguage {
    super::project_profile::infer_language(request, lower)
}

pub(super) fn infer_project_shape(request: &str, lower: &str) -> ProjectShape {
    super::project_profile::infer_shape(request, lower)
}

pub(super) fn infer_verification_requirement(
    request: &str,
    lower: &str,
    language: ProjectLanguage,
    shape: ProjectShape,
) -> VerificationRequirement {
    if matches!(infer_intent(request, lower), TaskIntent::Explain) {
        return VerificationRequirement::NotRequired;
    }
    if request_asks_for_test_artifact(request, lower)
        || contains_any(lower, &["verify", "validate", "check"])
        || contains_any(request, &["検証", "動作確認", "確認"])
    {
        return VerificationRequirement::Required {
            preferred_runner: preferred_runner_for_language(language),
        };
    }
    if matches!(
        shape,
        ProjectShape::Cli | ProjectShape::Library | ProjectShape::Api | ProjectShape::WebApp
    ) {
        return VerificationRequirement::Required {
            preferred_runner: preferred_runner_for_language(language),
        };
    }
    if matches!(shape, ProjectShape::Documentation) || request_asks_for_setup(request, lower) {
        VerificationRequirement::ArtifactOnly
    } else {
        VerificationRequirement::NotRequired
    }
}

pub(super) fn preferred_runner_for_language(language: ProjectLanguage) -> Option<&'static str> {
    match language {
        ProjectLanguage::Rust => Some("cargo test"),
        ProjectLanguage::Node => Some("npm test"),
        ProjectLanguage::Python => Some("pytest"),
        ProjectLanguage::Docs | ProjectLanguage::Unknown => None,
    }
}

pub(super) fn project_intent_confidence(
    intent: TaskIntent,
    language: ProjectLanguage,
    shape: ProjectShape,
    verification: VerificationRequirement,
) -> f32 {
    let mut confidence: f32 = 0.35;
    if !matches!(intent, TaskIntent::Build) {
        confidence += 0.15;
    }
    if !matches!(language, ProjectLanguage::Unknown) {
        confidence += 0.20;
    }
    if !matches!(shape, ProjectShape::Unknown) {
        confidence += 0.20;
    }
    if !matches!(verification, VerificationRequirement::NotRequired) {
        confidence += 0.10;
    }
    confidence.min(1.0)
}

pub(super) fn request_asks_for_data_task(request: &str, lower: &str) -> bool {
    contains_any(
        lower,
        &[
            "csv",
            "jsonl",
            "dataset",
            "spreadsheet",
            "data",
            "etl",
            "transform",
            "clean data",
            "summary.csv",
        ],
    ) || contains_any(request, &["データ", "CSV", "集計", "整形"])
}

pub(super) fn request_has_explicit_coding_subject(request: &str, lower: &str) -> bool {
    contains_any(
        lower,
        &[
            "api",
            "backend",
            "frontend",
            "cli",
            "library",
            "module",
            "script",
            "service",
            "component",
            "rust",
            "python",
            "node",
        ],
    ) || contains_any(
        request,
        &[
            "API",
            "バックエンド",
            "フロントエンド",
            "CLI",
            "ライブラリ",
            "モジュール",
            "スクリプト",
            "Rust",
            "Python",
        ],
    ) || mentions_stack_as_build_target(request, lower)
        || contains_implementation_file_hint(lower)
        || contains_callable_signature_hint(lower)
}

pub(super) fn request_asks_for_research_task(
    request: &str,
    lower: &str,
    intent: TaskIntent,
) -> bool {
    matches!(intent, TaskIntent::Explain)
        || contains_any(
            lower,
            &[
                "research",
                "investigate",
                "compare",
                "summarize",
                "analysis",
                "analyze",
                "report",
            ],
        )
        || contains_any(request, &["調査", "比較", "分析", "レポート"])
}

pub(super) fn request_asks_for_ops_task(request: &str, lower: &str) -> bool {
    contains_any(
        lower,
        &[
            "deploy",
            "deployment",
            "rollback",
            "runbook",
            "incident",
            "monitoring",
            "checklist",
            "release",
            "operation",
        ],
    ) || request_asks_for_command_observation_artifact(request, lower)
        || contains_any(
            request,
            &[
                "デプロイ",
                "ロールバック",
                "運用",
                "監視",
                "リリース",
                "手順",
            ],
        )
}

pub(super) fn request_asks_for_command_observation_artifact(request: &str, lower: &str) -> bool {
    let command_action =
        contains_any(lower, &["run ", "execute "]) || contains_any(request, &["実行", "起動"]);
    let observation_output =
        contains_any(
            lower,
            &[
                "observation",
                "observed",
                "capture",
                "captured",
                "stdout",
                "stderr",
                "exit code",
                "exit status",
                "current directory",
                "file list",
            ],
        ) || contains_any(request, &["観測", "結果", "標準出力", "終了コード"]);
    let artifact_action = contains_any(lower, &["write", "create", "document", "report"])
        || contains_any(request, &["書いて", "作成", "出力", "記録"]);
    command_action && observation_output && artifact_action
}

pub(super) fn mentions_stack_as_build_target(request: &str, lower: &str) -> bool {
    contains_any(
        lower,
        &[
            "with fastapi",
            "using fastapi",
            "fastapi app",
            "fastapi api",
            "with flask",
            "using flask",
            "flask app",
            "with django",
            "using django",
            "django app",
            "rust library",
            "rust crate",
            "rust package",
            "cargo project",
        ],
    ) || contains_any(
        request,
        &["FastAPIで", "Flaskで", "Djangoで", "Pythonで", "Rustで"],
    ) || (request.contains("Rust")
        && contains_any(request, &["ライブラリ", "クレート", "パッケージ"]))
}

/// Returns `true` when the request asks for any code-work signal
/// (production / edit action over a recognizable code subject) **without**
/// regard to support-artifact context.
///
/// This is the canonical input to [`infer_intent`]'s `Install` rule:
///
/// ```text
/// Install ⇔ asks_for_setup && !request_asks_for_code_work
/// ```
///
/// Equivalent to calling
/// [`request_asks_for_implementation_artifact`] with all three support
/// flags forced to `false`. Exposed at `pub(super)` so the behavior
/// schema extractor (`required_behavior::extract_required_artifacts`)
/// can use the *same* rule when deciding whether Setup is required —
/// keeping the two paths in lockstep (CB-004).
pub(super) fn request_asks_for_code_work(request: &str, lower: &str) -> bool {
    request_asks_for_implementation_artifact(request, lower, false, false, false)
}

pub(super) fn request_asks_for_implementation_artifact(
    request: &str,
    lower: &str,
    asks_for_tests: bool,
    asks_for_usage_docs: bool,
    asks_for_setup: bool,
) -> bool {
    // Issue #922 (PR2-001 / remediation): an explicit "do not edit / read-only"
    // instruction negates implementation artifacts too (you cannot create code
    // if no file may change). Reusing the WorkMode-classifier SSOT keeps the two
    // axes consistent (the classifier already routes such requests to
    // AnswerOnly), so a research request like "produce a report … but do not
    // edit any files" classifies as Research, not Coding, and then stays
    // answer-only with no obligation.
    if request_negates_implementation_artifacts(request, lower)
        || crate::modes::plan_act::request_has_explicit_no_edit(request)
        || request_asks_for_command_observation_artifact(request, lower)
    {
        return false;
    }
    let support_artifact_requested = asks_for_tests || asks_for_usage_docs || asks_for_setup;
    let production_action = contains_any(
        lower,
        &[
            "create",
            "build",
            "develop",
            "implement",
            "scaffold",
            "fix",
            "refactor",
        ],
    ) || contains_dotted_callable_change_action(lower)
        || contains_any(request, &["作成", "開発", "実装", "修正", "構築"]);
    let edit_action = production_action
        || contains_any(lower, &["write", "add", "update", "modify", "edit"])
        || contains_any(request, &["追加", "追記", "更新", "変更", "編集"]);
    let code_subject = contains_any(
        lower,
        &[
            "crud",
            "endpoint",
            "server",
            "backend",
            "frontend",
            "web app",
            "browser app",
            "cli",
            "component",
            "service",
            "module",
            "library",
            "crate",
            "package",
            "tool",
            "program",
            "command",
        ],
    ) || contains_ascii_token(lower, "api")
        || contains_any(
            request,
            &[
                "エンドポイント",
                "サーバ",
                "バックエンド",
                "フロントエンド",
                "アプリ",
                "機能",
                "ライブラリ",
                "クレート",
                "パッケージ",
                "ツール",
                "コマンド",
            ],
        )
        || mentions_stack_as_build_target(request, lower)
        || contains_implementation_file_hint(lower)
        || contains_callable_signature_hint(lower);

    if support_artifact_requested {
        production_action && code_subject
    } else {
        edit_action
    }
}

pub(super) fn request_asks_for_test_artifact(request: &str, lower: &str) -> bool {
    if request_negates_test_artifacts(request, lower) {
        return false;
    }
    contains_any(
        lower,
        &[
            "npm test",
            "cargo test",
            "node --test",
            "test code",
            "unit test",
            "unit tests",
            "integration test",
            "integration tests",
            "tests",
            "test file",
            "add test",
            "write test",
            "implement test",
            "create test",
            "pytest",
            "unittest",
            "spec",
        ],
    ) || contains_any(
        request,
        &[
            "テストコード",
            "テストを実装",
            "テストも実装",
            "テストを追加",
            "テストも追加",
            "テストを書く",
            "テストを作成",
            "テスト作成",
        ],
    )
}

pub(super) fn request_negates_test_artifacts(request: &str, lower: &str) -> bool {
    if negated_artifact_list_contains(lower, &["tests", "test files", "test file"]) {
        return true;
    }
    contains_any(
        lower,
        &[
            "do not create code or tests",
            "do not create tests or code",
            "do not create source code or tests",
            "do not create tests or source code",
            "do not create source code, tests",
            "do not write code or tests",
            "do not write source code or tests",
            "do not add code or tests",
            "do not add source code or tests",
            "do not implement code or tests",
            "do not implement source code or tests",
            "do not create tests",
            "do not add tests",
            "do not write tests",
            "do not implement tests",
            "don't create tests",
            "don't add tests",
            "don't write tests",
            "no tests",
            "no test files",
            "without tests",
            "skip tests",
            "avoid tests",
            "tests are not required",
            "tests not required",
            "test files are not required",
        ],
    ) || contains_any(
        request,
        &[
            "テスト不要",
            "テストは不要",
            "テストなし",
            "テスト無し",
            "テストを作成しない",
            "テストは作成しない",
            "テストを追加しない",
            "テストは追加しない",
            "テストを書かない",
            "テストは禁止",
        ],
    )
}

pub(super) fn request_negates_usage_docs_artifacts(request: &str, lower: &str) -> bool {
    if negated_artifact_list_contains(lower, &["docs", "documentation", "manual"]) {
        return true;
    }
    contains_any(
        lower,
        &[
            "no docs",
            "no documentation",
            "without docs",
            "without documentation",
        ],
    ) || contains_any(
        request,
        &[
            "ドキュメント不要",
            "ドキュメントなし",
            "ドキュメント無し",
            "ドキュメントを作成しない",
            "ドキュメントを追加しない",
        ],
    )
}

pub(super) fn request_negates_implementation_artifacts(request: &str, lower: &str) -> bool {
    contains_any(
        lower,
        &[
            "do not create code or tests",
            "do not create tests or code",
            "do not create code",
            "do not create source code",
            "do not add code",
            "do not add source code",
            "do not write code",
            "do not write source code",
            "do not implement code",
            "do not change code",
            "do not modify code",
            "don't create code",
            "don't create source code",
            "don't add code",
            "don't write code",
            "don't write source code",
            "don't implement code",
            "no code",
            "no source code",
            "no code changes",
            "without code",
            "without source code",
            "without code changes",
            "code changes are not required",
            "code changes not required",
        ],
    ) || contains_any(
        request,
        &[
            "コード不要",
            "コードは不要",
            "コードなし",
            "コード無し",
            "コードを作成しない",
            "コードは作成しない",
            "コードを追加しない",
            "コードは追加しない",
            "コードを書かない",
            "コード変更なし",
            "コード変更は不要",
            "実装しない",
        ],
    )
}

pub(super) fn request_forbidden_artifact_roles(request: &str, lower: &str) -> Vec<ArtifactRole> {
    let mut roles = Vec::new();
    push_role_if(
        &mut roles,
        request_negates_implementation_artifacts(request, lower),
        ArtifactRole::Implementation,
    );
    push_role_if(
        &mut roles,
        request_negates_test_artifacts(request, lower),
        ArtifactRole::Test,
    );
    push_role_if(
        &mut roles,
        request_negates_usage_docs_artifacts(request, lower),
        ArtifactRole::UsageDocs,
    );
    roles
}

fn push_role_if(roles: &mut Vec<ArtifactRole>, condition: bool, role: ArtifactRole) {
    if condition && !roles.contains(&role) {
        roles.push(role);
    }
}
