use serde_json::Value;

use crate::state::ToolCall;

pub fn strip_think_tags(input: &str) -> String {
    let mut out = input.to_string();
    loop {
        let Some(start) = out.find("<think>") else {
            break;
        };
        let Some(end_rel) = out[start..].find("</think>") else {
            break;
        };
        let end = start + end_rel + "</think>".len();
        out.replace_range(start..end, "");
    }
    out
}

pub fn extract_tool_calls(
    input: &str,
    allowed_tools: &[String],
) -> anyhow::Result<(Vec<ToolCall>, String)> {
    let mut remaining = strip_think_tags(input);
    let mut calls = Vec::new();
    for tag in ["anvil_tool_call", "tool_call"] {
        loop {
            let open = format!("<{tag}>");
            let close = format!("</{tag}>");
            let Some(start) = remaining.find(&open) else {
                break;
            };
            let Some(end_rel) = remaining[start + open.len()..].find(&close) else {
                return Err(anyhow::anyhow!("malformed XML tool call"));
            };
            let json_start = start + open.len();
            let json_end = json_start + end_rel;
            let raw = remaining[json_start..json_end].trim();
            let value: Value = serde_json::from_str(raw)?;
            let name = value
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow::anyhow!("tool call missing name"))?;
            if !allowed_tools.iter().any(|allowed| allowed == name) {
                return Err(anyhow::anyhow!("unknown tool in XML fallback: {name}"));
            }
            let arguments = value.get("arguments").cloned().unwrap_or(Value::Null);
            calls.push(ToolCall::new(name, arguments));
            let end = json_end + close.len();
            remaining.replace_range(start..end, "");
        }
    }
    Ok((calls, remaining.trim().to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_tool_call() {
        let (calls, text) = extract_tool_calls(
            r#"hi <anvil_tool_call>{"name":"Read","arguments":{"path":"a"}}</anvil_tool_call>"#,
            &["Read".to_string()],
        )
        .unwrap();
        assert_eq!(calls[0].name, "Read");
        assert_eq!(text, "hi");
    }
}
