//! Token estimation utilities shared across modules.
//!
//! Provides a unified `estimate_tokens` function that replaces the duplicated
//! per-module estimators with improved CJK and code-aware heuristics.

use std::collections::HashMap;

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

// ── Calibration constants ───────────────────────────────────────────

/// Default ratio when no calibration data is available.
pub const NO_CALIBRATION: f64 = 1.0;

/// EMA smoothing factor (0 < alpha < 1).
const EMA_ALPHA: f64 = 0.2;

/// Minimum samples before calibration ratio is applied.
const MIN_SAMPLES: usize = 5;

// ── TokenCalibrationStore ───────────────────────────────────────────

/// EMA state for a single model's token ratio.
#[derive(Debug, Clone)]
struct EmaState {
    /// Smoothed ratio (actual / estimated). Starts at 1.0.
    ratio: f64,
    /// Number of samples accumulated.
    sample_count: usize,
}

/// Accumulates actual vs estimated token ratios per model,
/// providing calibrated ratios via EMA.
#[derive(Debug, Clone, Default)]
pub struct TokenCalibrationStore {
    /// Model name -> EMA state
    entries: HashMap<String, EmaState>,
}

impl TokenCalibrationStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record an observation: actual prompt tokens vs estimated prompt tokens.
    /// Skips if estimated == 0 (zero-division guard).
    /// Ratio is clamped to [0.1, 10.0] to prevent divergence from malicious/buggy provider responses.
    pub fn update(&mut self, model: &str, actual: u64, estimated: usize) {
        if estimated == 0 {
            return;
        }
        let current_ratio = (actual as f64 / estimated as f64).clamp(0.1, 10.0);
        let entry = self.entries.entry(model.to_string()).or_insert(EmaState {
            ratio: current_ratio,
            sample_count: 0,
        });
        if entry.sample_count == 0 {
            entry.ratio = current_ratio;
        } else {
            entry.ratio = EMA_ALPHA * current_ratio + (1.0 - EMA_ALPHA) * entry.ratio;
        }
        entry.sample_count += 1;
    }

    /// Get the calibration ratio for a model.
    /// Returns 1.0 if fewer than MIN_SAMPLES have been recorded.
    pub fn get_ratio(&self, model: &str) -> f64 {
        self.entries
            .get(model)
            .filter(|e| e.sample_count >= MIN_SAMPLES)
            .map(|e| e.ratio)
            .unwrap_or(NO_CALIBRATION)
    }

    /// Return the number of samples recorded for a model (test inspection).
    #[cfg(test)]
    pub fn sample_count(&self, model: &str) -> usize {
        self.entries.get(model).map(|e| e.sample_count).unwrap_or(0)
    }

    /// Return the raw (unguarded) ratio for a model (test inspection).
    #[cfg(test)]
    pub fn raw_ratio(&self, model: &str) -> Option<f64> {
        self.entries.get(model).map(|e| e.ratio)
    }
}

