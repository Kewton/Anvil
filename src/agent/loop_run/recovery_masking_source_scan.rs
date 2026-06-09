// Issue #931 (Phase E / AC3): structural source-scan guard.
//
// This file is `include!`d from `recovery_masking_tests.rs` (so it shares that
// module's `#[cfg(test)]` scope). It is a pragmatic, line/statement-level
// tokenizer — NOT a full Rust parser. Its job is to FAIL when a new
// recovery/repair renderer routes a raw `hint.path` / `hint.reason` / `.reason`
// / `progress_path_display(...)` value into a model-facing prompt sink without a
// render-point mask. Heuristic limits are documented inline.
//
// Three layers:
//   1. function-scoped allowlist + taint pass — the known renderers (and their
//      mask-call expectation) are pinned by name; each fn body is brace-matched
//      out of its file and run through an intra-function taint pass.
//   2. broad smoke scan — every `src/agent/loop_run/*.rs` function that touches a
//      `RecoveryTargetHint` field / `progress_path_display` AND a prompt sink must
//      either be in the allowlist, contain a mask call, or carry a
//      `// #931-mask-checked: <reason>` standalone-line annotation.
//   3. self-verify — inline fixtures prove the taint pass FAILS on an unmasked
//      sample and PASSES on a masked sample (so the scanner cannot silently
//      no-op).

// Calls that sanitize a value at the render point. `recovery::` is a sanitize
// boundary (Issue #931 PR-review High-2): every path-taking `recovery.rs` builder
// (Choke A) masks its argument internally via `mask_recovery_path`, so a value
// passed into a `recovery::<builder>(...)` call is masked by construction. This
// keeps the double-protected callers (reply_retry / recovery_targets /
// forced_small_edit / scaffold_pipeline, which hand `progress_path_display`
// output to a `recovery::` builder) from being false positives.
const MASK_CALLS: &[&str] = &[
    "mask_and_cap_recovery_field",
    "mask_recovery_path",
    "mask_secrets",
    "recovery::",
];

// Raw, LLM/request-derived values that must be masked before reaching a prompt
// sink. `policy_target_path(` (Issue #931 CB-002): the artifact-directed Bash-
// rejection branch + `restricted_tool_policy_error` derive a model-facing target
// path from it. Bare `.reason` is intentionally NOT a source: it over-matches any
// `.reason` field in the now repo-wide broad smoke (Issue #931 PR-review High-1);
// every recovery reason value is a `hint.reason` / `target_hint.reason`, both
// caught by the `hint.reason` token, so coverage is unchanged for recovery code.
// Direct `policy.target` reaches prompts via `progress_path_display(` (also here).
const RAW_TAINT_SOURCES: &[&str] = &[
    "hint.path",
    "hint.reason",
    "progress_path_display(",
    "policy_target_path(",
];

const PROMPT_SINKS: &[&str] = &[
    "format!",
    "json!",
    "push_str",
    "push_system_note",
    "ConversationMessage",
];

// Sinks that put text in front of the MODEL (the request body), as opposed to a
// `log_llm_event(json!{...})` logging payload — which IS masked by the
// `mask_payload_inplace` last line of defense and so is NOT a wire leak. The
// repo-wide broad smoke (Issue #931 PR-review High-1) qualifies a function only
// when it reaches one of these, so logging `json!`/`format!` builders are not
// false positives.
const MODEL_SINKS: &[&str] = &["ConversationMessage", "push_system_note"];

const MASK_ANNOTATION: &str = "// #931-mask-checked:";

/// A renderer the structural guard pins by (file, fn). Every entry must still
/// exist (rename rot detection) and, when `expect_mask` is true, its body must
/// contain at least one render-point mask call.
struct AllowlistedRenderer {
    file: &'static str,
    fn_name: &'static str,
    expect_mask: bool,
}

const ALLOWLIST: &[AllowlistedRenderer] = &[
    // Choke B — recovery_messages.rs pure helpers (mask internally).
    rndr("recovery_messages.rs", "focused_edit_no_tool_note_for_target_body"),
    rndr("recovery_messages.rs", "focused_edit_no_tool_note_for_policy_arm"),
    rndr("recovery_messages.rs", "artifact_directed_recovery_message_body"),
    rndr("recovery_messages.rs", "artifact_directed_tool_policy_packet_body"),
    rndr("recovery_messages.rs", "verifier_repair_request_patch_message_body"),
    rndr("recovery_messages.rs", "deterministic_ui_recovery_continuation_note_body"),
    // Choke B — focused_edit_recovery.rs already-pure helpers (mask internally).
    rndr("focused_edit_recovery.rs", "focused_edit_guidance_note"),
    rndr("focused_edit_recovery.rs", "focused_edit_guidance_note_for_policy"),
    rndr("focused_edit_recovery.rs", "focused_edit_compact_anchor_note"),
    // Choke B — verifier_orchestration.rs note builders (mask internally).
    rndr("verifier_orchestration.rs", "task_contract_verifier_repair_note"),
    rndr("verifier_orchestration.rs", "verifier_repair_diagnostic_pending_note"),
    rndr("verifier_orchestration.rs", "task_contract_verifier_targeted_edit_required_note"),
    // Choke C — tool_policy.rs error producers (mask path token internally).
    rndr("tool_policy.rs", "focused_edit_tool_policy_error"),
    rndr("tool_policy.rs", "artifact_directed_tool_policy_error"),
    rndr("tool_policy.rs", "restricted_tool_policy_error"),
    // Choke C — the dispatcher's artifact-directed Bash-rejection branch renders a
    // `policy_target_path`-derived target (Issue #931 CB-002).
    rndr("tool_policy.rs", "effective_tool_policy_error_for_call_with_scope"),
    // Choke D — verifier diagnostic / repair json! wire payload builders.
    rndr("verifier_diagnostic_payload.rs", "safe_file_excerpts_payload"),
    rndr("verifier_diagnostic_payload.rs", "framework_findings_payload"),
    rndr("verifier_diagnostic_payload.rs", "failure_location_payload"),
    rndr("verifier_diagnostic_payload.rs", "changed_candidates_payload"),
    rndr("verifier_diagnostic_payload.rs", "push_candidate_if_absent"),
    rndr(
        "verifier_diagnostic_payload.rs",
        "exhausted_repair_targets_payload",
    ),
    rndr("verifier_orchestration.rs", "verifier_repair_pass_messages"),
    rndr("verifier_orchestration.rs", "verifier_repair_repeated_failure_invariant"),
];

