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

/// Compute the argument-summary budget for a progress line given the current
/// terminal width.
///
/// Subtracts the fixed chrome (`[iter N/M]  `, optional emoji, tool name, and
/// the two-space separator) plus 3 chars reserved for the `...` ellipsis that
/// [`truncate`] appends when the input exceeds the budget, then clamps the
/// result to `MIN_ARG_BUDGET`. When `cols` is `None`, the caller falls back to
/// the historical fixed budget.
pub(super) fn progress_available_width(
    cols: Option<u16>,
    tool_name: &str,
    iter_human: usize,
    max_iterations: usize,
    use_unicode: bool,
) -> usize {
    const DEFAULT_ARG_BUDGET: usize = 57;
    const MIN_ARG_BUDGET: usize = 20;
    const ELLIPSIS_RESERVE: usize = 3;

    let Some(cols) = cols else {
        return DEFAULT_ARG_BUDGET;
    };

    let iter_prefix = format!("[iter {iter_human}/{max_iterations}]  ");
    let emoji_width = if use_unicode {
        tool_emoji(tool_name).chars().count() + 1
    } else {
        0
    };
    let chrome = iter_prefix.len() + emoji_width + tool_name.chars().count() + 2;

    (cols as usize)
        .saturating_sub(chrome)
        .saturating_sub(ELLIPSIS_RESERVE)
        .max(MIN_ARG_BUDGET)
}

pub(super) fn progress_detail_budget(cols: Option<u16>, prefix: &str) -> usize {
    cols.map(|value| value as usize)
        .unwrap_or(96)
        .saturating_sub(prefix.chars().count())
        .max(24)
}

pub(super) fn format_progress_field(prefix: &str, value: &str, cols: Option<u16>) -> String {
    let budget = progress_detail_budget(cols, prefix);
    format!(
        "{prefix}{}",
        truncate(&sanitize_for_progress(value), budget)
    )
}

pub(super) fn is_utf8_locale(lang: &str) -> bool {
    let lower = lang.to_ascii_lowercase();
    lower
        .split(['.', '_', '@', ';', ',', ' '])
        .any(|t| t == "utf-8" || t == "utf8")
}

pub(crate) fn unicode_supported() -> bool {
    if std::env::var_os("ANVIL_NO_EMOJI").is_some_and(|v| !v.is_empty()) {
        return false;
    }
    for key in ["LC_ALL", "LC_CTYPE", "LANG"] {
        if let Ok(v) = std::env::var(key)
            && is_utf8_locale(&v)
        {
            return true;
        }
    }
    false
}

/// Returns true when the environment requests that color output be suppressed
/// (https://no-color.org/): `NO_COLOR` is set to any non-empty value.
pub(crate) fn no_color_requested() -> bool {
    std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty())
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

    #[test]
    fn progress_available_width_none_returns_default() {
        assert_eq!(progress_available_width(None, "Bash", 1, 12, false), 57);
    }

    #[test]
    fn progress_available_width_large_cols_returns_budget() {
        assert_eq!(
            progress_available_width(Some(200), "Bash", 1, 12, false),
            200 - "[iter 1/12]  ".len() - "Bash".len() - 2 - 3
        );
    }

    #[test]
    fn progress_available_width_small_cols_clamps_to_min() {
        assert_eq!(progress_available_width(Some(30), "Bash", 1, 12, false), 20);
    }

    #[test]
    fn progress_available_width_zero_cols_clamps_to_min() {
        assert_eq!(progress_available_width(Some(0), "Bash", 1, 12, false), 20);
    }

    #[test]
    fn progress_available_width_emoji_accounts_for_vs16() {
        assert_eq!(
            progress_available_width(Some(200), "Write", 1, 12, true),
            200 - "[iter 1/12]  ".len() - "✏️".chars().count() - 1 - "Write".len() - 2 - 3
        );
    }

    #[test]
    fn is_utf8_locale_table() {
        let true_cases = [
            "en_US.UTF-8",
            "en_US.utf-8",
            "C.UTF8",
            "C.utf8",
            "ja_JP.UTF-8@Modifier",
            "en_US.UTF-8;POSIX",
        ];
        let false_cases = [
            "",
            "C",
            "POSIX",
            "en_US.utf-800",
            "xutf8x",
            "utf-88",
            "en_US.ISO-8859-1",
        ];
        for c in true_cases {
            assert!(is_utf8_locale(c), "expected true for {c:?}");
        }
        for c in false_cases {
            assert!(!is_utf8_locale(c), "expected false for {c:?}");
        }
    }
}