/// Estimate tokens with calibration ratio applied.
///
/// `ratio` is obtained from `TokenCalibrationStore::get_ratio()`.
/// When ratio == 1.0 (no calibration data), this is equivalent to `estimate_tokens`.
pub fn estimate_tokens_calibrated(content: &str, kind: ContentKind, ratio: f64) -> usize {
    let base = estimate_tokens(content, kind);
    let result = base as f64 * ratio;
    // Guard against NaN/Infinity from malformed ratio
    if result.is_finite() {
        (result.ceil().max(1.0)) as usize
    } else {
        base
    }
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

    // ── TokenCalibrationStore tests ─────────────────────────────────

    #[test]
    fn ema_initial_sample() {
        let mut store = TokenCalibrationStore::new();
        store.update("model-a", 150, 100); // ratio = 1.5
        assert_eq!(store.sample_count("model-a"), 1);
        let raw = store.raw_ratio("model-a").unwrap();
        assert!(
            (raw - 1.5).abs() < f64::EPSILON,
            "first sample should initialize ratio"
        );
    }

    #[test]
    fn ema_convergence() {
        let mut store = TokenCalibrationStore::new();
        // Feed 20 samples with a fixed ratio of 1.5
        for _ in 0..20 {
            store.update("model-a", 150, 100);
        }
        let ratio = store.get_ratio("model-a");
        let mape = ((ratio - 1.5) / 1.5).abs();
        assert!(
            mape < 0.10,
            "EMA should converge within 10% MAPE, got ratio={ratio}"
        );
    }

    #[test]
    fn min_samples_guard() {
        let mut store = TokenCalibrationStore::new();
        for _ in 0..4 {
            store.update("model-a", 200, 100);
        }
        assert_eq!(store.sample_count("model-a"), 4);
        assert!(
            (store.get_ratio("model-a") - 1.0).abs() < f64::EPSILON,
            "fewer than 5 samples should return 1.0"
        );
    }

    #[test]
    fn min_samples_threshold() {
        let mut store = TokenCalibrationStore::new();
        for _ in 0..5 {
            store.update("model-a", 200, 100); // ratio = 2.0
        }
        assert_eq!(store.sample_count("model-a"), 5);
        let ratio = store.get_ratio("model-a");
        assert!(
            ratio > 1.0,
            "5+ samples should return calibrated ratio, got {ratio}"
        );
    }

    #[test]
    fn zero_estimated_guard() {
        let mut store = TokenCalibrationStore::new();
        store.update("model-a", 100, 0); // should skip
        assert_eq!(store.sample_count("model-a"), 0);
    }

    #[test]
    fn model_isolation() {
        let mut store = TokenCalibrationStore::new();
        for _ in 0..10 {
            store.update("model-a", 150, 100); // ratio = 1.5
            store.update("model-b", 300, 100); // ratio = 3.0
        }
        let ratio_a = store.get_ratio("model-a");
        let ratio_b = store.get_ratio("model-b");
        assert!(
            (ratio_a - ratio_b).abs() > 0.5,
            "models should have independent ratios"
        );
    }

    #[test]
    fn ratio_clamp() {
        let mut store = TokenCalibrationStore::new();
        // Extreme high ratio: actual=10000, estimated=1 => clamped to 10.0
        store.update("model-a", 10000, 1);
        let raw = store.raw_ratio("model-a").unwrap();
        assert!(
            (raw - 10.0).abs() < f64::EPSILON,
            "ratio should be clamped to 10.0"
        );

        // Extreme low ratio: actual=1, estimated=10000 => clamped to 0.1
        let mut store2 = TokenCalibrationStore::new();
        store2.update("model-b", 1, 10000);
        let raw2 = store2.raw_ratio("model-b").unwrap();
        assert!(
            (raw2 - 0.1).abs() < f64::EPSILON,
            "ratio should be clamped to 0.1"
        );
    }

    // ── estimate_tokens_calibrated tests ────────────────────────────

    #[test]
    fn calibrated_ratio_1_0() {
        let base = estimate_tokens("hello world", ContentKind::Text);
        let calibrated = estimate_tokens_calibrated("hello world", ContentKind::Text, 1.0);
        assert_eq!(base, calibrated, "ratio=1.0 should match estimate_tokens");
    }

    #[test]
    fn calibrated_applies_ratio() {
        let base = estimate_tokens("hello world", ContentKind::Text); // 3
        let calibrated = estimate_tokens_calibrated("hello world", ContentKind::Text, 2.0);
        assert_eq!(
            calibrated, 6,
            "ratio=2.0 should double the estimate (3*2=6)"
        );
        assert_eq!(base * 2, calibrated);
    }

    #[test]
    fn calibrated_nan_guard() {
        let base = estimate_tokens("hello world", ContentKind::Text);
        let calibrated = estimate_tokens_calibrated("hello world", ContentKind::Text, f64::NAN);
        assert_eq!(calibrated, base, "NaN ratio should fall back to base");
    }

    #[test]
    fn calibrated_infinity_guard() {
        let base = estimate_tokens("hello world", ContentKind::Text);
        let calibrated =
            estimate_tokens_calibrated("hello world", ContentKind::Text, f64::INFINITY);
        assert_eq!(calibrated, base, "Infinity ratio should fall back to base");
    }
}
