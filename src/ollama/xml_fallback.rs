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

        if let Some(body) = extract_unterminated_trailing_block(&remaining, open_tag, close_tag) {
            if let Some((name, arguments)) = parse_tool_call_object(&body.inner, allowed_tools) {
                extracted.push(ToolCall {
                    id: format!("xml-{}", extracted.len() + 1),
                    name,
                    arguments,
                });
                remaining.replace_range(body.start..body.end, "");
            }
        }
    }

    let tagged_regex =
        Regex::new(r#"(?s)<anvil_tool_call\s+name="([^"]+)">\s*(\{.*?\})\s*</anvil_tool_call>"#)
            .expect("valid regex");
    let cleaned_snapshot = remaining.clone();
    for captures in tagged_regex.captures_iter(&cleaned_snapshot) {
        let name = normalize_name(&captures[1], allowed_tools);
        if let Some(arguments) = parse_arguments(&captures[2]) {
            extracted.push(ToolCall {
                id: format!("xml-{}", extracted.len() + 1),
                name,
                arguments: normalize_tool_call_arguments(&captures[1], arguments),
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
                        arguments: normalize_tool_call_arguments(name_raw.trim(), arguments),
                    });
                }
            }
            remaining.replace_range(range.0..range.1, "");
        }
    }

    let function_regex = Regex::new(r#"(?s)<function\s+name="([^"]+)">\s*(\{.*?\})\s*</function>"#)
        .expect("valid regex");
    let cleaned_snapshot = remaining.clone();
    for captures in function_regex.captures_iter(&cleaned_snapshot) {
        let name = normalize_name(&captures[1], allowed_tools);
        if let Some(arguments) = parse_arguments(&captures[2]) {
            extracted.push(ToolCall {
                id: format!("xml-{}", extracted.len() + 1),
                name,
                arguments: normalize_tool_call_arguments(&captures[1], arguments),
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

fn extract_unterminated_trailing_block(
    text: &str,
    open: &str,
    close: &str,
) -> Option<ExtractedBlock> {
    let start = text.find(open)?;
    let after_open = start + open.len();
    if text[after_open..].contains(close) {
        return None;
    }

    let inner = text[after_open..].trim();
    if !json_looks_closed(inner) {
        return None;
    }

    Some(ExtractedBlock {
        inner: inner.to_string(),
        start,
        end: text.len(),
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
    let normalized_name = normalize_name(name, allowed_tools);
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
    Some((
        normalized_name.clone(),
        normalize_tool_call_arguments(&normalized_name, arguments),
    ))
}

fn strip_name_from_arguments(value: Value) -> Value {
    unwrap_argument_wrappers(value)
}

fn parse_arguments(raw: &str) -> Option<Value> {
    parse_json_relaxed(raw)
}

fn normalize_arguments_value(value: &Value) -> Option<Value> {
    match value {
        Value::String(raw) => parse_json_relaxed(raw)
            .map(unwrap_argument_wrappers)
            .or_else(|| Some(Value::String(raw.clone()))),
        other => Some(unwrap_argument_wrappers(other.clone())),
    }
}

fn take_first_alias(map: &mut serde_json::Map<String, Value>, aliases: &[&str]) -> Option<Value> {
    aliases.iter().find_map(|alias| map.remove(*alias))
}

fn maybe_insert_alias(map: &mut serde_json::Map<String, Value>, canonical: &str, aliases: &[&str]) {
    if map.contains_key(canonical) {
        return;
    }
    if let Some(value) = take_first_alias(map, aliases) {
        map.insert(canonical.to_string(), value);
    }
}

fn contains_tool_argument_keys(map: &serde_json::Map<String, Value>) -> bool {
    map.contains_key("path")
        || map.contains_key("file")
        || map.contains_key("file_path")
        || map.contains_key("filepath")
        || map.contains_key("filename")
        || map.contains_key("command")
        || map.contains_key("pattern")
        || map.contains_key("content")
        || map.contains_key("contents")
        || map.contains_key("body")
        || map.contains_key("text")
        || map.contains_key("old_string")
        || map.contains_key("new_string")
        || map.contains_key("replacement")
}

pub fn normalize_tool_call_arguments(name: &str, value: Value) -> Value {
    match unwrap_argument_wrappers(value) {
        Value::Object(mut map) => {
            let normalized_name = name.to_ascii_lowercase();
            for nested in map.values_mut() {
                let original = std::mem::take(nested);
                *nested = normalize_tool_call_arguments(name, original);
            }

            match normalized_name.as_str() {
                "read" | "write" | "edit" => {
                    maybe_insert_alias(
                        &mut map,
                        "path",
                        &["file", "file_path", "filepath", "filename"],
                    );
                }
                _ => {}
            }

            match normalized_name.as_str() {
                "write" => {
                    maybe_insert_alias(&mut map, "content", &["body", "text", "contents"]);
                }
                "edit" => {
                    maybe_insert_alias(
                        &mut map,
                        "old_string",
                        &["old", "old_text", "oldText", "find"],
                    );
                    maybe_insert_alias(
                        &mut map,
                        "new_string",
                        &["new", "new_text", "newText", "replacement", "replace_with"],
                    );
                }
                "bash" => {
                    maybe_insert_alias(&mut map, "command", &["cmd"]);
                }
                "grep" | "glob" => {
                    maybe_insert_alias(&mut map, "pattern", &["query", "glob"]);
                }
                _ => {}
            }

            Value::Object(map)
        }
        Value::Array(items) => Value::Array(
            items
                .into_iter()
                .map(|item| normalize_tool_call_arguments(name, item))
                .collect(),
        ),
        other => other,
    }
}

fn unwrap_argument_wrappers(value: Value) -> Value {
    match value {
        Value::Object(mut map) => {
            map.remove("name");
            map.remove("tool");

            if let Some(nested) = map
                .remove("arguments")
                .or_else(|| map.remove("args"))
                .or_else(|| map.remove("payload"))
                .or_else(|| map.remove("params"))
                .or_else(|| map.remove("input"))
                .or_else(|| map.remove("data"))
            {
                let unwrapped_nested = unwrap_argument_wrappers(nested);
                if let Value::Object(nested_map) = &unwrapped_nested
                    && contains_tool_argument_keys(nested_map)
                {
                    return unwrapped_nested;
                }
                map.insert("arguments".to_string(), unwrapped_nested);
            }

            Value::Object(map)
        }
        other => other,
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

fn json_looks_closed(raw: &str) -> bool {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return false;
    }

    let mut curly: i32 = 0;
    let mut square: i32 = 0;
    let mut in_string = false;
    let mut escaped = false;

    for ch in trimmed.chars() {
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
        if curly < 0 || square < 0 {
            return false;
        }
    }

    !in_string && curly == 0 && square == 0
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

    if let Some(fixed) = repair_terminal_bracket_swap(raw)
        && let Ok(parsed) = serde_json::from_str(&fixed)
    {
        return Some(parsed);
    }

    let without_commas = trailing_commas.replace_all(raw, "$1").into_owned();
    let quoted_keys = bare_keys.replace_all(&without_commas, "$1\"$2\"$3");
    let single_to_double = quoted_keys.replace('\'', "\"");
    serde_json::from_str(&single_to_double).ok().or_else(|| {
        repair_terminal_bracket_swap(&single_to_double)
            .and_then(|fixed| serde_json::from_str(&fixed).ok())
    })
}

fn repair_terminal_bracket_swap(raw: &str) -> Option<String> {
    let (curly, square, in_string) = bracket_balance(raw);
    if in_string {
        return None;
    }

    match (curly, square) {
        (1, -1) => replace_last_non_ws(raw, ']', '}'),
        (-1, 1) => replace_last_non_ws(raw, '}', ']'),
        _ => None,
    }
}

fn bracket_balance(raw: &str) -> (i32, i32, bool) {
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
    (curly, square, in_string)
}

fn replace_last_non_ws(raw: &str, from: char, to: char) -> Option<String> {
    let (idx, ch) = raw
        .char_indices()
        .rev()
        .find(|(_, ch)| !ch.is_whitespace())?;
    if ch != from {
        return None;
    }
    let mut fixed = raw.to_string();
    fixed.replace_range(idx..idx + ch.len_utf8(), &to.to_string());
    Some(fixed)
}

fn normalize_name(name: &str, allowed_tools: &[String]) -> String {
    allowed_tools
        .iter()
        .find(|candidate| candidate.eq_ignore_ascii_case(name))
        .cloned()
        .unwrap_or_else(|| name.to_string())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{extract_tool_calls, json_looks_closed, normalize_tool_call_arguments};

    #[test]
    fn unwraps_nested_arguments_wrapper() {
        let allowed = vec!["Write".to_string()];
        let input = r#"<anvil_tool_call>{"arguments":{"name":"Write","arguments":{"path":"plans/plan.md","content":"hello"}}}</anvil_tool_call>"#;
        let (tool_calls, remaining) = extract_tool_calls(input, &allowed);
        assert!(remaining.is_empty(), "remaining={remaining:?}");
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0].name, "Write");
        assert_eq!(
            tool_calls[0].arguments,
            json!({"path":"plans/plan.md","content":"hello"})
        );
    }

    #[test]
    fn normalizes_write_aliases() {
        let args =
            normalize_tool_call_arguments("Write", json!({"file":"plans/plan.md","body":"hello"}));
        assert_eq!(args, json!({"path":"plans/plan.md","content":"hello"}));
    }

    #[test]
    fn normalizes_write_file_path_and_contents_aliases() {
        let args = normalize_tool_call_arguments(
            "Write",
            json!({"file_path":"plans/plan.md","contents":"hello"}),
        );
        assert_eq!(args, json!({"path":"plans/plan.md","content":"hello"}));
    }

    #[test]
    fn normalizes_edit_aliases() {
        let args = normalize_tool_call_arguments(
            "Edit",
            json!({"file":"README.md","find":"old","replacement":"new"}),
        );
        assert_eq!(
            args,
            json!({"path":"README.md","old_string":"old","new_string":"new"})
        );
    }

    #[test]
    fn unwraps_nested_arguments_with_file_alias() {
        let allowed = vec!["Write".to_string()];
        let input = r#"<anvil_tool_call>{"arguments":{"name":"Write","arguments":{"file":"plans/plan.md","body":"hello"}}}</anvil_tool_call>"#;
        let (tool_calls, remaining) = extract_tool_calls(input, &allowed);
        assert!(remaining.is_empty(), "remaining={remaining:?}");
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0].name, "Write");
        assert_eq!(
            tool_calls[0].arguments,
            json!({"path":"plans/plan.md","content":"hello"})
        );
    }

    #[test]
    fn unwraps_payload_wrapper_with_file_path_alias() {
        let allowed = vec!["Write".to_string()];
        let input = r#"<anvil_tool_call>{"name":"Write","payload":{"params":{"file_path":"plans/plan.md","contents":"hello"}}}</anvil_tool_call>"#;
        let (tool_calls, remaining) = extract_tool_calls(input, &allowed);
        assert!(remaining.is_empty(), "remaining={remaining:?}");
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0].name, "Write");
        assert_eq!(
            tool_calls[0].arguments,
            json!({"path":"plans/plan.md","content":"hello"})
        );
    }

    #[test]
    fn salvages_unterminated_trailing_tool_call_when_json_is_closed() {
        let allowed = vec!["Write".to_string()];
        let input = r#"<anvil_tool_call>{"name":"Write","arguments":{"path":"plans/plan.md","content":"hello"}}"#;
        let (tool_calls, remaining) = extract_tool_calls(input, &allowed);
        assert!(remaining.is_empty(), "remaining={remaining:?}");
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0].name, "Write");
        assert_eq!(
            tool_calls[0].arguments,
            json!({"path":"plans/plan.md","content":"hello"})
        );
    }

    #[test]
    fn repairs_terminal_bracket_swap_in_tool_call_json() {
        let allowed = vec!["Edit".to_string()];
        let input = r#"<anvil_tool_call>{"name":"Edit","arguments":{"path":"src/app/page.tsx","old_string":"old","new_string":"new"}]</anvil_tool_call>"#;
        let (tool_calls, remaining) = extract_tool_calls(input, &allowed);
        assert!(remaining.is_empty(), "remaining={remaining:?}");
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0].name, "Edit");
        assert_eq!(
            tool_calls[0].arguments,
            json!({"path":"src/app/page.tsx","old_string":"old","new_string":"new"})
        );
    }

    #[test]
    fn does_not_salvage_unterminated_trailing_tool_call_when_json_is_open() {
        let allowed = vec!["Write".to_string()];
        let input = r#"<anvil_tool_call>{"name":"Write","arguments":{"path":"plans/plan.md","content":"hello"}"#;
        let (tool_calls, remaining) = extract_tool_calls(input, &allowed);
        assert!(tool_calls.is_empty());
        assert_eq!(remaining, input);
    }

    #[test]
    fn detects_closed_json_shape() {
        assert!(json_looks_closed(r#"{"a":[1,2],"b":"x"}"#));
        assert!(!json_looks_closed(r#"{"a":[1,2],"b":"x""#));
        assert!(!json_looks_closed(r#"{"a":"unterminated}"#));
    }
}