const fn rndr(file: &'static str, fn_name: &'static str) -> AllowlistedRenderer {
    AllowlistedRenderer {
        file,
        fn_name,
        expect_mask: true,
    }
}

/// Source of each scanned file (compiled in via `include_str!`, so the scan runs
/// against the SAME bytes the compiler saw — no filesystem races / path issues).
fn loop_run_sources() -> Vec<(&'static str, &'static str)> {
    vec![
        ("recovery_messages.rs", include_str!("recovery_messages.rs")),
        ("focused_edit_recovery.rs", include_str!("focused_edit_recovery.rs")),
        ("verifier_orchestration.rs", include_str!("verifier_orchestration.rs")),
        (
            "verifier_diagnostic_payload.rs",
            include_str!("verifier_diagnostic_payload.rs"),
        ),
        ("tool_policy.rs", include_str!("tool_policy.rs")),
        ("build_request_messages.rs", include_str!("build_request_messages.rs")),
    ]
}

/// Extract the body (between the first `{` after the signature and its matching
/// `}`) of `fn <fn_name>(` from `source`. Returns `None` if not found.
///
/// Heuristic limit: this is a brace counter that ignores braces inside string /
/// char literals and line/block comments. It does NOT handle raw-string `r#"…"#`
/// delimiters with embedded unbalanced braces inside `#"` payloads beyond simple
/// string skipping — acceptable for these renderer bodies, which do not contain
/// such pathological literals.
fn extract_fn_body(source: &str, fn_name: &str) -> Option<String> {
    let needle = format!("fn {fn_name}(");
    let start = source.find(&needle)?;
    let after = &source[start..];
    // Find the first `{` that opens the body (skip the signature, incl. return
    // type / where-clause). We accept the first unescaped `{` after the matching
    // `)` of the signature.
    // advance to end of signature: balance the `(` … `)` first.
    let mut paren_depth = 0usize;
    let mut sig_closed = false;
    let mut body_open_idx = None;
    for (idx, ch) in after.char_indices() {
        match ch {
            '(' => paren_depth += 1,
            ')' => {
                paren_depth = paren_depth.saturating_sub(1);
                if paren_depth == 0 {
                    sig_closed = true;
                }
            }
            '{' if sig_closed => {
                body_open_idx = Some(idx);
                break;
            }
            _ => {}
        }
    }
    let body_open = body_open_idx?;
    let body_slice = &after[body_open..];
    let mut depth = 0i32;
    let mut in_str = false;
    let mut in_char = false;
    let mut in_line_comment = false;
    let mut in_block_comment = false;
    let bytes: Vec<char> = body_slice.chars().collect();
    let mut end = None;
    let mut i = 0;
    // `escaped` is true when the current char is preceded by an ODD run of
    // backslashes inside a string / char literal (so `'\\'` and `"a\"b"` close
    // correctly). Counting the run avoids the naive `prev != '\\'` bug.
    while i < bytes.len() {
        let ch = bytes[i];
        let next = bytes.get(i + 1).copied().unwrap_or('\0');
        if in_line_comment {
            if ch == '\n' {
                in_line_comment = false;
            }
        } else if in_block_comment {
            if ch == '*' && next == '/' {
                in_block_comment = false;
                i += 1;
            }
        } else if in_str || in_char {
            if ch == '\\' {
                // Skip the escaped char (could be `\\`, `\"`, `\'`, `\n`, ...).
                i += 2;
                continue;
            }
            if in_str && ch == '"' {
                in_str = false;
            } else if in_char && ch == '\'' {
                in_char = false;
            }
        } else {
            match ch {
                '/' if next == '/' => {
                    in_line_comment = true;
                    i += 1;
                }
                '/' if next == '*' => {
                    in_block_comment = true;
                    i += 1;
                }
                '"' => in_str = true,
                '\'' => {
                    // Distinguish a char literal (`'x'`, `'\n'`, `'\\'`) from a
                    // lifetime (`'a`, `'static`). A char literal has a closing
                    // quote within two/three chars; a lifetime does not.
                    let two = bytes.get(i + 2).copied().unwrap_or('\0');
                    let is_escape_char = next == '\\';
                    let is_plain_char = two == '\'';
                    if is_escape_char || is_plain_char {
                        in_char = true;
                    }
                    // else: lifetime — ignore.
                }
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(i + 1);
                        break;
                    }
                }
                _ => {}
            }
        }
        i += 1;
    }
    let end = end?;
    Some(bytes[..end].iter().collect())
}

