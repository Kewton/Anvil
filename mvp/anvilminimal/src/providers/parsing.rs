use serde_json::{Value, json};

use crate::tools::registry::ToolSpec;
use crate::util;

pub fn truncate_for_log(value: &str, max: usize) -> String {
    util::excerpt_with_marker(value, max, "...[truncated]")
}

pub fn tool_names(tools: &[ToolSpec]) -> Vec<String> {
    tools
        .iter()
        .map(|tool| tool.function.name.clone())
        .collect()
}

pub fn sanitized_tool_schema(tool: &ToolSpec) -> Value {
    json!({
        "type": "function",
        "name": tool.function.name,
        "description": tool.function.description,
        "parameters": sanitize_schema(&tool.function.parameters),
    })
}

pub fn sanitize_schema(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut out = serde_json::Map::new();
            for (key, value) in map {
                if matches!(
                    key.as_str(),
                    "$schema" | "$id" | "unevaluatedProperties" | "dependentSchemas"
                ) {
                    continue;
                }
                out.insert(key.clone(), sanitize_schema(value));
            }
            Value::Object(out)
        }
        Value::Array(values) => Value::Array(values.iter().map(sanitize_schema).collect()),
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_sanitizer_drops_unsupported_keyword() {
        let value = json!({"type":"object","$schema":"x","properties":{"a":{"type":"string"}}});
        let sanitized = sanitize_schema(&value);
        assert!(sanitized.get("$schema").is_none());
        assert_eq!(sanitized["properties"]["a"]["type"], "string");
    }

    #[test]
    fn truncate_for_log_handles_multibyte_boundary() {
        let value = "prefix日本語除外suffix";
        let truncated = truncate_for_log(value, 10);
        assert!(truncated.starts_with("prefix日"));
        assert!(truncated.ends_with("...[truncated]"));
    }
}
