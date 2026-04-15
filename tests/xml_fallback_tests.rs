use anvil::ollama::xml_fallback::{extract_tool_calls, strip_think_tags};
use serde_json::json;

#[test]
fn strips_think_tags() {
    let text = "<think>private</think>done";
    assert_eq!(strip_think_tags(text), "done");
}

#[test]
fn extracts_json_tool_call_blocks() {
    let (calls, content) = extract_tool_calls(
        r#"before
<tool_call>{"name":"Read","arguments":{"path":"README.md"}}</tool_call>
after"#,
        &["Read".to_string()],
    );
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "Read");
    assert_eq!(calls[0].arguments, json!({"path":"README.md"}));
    assert!(content.contains("before"));
    assert!(content.contains("after"));
}

#[test]
fn extracts_tagged_tool_call_blocks() {
    let (calls, _) = extract_tool_calls(
        r#"<tool_call name="Write">{"path":"plan.md","content":"hi"}</tool_call>"#,
        &["Write".to_string()],
    );
    assert_eq!(calls[0].name, "Write");
}