fn body_contains_mask_call(body: &str) -> bool {
    MASK_CALLS.iter().any(|m| body.contains(m))
}

/// Intra-function taint pass. Tracks `let <name> = <init>;` locals whose init
/// mentions a raw taint source WITHOUT a mask call; flags a leak if a tainted
/// local OR a raw field expression reaches a prompt sink on a line lacking a
/// mask call / annotation.
///
/// Returns `Some(reason)` describing the first leak, or `None` if clean.
///
/// Heuristic limits (documented intentionally):
/// - Statement granularity is line-based; a `let` whose init spans multiple
///   lines is approximated by joining until the line ending in `;` (best effort).
/// - Cross-function indirection (a helper returning a raw value) is OUT of scope;
///   that residual gap is covered by the enumerated mask tests + convention doc.
fn taint_scan(body: &str) -> Option<String> {
    // Collapse multi-line `let ... ;` statements so an init split over lines is
    // examined as one unit (best effort).
    let logical_lines = logical_statements(body);
    let mut tainted_locals: Vec<String> = Vec::new();
    for line in &logical_lines {
        let trimmed = line.trim();
        if trimmed.starts_with(MASK_ANNOTATION) || line.contains(MASK_ANNOTATION) {
            // An explicit standalone annotation whitelists this statement.
            continue;
        }
        let line_has_mask = body_contains_mask_call(line);

        // Detect a `let <name> = <init>` binding.
        if let Some(name) = parse_let_binding_name(trimmed) {
            // The local is raw-tainted iff its init references a raw source in a
            // VALUE position and does NOT itself apply a mask. A
            // `let x = mask_*(hint.path)` is sanitized (mask). A raw source used
            // only as a comparison predicate (`.filter(|h| h.path == target)`) is
            // NOT a value taint — that occurrence does not flow into `x`'s value.
            let code = strip_string_literals_keep_interpolations(line);
            if init_has_value_taint(&code) && !line_has_mask {
                // The binding's initializer may span multiple logical lines (e.g.
                // `let x = policy_target_path(p).map(|t| { mask(t) }).unwrap();`)
                // where the render-point mask lands on a later line than this
                // chunk. Before tainting, check the FULL paren/brace-balanced
                // binding text for a mask call so a multi-line masked construction
                // is not a false positive. (The line-based splitter alone would
                // miss it.) This does NOT relax the per-field `json!` sink check,
                // which flows through the non-let sink branch below.
                let masked_in_full_binding =
                    let_binding_text(body, &name).is_some_and(body_contains_mask_call);
                if !masked_in_full_binding {
                    tainted_locals.push(name);
                }
            }
            continue;
        }

        // Non-let line. Does it reach a prompt sink?
        let reaches_sink = PROMPT_SINKS.iter().any(|s| line.contains(s));
        if !reaches_sink {
            continue;
        }
        if line_has_mask {
            // Masked at the sink line — safe.
            continue;
        }
        // Strip string-literal contents before identifier matching: a word inside
        // a prompt's natural-language text (e.g. "Repair target:") is data, not a
        // reference to a tainted local. We keep `{ident}` interpolation markers by
        // preserving the brace-delimited names as bare identifiers.
        let code = strip_string_literals_keep_interpolations(line);
        let code_has_raw_source = RAW_TAINT_SOURCES.iter().any(|s| code.contains(s));
        // A raw field expression flowing straight into a sink (outside strings).
        if code_has_raw_source {
            return Some(format!("raw taint source reaches prompt sink: {}", trimmed));
        }
        // A previously-tainted local referenced at this sink (outside strings, but
        // including `{ident}` interpolations which we surfaced as bare names).
        for local in &tainted_locals {
            if mentions_identifier(&code, local) {
                return Some(format!(
                    "raw-tainted local `{local}` reaches prompt sink: {}",
                    trimmed
                ));
            }
        }
    }
    // Multi-line sink-macro span pass (Issue #931 PR-review High-2): the line-based
    // pass above only flags a raw source when it shares a logical line with the
    // sink token, so a `json!({ ... })` / multi-line `format!(...)` whose raw
    // `hint.path` / `hint.reason` field lands on its OWN line (the field/arg line
    // does NOT contain `json!` / `format!`) is missed. Scan each paren-balanced
    // `json!` / `format!` invocation span and require every physical line bearing
    // a raw source to carry a render-point mask call (or annotation) on that line.
    macro_span_leaks(body)
}

