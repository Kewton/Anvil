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

    for tag in ["anvil_tool_call"] {
        let tagged_regex = Regex::new(&format!(
            r#"(?s)<{tag}\s+name="([^"]+)">\s*(\{{.*?\}})\s*</{tag}>"#
        ))
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
            Regex::new(&format!(r"(?s)<{tag}>\s*(\{{.*?\}})\s*</{tag}>")).expect("valid regex");
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
    }

    let function_regex = Regex::new(r#"(?s)<function\s+name="([^"]+)">\s*(\{.*?\})\s*</function>"#)
        .expect("valid regex");
    for captures in function_regex.captures_iter(&cleaned) {
        let name = normalize_name(&captures[1], allowed_tools);
        if let Some(arguments) = parse_arguments(&captures[2]) {
            extracted.push(ToolCall {
                id: format!("xml-{}", extracted.len() + 1),
                name,
                arguments,
            });
        }
    }
    remaining = function_regex.replace_all(&remaining, "").into_owned();

    (extracted, remaining.trim().to_string())
}

fn parse_tool_call_object(raw: &str, allowed_tools: &[String]) -> Option<(String, Value)> {
    let value = parse_json_relaxed(raw)?;
    let object = value.as_object()?;
    let name = object
        .get("name")
        .or_else(|| object.get("tool"))
        .and_then(Value::as_str)?;
    let arguments = object
        .get("arguments")
        .and_then(normalize_arguments_value)
        .or_else(|| object.get("args").cloned())
        .unwrap_or_else(|| {
            let mut remaining = object.clone();
            remaining.remove("name");
            remaining.remove("tool");
            remaining.remove("arguments");
            remaining.remove("args");
            if remaining.is_empty() {
                Value::Object(Default::default())
            } else {
                Value::Object(remaining)
            }
        });
    Some((normalize_name(name, allowed_tools), arguments))
}

fn parse_arguments(raw: &str) -> Option<Value> {
    parse_json_relaxed(raw)
}

fn normalize_arguments_value(value: &Value) -> Option<Value> {
    match value {
        Value::String(raw) => parse_json_relaxed(raw).or_else(|| Some(Value::String(raw.clone()))),
        other => Some(other.clone()),
    }
}

fn parse_json_relaxed(raw: &str) -> Option<Value> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }

    for candidate in [trimmed.to_string(), strip_markdown_fence(trimmed)] {
        if let Ok(parsed) = serde_json::from_str(&candidate) {
            return Some(parsed);
        }
        if let Some(parsed) = repair_json_candidate(&candidate) {
            return Some(parsed);
        }
    }

    None
}

fn strip_markdown_fence(raw: &str) -> String {
    let trimmed = raw.trim();
    if !trimmed.starts_with("```") {
        return trimmed.to_string();
    }

    let mut lines = trimmed.lines();
    let _ = lines.next();
    let mut body = lines.collect::<Vec<_>>();
    if matches!(body.last(), Some(last) if last.trim_start().starts_with("```")) {
        body.pop();
    }
    body.join("\n")
}

fn repair_json_candidate(raw: &str) -> Option<Value> {
    let trailing_commas = Regex::new(r",\s*([}\]])").expect("valid regex");
    let bare_keys =
        Regex::new(r#"([{\[,]\s*)([A-Za-z_][A-Za-z0-9_-]*)(\s*:)"#).expect("valid regex");

    let without_commas = trailing_commas.replace_all(raw, "$1").into_owned();
    let quoted_keys = bare_keys.replace_all(&without_commas, "$1\"$2\"$3");
    let single_to_double = quoted_keys.replace('\'', "\"");
    serde_json::from_str(&single_to_double).ok()
}

fn normalize_name(name: &str, allowed_tools: &[String]) -> String {
    allowed_tools
        .iter()
        .find(|candidate| candidate.eq_ignore_ascii_case(name))
        .cloned()
        .unwrap_or_else(|| name.to_string())
}
