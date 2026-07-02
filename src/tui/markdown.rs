//! Terminal markdown renderer for assistant responses (issue #431).
//!
//! Public entry point: [`MarkdownRenderer`], a line-buffered state machine
//! that converts a stream of markdown chunks into SGR-decorated terminal
//! output. The renderer is deliberately narrow — it supports only the five
//! elements listed in the design policy (code fences, inline code, headings
//! H1/H2/H3, bold `**...**`, and `- ` list bullets) and emits exclusively
//! SGR escapes (`^\x1b\[[0-9;]*m`) so it cannot break the fixed footer's
//! DECSTBM scroll region.
//!
//! Invariants (enforced by tests):
//! 1. `MarkdownRenderer::new` does not read env / `is_terminal`; all runtime
//!    signals are resolved in [`color_enabled_for_markdown`] and
//!    [`markdown_unicode_enabled`] at the call site.
//! 2. Output contains only SGR escapes (no CSI movement, no scroll, no clear).
//! 3. `<think>...</think>` spans (including those that cross chunks or occur
//!    multiple times on one line) are removed from visible output entirely.
//! 4. `line_buffer` is bounded by [`MAX_BUFFERED_LINE_CHARS`]; on overflow the
//!    current buffer is sanitized and flushed as plain text (fail-open).
//! 5. After `flush()` the renderer is reset to its initial state.

use std::io::IsTerminal;

// Color constants. SGR-only (`\x1b[...m`) by design — see AC9.
const MD_CODE_FENCE_COLOR: &str = "\x1b[32m"; // green
const MD_INLINE_CODE_COLOR: &str = "\x1b[36m"; // cyan
const MD_H1_COLOR: &str = "\x1b[1m\x1b[35m"; // bold + magenta
const MD_H2_COLOR: &str = "\x1b[1m\x1b[33m"; // bold + yellow
const MD_H3_COLOR: &str = "\x1b[1m\x1b[34m"; // bold + blue
const MD_BOLD: &str = "\x1b[1m";
const MD_RESET: &str = "\x1b[0m";

/// Soft cap on the length of `MarkdownRenderer::line_buffer`. When the buffer
/// grows past this without hitting a `\n`, the renderer fails open: sanitizes
/// the current buffer, emits it as plain text, and resets the buffer. Prevents
/// unbounded memory growth from models that stream a single giant line or
/// forget to close a `<think>` / fence.
pub(crate) const MAX_BUFFERED_LINE_CHARS: usize = 64 * 1024;

/// Assistant-response markdown renderer.
///
/// Purity contract: construction does not touch env / `is_terminal`. Callers
/// pass `color_enabled` and `utf8` after resolving them via
/// [`color_enabled_for_markdown`] and [`markdown_unicode_enabled`].
pub struct MarkdownRenderer {
    line_buffer: String,
    in_code_block: bool,
    in_think_block: bool,
    color_enabled: bool,
    utf8: bool,
}

impl MarkdownRenderer {
    /// Construct a renderer with the two static behavior flags resolved.
    ///
    /// - `color_enabled=false` strips all markdown symbols and emits plain text.
    /// - `utf8=false` falls back to `* ` for list bullets (ASCII).
    pub fn new(color_enabled: bool, utf8: bool) -> Self {
        Self {
            line_buffer: String::new(),
            in_code_block: false,
            in_think_block: false,
            color_enabled,
            utf8,
        }
    }