/// Scan each paren-balanced `json!(...)` / `format!(...)` invocation in `body`. A
/// physical line inside a span that references a raw source (outside string prose)
/// must apply a render-point mask on that same line, else it is a leak. Per-line
/// granularity matches the one-field-per-line `json!` and one-arg-per-line
/// `format!` shapes the recovery/repair renderers use. Returns the first leak.
fn macro_span_leaks(body: &str) -> Option<String> {
    let body_lines: Vec<&str> = body.lines().collect();
    for macro_token in ["json!", "format!"] {
        let mut from = 0;
        while let Some(rel) = body[from..].find(macro_token) {
            let tok_start = from + rel;
            from = tok_start + macro_token.len();
            // Skip a `json!`/`format!` that is an argument to a LOGGING call
            // (`log_llm_event(...)` / `tracing::...`): those payloads are masked by
            // the `mask_payload_inplace` last line of defense and never reach the
            // model wire, so a raw `target_hint.path` there is not a leak (Issue
            // #931 PR-review High-1). A model-facing `let payload = json!({...})`
            // that is later embedded into a `ConversationMessage` has no logging
            // call in its statement prefix and is still checked.
            if macro_in_logging_context(body, tok_start) {
                continue;
            }
            // The span starts at the `(` (or `{` for `json!{...}`) that opens the
            // macro invocation; extract its delimiter-balanced text.
            let Some(span) = balanced_call_span(&body[tok_start..]) else {
                continue;
            };
            // Annotation whitelisting (DR3-002, standalone preceding line). The
            // whole invocation is whitelisted if its own line or the immediately
            // preceding physical line carries the annotation.
            let macro_line_idx = body[..tok_start].matches('\n').count();
            let macro_line = body_lines.get(macro_line_idx).copied().unwrap_or("");
            let line_before_macro = macro_line_idx
                .checked_sub(1)
                .and_then(|i| body_lines.get(i).copied())
                .unwrap_or("");
            if macro_line.contains(MASK_ANNOTATION)
                || line_before_macro.trim().starts_with(MASK_ANNOTATION)
            {
                continue;
            }
            let span_lines: Vec<&str> = span.lines().collect();
            for (k, raw_line) in span_lines.iter().enumerate() {
                if raw_line.contains(MASK_ANNOTATION) {
                    continue;
                }
                let code = strip_string_literals_keep_interpolations(raw_line);
                if !RAW_TAINT_SOURCES.iter().any(|s| code.contains(s)) {
                    continue;
                }
                if body_contains_mask_call(raw_line) {
                    continue;
                }
                // A standalone annotation on the line preceding this field/arg
                // (within the span, or the macro line for the first span line).
                let prev = if k > 0 {
                    span_lines[k - 1]
                } else {
                    macro_line
                };
                if prev.trim().starts_with(MASK_ANNOTATION) {
                    continue;
                }
                return Some(format!(
                    "raw taint source reaches `{macro_token}` sink (multi-line span): {}",
                    raw_line.trim()
                ));
            }
        }
    }
    None
}

/// Calls whose `json!`/`format!` argument is a LOGGING payload (masked by
/// `mask_payload_inplace`), not a model-facing prompt.
const LOGGING_CALLS: &[&str] = &["log_llm_event", "tracing::"];

/// Whether the `json!`/`format!` at `tok_start` is an argument to a logging call.
/// Heuristic: scan the current statement (back to the previous `;` / `{` / `}`)
/// up to the macro token for a `LOGGING_CALLS` name. A model-facing
/// `let payload = serde_json::json!({...})` has no such name in its statement
/// prefix and is therefore still scanned.
fn macro_in_logging_context(body: &str, tok_start: usize) -> bool {
    let stmt_start = body[..tok_start]
        .rfind([';', '{', '}'])
        .map(|i| i + 1)
        .unwrap_or(0);
    let prefix = &body[stmt_start..tok_start];
    LOGGING_CALLS.iter().any(|c| prefix.contains(c))
}

/// Given source starting at a macro token (e.g. `json!({...})`), return the text
/// from the opening delimiter (`(` or `{`) through its matched close, ignoring
/// braces/parens inside string/char literals and comments. `None` if not found.
fn balanced_call_span(src: &str) -> Option<String> {
    let chars: Vec<char> = src.chars().collect();
    // Find the first `(` or `{` that opens the invocation.
    let mut open_idx = None;
    for (i, &ch) in chars.iter().enumerate() {
        if ch == '(' || ch == '{' {
            open_idx = Some(i);
            break;
        }
        // A non-delimiter, non-whitespace, non-`!` char before the opener means
        // this `format!`/`json!` was a substring of an identifier — bail.
        if !ch.is_whitespace() && ch != '!' && !ch.is_alphanumeric() && ch != '_' {
            return None;
        }
    }
    let open = open_idx?;
    let opener = chars[open];
    let closer = if opener == '(' { ')' } else { '}' };
    let mut depth = 0i32;
    let mut in_str = false;
    let mut in_char = false;
    let mut i = open;
    let mut byte_idx: usize = src.char_indices().nth(open).map(|(b, _)| b).unwrap_or(0);
    while i < chars.len() {
        let ch = chars[i];
        let next = chars.get(i + 1).copied().unwrap_or('\0');
        if in_str || in_char {
            if ch == '\\' {
                byte_idx += ch.len_utf8() + next.len_utf8();
                i += 2;
                continue;
            }
            if in_str && ch == '"' {
                in_str = false;
            } else if in_char && ch == '\'' {
                in_char = false;
            }
        } else {
            match ch {
                '"' => in_str = true,
                '\'' => {
                    let two = chars.get(i + 2).copied().unwrap_or('\0');
                    if next == '\\' || two == '\'' {
                        in_char = true;
                    }
                }
                c if c == opener => depth += 1,
                c if c == closer => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(src[..byte_idx + ch.len_utf8()].to_string());
                    }
                }
                _ => {}
            }
        }
        byte_idx += ch.len_utf8();
        i += 1;
    }
    None
}

