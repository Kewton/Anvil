use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolArgsRecovery {
    pub arguments: Value,
    pub changed: bool,
    pub changes: Vec<&'static str>,
}

pub fn recover_tool_arguments(name: &str, value: Value) -> ToolArgsRecovery {
    let mut changes = Vec::new();
    let arguments = recover_tool_arguments_inner(name, value, &mut changes);
    changes.sort_unstable();
    changes.dedup();
    ToolArgsRecovery {
        changed: !changes.is_empty(),
        arguments,
        changes,
    }
}

fn recover_tool_arguments_inner(
    name: &str,
    value: Value,
    changes: &mut Vec<&'static str>,
) -> Value {
    match unwrap_argument_wrappers(value, changes) {
        Value::String(raw) => match serde_json::from_str::<Value>(&raw) {
            Ok(parsed @ Value::Object(_)) => {
                changes.push("json_string_arguments_decoded");
                recover_tool_arguments_inner(name, parsed, changes)
            }
            Ok(other) => other,
            Err(_) => Value::String(raw),
        },
        Value::Object(mut map) => {
            for nested in map.values_mut() {
                let original = std::mem::take(nested);
                *nested = recover_nested_argument_value(name, original, changes);
            }
            normalize_aliases(name, &mut map, changes);
            Value::Object(map)
        }
        Value::Array(values) => Value::Array(
            values
                .into_iter()
                .map(|item| recover_tool_arguments_inner(name, item, changes))
                .collect(),
        ),
        other => other,
    }
}

fn recover_nested_argument_value(
    name: &str,
    value: Value,
    changes: &mut Vec<&'static str>,
) -> Value {
    match value {
        Value::Object(_) | Value::Array(_) => recover_tool_arguments_inner(name, value, changes),
        other => other,
    }
}

fn unwrap_argument_wrappers(value: Value, changes: &mut Vec<&'static str>) -> Value {
    match value {
        Value::Object(mut map) => {
            if map.remove("name").is_some() || map.remove("tool").is_some() {
                changes.push("tool_name_removed_from_arguments");
            }
            if let Some(nested) = map
                .remove("arguments")
                .or_else(|| map.remove("args"))
                .or_else(|| map.remove("payload"))
                .or_else(|| map.remove("params"))
                .or_else(|| map.remove("input"))
                .or_else(|| map.remove("data"))
            {
                let unwrapped = unwrap_argument_wrappers(nested, changes);
                let unwrapped = decode_json_string_object(unwrapped, changes);
                if let Value::Object(nested_map) = &unwrapped
                    && contains_tool_argument_keys(nested_map)
                {
                    changes.push("argument_wrapper_unwrapped");
                    return unwrapped;
                }
                map.insert("arguments".to_string(), unwrapped);
                changes.push("argument_wrapper_preserved");
            }
            Value::Object(map)
        }
        other => other,
    }
}

fn decode_json_string_object(value: Value, changes: &mut Vec<&'static str>) -> Value {
    match value {
        Value::String(raw) => match serde_json::from_str::<Value>(&raw) {
            Ok(parsed @ Value::Object(_)) => {
                changes.push("json_string_arguments_decoded");
                parsed
            }
            Ok(other) => other,
            Err(_) => Value::String(raw),
        },
        other => other,
    }
}