    /// Append a chunk of streamed text. Returns any full lines that became
    /// emit-ready (with trailing `\n`). Residual characters past the last
    /// newline stay in `line_buffer` for the next call.
    pub fn push_chunk(&mut self, chunk: &str) -> String {
        let mut out = String::new();
        // Fail-open guard: if appending the chunk would blow past the cap,
        // drain whatever we have first so memory stays bounded.
        if self.line_buffer.chars().count() + chunk.chars().count() > MAX_BUFFERED_LINE_CHARS {
            self.force_drain_buffer(&mut out);
        }
        self.line_buffer.push_str(chunk);

        // Extract every complete line (terminated by `\n`) from the buffer.
        while let Some(idx) = self.line_buffer.find('\n') {
            let line: String = self.line_buffer.drain(..=idx).collect();
            // `line` still has the trailing `\n`; strip it for processing.
            let body = line.strip_suffix('\n').unwrap_or(&line);
            self.process_line(body, true, &mut out);
        }

        // Secondary check: a chunk with no newline can still blow the cap.
        if self.line_buffer.chars().count() > MAX_BUFFERED_LINE_CHARS {
            self.force_drain_buffer(&mut out);
        }

        out
    }

    /// Flush any residual buffer without appending a trailing newline, and
    /// reset internal state so the next turn starts clean.
    pub fn flush(&mut self) -> String {
        let mut out = String::new();
        if !self.line_buffer.is_empty() {
            let body = std::mem::take(&mut self.line_buffer);
            self.process_line(&body, false, &mut out);
        }
        // Hard reset so stale state can't leak into the next turn.
        self.line_buffer.clear();
        self.in_code_block = false;
        self.in_think_block = false;
        out
    }

    /// Drain the current buffer as sanitized plain text (fail-open). Used when
    /// `MAX_BUFFERED_LINE_CHARS` is exceeded. `in_code_block` / `in_think_block`
    /// are preserved so the next chunk can still resolve matching fences/tags.
    ///
    /// Retains the last few bytes of the buffer so a partial opening tag like
    /// `<thi` can rejoin the next chunk's `nk>...` and still match as
    /// `<think>`. Without this suffix retention, the next chunk would start at
    /// `nk>SECRET</think>...` with no matching `<think>`, leaking the secret.
    /// The retained suffix is at least as large as `<think>` / `</think>` /
    /// ```` ``` ```` (longest = 8 bytes for `</think>`), rounded up to 16.
    fn force_drain_buffer(&mut self, out: &mut String) {
        const SUFFIX_RETAIN: usize = 16;
        // Split at a char boundary so we never bisect a multibyte char.
        let len = self.line_buffer.len();
        let mut split = len.saturating_sub(SUFFIX_RETAIN);
        while split < len && !self.line_buffer.is_char_boundary(split) {
            split += 1;
        }
        let suffix = self.line_buffer.split_off(split);
        let body = std::mem::take(&mut self.line_buffer);
        self.line_buffer = suffix;
        // Strip think content first so a never-closing <think> can't leak.
        let visible = self.strip_think(&body);
        out.push_str(&sanitize(&visible));
        out.push('\n');
    }

    /// Core per-line dispatch. `append_newline=true` means this line came from
    /// a `\n`-terminated segment inside `push_chunk` (so the emitted form
    /// should end with `\n`); `false` means we're flushing a residual from
    /// `flush()` (no trailing `\n`).
    fn process_line(&mut self, line: &str, append_newline: bool, out: &mut String) {
        let visible = self.strip_think(line);
        // `<think>...</think>` that consumed the entire line produces nothing
        // visible. Suppress the newline too so think blocks are invisible.
        if visible.is_empty() && (self.in_think_block || contained_think_only(line)) {
            return;
        }

        // Code fence toggle: accept language-tagged fences like ```rust.
        if visible.trim_start().starts_with("```") {
            self.in_code_block = !self.in_code_block;
            // Fence line itself is consumed (not emitted).
            return;
        }

        if self.in_code_block {
            // Each code block line: 2-space indent + green + sanitized body + reset.
            let sanitized = sanitize(&visible);
            if self.color_enabled {
                out.push_str("  ");
                out.push_str(MD_CODE_FENCE_COLOR);
                out.push_str(&sanitized);
                out.push_str(MD_RESET);
            } else {
                out.push_str("  ");
                out.push_str(&sanitized);
            }
            if append_newline {
                out.push('\n');
            }
            return;
        }

        // Regular line: heading / list / inline replacement.
        let rendered = render_line(&visible, self.color_enabled, self.utf8);
        out.push_str(&rendered);
        if append_newline {
            out.push('\n');
        }
    }

