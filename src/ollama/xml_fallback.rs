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

    for (open_tag, close_tag) in [
        ("<anvil_tool_call>", "</anvil_tool_call>"),
        ("<function_call>", "</function_call>"),
    ] {
        while let Some(body) = extract_between(&remaining, open_tag, close_tag) {
            if let Some((name, arguments)) = parse_tool_call_object(&body.inner, allowed_tools) {
                extracted.push(ToolCall {
                    id: format!("xml-{}", extracted.len() + 1),
                    name,
                    arguments,
                });
            }
            remaining.replace_range(body.start..body.end, "");
        }
    }

    let tagged_regex = Regex::new(
        r#"(?s)<anvil_tool_call\s+name="([^"]+)">\s*(\{.*?\})\s*</anvil_tool_call>"#,
    )
    .expect("valid regex");
    let cleaned_snapshot = remaining.clone();
    for captures in tagged_regex.captures_iter(&cleaned_snapshot) {
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

    for (open_tag, close_tag) in [("<function=", "</function>")] {
        while let Some(range) = remaining.find(open_tag).and_then(|start| {
            remaining[start..]
                .find(close_tag)
                .map(|end_rel| (start, start + end_rel + close_tag.len()))
        }) {
            let block = &remaining[range.0..range.1];
            if let Some(after_eq) = block.strip_prefix(open_tag)
                && let Some(gt_pos) = after_eq.find('>')
            {
                let name_raw = &after_eq[..gt_pos];
                let body = &after_eq[gt_pos + 1..after_eq.len() - close_tag.len()];
                let name = normalize_name(name_raw.trim(), allowed_tools);
                if let Some(arguments) = parse_arguments(body.trim()) {
                    extracted.push(ToolCall {
                        id: format!("xml-{}", extracted.len() + 1),
                        name,
                        arguments,
                    });
                }
            }
            remaining.replace_range(range.0..range.1, "");
        }
    }

    let function_regex =
        Regex::new(r#"(?s)<function\s+name="([^"]+)">\s*(\{.*?\})\s*</function>"#)
            .expect("valid regex");
    let cleaned_snapshot = remaining.clone();
    for captures in function_regex.captures_iter(&cleaned_snapshot) {
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

struct ExtractedBlock {
    inner: String,
    start: usize,
    end: usize,
}

fn extract_between(text: &str, open: &str, close: &str) -> Option<ExtractedBlock> {
    let start = text.find(open)?;
    let after_open = start + open.len();
    let end_rel = text[after_open..].find(close)?;
    let inner_end = after_open + end_rel;
    let end = inner_end + close.len();
    Some(ExtractedBlock {
        inner: text[after_open..inner_end].trim().to_string(),
        start,
        end,
    })
}

fn parse_tool_call_object(raw: &str, allowed_tools: &[String]) -> Option<(String, Value)> {
    let value = parse_json_relaxed(raw)?;
    let object = value.as_object()?;
    let nested_arguments_object = object
        .get("arguments")
        .or_else(|| object.get("args"))
        .and_then(Value::as_object);
    let name = object
        .get("name")
        .or_else(|| object.get("tool"))
        .and_then(Value::as_str)
        .or_else(|| {
            nested_arguments_object
                .and_then(|inner| inner.get("name").or_else(|| inner.get("tool")))
                .and_then(Value::as_str)
        })?;
    let arguments = object
        .get("arguments")
        .and_then(normalize_arguments_value)
        .or_else(|| object.get("args").cloned())
        .map(strip_name_from_arguments)
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

fn strip_name_from_arguments(value: Value) -> Value {
    match value {
        Value::Object(mut map) => {
            map.remove("name");
            map.remove("tool");
            Value::Object(map)
        }
        other => other,
    }
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
        let balanced = balance_braces(&candidate);
        if balanced != candidate {
            if let Ok(parsed) = serde_json::from_str(&balanced) {
                return Some(parsed);
            }
            if let Some(parsed) = repair_json_candidate(&balanced) {
                return Some(parsed);
            }
        }
    }

    None
}

fn balance_braces(raw: &str) -> String {
    let mut curly: i32 = 0;
    let mut square: i32 = 0;
    let mut in_string = false;
    let mut escaped = false;
    for ch in raw.chars() {
        if escaped {
            escaped = false;
            continue;
        }
        match ch {
            '\\' if in_string => escaped = true,
            '"' => in_string = !in_string,
            '{' if !in_string => curly += 1,
            '}' if !in_string => curly -= 1,
            '[' if !in_string => square += 1,
            ']' if !in_string => square -= 1,
            _ => {}
        }
    }
    let mut fixed = raw.to_string();
    while square > 0 {
        fixed.push(']');
        square -= 1;
    }
    while curly > 0 {
        fixed.push('}');
        curly -= 1;
    }
    fixed
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
