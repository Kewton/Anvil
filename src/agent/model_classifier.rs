//! Model size classification for tool protocol mode and prompt tier selection.
//!
//! Determines whether a model should use JSON or tag-based tool protocol
//! and which prompt tier (Full/Compact/Tiny) to use, based on configuration
//! overrides and model name heuristics.

use regex::Regex;

/// Tool protocol mode for agent-model communication.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ToolProtocolMode {
    /// JSON format (default, for large models).
    #[default]
    Json,
    /// Tag-based format (for small models, <= 13B).
    TagBased,
}

/// System prompt verbosity tier.
///
/// Controls how much of the system prompt is included, allowing smaller
/// models to receive a more concise prompt that fits their context window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PromptTier {
    /// Full prompt with all sections (default, for large models >13B).
    #[default]
    Full,
    /// Compact prompt: basic tools + rules, guides omitted (for 7B-13B models).
    Compact,
    /// Tiny prompt: minimal tool syntax only (for <7B models).
    Tiny,
}

/// Model size classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelSizeClass {
    /// Small model (<7B parameters).
    Small,
    /// Medium model (7B-13B parameters).
    Medium,
    /// Large model (>13B parameters or unknown).
    Large,
}

/// Combined model capability assessment.
///
/// Produced by [`classify_model_capability`] and used to drive prompt tier
/// selection and protocol mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelCapability {
    pub size_class: ModelSizeClass,
    pub protocol_mode: ToolProtocolMode,
    pub prompt_tier: PromptTier,
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

/// Classify a model's capabilities from its name and config overrides.
///
/// Priority: `config_prompt_tier` > model name heuristic.
/// The `config_tag_protocol` override is forwarded to [`determine_protocol_mode`].
pub fn classify_model_capability(
    model_name: &str,
    config_tag_protocol: Option<bool>,
    config_prompt_tier: Option<&str>,
) -> ModelCapability {
    let protocol_mode = determine_protocol_mode(model_name, config_tag_protocol);

    // Config override for prompt tier takes priority
    if let Some(tier_str) = config_prompt_tier
        && let Some(tier) = parse_prompt_tier(tier_str)
    {
        return ModelCapability {
            size_class: classify_size_class(model_name),
            protocol_mode,
            prompt_tier: tier,
        };
    }

    // Auto-detect from model name
    let size_class = classify_size_class(model_name);
    let prompt_tier = match size_class {
        ModelSizeClass::Small => PromptTier::Tiny,
        ModelSizeClass::Medium => PromptTier::Compact,
        ModelSizeClass::Large => PromptTier::Full,
    };

    ModelCapability {
        size_class,
        protocol_mode,
        prompt_tier,
    }
}

/// Parse a prompt tier string into a [`PromptTier`].
///
/// Accepts "full", "compact", "tiny" (case-insensitive). Returns `None` for
/// unrecognised values (caller falls back to auto-detect).
pub fn parse_prompt_tier(s: &str) -> Option<PromptTier> {
    match s.to_lowercase().as_str() {
        "full" => Some(PromptTier::Full),
        "compact" => Some(PromptTier::Compact),
        "tiny" => Some(PromptTier::Tiny),
        _ => None,
    }
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

/// Classify model size class from its name.
///
/// 1. Extract parameter count from `:NNb` suffix.
/// 2. Fall back to family name dictionary.
/// 3. Unknown models default to Large (safe side).
fn classify_size_class(model_name: &str) -> ModelSizeClass {
    let re = Regex::new(r":([0-9]+)[bB]").expect("valid regex");
    if let Some(caps) = re.captures(model_name)
        && let Ok(size) = caps[1].parse::<u64>()
    {
        return match size {
            0..7 => ModelSizeClass::Small,
            7..=13 => ModelSizeClass::Medium,
            _ => ModelSizeClass::Large,
        };
    }

    // Check model family name dictionary — these are all small/medium
    let lower = model_name.to_lowercase();
    for family in SMALL_MODEL_FAMILIES {
        if lower.contains(family) {
            return ModelSizeClass::Medium;
        }
    }

    // Unknown model — default to Large (safe side)
    ModelSizeClass::Large
}

/// Classify model size from its name and return the appropriate protocol mode.
///
/// Delegates to [`classify_size_class`] for the size determination, then maps
/// Small/Medium → TagBased, Large → Json.
fn classify_model_size(model_name: &str) -> ToolProtocolMode {
    match classify_size_class(model_name) {
        ModelSizeClass::Small | ModelSizeClass::Medium => ToolProtocolMode::TagBased,
        ModelSizeClass::Large => ToolProtocolMode::Json,
    }
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