    /// Strip `<think>...</think>` spans from `line`, tracking state across
    /// invocations so chunk-split tags still work. Same-line single, same-line
    /// multiple, and cross-chunk forms are all handled.
    fn strip_think(&mut self, line: &str) -> String {
        let mut out = String::new();
        let mut remaining = line;
        loop {
            if self.in_think_block {
                match remaining.find("</think>") {
                    Some(idx) => {
                        self.in_think_block = false;
                        remaining = &remaining[idx + "</think>".len()..];
                        continue;
                    }
                    None => {
                        // Inside a think block, no close tag this line → drop all.
                        return out;
                    }
                }
            }
            match remaining.find("<think>") {
                Some(idx) => {
                    out.push_str(&remaining[..idx]);
                    self.in_think_block = true;
                    remaining = &remaining[idx + "<think>".len()..];
                    continue;
                }
                None => {
                    out.push_str(remaining);
                    return out;
                }
            }
        }
    }
}

/// Return true if `line` contained a `<think>` span that fully consumed it.
/// Used so same-line complete think blocks don't emit a blank line.
fn contained_think_only(line: &str) -> bool {
    let stripped = line.trim();
    stripped.starts_with("<think>") && stripped.ends_with("</think>")
}

/// Render a single non-code, non-fence, non-think line to ANSI-decorated text.
/// Heading + list are matched at the line start; inline bold and backtick code
/// are replaced in a single left-to-right pass after the line prefix is
/// resolved. All user-supplied substrings are [`sanitize`]-filtered before
/// ANSI escapes are concatenated, so the returned string contains only
/// sanitized content plus SGR escapes.
pub(crate) fn render_line(line: &str, color_enabled: bool, utf8: bool) -> String {
    // Heading ladder: check longest prefix first so `### ` is not swallowed
    // by the `# ` / `## ` arms.
    if let Some(rest) = line.strip_prefix("### ") {
        return wrap_heading(rest, MD_H3_COLOR, color_enabled);
    }
    if let Some(rest) = line.strip_prefix("## ") {
        return wrap_heading(rest, MD_H2_COLOR, color_enabled);
    }
    if let Some(rest) = line.strip_prefix("# ") {
        return wrap_heading(rest, MD_H1_COLOR, color_enabled);
    }

    // List bullet: replace leading `- ` with `● ` (utf8) or `* ` (ascii). Indent
    // preservation is out-of-scope; `- ` only matches when it starts the line.
    if let Some(rest) = line.strip_prefix("- ") {
        let bullet = if utf8 { "● " } else { "* " };
        let body = render_inline(rest, color_enabled);
        let mut out = String::with_capacity(bullet.len() + body.len());
        out.push_str(bullet);
        out.push_str(&body);
        return out;
    }

    render_inline(line, color_enabled)
}

fn wrap_heading(body: &str, color: &str, color_enabled: bool) -> String {
    // Headings run the body through the inline renderer so `**bold**` and
    // `` `code` `` inside a heading pick up SGR (and have their symbols
    // stripped in plain-text mode — AC6). The heading SGR wraps the whole
    // inline-rendered body.
    let inline = render_inline(body, color_enabled);
    if color_enabled {
        format!("{color}{inline}{MD_RESET}")
    } else {
        inline
    }
}

