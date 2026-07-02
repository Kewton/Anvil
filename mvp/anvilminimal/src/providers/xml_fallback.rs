use serde_json::Value;

use crate::state::ToolCall;
use crate::tools::args_recovery::recover_tool_arguments;

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
    for tag in ["anvil_tool_call", "tool_call", "function_call"] {
        extract_plain_tag_calls(&mut remaining, tag, allowed_tools, &mut calls)?;
    }
    for tag in ["anvil_tool_call", "function"] {
        extract_named_tag_calls(&mut remaining, tag, allowed_tools, &mut calls)?;
    }
    extract_function_equals_calls(&mut remaining, allowed_tools, &mut calls)?;
    Ok((calls, remaining.trim().to_string()))
}

fn extract_plain_tag_calls(
    remaining: &mut String,
    tag: &str,
    allowed_tools: &[String],
    calls: &mut Vec<ToolCall>,
) -> anyhow::Result<()> {
    loop {
        let open = format!("<{tag}>");
        let close = format!("</{tag}>");
        let Some(start) = remaining.find(&open) else {
            break;
        };
        let json_start = start + open.len();
        let Some(end_rel) = remaining[json_start..].find(&close) else {
            let raw = remaining[json_start..].trim();
            if json_looks_closed(raw) {
                calls.push(parse_tool_call(raw, None, allowed_tools)?);
                remaining.replace_range(start..remaining.len(), "");
                continue;
            }
            return Err(anyhow::anyhow!("malformed XML tool call"));
        };
        let json_end = json_start + end_rel;
        let raw = remaining[json_start..json_end].trim();
        calls.push(parse_tool_call(raw, None, allowed_tools)?);
        let end = json_end + close.len();
        remaining.replace_range(start..end, "");
    }
    Ok(())
}

fn extract_named_tag_calls(
    remaining: &mut String,
    tag: &str,
    allowed_tools: &[String],
    calls: &mut Vec<ToolCall>,
) -> anyhow::Result<()> {
    let open_prefix = format!("<{tag} name=\"");
    let close = format!("</{tag}>");
    loop {
        let Some(start) = remaining.find(&open_prefix) else {
            break;
        };
        let name_start = start + open_prefix.len();
        let Some(name_end_rel) = remaining[name_start..].find('"') else {
            return Err(anyhow::anyhow!("malformed XML tool call"));
        };
        let name_end = name_start + name_end_rel;
        let name = remaining[name_start..name_end].to_string();
        let after_name = name_end + 1;
        let Some(gt_rel) = remaining[after_name..].find('>') else {
            return Err(anyhow::anyhow!("malformed XML tool call"));
        };
        let body_start = after_name + gt_rel + 1;
        let Some(end_rel) = remaining[body_start..].find(&close) else {
            return Err(anyhow::anyhow!("malformed XML tool call"));
        };
        let body_end = body_start + end_rel;
        let raw = remaining[body_start..body_end].trim();
        calls.push(parse_tool_call(raw, Some(&name), allowed_tools)?);
        let end = body_end + close.len();
        remaining.replace_range(start..end, "");
    }
    Ok(())
}

fn extract_function_equals_calls(
    remaining: &mut String,
    allowed_tools: &[String],
    calls: &mut Vec<ToolCall>,
) -> anyhow::Result<()> {
    let open = "<function=";
    let close = "</function>";
    loop {
        let Some(start) = remaining.find(open) else {
            break;
        };
        let name_start = start + open.len();
        let Some(gt_rel) = remaining[name_start..].find('>') else {
            return Err(anyhow::anyhow!("malformed XML tool call"));
        };
        let name_end = name_start + gt_rel;
        let name = remaining[name_start..name_end]
            .trim()
            .trim_matches('"')
            .to_string();
        let body_start = name_end + 1;
        let Some(end_rel) = remaining[body_start..].find(close) else {
            return Err(anyhow::anyhow!("malformed XML tool call"));
        };
        let body_end = body_start + end_rel;
        let raw = remaining[body_start..body_end].trim();
        calls.push(parse_tool_call(raw, Some(&name), allowed_tools)?);
        let end = body_end + close.len();
        remaining.replace_range(start..end, "");
    }
    Ok(())
}

