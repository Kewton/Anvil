use anvil::logging::{LLM_IO_PROMPT_SCHEMA_VERSION, build_prompt_log_metrics};
use anvil::session::store::ConversationMessage;

#[test]
fn prompt_metrics_schema_is_stable() {
    let messages = vec![
        ConversationMessage::system("You are Anvil.".to_string()),
        ConversationMessage::user("Inspect src/lib.rs".to_string()),
        ConversationMessage::tool("Read".to_string(), "pub fn run() {}".to_string()),
    ];
    let final_prompt = "<|im_start|>system\nYou are Anvil.<|im_end|>\n".to_string();

    let metrics = build_prompt_log_metrics(&messages, final_prompt.clone());
    let value = serde_json::to_value(&metrics).expect("metrics should serialize");

    assert_eq!(value["schema_version"], LLM_IO_PROMPT_SCHEMA_VERSION);
    assert_eq!(value["final_prompt"], final_prompt);
    assert_eq!(value["prompt_char_count"], final_prompt.chars().count());
    assert!(value["approx_prompt_tokens"].as_u64().unwrap() > 0);

    let blocks = value["injection_blocks"].as_array().unwrap();
    assert_eq!(blocks.len(), 3);

    assert_eq!(blocks[0]["index"], 0);
    assert_eq!(blocks[0]["kind"], "system_prompt");
    assert_eq!(blocks[0]["role"], "system");
    assert_eq!(blocks[0]["name"], serde_json::Value::Null);
    assert_eq!(blocks[0]["tool_call_count"], 0);

    assert_eq!(blocks[1]["kind"], "user_message");
    assert_eq!(blocks[1]["role"], "user");

    assert_eq!(blocks[2]["kind"], "tool_result");
    assert_eq!(blocks[2]["role"], "tool");
    assert_eq!(blocks[2]["name"], "Read");
    assert_eq!(blocks[2]["char_count"], "pub fn run() {}".chars().count());
    assert!(blocks[2]["approx_tokens"].as_u64().unwrap() > 0);
}