fn normalize_aliases(
    name: &str,
    map: &mut serde_json::Map<String, Value>,
    changes: &mut Vec<&'static str>,
) {
    let normalized_name = name.to_ascii_lowercase();
    match normalized_name.as_str() {
        "read" | "write" | "edit" => {
            maybe_insert_alias(
                map,
                "path",
                &["file", "file_path", "filepath", "filename"],
                "path_alias_normalized",
                changes,
            );
        }
        _ => {}
    }

    match normalized_name.as_str() {
        "write" => maybe_insert_alias(
            map,
            "content",
            &["body", "text", "contents"],
            "content_alias_normalized",
            changes,
        ),
        "edit" => {
            maybe_insert_alias(
                map,
                "old_string",
                &["old", "old_text", "oldText", "find"],
                "old_string_alias_normalized",
                changes,
            );
            maybe_insert_alias(
                map,
                "new_string",
                &["new", "new_text", "newText", "replacement", "replace_with"],
                "new_string_alias_normalized",
                changes,
            );
        }
        "bash" => maybe_insert_alias(
            map,
            "command",
            &["cmd"],
            "command_alias_normalized",
            changes,
        ),
        "grep" => maybe_insert_alias(
            map,
            "pattern",
            &["query"],
            "pattern_alias_normalized",
            changes,
        ),
        "glob" => maybe_insert_alias(
            map,
            "pattern",
            &["query", "glob"],
            "pattern_alias_normalized",
            changes,
        ),
        _ => {}
    }
}

fn maybe_insert_alias(
    map: &mut serde_json::Map<String, Value>,
    canonical: &str,
    aliases: &[&str],
    change: &'static str,
    changes: &mut Vec<&'static str>,
) {
    if map.contains_key(canonical) {
        return;
    }
    for alias in aliases {
        if let Some(value) = map.remove(*alias) {
            map.insert(canonical.to_string(), value);
            changes.push(change);
            return;
        }
    }
}

fn contains_tool_argument_keys(map: &serde_json::Map<String, Value>) -> bool {
    [
        "path",
        "file",
        "file_path",
        "filepath",
        "filename",
        "command",
        "cmd",
        "pattern",
        "query",
        "content",
        "contents",
        "body",
        "text",
        "old_string",
        "new_string",
        "replacement",
    ]
    .iter()
    .any(|key| map.contains_key(*key))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn unwraps_nested_arguments_and_aliases_write_args() {
        let recovered = recover_tool_arguments(
            "Write",
            json!({"arguments":{"file":"src/app/page.tsx","body":"ok","name":"Write"}}),
        );
        assert!(recovered.changed);
        assert_eq!(
            recovered.arguments,
            json!({"path":"src/app/page.tsx","content":"ok"})
        );
        assert!(recovered.changes.contains(&"argument_wrapper_unwrapped"));
        assert!(recovered.changes.contains(&"path_alias_normalized"));
        assert!(recovered.changes.contains(&"content_alias_normalized"));
    }

    #[test]
    fn decodes_json_string_arguments_when_they_contain_an_object() {
        let recovered = recover_tool_arguments(
            "Bash",
            Value::String(r#"{"cmd":"npm run build"}"#.to_string()),
        );
        assert_eq!(recovered.arguments, json!({"command":"npm run build"}));
        assert!(recovered.changes.contains(&"json_string_arguments_decoded"));
        assert!(recovered.changes.contains(&"command_alias_normalized"));
    }

    #[test]
    fn unwraps_json_string_inside_arguments_wrapper() {
        let recovered = recover_tool_arguments(
            "Write",
            json!({"arguments":"{\"file\":\"notes.txt\",\"body\":\"ok\"}"}),
        );
        assert_eq!(
            recovered.arguments,
            json!({"path":"notes.txt","content":"ok"})
        );
        assert!(recovered.changes.contains(&"json_string_arguments_decoded"));
        assert!(recovered.changes.contains(&"argument_wrapper_unwrapped"));
    }

    #[test]
    fn does_not_rewrite_arbitrary_string_arguments() {
        let recovered = recover_tool_arguments("Write", Value::String("hello".to_string()));
        assert_eq!(recovered.arguments, Value::String("hello".to_string()));
        assert!(!recovered.changed);
    }

    #[test]
    fn does_not_decode_string_content_inside_object() {
        let recovered =
            recover_tool_arguments("Write", json!({"path":"package.json","content":"{}"}));
        assert_eq!(recovered.arguments["content"], "{}");
        assert!(!recovered.changed);
    }
}