fn parse_tool_call(
    raw: &str,
    default_name: Option<&str>,
    allowed_tools: &[String],
) -> anyhow::Result<ToolCall> {
    let value = parse_json_relaxed(raw)?;
    let object = value
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("XML tool call payload must be an object"))?;
    let nested_arguments = object
        .get("arguments")
        .or_else(|| object.get("args"))
        .and_then(Value::as_object);
    let raw_name = default_name
        .or_else(|| {
            object
                .get("name")
                .or_else(|| object.get("tool"))
                .and_then(Value::as_str)
        })
        .or_else(|| {
            nested_arguments
                .and_then(|inner| inner.get("name").or_else(|| inner.get("tool")))
                .and_then(Value::as_str)
        })
        .map(str::to_string)
        .or_else(|| infer_tool_name_from_arguments(object, nested_arguments, allowed_tools))
        .ok_or_else(|| anyhow::anyhow!("tool call missing name"))?;
    let name = normalize_allowed_tool_name(&raw_name, allowed_tools)?;
    let arguments = if default_name.is_some()
        && object.get("arguments").is_none()
        && object.get("args").is_none()
        && object.get("name").is_none()
        && object.get("tool").is_none()
    {
        Value::Object(object.clone())
    } else {
        object
            .get("arguments")
            .or_else(|| object.get("args"))
            .cloned()
            .unwrap_or_else(|| {
                let mut remaining = object.clone();
                remaining.remove("name");
                remaining.remove("tool");
                remaining.remove("arguments");
                remaining.remove("args");
                Value::Object(remaining)
            })
    };
    let arguments = recover_tool_arguments(&name, arguments).arguments;
    Ok(ToolCall::new(name, arguments))
}

fn normalize_allowed_tool_name(name: &str, allowed_tools: &[String]) -> anyhow::Result<String> {
    allowed_tools
        .iter()
        .find(|allowed| allowed.eq_ignore_ascii_case(name))
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("unknown tool in XML fallback: {name}"))
}

fn infer_tool_name_from_arguments(
    object: &serde_json::Map<String, Value>,
    nested_arguments: Option<&serde_json::Map<String, Value>>,
    allowed_tools: &[String],
) -> Option<String> {
    let args = nested_arguments.unwrap_or(object);
    let mut candidates = Vec::new();
    if has_any_key(args, &["command", "cmd"]) {
        maybe_push_allowed_tool(&mut candidates, "Bash", allowed_tools);
    }
    if has_any_key(args, &["path", "file", "file_path", "filepath", "filename"]) {
        if has_any_key(args, &["old_string", "old", "old_text", "oldText", "find"])
            && has_any_key(
                args,
                &[
                    "new_string",
                    "new",
                    "new_text",
                    "newText",
                    "replacement",
                    "replace_with",
                ],
            )
        {
            maybe_push_allowed_tool(&mut candidates, "Edit", allowed_tools);
        } else if has_any_key(args, &["content", "contents", "body", "text"]) {
            maybe_push_allowed_tool(&mut candidates, "Write", allowed_tools);
        } else {
            maybe_push_allowed_tool(&mut candidates, "Read", allowed_tools);
        }
    }
    if has_any_key(args, &["pattern", "query", "glob"]) {
        maybe_push_allowed_tool(&mut candidates, "Grep", allowed_tools);
        maybe_push_allowed_tool(&mut candidates, "Glob", allowed_tools);
    }
    candidates.dedup();
    (candidates.len() == 1).then(|| candidates.remove(0))
}

fn has_any_key(map: &serde_json::Map<String, Value>, keys: &[&str]) -> bool {
    keys.iter().any(|key| map.contains_key(*key))
}

fn maybe_push_allowed_tool(candidates: &mut Vec<String>, name: &str, allowed_tools: &[String]) {
    if let Some(allowed) = allowed_tools
        .iter()
        .find(|candidate| candidate.eq_ignore_ascii_case(name))
    {
        candidates.push(allowed.clone());
    }
}

fn parse_json_relaxed(raw: &str) -> anyhow::Result<Value> {
    let trimmed = raw.trim();
    for candidate in [trimmed.to_string(), strip_markdown_fence(trimmed)] {
        if let Ok(parsed) = serde_json::from_str(&candidate) {
            return Ok(parsed);
        }
        if let Some(parsed) = repair_json_candidate(&candidate) {
            return Ok(parsed);
        }
        let balanced = balance_braces(&candidate);
        if balanced != candidate {
            if let Ok(parsed) = serde_json::from_str(&balanced) {
                return Ok(parsed);
            }
            if let Some(parsed) = repair_json_candidate(&balanced) {
                return Ok(parsed);
            }
        }
    }
    serde_json::from_str(trimmed).map_err(Into::into)
}

fn repair_json_candidate(raw: &str) -> Option<Value> {
    let mut fixed = raw.trim().to_string();
    fixed = fixed.replace('\'', "\"");
    fixed = strip_trailing_commas(&fixed);
    fixed = quote_bare_keys(&fixed);
    serde_json::from_str(&fixed).ok()
}

fn strip_trailing_commas(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == ',' {
            let mut lookahead = chars.clone();
            while matches!(lookahead.peek(), Some(next) if next.is_whitespace()) {
                lookahead.next();
            }
            if matches!(lookahead.peek(), Some('}' | ']')) {
                continue;
            }
        }
        out.push(ch);
    }
    out
}

