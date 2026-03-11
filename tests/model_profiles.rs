use anvil::config::model_profiles::{ModelProfile, profile_for_model};

#[test]
fn profile_for_qwen35_35b_prefers_large_context_local_defaults() {
    let profile = profile_for_model("qwen3.5:35b");

    assert_eq!(profile.name, "qwen3.5:35b");
    assert!(profile.max_context_tokens >= 200_000);
    assert!(profile.summary_trigger_tokens >= 40_000);
    assert!(profile.tool_context_tokens >= 48_000);
    assert!(profile.tool_temperature <= 0.2);
}

#[test]
fn unknown_model_uses_safe_default_profile() {
    let profile = profile_for_model("unknown");

    assert_eq!(profile.name, "unknown");
    assert_eq!(profile, ModelProfile::default_for("unknown"));
}
