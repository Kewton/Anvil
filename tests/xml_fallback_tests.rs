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
<anvil_tool_call>{"name":"Read","arguments":{"path":"README.md"}}</anvil_tool_call>
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
fn extracts_custom_tool_call_blocks() {
    let (calls, content) = extract_tool_calls(
        r#"before
<anvil_tool_call>{"name":"Read","arguments":{"path":"README.md"}}</anvil_tool_call>
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
        r#"<anvil_tool_call name="Write">{"path":"plan.md","content":"hi"}</anvil_tool_call>"#,
        &["Write".to_string()],
    );
    assert_eq!(calls[0].name, "Write");
}

#[test]
fn extracts_function_name_blocks() {
    let (calls, content) = extract_tool_calls(
        r#"<function name="Edit">{"path":"app/page.tsx","old_string":"a","new_string":"b"}</function>"#,
        &["Edit".to_string()],
    );
    assert_eq!(calls[0].name, "Edit");
    assert_eq!(
        calls[0].arguments,
        json!({"path":"app/page.tsx","old_string":"a","new_string":"b"})
    );
    assert!(content.is_empty());
}