/// Apply inline `**bold**` and `` `code` `` replacements to `text`. A single
/// forward pass; mismatched delimiters degrade gracefully to plain text.
/// All slices that come from `text` are run through [`sanitize`] before they
/// are appended, so the final string has only (sanitized text) + SGR escapes.
fn render_inline(text: &str, color_enabled: bool) -> String {
    let mut out = String::with_capacity(text.len());
    let bytes = text.as_bytes();
    let mut i = 0;
    let mut literal_start = 0;
    while i < bytes.len() {
        // Inline bold: `**...**`
        if i + 1 < bytes.len()
            && bytes[i] == b'*'
            && bytes[i + 1] == b'*'
            && let Some(end_rel) = find_subslice(&bytes[i + 2..], b"**")
        {
            // Flush any plain prefix text since the last delimiter.
            if literal_start < i {
                out.push_str(&sanitize(&text[literal_start..i]));
            }
            let inner = &text[i + 2..i + 2 + end_rel];
            let sanitized_inner = sanitize(inner);
            if color_enabled {
                out.push_str(MD_BOLD);
                out.push_str(&sanitized_inner);
                out.push_str(MD_RESET);
            } else {
                out.push_str(&sanitized_inner);
            }
            i += 2 + end_rel + 2;
            literal_start = i;
            continue;
        }
        // Inline code: `` `code` ``
        if bytes[i] == b'`'
            && let Some(end_rel) = find_byte(&bytes[i + 1..], b'`')
        {
            if literal_start < i {
                out.push_str(&sanitize(&text[literal_start..i]));
            }
            let inner = &text[i + 1..i + 1 + end_rel];
            let sanitized_inner = sanitize(inner);
            if color_enabled {
                out.push_str(MD_INLINE_CODE_COLOR);
                out.push_str(&sanitized_inner);
                out.push_str(MD_RESET);
            } else {
                out.push_str(&sanitized_inner);
            }
            i += 1 + end_rel + 1;
            literal_start = i;
            continue;
        }
        // Regular UTF-8 char: advance to the next char boundary.
        i = next_char_boundary(text, i);
    }
    // Flush any remaining plain tail.
    if literal_start < bytes.len() {
        out.push_str(&sanitize(&text[literal_start..]));
    }
    out
}

fn find_subslice(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || needle.len() > hay.len() {
        return None;
    }
    for start in 0..=hay.len() - needle.len() {
        if &hay[start..start + needle.len()] == needle {
            return Some(start);
        }
    }
    None
}

fn find_byte(hay: &[u8], needle: u8) -> Option<usize> {
    hay.iter().position(|&b| b == needle)
}

fn next_char_boundary(s: &str, from: usize) -> usize {
    let bytes = s.as_bytes();
    let mut end = from + 1;
    while end < bytes.len() && !s.is_char_boundary(end) {
        end += 1;
    }
    end
}

/// Sanitize terminal-sensitive characters. Replaces ESC, C0 (except `\t`/`\n`),
/// DEL, C1 (including 8-bit CSI U+009B / OSC U+009D), and Unicode bidi
/// controls with `?`. All other text survives.
pub(crate) fn sanitize(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        let cp = ch as u32;
        let is_c0 = cp < 0x20 && ch != '\t' && ch != '\n';
        let is_del = cp == 0x7F;
        let is_c1 = (0x80..=0x9F).contains(&cp);
        // Bidi controls: PDF/LRE/RLE/LRO/RLO (U+202A..U+202E),
        // FSI/LRI/RLI/PDI (U+2066..U+2069), plus the single-char marks
        // LRM (U+200E), RLM (U+200F), and Arabic Letter Mark (U+061C).
        let is_bidi = matches!(
            cp,
            0x202A..=0x202E | 0x2066..=0x2069 | 0x200E | 0x200F | 0x061C
        );
        if is_c0 || is_del || is_c1 || is_bidi {
            out.push('?');
        } else {
            out.push(ch);
        }
    }
    out
}

// ---- Caller-side helpers (env / is_terminal live here, not in the renderer) ----

/// True when `ANVIL_NO_MARKDOWN` is set to any non-empty value (complete bypass).
pub(crate) fn markdown_fully_disabled() -> bool {
    markdown_fully_disabled_with(|key| {
        std::env::var_os(key).map(|value| value.to_string_lossy().into_owned())
    })
}

