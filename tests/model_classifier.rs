use anvil::agent::{
    ModelSizeClass, PromptTier, ToolProtocolMode,
    model_classifier::{classify_model_capability, determine_protocol_mode},
};

#[test]
fn config_true_forces_tag_based() {
    assert_eq!(
        determine_protocol_mode("any-model", Some(true)),
        ToolProtocolMode::TagBased,
    );
}

#[test]
fn config_false_forces_json() {
    assert_eq!(
        determine_protocol_mode("any-model", Some(false)),
        ToolProtocolMode::Json,
    );
}

#[test]
fn small_model_8b_returns_tag_based() {
    assert_eq!(
        determine_protocol_mode("llama3:8b", None),
        ToolProtocolMode::TagBased,
    );
}

#[test]
fn large_model_70b_returns_json() {
    assert_eq!(
        determine_protocol_mode("llama3:70b", None),
        ToolProtocolMode::Json,
    );
}

#[test]
fn boundary_13b_returns_tag_based() {
    assert_eq!(
        determine_protocol_mode("llama3:13b", None),
        ToolProtocolMode::TagBased,
    );
}

#[test]
fn boundary_14b_returns_json() {
    assert_eq!(
        determine_protocol_mode("llama3:14b", None),
        ToolProtocolMode::Json,
    );
}

#[test]
fn family_gemma_returns_tag_based() {
    assert_eq!(
        determine_protocol_mode("gemma2", None),
        ToolProtocolMode::TagBased,
    );
}

#[test]
fn unknown_model_returns_json() {
    assert_eq!(
        determine_protocol_mode("unknown-model", None),
        ToolProtocolMode::Json,
    );
}

// --- classify_model_capability tests ---

#[test]
fn capability_small_model_3b_returns_tiny() {
    let cap = classify_model_capability("phi3:3b", None, None);
    assert_eq!(cap.size_class, ModelSizeClass::Small);
    assert_eq!(cap.prompt_tier, PromptTier::Tiny);
    assert_eq!(cap.protocol_mode, ToolProtocolMode::TagBased);
}

#[test]
fn capability_medium_model_8b_returns_compact() {
    let cap = classify_model_capability("llama3:8b", None, None);
    assert_eq!(cap.size_class, ModelSizeClass::Medium);
    assert_eq!(cap.prompt_tier, PromptTier::Compact);
    assert_eq!(cap.protocol_mode, ToolProtocolMode::TagBased);
}

#[test]
fn capability_large_model_70b_returns_full() {
    let cap = classify_model_capability("llama3:70b", None, None);
    assert_eq!(cap.size_class, ModelSizeClass::Large);
    assert_eq!(cap.prompt_tier, PromptTier::Full);
    assert_eq!(cap.protocol_mode, ToolProtocolMode::Json);
}

#[test]
fn capability_unknown_model_defaults_to_full() {
    let cap = classify_model_capability("unknown-model", None, None);
    assert_eq!(cap.size_class, ModelSizeClass::Large);
    assert_eq!(cap.prompt_tier, PromptTier::Full);
}

#[test]
fn capability_config_tier_overrides_auto() {
    // Force tiny tier on a large model
    let cap = classify_model_capability("llama3:70b", None, Some("tiny"));
    assert_eq!(cap.prompt_tier, PromptTier::Tiny);
    // Size class still reflects actual model
    assert_eq!(cap.size_class, ModelSizeClass::Large);
}

#[test]
fn capability_config_tier_compact() {
    let cap = classify_model_capability("llama3:70b", None, Some("compact"));
    assert_eq!(cap.prompt_tier, PromptTier::Compact);
}

#[test]
fn capability_config_tier_full_on_small_model() {
    let cap = classify_model_capability("phi3:3b", None, Some("full"));
    assert_eq!(cap.prompt_tier, PromptTier::Full);
    assert_eq!(cap.size_class, ModelSizeClass::Small);
}

#[test]
fn capability_invalid_tier_falls_back_to_auto() {
    let cap = classify_model_capability("llama3:8b", None, Some("invalid"));
    // Should fall back to auto-detect (Medium -> Compact)
    assert_eq!(cap.prompt_tier, PromptTier::Compact);
}

#[test]
fn capability_boundary_7b_is_medium() {
    let cap = classify_model_capability("model:7b", None, None);
    assert_eq!(cap.size_class, ModelSizeClass::Medium);
    assert_eq!(cap.prompt_tier, PromptTier::Compact);
}

#[test]
fn capability_boundary_13b_is_medium() {
    let cap = classify_model_capability("model:13b", None, None);
    assert_eq!(cap.size_class, ModelSizeClass::Medium);
    assert_eq!(cap.prompt_tier, PromptTier::Compact);
}

#[test]
fn capability_boundary_14b_is_large() {
    let cap = classify_model_capability("model:14b", None, None);
    assert_eq!(cap.size_class, ModelSizeClass::Large);
    assert_eq!(cap.prompt_tier, PromptTier::Full);
}

#[test]
fn capability_family_gemma_is_medium() {
    let cap = classify_model_capability("gemma2", None, None);
    assert_eq!(cap.size_class, ModelSizeClass::Medium);
    assert_eq!(cap.prompt_tier, PromptTier::Compact);
}

#[test]
fn capability_tag_protocol_override_preserved() {
    let cap = classify_model_capability("llama3:70b", Some(true), None);
    assert_eq!(cap.protocol_mode, ToolProtocolMode::TagBased);
    assert_eq!(cap.prompt_tier, PromptTier::Full);
}

#[test]
fn prompt_tier_default_is_full() {
    assert_eq!(PromptTier::default(), PromptTier::Full);
}
