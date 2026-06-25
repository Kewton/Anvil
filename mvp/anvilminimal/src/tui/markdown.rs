use std::io::{self, IsTerminal, Write};

use crate::tui::OutputRenderer;

const MD_CODE_FENCE_COLOR: &str = "\x1b[32m";
const MD_INLINE_CODE_COLOR: &str = "\x1b[36m";
const MD_H1_COLOR: &str = "\x1b[1m\x1b[35m";
const MD_H2_COLOR: &str = "\x1b[1m\x1b[33m";
const MD_H3_COLOR: &str = "\x1b[1m\x1b[34m";
const MD_BOLD: &str = "\x1b[1m";
const MD_RESET: &str = "\x1b[0m";

pub const MAX_BUFFERED_LINE_CHARS: usize = 64 * 1024;

pub struct MarkdownRenderer {
    line_buffer: String,
    in_code_block: bool,
    in_think_block: bool,
    color_enabled: bool,
    utf8: bool,
}

impl MarkdownRenderer {
    pub fn new(color_enabled: bool, utf8: bool) -> Self {
        Self {
            line_buffer: String::new(),
            in_code_block: false,
            in_think_block: false,
            color_enabled,
            utf8,
        }
    }

    pub fn push_chunk(&mut self, chunk: &str) -> String {
        let mut out = String::new();
        if self.line_buffer.chars().count() + chunk.chars().count() > MAX_BUFFERED_LINE_CHARS {
            self.force_drain_buffer(&mut out);
        }
        self.line_buffer.push_str(chunk);
        while let Some(idx) = self.line_buffer.find('\n') {
            let line: String = self.line_buffer.drain(..=idx).collect();
            let body = line.strip_suffix('\n').unwrap_or(&line);
            self.process_line(body, true, &mut out);
        }
        if self.line_buffer.chars().count() > MAX_BUFFERED_LINE_CHARS {
            self.force_drain_buffer(&mut out);
        }
        out
    }

    pub fn flush(&mut self) -> String {
        let mut out = String::new();
        if !self.line_buffer.is_empty() {
            let body = std::mem::take(&mut self.line_buffer);
            self.process_line(&body, false, &mut out);
        }
        self.line_buffer.clear();
        self.in_code_block = false;
        self.in_think_block = false;
        out
    }

    fn force_drain_buffer(&mut self, out: &mut String) {
        const SUFFIX_RETAIN: usize = 16;
        let len = self.line_buffer.len();
        let mut split = len.saturating_sub(SUFFIX_RETAIN);
        while split < len && !self.line_buffer.is_char_boundary(split) {
            split += 1;
        }
        let suffix = self.line_buffer.split_off(split);
        let body = std::mem::take(&mut self.line_buffer);
        self.line_buffer = suffix;
        let visible = self.strip_think(&body);
        out.push_str(&sanitize(&visible));
        out.push('\n');
    }

    fn process_line(&mut self, line: &str, append_newline: bool, out: &mut String) {
        let visible = self.strip_think(line);
        if visible.is_empty() && (self.in_think_block || contained_think_only(line)) {
            return;
        }
        if visible.trim_start().starts_with("```") {
            self.in_code_block = !self.in_code_block;
            return;
        }
        if self.in_code_block {
            out.push_str("  ");
            if self.color_enabled {
                out.push_str(MD_CODE_FENCE_COLOR);
            }
            out.push_str(&sanitize(&visible));
            if self.color_enabled {
                out.push_str(MD_RESET);
            }
            if append_newline {
                out.push('\n');
            }
            return;
        }
        out.push_str(&render_line(&visible, self.color_enabled, self.utf8));
        if append_newline {
            out.push('\n');
        }
    }

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
                    None => return out,
                }
            }
            match remaining.find("<think>") {
                Some(idx) => {
                    out.push_str(&remaining[..idx]);
                    self.in_think_block = true;
                    remaining = &remaining[idx + "<think>".len()..];
                }
                None => {
                    out.push_str(remaining);
                    return out;
                }
            }
        }
    }
}

fn contained_think_only(line: &str) -> bool {
    let stripped = line.trim();
    stripped.starts_with("<think>") && stripped.ends_with("</think>")
}

pub fn render_line(line: &str, color_enabled: bool, utf8: bool) -> String {
    if let Some(rest) = line.strip_prefix("### ") {
        return wrap_heading(rest, MD_H3_COLOR, color_enabled);
    }
    if let Some(rest) = line.strip_prefix("## ") {
        return wrap_heading(rest, MD_H2_COLOR, color_enabled);
    }
    if let Some(rest) = line.strip_prefix("# ") {
        return wrap_heading(rest, MD_H1_COLOR, color_enabled);
    }
    if let Some(rest) = line.strip_prefix("- ") {
        let bullet = if utf8 { "● " } else { "* " };
        return format!("{bullet}{}", render_inline(rest, color_enabled));
    }
    render_inline(line, color_enabled)
}

fn wrap_heading(body: &str, color: &str, color_enabled: bool) -> String {
    let inline = render_inline(body, color_enabled);
    if color_enabled {
        format!("{color}{inline}{MD_RESET}")
    } else {
        inline
    }
}