fn markdown_fully_disabled_with(get_env: impl Fn(&str) -> Option<String>) -> bool {
    get_env("ANVIL_NO_MARKDOWN").is_some_and(|value| !value.is_empty())
}

/// True when ANSI color should be emitted: `NO_COLOR` unset/empty **and**
/// stdout is a TTY. Reuses the existing POSIX-compliant
/// `no_color_requested` helper from `turn.rs`.
pub(crate) fn color_enabled_for_markdown() -> bool {
    color_enabled_for_markdown_with(
        crate::agent::loop_run::no_color_requested(),
        std::io::stdout().is_terminal(),
    )
}

fn color_enabled_for_markdown_with(no_color_requested: bool, stdout_is_terminal: bool) -> bool {
    !no_color_requested && stdout_is_terminal
}

/// True when unicode-enabled symbols (e.g. `●`) should be used. Reuses the
/// same locale + `ANVIL_NO_EMOJI` logic as the spinner / progress line.
pub(crate) fn markdown_unicode_enabled() -> bool {
    crate::agent::loop_run::unicode_supported()
}

#[cfg(test)]
mod tests {
    use super::*;
    use regex::Regex;

    // -------- render_line: headings (AC3) --------
    #[test]
    fn render_line_headings_h1_h2_h3_colors() {
        assert_eq!(
            render_line("# Title", true, true),
            format!("{}{}{}", MD_H1_COLOR, "Title", MD_RESET)
        );
        assert_eq!(
            render_line("## Title", true, true),
            format!("{}{}{}", MD_H2_COLOR, "Title", MD_RESET)
        );
        assert_eq!(
            render_line("### Title", true, true),
            format!("{}{}{}", MD_H3_COLOR, "Title", MD_RESET)
        );
    }

    #[test]
    fn render_line_heading_plain_when_no_color() {
        assert_eq!(render_line("# Title", false, true), "Title");
        assert_eq!(render_line("## Title", false, true), "Title");
        assert_eq!(render_line("### Title", false, true), "Title");
    }

    // -------- render_line: list bullets (AC5) --------
    #[test]
    fn render_line_list_utf8_vs_ascii() {
        assert_eq!(render_line("- item", true, true), "● item");
        assert_eq!(render_line("- item", true, false), "* item");
        // Bullet stays even when color is disabled — symbol replacement is
        // non-color by design.
        assert_eq!(render_line("- item", false, true), "● item");
        assert_eq!(render_line("- item", false, false), "* item");
    }

    // -------- render_line: bold + inline code (AC2, AC4) --------
    #[test]
    fn render_line_bold_sgr() {
        assert_eq!(
            render_line("use **bold** words", true, true),
            format!("use {}{}{} words", MD_BOLD, "bold", MD_RESET)
        );
    }

    #[test]
    fn render_line_inline_code_cyan() {
        assert_eq!(
            render_line("run `cargo test`", true, true),
            format!("run {}{}{}", MD_INLINE_CODE_COLOR, "cargo test", MD_RESET)
        );
    }

    #[test]
    fn render_line_bold_and_inline_code_combined() {
        let got = render_line("use **bold** and `code`", true, true);
        let expected = format!(
            "use {}{}{} and {}{}{}",
            MD_BOLD, "bold", MD_RESET, MD_INLINE_CODE_COLOR, "code", MD_RESET
        );
        assert_eq!(got, expected);
    }

    // -------- render_line: plain when color disabled (AC6) --------
    #[test]
    fn render_line_no_color_strips_symbols() {
        assert_eq!(
            render_line("use **bold** and `code`", false, true),
            "use bold and code"
        );
    }

