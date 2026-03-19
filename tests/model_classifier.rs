use anvil::agent::{ToolProtocolMode, model_classifier::determine_protocol_mode};

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
