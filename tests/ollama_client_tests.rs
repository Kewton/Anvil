use anvil::ollama::client::{
    parse_chat_response, parse_generate_response, parse_tags_response, should_use_native_tool_calls,
};

#[test]
fn parses_tags_and_generate_payloads() {
    let tags = parse_tags_response(
        "{\"models\":[{\"name\":\"qwen3:8b\",\"details\":{}},{\"name\":\"gemma4:31b\",\"details\":{}}]}",
    )
    .unwrap();
    assert_eq!(tags, vec!["qwen3:8b", "gemma4:31b"]);

    let body = "{\"response\":\"Here you go.<anvil_tool_call>{\\\"name\\\":\\\"Read\\\",\\\"arguments\\\":{\\\"path\\\":\\\"README.md\\\"}}</anvil_tool_call>\"}";
    let reply = parse_generate_response(body, &["Read".to_string(), "Write".to_string()]).unwrap();
    assert_eq!(reply.tool_calls.len(), 1);
    assert_eq!(reply.tool_calls[0].name, "Read");
    assert_eq!(reply.tool_calls[0].arguments["path"], "README.md");
    assert!(reply.content.starts_with("Here you go."));
}

#[test]
fn native_tools_are_disabled_for_all_models() {
    assert!(should_use_native_tool_calls("qwen3.5:122b"));
    assert!(should_use_native_tool_calls("qwen3.6:27b-coding-nvfp4"));
    assert!(!should_use_native_tool_calls("qwen3:8b"));
    assert!(!should_use_native_tool_calls("gemma4:31b"));
    assert!(!should_use_native_tool_calls(""));
}

#[test]
fn parses_native_chat_tool_calls() {
    let body = r#"{
      "message": {
        "content": "",
        "tool_calls": [
          {
            "id": "call_write",
            "function": {
              "name": "Write",
              "arguments": {
                "path": "hello.txt",
                "content": "HELLO"
              }
            }
          }
        ]
      }
    }"#;
    let reply = parse_chat_response(body, &["Write".to_string()]).unwrap();
    assert_eq!(reply.tool_calls.len(), 1);
    assert_eq!(reply.tool_calls[0].name, "Write");
    assert_eq!(reply.tool_calls[0].arguments["content"], "HELLO");
}