    // -------- SGR-only invariant (AC9) --------
    #[test]
    fn output_has_only_sgr_csi() {
        // Build a representative sample of decorated output.
        let samples = [
            render_line("# h1", true, true),
            render_line("## h2", true, true),
            render_line("### h3", true, true),
            render_line("- item", true, true),
            render_line("use **bold** and `code`", true, true),
        ];
        // Matches any CSI sequence: ESC [ <params> <final_byte>. `final_byte`
        // must be `m` for SGR.
        let csi_re = Regex::new(r"\x1b\[[0-9;]*([A-Za-z])").unwrap();
        for sample in &samples {
            for cap in csi_re.captures_iter(sample) {
                let final_byte = &cap[1];
                assert_eq!(
                    final_byte, "m",
                    "non-SGR CSI detected in sample {sample:?} (final={final_byte})"
                );
            }
        }
    }

    // -------- Purity: no env read inside renderer (AC17) --------
    #[test]
    fn render_line_is_env_independent() {
        // This test intentionally does NOT lock the ENV_MUTEX: `MarkdownRenderer::new`
        // + `render_line` must be deterministic regardless of NO_COLOR state.
        // The result should depend only on the passed flags.
        let got = render_line("**x**", true, true);
        assert_eq!(got, format!("{}x{}", MD_BOLD, MD_RESET));
    }

    // -------- Streaming (AC8) --------
    #[test]
    fn push_chunk_single_line_bold() {
        let mut r = MarkdownRenderer::new(true, true);
        let out = r.push_chunk("hello **bold** world\n");
        assert_eq!(out, format!("hello {}bold{} world\n", MD_BOLD, MD_RESET));
    }

    #[test]
    fn push_chunk_split_grains_1_to_8_bytes_equals_nonsplit() {
        let input = "hello **bold** and `code`\nnext line\n";
        let baseline = {
            let mut r = MarkdownRenderer::new(true, true);
            let mut out = r.push_chunk(input);
            out.push_str(&r.flush());
            out
        };
        for grain in 1..=8 {
            let mut r = MarkdownRenderer::new(true, true);
            let mut out = String::new();
            let mut start = 0;
            while start < input.len() {
                let mut end = (start + grain).min(input.len());
                while !input.is_char_boundary(end) {
                    end += 1;
                }
                out.push_str(&r.push_chunk(&input[start..end]));
                start = end;
            }
            out.push_str(&r.flush());
            assert_eq!(out, baseline, "grain={grain}");
        }
    }

    // -------- Code fence (AC1) --------
    #[test]
    fn code_block_green_with_indent_and_language_tag() {
        let mut r = MarkdownRenderer::new(true, true);
        let mut out = r.push_chunk("```rust\n");
        out.push_str(&r.push_chunk("fn main() {}\n"));
        out.push_str(&r.push_chunk("```\n"));
        // Fence lines are consumed. The single code line gets indented +
        // green + reset + newline.
        let expected = format!("  {}fn main() {{}}{}\n", MD_CODE_FENCE_COLOR, MD_RESET);
        assert_eq!(out, expected);
    }

    #[test]
    fn code_block_plain_when_no_color() {
        let mut r = MarkdownRenderer::new(false, true);
        let mut out = r.push_chunk("```\n");
        out.push_str(&r.push_chunk("hi\n"));
        out.push_str(&r.push_chunk("```\n"));
        assert_eq!(out, "  hi\n");
    }

    // -------- Think block (AC18) --------
    #[test]
    fn strip_think_single_line() {
        let mut r = MarkdownRenderer::new(true, true);
        let out = r.push_chunk("<think>secret</think>hello\n");
        assert_eq!(out, "hello\n");
    }

    #[test]
    fn strip_think_cross_chunk() {
        let mut r = MarkdownRenderer::new(true, true);
        let mut out = r.push_chunk("before <think>s");
        out.push_str(&r.push_chunk("ecret</think>after\n"));
        assert_eq!(out, "before after\n");
    }

