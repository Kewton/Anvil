use serde_json::{Value, json};

use crate::tools::registry::ToolSpec;

pub fn truncate_for_log(value: &str, max: usize) -> String {
    if value.len() <= max {
        value.to_string()
    } else {
        format!("{}...[truncated]", &value[..max])
    }
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
}
