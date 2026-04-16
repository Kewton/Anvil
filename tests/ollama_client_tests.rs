use anvil::ollama::client::{
    parse_chat_response, parse_tags_response, should_use_native_tool_calls,
};

#[test]
fn parses_tags_and_chat_payloads() {
    let models =
        parse_tags_response(r#"{"models":[{"name":"qwen3:8b"},{"name":"qwen3:1.7b"}]}"#).unwrap();
    assert_eq!(
        models,
        vec!["qwen3:8b".to_string(), "qwen3:1.7b".to_string()]
    );

    let reply = parse_chat_response(
        r#"{"message":{"content":"<tool_call>{\"name\":\"Read\",\"arguments\":{\"path\":\"README.md\"}}</tool_call>done","tool_calls":[]}}"#,
        &["Read".to_string()],
    )
    .unwrap();
    assert_eq!(reply.tool_calls.len(), 1);
    assert_eq!(reply.tool_calls[0].name, "Read");
    assert_eq!(reply.content, "done");
}

#[test]
fn disables_native_tools_for_qwen3_family() {
    assert!(!should_use_native_tool_calls("qwen3.5:122b"));
    assert!(!should_use_native_tool_calls("qwen3:8b"));
    assert!(should_use_native_tool_calls("gemma4:31b"));
}