    #[test]
    fn strip_think_multi_occurrence_on_one_line() {
        let mut r = MarkdownRenderer::new(true, true);
        let out = r.push_chunk("a<think>x</think>b<think>y</think>c\n");
        assert_eq!(out, "abc\n");
    }

    #[test]
    fn strip_think_entire_line_suppresses_newline() {
        let mut r = MarkdownRenderer::new(true, true);
        let out = r.push_chunk("<think>only think</think>\n");
        assert_eq!(out, "");
    }

    // -------- flush() (AC19) --------
    #[test]
    fn flush_emits_remainder_and_resets_state() {
        let mut r = MarkdownRenderer::new(true, true);
        r.push_chunk("**bo");
        // buffer has "**bo" (no newline) so nothing streamed yet.
        let tail = r.flush();
        // render_inline("**bo") has no closing `**`, so it falls through as "**bo".
        assert_eq!(tail, "**bo");
        // Internal state must be reset.
        assert!(r.line_buffer.is_empty());
        assert!(!r.in_code_block);
        assert!(!r.in_think_block);
    }

    #[test]
    fn flush_with_complete_bold_emits_decorated() {
        let mut r = MarkdownRenderer::new(true, true);
        r.push_chunk("**bold**");
        let tail = r.flush();
        assert_eq!(tail, format!("{}bold{}", MD_BOLD, MD_RESET));
    }

    // -------- Sanitize (security) --------
    #[test]
    fn sanitize_replaces_c0_del_c1_and_bidi_controls() {
        assert_eq!(sanitize("hello\x1bworld"), "hello?world");
        assert_eq!(sanitize("bell\x07x"), "bell?x");
        assert_eq!(sanitize("del\x7Fx"), "del?x");
        // C1 / 8-bit CSI + OSC
        assert_eq!(sanitize("csi\u{009B}x"), "csi?x");
        assert_eq!(sanitize("osc\u{009D}x"), "osc?x");
        // Bidi
        assert_eq!(sanitize("bidi\u{202E}x"), "bidi?x");
        assert_eq!(sanitize("iso\u{2066}x"), "iso?x");
        // Normal UTF-8 survives (including C1-range-looking-but-above text).
        assert_eq!(sanitize("caf\u{00E9}"), "caf\u{00E9}");
        // Tab / newline preserved in sanitize (line framing already removes
        // newlines elsewhere, tab is not terminal-hostile).
        assert_eq!(sanitize("a\tb"), "a\tb");
    }

    // -------- MAX_BUFFERED_LINE_CHARS overflow (fail-open) --------
    #[test]
    fn buffer_cap_forces_bounded_memory_on_newline_free_stream() {
        let mut r = MarkdownRenderer::new(true, true);
        // Build an input bigger than the cap without any newline.
        let big = "a".repeat(MAX_BUFFERED_LINE_CHARS + 100);
        let mut out = r.push_chunk(&big);
        out.push_str(&r.flush());
        // Bounded memory: buffer must not hold more than the cap at any time.
        // After flush the buffer is empty.
        assert!(r.line_buffer.is_empty());
        // Output must contain the drained characters (fail-open): sum of 'a'
        // characters should equal the input length.
        let a_count = out.chars().filter(|&c| c == 'a').count();
        assert_eq!(a_count, MAX_BUFFERED_LINE_CHARS + 100);
    }

    // -------- CB-001: overflow drain must not split a `<think>` open tag --------
    #[test]
    fn overflow_with_split_think_tag_does_not_leak_inner_text() {
        let mut r = MarkdownRenderer::new(true, true);
        // Build a chunk that forces force_drain_buffer to fire with the last
        // bytes of the buffer being the start of a `<think>` open tag. We want
        // the drain to happen while the buffer already ends with `<thi`, so
        // the next chunk's `nk>SECRET</think>` could otherwise leak.
        // Strategy: single chunk just over the cap whose tail is `<thi`.
        let mut big = "x".repeat(MAX_BUFFERED_LINE_CHARS + 10);
        big.push_str("<thi");
        let mut out = r.push_chunk(&big);
        // Completion of the open tag + secret payload in the next chunk.
        out.push_str(&r.push_chunk("nk>SECRET</think>after\n"));
        out.push_str(&r.flush());
        // The secret content of the <think> block must not appear in output.
        assert!(
            !out.contains("SECRET"),
            "SECRET leaked across a split-tag overflow drain: {out:?}"
        );
    }