fn render_inline(text: &str, color_enabled: bool) -> String {
    let mut out = String::with_capacity(text.len());
    let bytes = text.as_bytes();
    let mut i = 0;
    let mut literal_start = 0;
    while i < bytes.len() {
        if i + 1 < bytes.len()
            && bytes[i] == b'*'
            && bytes[i + 1] == b'*'
            && let Some(end_rel) = find_subslice(&bytes[i + 2..], b"**")
        {
            if literal_start < i {
                out.push_str(&sanitize(&text[literal_start..i]));
            }
            let inner = sanitize(&text[i + 2..i + 2 + end_rel]);
            if color_enabled {
                out.push_str(MD_BOLD);
                out.push_str(&inner);
                out.push_str(MD_RESET);
            } else {
                out.push_str(&inner);
            }
            i += 2 + end_rel + 2;
            literal_start = i;
            continue;
        }
        if bytes[i] == b'`'
            && let Some(end_rel) = find_byte(&bytes[i + 1..], b'`')
        {
            if literal_start < i {
                out.push_str(&sanitize(&text[literal_start..i]));
            }
            let inner = sanitize(&text[i + 1..i + 1 + end_rel]);
            if color_enabled {
                out.push_str(MD_INLINE_CODE_COLOR);
                out.push_str(&inner);
                out.push_str(MD_RESET);
            } else {
                out.push_str(&inner);
            }
            i += 1 + end_rel + 1;
            literal_start = i;
            continue;
        }
        i = next_char_boundary(text, i);
    }
    if literal_start < bytes.len() {
        out.push_str(&sanitize(&text[literal_start..]));
    }
    out
}

fn find_subslice(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || needle.len() > hay.len() {
        return None;
    }
    (0..=hay.len() - needle.len()).find(|&start| &hay[start..start + needle.len()] == needle)
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

pub fn sanitize(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        let cp = ch as u32;
        let is_c0 = cp < 0x20 && ch != '\t' && ch != '\n';
        let is_del = cp == 0x7F;
        let is_c1 = (0x80..=0x9F).contains(&cp);
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

pub struct TerminalMarkdownRenderer {
    color_enabled: bool,
    utf8: bool,
}

impl TerminalMarkdownRenderer {
    pub fn new(color_enabled: bool, utf8: bool) -> Self {
        Self {
            color_enabled,
            utf8,
        }
    }

    pub fn for_stdout() -> Self {
        let disabled = crate::tui::terminal::env_non_empty("ANVIL_NO_MARKDOWN");
        let color_enabled =
            !disabled && !crate::tui::terminal::no_color() && io::stdout().is_terminal();
        let utf8 = crate::tui::terminal::utf8_locale();
        Self::new(color_enabled, utf8)
    }

    pub fn render_to_string(&self, raw_text: &str) -> String {
        if crate::tui::terminal::env_non_empty("ANVIL_NO_MARKDOWN") {
            return raw_text.to_string();
        }
        let mut renderer = MarkdownRenderer::new(self.color_enabled, self.utf8);
        let mut out = renderer.push_chunk(raw_text);
        out.push_str(&renderer.flush());
        out
    }
}

impl OutputRenderer for TerminalMarkdownRenderer {
    fn render_assistant(&self, raw_text: &str) -> anyhow::Result<()> {
        let rendered = self.render_to_string(raw_text);
        let mut stdout = io::stdout().lock();
        stdout.write_all(rendered.as_bytes())?;
        if !rendered.ends_with('\n') {
            stdout.write_all(b"\n")?;
        }
        stdout.flush()?;
        Ok(())
    }
}

#[derive(Debug, Clone, Copy)]
pub struct PlainRenderer;

impl OutputRenderer for PlainRenderer {
    fn render_assistant(&self, raw_text: &str) -> anyhow::Result<()> {
        println!("{raw_text}");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use regex::Regex;

    #[test]
    fn markdown_headings_and_inline_sgr() {
        let got = render_line("# hello **world** `code`", true, true);
        assert!(got.starts_with(MD_H1_COLOR));
        assert!(got.contains(MD_BOLD));
        assert!(got.contains(MD_INLINE_CODE_COLOR));
        assert!(!got.contains("**"));
        assert!(!got.contains('`'));
    }

    #[test]
    fn markdown_plain_strips_symbols() {
        assert_eq!(
            render_line("# hello **world** `code`", false, true),
            "hello world code"
        );
    }

    #[test]
    fn markdown_output_has_only_sgr_csi() {
        let samples = [
            render_line("# h1", true, true),
            render_line("## h2", true, true),
            render_line("### h3", true, true),
            render_line("- item", true, true),
            render_line("use **bold** and `code`", true, true),
        ];
        let csi_re = Regex::new(r"\x1b\[[0-9;]*([A-Za-z])").unwrap();
        for sample in samples {
            for cap in csi_re.captures_iter(&sample) {
                assert_eq!(&cap[1], "m", "non-SGR CSI detected in {sample:?}");
            }
        }
    }

    #[test]
    fn markdown_think_cross_chunk_is_hidden() {
        let mut r = MarkdownRenderer::new(true, true);
        let mut out = r.push_chunk("before <think>s");
        out.push_str(&r.push_chunk("ecret</think>after\n"));
        assert_eq!(out, "before after\n");
    }

    #[test]
    fn markdown_buffer_cap_forces_bounded_memory() {
        let mut r = MarkdownRenderer::new(true, true);
        let big = "a".repeat(MAX_BUFFERED_LINE_CHARS + 100);
        let mut out = r.push_chunk(&big);
        out.push_str(&r.flush());
        assert!(r.line_buffer.is_empty());
        assert_eq!(
            out.chars().filter(|&ch| ch == 'a').count(),
            MAX_BUFFERED_LINE_CHARS + 100
        );
    }

    #[test]
    fn markdown_sanitize_strips_bidi_and_controls() {
        assert_eq!(sanitize("x\x1b[31m"), "x?[31m");
        assert_eq!(sanitize("bidi\u{202E}x"), "bidi?x");
        assert_eq!(sanitize("caf\u{00E9}"), "caf\u{00E9}");
    }
}