fn quote_bare_keys(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    let mut expecting_key = false;
    let mut in_string = false;
    let mut escaped = false;
    while let Some(ch) = chars.next() {
        if escaped {
            out.push(ch);
            escaped = false;
            continue;
        }
        match ch {
            '\\' if in_string => {
                out.push(ch);
                escaped = true;
            }
            '"' => {
                out.push(ch);
                in_string = !in_string;
                expecting_key = false;
            }
            '{' | ',' if !in_string => {
                out.push(ch);
                expecting_key = true;
            }
            ':' if !in_string => {
                out.push(ch);
                expecting_key = false;
            }
            ch if !in_string && expecting_key && (ch.is_ascii_alphabetic() || ch == '_') => {
                let mut key = String::new();
                key.push(ch);
                while let Some(next) = chars.peek() {
                    if next.is_ascii_alphanumeric() || matches!(next, '_' | '-') {
                        key.push(*next);
                        chars.next();
                    } else {
                        break;
                    }
                }
                out.push('"');
                out.push_str(&key);
                out.push('"');
                expecting_key = false;
            }
            ch => out.push(ch),
        }
    }
    out
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
    !trimmed.is_empty() && balance_braces(trimmed) == trimmed
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

    #[test]
    fn extracts_source_style_function_call_tag() {
        let (calls, text) = extract_tool_calls(
            r#"hi <function_call>{"name":"Write","arguments":{"path":"provider-probe.txt","content":"ok"}}</function_call>"#,
            &["Write".to_string()],
        )
        .unwrap();
        assert_eq!(calls[0].name, "Write");
        assert_eq!(calls[0].arguments["path"], "provider-probe.txt");
        assert_eq!(text, "hi");
    }

    #[test]
    fn recovers_tool_like_argument_aliases() {
        let (calls, _) = extract_tool_calls(
            r#"<function_call>{"name":"Write","arguments":{"file":"provider-probe.txt","body":"ok"}}</function_call>"#,
            &["Write".to_string()],
        )
        .unwrap();
        assert_eq!(calls[0].arguments["path"], "provider-probe.txt");
        assert_eq!(calls[0].arguments["content"], "ok");
    }

    #[test]
    fn extracts_source_style_named_anvil_tool_call() {
        let (calls, text) = extract_tool_calls(
            r#"hi <anvil_tool_call name="Write">{"file":"provider-probe.txt","body":"ok"}</anvil_tool_call>"#,
            &["Write".to_string()],
        )
        .unwrap();
        assert_eq!(calls[0].name, "Write");
        assert_eq!(calls[0].arguments["path"], "provider-probe.txt");
        assert_eq!(calls[0].arguments["content"], "ok");
        assert_eq!(text, "hi");
    }

    #[test]
    fn extracts_source_style_function_equals_tag() {
        let (calls, _) = extract_tool_calls(
            r#"<function=Write>{"file":"provider-probe.txt","body":"ok"}</function>"#,
            &["Write".to_string()],
        )
        .unwrap();
        assert_eq!(calls[0].name, "Write");
        assert_eq!(calls[0].arguments["path"], "provider-probe.txt");
        assert_eq!(calls[0].arguments["content"], "ok");
    }

    #[test]
    fn infers_unambiguous_tool_name_from_arguments() {
        let (calls, _) = extract_tool_calls(
            r#"<function_call>{"arguments":{"file":"provider-probe.txt","body":"ok"}}</function_call>"#,
            &["Write".to_string(), "Read".to_string()],
        )
        .unwrap();
        assert_eq!(calls[0].name, "Write");
        assert_eq!(calls[0].arguments["path"], "provider-probe.txt");
        assert_eq!(calls[0].arguments["content"], "ok");
    }

    #[test]
    fn recovers_unterminated_closed_tool_call() {
        let (calls, text) = extract_tool_calls(
            r#"before <function_call>{"name":"Read","arguments":{"path":"README.md"}}"#,
            &["Read".to_string()],
        )
        .unwrap();
        assert_eq!(calls[0].name, "Read");
        assert_eq!(calls[0].arguments["path"], "README.md");
        assert_eq!(text, "before");
    }

    #[test]
    fn repairs_relaxed_json_payload() {
        let (calls, _) = extract_tool_calls(
            r#"<function_call>{name:'Write', arguments:{file:'provider-probe.txt', body:'ok',},}</function_call>"#,
            &["Write".to_string()],
        )
        .unwrap();
        assert_eq!(calls[0].name, "Write");
        assert_eq!(calls[0].arguments["path"], "provider-probe.txt");
        assert_eq!(calls[0].arguments["content"], "ok");
    }

    #[test]
    fn rejects_ambiguous_inferred_tool_name() {
        let err = extract_tool_calls(
            r#"<function_call>{"arguments":{"query":"TODO"}}</function_call>"#,
            &["Grep".to_string(), "Glob".to_string()],
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("tool call missing name"));
    }
}
