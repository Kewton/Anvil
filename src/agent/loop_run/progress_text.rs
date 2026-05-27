/// UTF-8-safe truncation: keeps at most `max` characters and appends `...`
/// when the input was longer. Never splits a multi-byte code point.
pub(super) fn truncate(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((byte_idx, _)) => {
            let mut out = String::with_capacity(byte_idx + 3);
            out.push_str(&s[..byte_idx]);
            out.push_str("...");
            out
        }
        None => s.to_string(),
    }
}

/// Replace control characters (C0, DEL, and C1) with spaces, then trim
/// trailing whitespace. Required for model-derived text so that newlines or
/// ANSI escape sequences cannot be injected into the terminal. C1
/// (`U+0080..=U+009F`) is included because some terminals interpret 8-bit CSI
/// (`U+009B`) and OSC (`U+009D`) equivalently to `ESC [` and `ESC ]`.
pub(super) fn sanitize_for_progress(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        let cp = ch as u32;
        if cp < 0x20 || cp == 0x7F || (0x80..=0x9F).contains(&cp) {
            out.push(' ');
        } else {
            out.push(ch);
        }
    }
    out.trim_end().to_string()
}

const COLOR_RESET: &str = "\x1b[0m";

pub(super) fn tool_color(tool_name: &str) -> &'static str {
    match tool_name {
        "Write" => "\x1b[38;5;198m",
        "Read" => "\x1b[38;5;87m",
        "Edit" => "\x1b[38;5;208m",
        "Bash" => "\x1b[38;5;226m",
        "Glob" => "\x1b[38;5;51m",
        "Grep" => "\x1b[38;5;39m",
        _ => "\x1b[38;5;245m",
    }
}

pub(super) fn tool_emoji(tool_name: &str) -> &'static str {
    match tool_name {
        "Write" => "✏️",
        "Read" => "📄",
        "Edit" => "📝",
        "Bash" => "⚡",
        "Glob" => "🔍",
        "Grep" => "🔎",
        _ => "🔧",
    }
}

pub(super) fn paint(s: &str, color: &str, use_color: bool) -> String {
    if use_color && !color.is_empty() {
        format!("{color}{s}{COLOR_RESET}")
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_short_strings_verbatim() {
        assert_eq!(truncate("hello", 10), "hello");
        assert_eq!(truncate("exact", 5), "exact");
    }

    #[test]
    fn truncates_long_strings_with_ellipsis() {
        assert_eq!(truncate("abcdefgh", 3), "abc...");
    }

    #[test]
    fn never_splits_multibyte_code_points() {
        assert_eq!(truncate("あいうえお", 2), "あい...");
    }

    #[test]
    fn sanitize_removes_newline() {
        assert_eq!(sanitize_for_progress("hello\nworld"), "hello world");
    }

    #[test]
    fn sanitize_removes_escape() {
        assert_eq!(sanitize_for_progress("red\x1b[31m!"), "red [31m!");
    }

    #[test]
    fn sanitize_passthrough_normal() {
        assert_eq!(sanitize_for_progress("hello world"), "hello world");
    }

    #[test]
    fn tool_style_all_mappings() {
        let cases: &[(&str, &str, &str)] = &[
            ("Write", "\x1b[38;5;198m", "✏\u{fe0f}"),
            ("Read", "\x1b[38;5;87m", "📄"),
            ("Edit", "\x1b[38;5;208m", "📝"),
            ("Bash", "\x1b[38;5;226m", "⚡"),
            ("Glob", "\x1b[38;5;51m", "🔍"),
            ("Grep", "\x1b[38;5;39m", "🔎"),
            ("Unknown", "\x1b[38;5;245m", "🔧"),
        ];
        for (name, expected_color, expected_emoji) in cases {
            assert_eq!(tool_color(name), *expected_color, "color for {name}");
            assert_eq!(tool_emoji(name), *expected_emoji, "emoji for {name}");
        }
    }

    #[test]
    fn paint_wraps_with_reset_when_enabled() {
        assert_eq!(
            paint("Bash", tool_color("Bash"), true),
            "\x1b[38;5;226mBash\x1b[0m"
        );
        assert_eq!(paint("Bash", tool_color("Bash"), false), "Bash");
    }
}