    // -------- CB-002: bidi sanitization must cover LRM / RLM / ALM --------
    #[test]
    fn sanitize_strips_lrm_rlm_alm_bidi_marks() {
        // U+200E LEFT-TO-RIGHT MARK
        assert_eq!(sanitize("lrm\u{200E}x"), "lrm?x");
        // U+200F RIGHT-TO-LEFT MARK
        assert_eq!(sanitize("rlm\u{200F}x"), "rlm?x");
        // U+061C ARABIC LETTER MARK
        assert_eq!(sanitize("alm\u{061C}x"), "alm?x");
    }

    // -------- CB-003: headings should process inline markdown --------
    #[test]
    fn render_line_heading_with_inline_bold_and_code() {
        let got = render_line("# hello **world** `code`", true, true);
        // Heading wrapper (H1 SGR + RESET) must be present and wrap the whole body.
        assert!(
            got.starts_with(MD_H1_COLOR),
            "missing heading prefix: {got:?}"
        );
        assert!(got.ends_with(MD_RESET), "missing reset suffix: {got:?}");
        // Inline bold + inline code SGR must also appear inside the heading body.
        assert!(
            got.contains(MD_BOLD),
            "missing bold SGR inside heading: {got:?}"
        );
        assert!(
            got.contains(MD_INLINE_CODE_COLOR),
            "missing inline code SGR inside heading: {got:?}"
        );
        // Literal `**` and backticks must be gone.
        assert!(!got.contains("**"), "literal `**` leaked: {got:?}");
        assert!(!got.contains('`'), "literal backtick leaked: {got:?}");
        // And the actual words survive.
        assert!(got.contains("hello"));
        assert!(got.contains("world"));
        assert!(got.contains("code"));
    }

    #[test]
    fn render_line_heading_with_inline_strips_symbols_when_no_color() {
        // AC6: plain text mode still strips inline markup symbols.
        let got = render_line("# hello **world** `code`", false, true);
        assert_eq!(got, "hello world code");
    }

    // -------- CB-004: non-streaming path must preserve leading whitespace
    // after think removal (renderer does not trim; xml_fallback's
    // strip_think_tags does, so we feed the renderer directly). --------
    #[test]
    fn non_streaming_path_preserves_leading_whitespace_after_think_removal() {
        let input = "<think>x</think>\n\nhello";
        let mut r = MarkdownRenderer::new(false, true);
        let mut body = r.push_chunk(input);
        body.push_str(&r.flush());
        // `<think>x</think>` on line 1 leaves nothing visible (and suppresses its newline),
        // then line 2 is an empty line (preserved), then `hello` flushes with no trailing `\n`.
        assert_eq!(body, "\nhello");
    }

    // -------- Caller-side helpers --------
    #[test]
    fn markdown_fully_disabled_reads_env() {
        assert!(!markdown_fully_disabled_with(|_| None));
        assert!(!markdown_fully_disabled_with(|key| {
            (key == "ANVIL_NO_MARKDOWN").then(String::new)
        }));
        assert!(markdown_fully_disabled_with(|key| {
            (key == "ANVIL_NO_MARKDOWN").then(|| "1".to_string())
        }));
    }

    #[test]
    fn color_enabled_for_markdown_is_false_under_cargo_non_tty() {
        assert!(!color_enabled_for_markdown_with(false, false));
        assert!(!color_enabled_for_markdown_with(true, true));
        assert!(color_enabled_for_markdown_with(false, true));
    }
}
