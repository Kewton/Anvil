use anvil::config::{PartialConfig, merge_partial_configs, parse_key_value_config};

#[test]
fn parses_key_value_config() {
    let map = parse_key_value_config(
        r#"
        model = qwen3:8b
        sidecar-model = qwen3:1.7b
        # comment
        yes_mode = true
        "#,
    );
    assert_eq!(map.get("model").unwrap(), "qwen3:8b");
    assert_eq!(map.get("sidecar_model").unwrap(), "qwen3:1.7b");
    assert_eq!(map.get("yes_mode").unwrap(), "true");
}

#[test]
fn merge_prefers_later_sources() {
    let merged = merge_partial_configs(&[
        PartialConfig {
            model: Some("file-model".into()),
            yes_mode: Some(false),
            chat_timeout_secs: Some(120),
            ..PartialConfig::default()
        },
        PartialConfig {
            model: Some("env-model".into()),
            chat_timeout_secs: Some(240),
            ..PartialConfig::default()
        },
        PartialConfig {
            model: Some("cli-model".into()),
            yes_mode: Some(true),
            chat_retries: Some(3),
            ..PartialConfig::default()
        },
    ]);
    assert_eq!(merged.model.as_deref(), Some("cli-model"));
    assert_eq!(merged.yes_mode, Some(true));
    assert_eq!(merged.chat_timeout_secs, Some(240));
    assert_eq!(merged.chat_retries, Some(3));
}
