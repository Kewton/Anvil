//! Token estimation utilities shared across modules.
//!
//! Provides a unified `estimate_tokens` function that replaces the duplicated
//! per-module estimators with improved CJK and code-aware heuristics.

/// Fixed token count for a single image in provider requests.
pub const IMAGE_TOKENS: usize = 300;

/// Content kind for token estimation.
/// Callers map message role information to the appropriate kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentKind {
    /// Normal text (user input, assistant response, system prompt, etc.)
    Text,
    /// Code / tool output (Tool role messages, etc.)
    Code,
    /// Image content — fixed 300 tokens per image.
    Image,
}

impl ContentKind {
    /// Derive the appropriate `ContentKind` from a session message role.
    ///
    /// Tool messages typically contain code or structured output, so they
    /// use the `Code` ratio.  All other roles use the `Text` ratio.
    pub fn from_message_role(role: crate::session::MessageRole) -> Self {
        match role {
            crate::session::MessageRole::Tool => Self::Code,
            _ => Self::Text,
        }
    }
}

// ── Ratio constants ──────────────────────────────────────────────────
// These ratios approximate how many characters map to one LLM token.
// They are empirical averages measured against common tokenizers
// (cl100k_base / o200k_base) and intentionally err on the side of
// over-estimation so that context-window budgets stay safe.

/// Average chars-per-token for code / tool output.
/// Code has more punctuation and short identifiers, so the ratio is lower.
const CHARS_PER_TOKEN_CODE: f64 = 3.5;

/// Average chars-per-token for CJK text.
/// CJK ideographs are typically encoded as 1-2 tokens each.
const CHARS_PER_TOKEN_CJK: f64 = 1.5;

/// Average chars-per-token for Latin / other text.
/// English prose averages ~4 characters per token in common tokenizers.
const CHARS_PER_TOKEN_OTHER: usize = 4;

/// Estimate the number of tokens in `content` using a heuristic appropriate
/// for `kind`.
///
/// - `ContentKind::Code`: `ceil(chars / CHARS_PER_TOKEN_CODE)`, minimum 1.
/// - `ContentKind::Text`: 1-pass scan — CJK chars use
///   `ceil(count / CHARS_PER_TOKEN_CJK)`, other chars use
///   `ceil(count / CHARS_PER_TOKEN_OTHER)`, minimum 1.
pub fn estimate_tokens(content: &str, kind: ContentKind) -> usize {
    match kind {
        ContentKind::Image => IMAGE_TOKENS,
        ContentKind::Code => {
            let chars = content.chars().count();
            (chars as f64 / CHARS_PER_TOKEN_CODE).ceil().max(1.0) as usize
        }
        ContentKind::Text => {
            let mut cjk_count = 0usize;
            let mut other_count = 0usize;

            for ch in content.chars() {
                if is_cjk_character(ch) {
                    cjk_count += 1;
                } else {
                    other_count += 1;
                }
            }

            let cjk_tokens = (cjk_count as f64 / CHARS_PER_TOKEN_CJK).ceil() as usize;
            let other_tokens = other_count.div_ceil(CHARS_PER_TOKEN_OTHER);
            (cjk_tokens + other_tokens).max(1)
        }
    }
}

fn is_cjk_character(ch: char) -> bool {
    matches!(
        ch,
        '\u{4E00}'..='\u{9FFF}'   // CJK Unified Ideographs
        | '\u{3400}'..='\u{4DBF}' // CJK Unified Ideographs Extension A
        | '\u{3040}'..='\u{309F}' // Hiragana
        | '\u{30A0}'..='\u{30FF}' // Katakana
        | '\u{AC00}'..='\u{D7AF}' // Hangul Syllables
        | '\u{F900}'..='\u{FAFF}' // CJK Compatibility Ideographs
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn estimate_tokens_ascii() {
        // "hello world" = 11 chars, ceil(11/4) = 3
        let tokens = estimate_tokens("hello world", ContentKind::Text);
        assert_eq!(tokens, 3);
    }

    #[test]
    fn estimate_tokens_japanese() {
        // "こんにちは世界" = 7 CJK chars, ceil(7/1.5) = 5
        let tokens = estimate_tokens("こんにちは世界", ContentKind::Text);
        assert_eq!(tokens, 5);
    }

    #[test]
    fn estimate_tokens_mixed_ja_en() {
        // "Hello世界" = 5 ASCII + 2 CJK
        // ASCII: ceil(5/4) = 2, CJK: ceil(2/1.5) = 2 => total 4
        let tokens = estimate_tokens("Hello世界", ContentKind::Text);
        assert_eq!(tokens, 4);
    }

    #[test]
    fn estimate_tokens_code_kind() {
        // "fn main() {}" = 13 chars, ceil(13/3.5) = 4
        let tokens = estimate_tokens("fn main() {}", ContentKind::Code);
        assert_eq!(tokens, 4);
    }

    #[test]
    fn estimate_tokens_empty() {
        // Empty string should return minimum 1
        assert_eq!(estimate_tokens("", ContentKind::Text), 1);
        assert_eq!(estimate_tokens("", ContentKind::Code), 1);
    }

    #[test]
    fn estimate_tokens_image_returns_fixed_300() {
        // Image content always returns a fixed 300 tokens regardless of input
        assert_eq!(estimate_tokens("", ContentKind::Image), 300);
        assert_eq!(estimate_tokens("anything", ContentKind::Image), 300);
        assert_eq!(
            estimate_tokens("long content here", ContentKind::Image),
            300
        );
    }

    #[test]
    fn estimate_tokens_emoji() {
        // Emoji are not CJK, treated as other chars
        // "😀😁😂" = 3 chars, ceil(3/4) = 1
        let tokens = estimate_tokens("😀😁😂", ContentKind::Text);
        assert_eq!(tokens, 1);
    }
}