/// Whether `code` (string literals already stripped) uses a raw taint source in
/// a VALUE position — i.e. an occurrence that is NOT part of an equality
/// comparison predicate (`hint.path == x` / `x != hint.reason`).
///
/// This is what separates a genuine alias (`let raw = hint.path.clone();`) from
/// an incidental predicate mention (`.filter(|h| h.path == target)`), which does
/// not contribute to the bound value.
fn init_has_value_taint(code: &str) -> bool {
    for src in RAW_TAINT_SOURCES {
        let mut from = 0;
        while let Some(pos) = code[from..].find(src) {
            let abs = from + pos;
            let after = &code[abs + src.len()..];
            let before = &code[..abs];
            let after_trim = after.trim_start();
            let before_trim = before.trim_end();
            let is_comparison = after_trim.starts_with("==")
                || after_trim.starts_with("!=")
                || before_trim.ends_with("==")
                || before_trim.ends_with("!=");
            if !is_comparison {
                return true;
            }
            from = abs + src.len();
        }
    }
    false
}

/// Return the full source text of the `let [mut] <name> = ... ;` binding within
/// `body`, using paren/brace-depth matching (ignoring string/char literals) to
/// find the terminating `;` at depth 0. Used so a multi-line masked initializer
/// (e.g. a `.map(|t| { mask(t) })` chain) is recognized as masked even though the
/// `let` chunk and the mask call land on different logical lines. `None` if not
/// found. Heuristic: matches the first `let <name>`; sufficient for the single-
/// binding renderer bodies the guard scans.
fn let_binding_text<'a>(body: &'a str, name: &str) -> Option<&'a str> {
    let start = body
        .find(&format!("let {name}"))
        .or_else(|| body.find(&format!("let mut {name}")))?;
    let rest = &body[start..];
    let chars: Vec<char> = rest.chars().collect();
    let mut paren = 0i32;
    let mut brace = 0i32;
    let mut byte_idx = 0usize;
    let mut in_str = false;
    let mut in_char = false;
    let mut i = 0;
    while i < chars.len() {
        let ch = chars[i];
        let next = chars.get(i + 1).copied().unwrap_or('\0');
        if in_str || in_char {
            if ch == '\\' {
                byte_idx += ch.len_utf8() + next.len_utf8();
                i += 2;
                continue;
            }
            if in_str && ch == '"' {
                in_str = false;
            } else if in_char && ch == '\'' {
                in_char = false;
            }
        } else {
            match ch {
                '"' => in_str = true,
                '\'' => {
                    // char literal vs lifetime: only enter char mode if it closes
                    // within two/three chars (`'x'`, `'\n'`).
                    let two = chars.get(i + 2).copied().unwrap_or('\0');
                    if next == '\\' || two == '\'' {
                        in_char = true;
                    }
                }
                '(' => paren += 1,
                ')' => paren -= 1,
                '{' => brace += 1,
                '}' => brace -= 1,
                ';' if paren == 0 && brace == 0 => {
                    return Some(&rest[..byte_idx + 1]);
                }
                _ => {}
            }
        }
        byte_idx += ch.len_utf8();
        i += 1;
    }
    None
}

/// Join physical lines into logical statements terminated by `;`, `{`, or `}`.
/// This is a best-effort statement splitter for the taint pass.
fn logical_statements(body: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    for raw in body.lines() {
        if !current.is_empty() {
            current.push('\n');
        }
        current.push_str(raw);
        let t = raw.trim_end();
        if t.ends_with(';') || t.ends_with('{') || t.ends_with('}') || t.ends_with(',') {
            out.push(std::mem::take(&mut current));
        }
    }
    if !current.trim().is_empty() {
        out.push(current);
    }
    out
}

/// Parse `let [mut] <name> = ...` and return `<name>`; `None` if not a let
/// binding with a simple identifier (tuple / struct destructuring is ignored,
/// which is acceptable: such bindings are rare in these renderers and are still
/// caught by the sink-line raw-source check).
fn parse_let_binding_name(stmt: &str) -> Option<String> {
    let rest = stmt.strip_prefix("let ")?;
    let rest = rest.strip_prefix("mut ").unwrap_or(rest);
    let name: String = rest
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect();
    if name.is_empty() {
        return None;
    }
    // Require an `=` to qualify as a value binding we can taint-track.
    rest.contains('=').then_some(name)
}

