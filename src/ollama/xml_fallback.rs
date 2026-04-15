use regex::Regex;
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Value,
}

pub fn strip_think_tags(text: &str) -> String {
    let regex = Regex::new(r"(?s)<think>.*?</think>").expect("valid regex");
    regex.replace_all(text, "").trim().to_string()
}

pub fn extract_tool_calls(text: &str, allowed_tools: &[String]) -> (Vec<ToolCall>, String) {
    let cleaned = strip_think_tags(text);
    let mut extracted = Vec::new();
    let mut remaining = cleaned.clone();

    let tagged_regex = Regex::new(r#"(?s)<tool_call\s+name="([^"]+)">\s*(\{.*?\})\s*</tool_call>"#)
        .expect("valid regex");
    for captures in tagged_regex.captures_iter(&cleaned) {
        let name = normalize_name(&captures[1], allowed_tools);
        if let Some(arguments) = parse_arguments(&captures[2]) {
            extracted.push(ToolCall {
                id: format!("xml-{}", extracted.len() + 1),
                name,
                arguments,
            });
        }
    }
    remaining = tagged_regex.replace_all(&remaining, "").into_owned();

    let json_regex =
        Regex::new(r"(?s)<tool_call>\s*(\{.*?\})\s*</tool_call>").expect("valid regex");
    for captures in json_regex.captures_iter(&cleaned) {
        if let Some((name, arguments)) = parse_tool_call_object(&captures[1], allowed_tools) {
            extracted.push(ToolCall {
                id: format!("xml-{}", extracted.len() + 1),
                name,
                arguments,
            });
        }
    }
    remaining = json_regex.replace_all(&remaining, "").into_owned();

    (extracted, remaining.trim().to_string())
}

fn parse_tool_call_object(raw: &str, allowed_tools: &[String]) -> Option<(String, Value)> {
    let value: Value = serde_json::from_str(raw).ok()?;
    let object = value.as_object()?;
    let name = object
        .get("name")
        .or_else(|| object.get("tool"))
        .and_then(Value::as_str)?;
    let arguments = object
        .get("arguments")
        .cloned()
        .or_else(|| object.get("args").cloned())
        .unwrap_or(Value::Object(Default::default()));
    Some((normalize_name(name, allowed_tools), arguments))
}

fn parse_arguments(raw: &str) -> Option<Value> {
    serde_json::from_str(raw).ok()
}

fn normalize_name(name: &str, allowed_tools: &[String]) -> String {
    allowed_tools
        .iter()
        .find(|candidate| candidate.eq_ignore_ascii_case(name))
        .cloned()
        .unwrap_or_else(|| name.to_string())
}
