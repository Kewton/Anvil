//! Model size classification for tool protocol mode selection.
//!
//! Determines whether a model should use JSON or tag-based tool protocol
//! based on configuration overrides and model name heuristics.

use regex::Regex;

/// Tool protocol mode for agent-model communication.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolProtocolMode {
    /// JSON format (default, for large models).
    Json,
    /// Tag-based format (for small models, <= 13B).
    TagBased,
}

/// Determine the tool protocol mode from config override and model name.
///
/// Priority: `config_tag_protocol` flag > model name heuristic.
pub fn determine_protocol_mode(
    model_name: &str,
    config_tag_protocol: Option<bool>,
) -> ToolProtocolMode {
    if let Some(flag) = config_tag_protocol {
        return if flag {
            ToolProtocolMode::TagBased
        } else {
            ToolProtocolMode::Json
        };
    }
    classify_model_size(model_name)
}

/// Small-model family names that default to tag-based protocol.
const SMALL_MODEL_FAMILIES: &[&str] = &[
    "gemma",
    "phi",
    "qwen2",
    "stablelm",
    "tinyllama",
    "codegemma",
];

/// Classify model size from its name and return the appropriate protocol mode.
///
/// 1. Extract parameter count from `:NNb` suffix — <= 13 means TagBased.
/// 2. Fall back to family name dictionary.
/// 3. Unknown models default to Json (safe side).
fn classify_model_size(model_name: &str) -> ToolProtocolMode {
    // Try to extract parameter count from suffix like ":8b", ":70b"
    let re = Regex::new(r":([0-9]+)[bB]").expect("valid regex");
    if let Some(caps) = re.captures(model_name)
        && let Ok(size) = caps[1].parse::<u64>()
    {
        return if size <= 13 {
            ToolProtocolMode::TagBased
        } else {
            ToolProtocolMode::Json
        };
    }

    // Check model family name dictionary
    let lower = model_name.to_lowercase();
    for family in SMALL_MODEL_FAMILIES {
        if lower.contains(family) {
            return ToolProtocolMode::TagBased;
        }
    }

    // Unknown model — default to Json (safe side)
    ToolProtocolMode::Json
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regex_extracts_size() {
        let re = Regex::new(r":([0-9]+)[bB]").unwrap();
        let caps = re.captures("llama3:8b").unwrap();
        assert_eq!(&caps[1], "8");
    }

    #[test]
    fn small_families_recognized() {
        assert_eq!(classify_model_size("gemma2"), ToolProtocolMode::TagBased);
        assert_eq!(classify_model_size("phi3"), ToolProtocolMode::TagBased);
        assert_eq!(
            classify_model_size("qwen2-instruct"),
            ToolProtocolMode::TagBased
        );
    }
}