/// Remove string-literal *prose* from `line` so identifier matching only sees
/// code, BUT surface `{ident}` interpolation names (and the trailing `, expr`
/// positional args) as bare identifiers so an aliased leak like
/// `format!("{raw}")` is still detected.
///
/// Heuristic: outside strings, keep text verbatim. Inside a `"..."` string, drop
/// the prose but emit each `{name}` capture (the inline-format interpolation) as
/// ` name `. Doubled braces `{{` / `}}` are literal and ignored.
fn strip_string_literals_keep_interpolations(line: &str) -> String {
    let chars: Vec<char> = line.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    let mut in_str = false;
    while i < chars.len() {
        let ch = chars[i];
        let next = chars.get(i + 1).copied().unwrap_or('\0');
        if in_str {
            if ch == '\\' {
                i += 2;
                continue;
            }
            if ch == '"' {
                in_str = false;
                out.push(' ');
                i += 1;
                continue;
            }
            if ch == '{' && next == '{' {
                i += 2;
                continue;
            }
            if ch == '{' {
                // Capture the interpolation name up to `}` or `:` (format spec).
                let mut j = i + 1;
                let mut name = String::new();
                while j < chars.len() && chars[j] != '}' && chars[j] != ':' {
                    name.push(chars[j]);
                    j += 1;
                }
                out.push(' ');
                out.push_str(name.trim());
                out.push(' ');
                // Advance to the closing `}` (if present).
                while j < chars.len() && chars[j] != '}' {
                    j += 1;
                }
                i = j + 1;
                continue;
            }
            // Ordinary prose char inside the string — drop it.
            i += 1;
            continue;
        }
        if ch == '"' {
            in_str = true;
            out.push(' ');
            i += 1;
            continue;
        }
        out.push(ch);
        i += 1;
    }
    out
}

/// Whether `line` references identifier `ident` at a word boundary (so `xs`
/// doesn't match `x`).
fn mentions_identifier(line: &str, ident: &str) -> bool {
    let mut search_from = 0;
    while let Some(pos) = line[search_from..].find(ident) {
        let abs = search_from + pos;
        let before_ok = abs == 0
            || !line[..abs]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_alphanumeric() || c == '_');
        let after_idx = abs + ident.len();
        let after_ok = after_idx >= line.len()
            || !line[after_idx..]
                .chars()
                .next()
                .is_some_and(|c| c.is_alphanumeric() || c == '_');
        if before_ok && after_ok {
            return true;
        }
        search_from = abs + ident.len();
    }
    false
}

// ---------------------------------------------------------------------------
// Layer 1: function-scoped allowlist + taint pass.
// ---------------------------------------------------------------------------

#[test]
fn source_scan_allowlisted_renderers_exist_and_mask() {
    let sources = loop_run_sources();
    for entry in ALLOWLIST {
        let source = sources
            .iter()
            .find(|(name, _)| *name == entry.file)
            .map(|(_, src)| *src)
            .unwrap_or_else(|| panic!("allowlist file not registered in scan: {}", entry.file));
        let body = extract_fn_body(source, entry.fn_name).unwrap_or_else(|| {
            panic!(
                "allowlisted renderer `{}::{}` not found (rename rot?). Update the \
                 #931 source-scan allowlist when renaming a recovery renderer.",
                entry.file, entry.fn_name
            )
        });
        if entry.expect_mask {
            assert!(
                body_contains_mask_call(&body),
                "allowlisted renderer `{}::{}` no longer calls a render-point mask \
                 ({MASK_CALLS:?}); a recovery prompt may now leak a raw path/reason.",
                entry.file,
                entry.fn_name
            );
        }
        // Taint pass: even with a mask call somewhere, a raw value must not slip
        // into a sink without masking on its own path.
        if let Some(reason) = taint_scan(&body) {
            panic!(
                "taint leak in allowlisted renderer `{}::{}`: {reason}",
                entry.file, entry.fn_name
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Layer 2: broad smoke scan over src/agent/loop_run/*.rs.
// ---------------------------------------------------------------------------

/// Read EVERY `src/agent/loop_run/*.rs` source file at test time (Issue #931
/// PR-review High-1). The broad smoke runs over the whole directory — not a fixed
/// include_str! list — so a NEW renderer added in any loop_run module is linted
/// without anyone remembering to register it. Test-only modules (`*_tests.rs`,
/// the `recovery_masking_*` scan/test files) are excluded: they are not
/// production renderers and legitimately reference raw fixture paths.
fn all_loop_run_sources() -> Vec<(String, String)> {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/src/agent/loop_run");
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).expect("read loop_run dir") {
        let path = entry.expect("dir entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        if name.ends_with("_tests.rs") || name.starts_with("recovery_masking_") {
            continue;
        }
        let src = std::fs::read_to_string(&path).expect("read loop_run source");
        out.push((name, src));
    }
    out
}

#[test]
fn source_scan_broad_smoke_catches_unmasked_renderers() {
    // For each scanned file, find every non-allowlisted fn whose body co-occurs a
    // raw taint source with a prompt sink, then run the precise intra-function
    // taint pass on it. A function fails ONLY if the taint pass finds a raw value
    // actually flowing into a sink AND the body neither masks nor carries a
    // `// #931-mask-checked:` annotation. (The coarse co-occurrence alone is not
    // enough — e.g. a `log_llm_event(json!{ "reason": validation.reason })` with a
    // static-str `.reason` and a hashed path is a LOGGING event, not a leak.)
    let mut qualifying_fns = 0usize;
    for (file, source) in all_loop_run_sources() {
        let file = file.as_str();
        for fn_name in enumerate_fn_names(&source) {
            let Some(body) = extract_fn_body(&source, &fn_name) else {
                continue;
            };
            let has_raw_source = RAW_TAINT_SOURCES.iter().any(|s| body.contains(s));
            // Only MODEL-facing functions qualify: a function that builds a `json!`
            // / `format!` solely for `log_llm_event` (no ConversationMessage /
            // push_system_note) is masked by `mask_payload_inplace` and is not a
            // wire leak (Issue #931 PR-review High-1).
            let has_model_sink = MODEL_SINKS.iter().any(|s| body.contains(s));
            if !has_raw_source || !has_model_sink {
                continue;
            }
            qualifying_fns += 1;
            let in_allowlist = ALLOWLIST
                .iter()
                .any(|e| e.file == file && e.fn_name == fn_name);
            if in_allowlist {
                continue;
            }
            if body.contains(MASK_ANNOTATION) {
                continue;
            }
            if let Some(reason) = taint_scan(&body) {
                panic!(
                    "broad smoke: `{file}::{fn_name}` leaks a raw recovery taint into a \
                     prompt sink ({reason}) and is not allowlisted/annotated. Add a \
                     render-point mask (mask_and_cap_recovery_field / mask_recovery_path \
                     / mask_secrets), add it to the #931 allowlist, or add a standalone \
                     `// #931-mask-checked: <reason>` line."
                );
            }
        }
    }
    // Anti-no-op guard: if the tokenizer regresses (e.g. fn enumeration / body
    // extraction silently returns nothing), the scan would vacuously pass. A
    // conservative floor proves the scan visited real qualifying functions —
    // these are the renderers / wire builders that literally reference a raw
    // taint source AND a prompt sink in-body (the pure `&str` helpers that mask a
    // pre-resolved arg do NOT qualify here, which is correct). The floor is set
    // well below the current count (14) so ordinary refactors don't trip it.
    const MIN_QUALIFYING_FNS: usize = 10;
    assert!(
        qualifying_fns >= MIN_QUALIFYING_FNS,
        "broad smoke visited only {qualifying_fns} qualifying fns (< {MIN_QUALIFYING_FNS}); \
         the source tokenizer likely regressed and the guard is no longer effective."
    );
}

/// Enumerate top-level + nested `fn <name>(` identifiers in `source`. Heuristic:
/// matches any `fn <ident>(`; closures and `fn` pointers in type position are
/// skipped because they lack a following `(` immediately after an identifier in
/// that pattern (best effort).
fn enumerate_fn_names(source: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut search_from = 0;
    while let Some(pos) = source[search_from..].find("fn ") {
        let abs = search_from + pos;
        search_from = abs + 3;
        // Ensure `fn` is a word (preceded by whitespace / start / `(`-less).
        let before_ok = abs == 0
            || source[..abs]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_whitespace() || c == '(' || c == ',');
        if !before_ok {
            continue;
        }
        let rest = &source[abs + 3..];
        let name: String = rest
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        if name.is_empty() {
            continue;
        }
        // Confirm an open paren follows the name (allow generics `<...>` between).
        let after_name = &rest[name.len()..];
        let after_trim = after_name.trim_start();
        if (after_trim.starts_with('(') || after_trim.starts_with('<'))
            && !names.contains(&name)
        {
            names.push(name);
        }
    }
    names
}

// ---------------------------------------------------------------------------
// Layer 3: self-verify — the scanner must FAIL on raw, PASS on masked.
// ---------------------------------------------------------------------------

#[test]
fn source_scan_self_verifies_on_fixtures() {
    // Unmasked: a raw hint.path flows straight into a format! sink → must taint.
    let leaky = "fn leaky(hint: &Hint) -> String {\n    \
        format!(\"target {}\", hint.path)\n}";
    let leaky_body = extract_fn_body(leaky, "leaky").expect("fixture body");
    assert!(
        taint_scan(&leaky_body).is_some(),
        "self-verify: scanner must FLAG an unmasked raw hint.path → format! leak"
    );

    // Unmasked via aliasing local: `let raw = hint.path; format!(\"{raw}\")`.
    let aliased = "fn aliased(hint: &Hint) -> String {\n    \
        let raw = hint.path.clone();\n    \
        format!(\"{raw}\")\n}";
    let aliased_body = extract_fn_body(aliased, "aliased").expect("fixture body");
    assert!(
        taint_scan(&aliased_body).is_some(),
        "self-verify: scanner must FLAG a raw-tainted local reaching a sink"
    );

    // Masked at the binding: `let p = mask_and_cap_recovery_field(hint.path);`.
    let masked = "fn masked(hint: &Hint) -> String {\n    \
        let p = mask_and_cap_recovery_field(&hint.path);\n    \
        format!(\"target {p}\")\n}";
    let masked_body = extract_fn_body(masked, "masked").expect("fixture body");
    assert!(
        taint_scan(&masked_body).is_none(),
        "self-verify: scanner must PASS when the raw value is masked at the binding"
    );

    // Masked at the sink line directly.
    let masked_inline = "fn masked_inline(hint: &Hint) -> String {\n    \
        format!(\"target {}\", mask_secrets(&hint.path))\n}";
    let masked_inline_body =
        extract_fn_body(masked_inline, "masked_inline").expect("fixture body");
    assert!(
        taint_scan(&masked_inline_body).is_none(),
        "self-verify: scanner must PASS when the sink line masks inline"
    );

    // Annotated standalone-line whitelist.
    let annotated = "fn annotated(hint: &Hint) -> String {\n    \
        // #931-mask-checked: path is a compile-time constant, not LLM-derived\n    \
        format!(\"target {}\", hint.path)\n}";
    let annotated_body = extract_fn_body(annotated, "annotated").expect("fixture body");
    assert!(
        taint_scan(&annotated_body).is_none(),
        "self-verify: a `// #931-mask-checked:` line must whitelist the statement"
    );

    // Issue #931 CB-002: the artifact-directed Bash-rejection shape. A raw
    // `policy_target_path`-derived target reaching a `format!` sink MUST taint.
    let policy_raw = "fn policy_raw(policy: &P, work_root: &Path) -> String {\n    \
        let target_display = policy_target_path(policy)\n        \
            .map(|t| t.strip_prefix(work_root).unwrap_or(t).to_string_lossy().replace('x', \"/\"))\n        \
            .unwrap_or_else(|| \"the active target\".to_string());\n    \
        format!(\"rejected Bash on {target_display}\")\n}";
    let policy_raw_body = extract_fn_body(policy_raw, "policy_raw").expect("fixture body");
    assert!(
        taint_scan(&policy_raw_body).is_some(),
        "self-verify: an unmasked policy_target_path-derived target reaching a sink must FLAG"
    );

    // The SAME shape with the mask applied INSIDE the multi-line `.map(|t| { ... })`
    // initializer MUST pass — proving the full-binding mask check (let_binding_text)
    // handles the cross-line masked construction without a false positive.
    let policy_masked = "fn policy_masked(policy: &P, work_root: &Path) -> String {\n    \
        let target_display = policy_target_path(policy)\n        \
            .map(|t| {\n            \
                mask_and_cap_recovery_field(&t.strip_prefix(work_root).unwrap_or(t).to_string_lossy().replace('x', \"/\"))\n        \
            })\n        \
            .unwrap_or_else(|| \"the active target\".to_string());\n    \
        format!(\"rejected Bash on {target_display}\")\n}";
    let policy_masked_body = extract_fn_body(policy_masked, "policy_masked").expect("fixture body");
    assert!(
        taint_scan(&policy_masked_body).is_none(),
        "self-verify: a policy_target_path value masked inside a multi-line .map must PASS"
    );

    // Issue #931 PR-review High-2: a MULTI-LINE `json!` whose raw `hint.path` field
    // lands on its OWN line (the field line does not contain the `json!` token) MUST
    // be flagged by the macro-span pass — the line-based pass alone misses it.
    let multiline_json_raw = "fn ml_raw(hint: &H) -> M {\n    \
        let payload = serde_json::json!({\n        \
            \"path\": hint.path,\n    \
        });\n    \
        ConversationMessage::user(format!(\"{payload}\"))\n}";
    let ml_raw_body = extract_fn_body(multiline_json_raw, "ml_raw").expect("fixture body");
    assert!(
        taint_scan(&ml_raw_body).is_some(),
        "self-verify: a raw hint.path on its own line inside a multi-line json! must FLAG"
    );

    // The same multi-line `json!` with the field masked MUST pass.
    let multiline_json_masked = "fn ml_masked(hint: &H) -> M {\n    \
        let payload = serde_json::json!({\n        \
            \"path\": mask_secrets(&hint.path),\n    \
        });\n    \
        ConversationMessage::user(format!(\"{payload}\"))\n}";
    let ml_masked_body = extract_fn_body(multiline_json_masked, "ml_masked").expect("fixture body");
    assert!(
        taint_scan(&ml_masked_body).is_none(),
        "self-verify: a masked field inside a multi-line json! must PASS"
    );

    // A multi-line `json!` that is an argument to a LOGGING call is masked by
    // `mask_payload_inplace` and must NOT be flagged even with a raw field.
    let logging_json = "fn lg(hint: &H) {\n    \
        log_llm_event(\n        \
            \"event.name\",\n        \
            serde_json::json!({\n            \
                \"path\": hint.path,\n        \
            }),\n    \
        );\n}";
    let lg_body = extract_fn_body(logging_json, "lg").expect("fixture body");
    assert!(
        taint_scan(&lg_body).is_none(),
        "self-verify: a raw field inside a log_llm_event json! must PASS (logging defense)"
    );
}

#[test]
fn source_scan_extract_fn_body_handles_braces_in_strings() {
    // The brace counter must not be confused by `{` / `}` inside string literals.
    let src = "fn tricky() -> String {\n    \
        let s = \"a } b { c\";\n    \
        format!(\"{s}\")\n}\n\
        fn after() {}";
    let body = extract_fn_body(src, "tricky").expect("body");
    assert!(body.contains("format!"));
    assert!(!body.contains("fn after"));
}
