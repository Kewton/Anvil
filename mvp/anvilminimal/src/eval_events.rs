use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

const SNIPPET_LIMIT: usize = 500;

pub fn path_from_env() -> Option<PathBuf> {
    std::env::var_os("ANVIL_EVAL_EVENTS")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

pub fn emit(path: Option<&Path>, mut event: Value) {
    let Some(path) = path else {
        return;
    };
    if let Value::Object(ref mut object) = event {
        object
            .entry("schema_version")
            .or_insert_with(|| Value::String("1".to_string()));
    }
    if let Err(err) = append(path, &event) {
        eprintln!("warning: failed to write ANVIL_EVAL_EVENTS: {err}");
    }
}

fn append(path: &Path, event: &Value) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(file, "{}", serde_json::to_string(event)?)?;
    Ok(())
}

pub fn argument_shape(arguments: &Value) -> Value {
    match arguments {
        Value::Object(map) => {
            let mut keys = map.keys().cloned().collect::<Vec<_>>();
            keys.sort();
            json!({
                "arguments_type": "object",
                "argument_keys": keys,
            })
        }
        Value::String(value) => json!({
            "arguments_type": "string",
            "argument_len": value.chars().count(),
        }),
        Value::Array(values) => json!({
            "arguments_type": "array",
            "argument_len": values.len(),
        }),
        Value::Null => json!({
            "arguments_type": "null",
        }),
        Value::Bool(_) => json!({
            "arguments_type": "bool",
        }),
        Value::Number(_) => json!({
            "arguments_type": "number",
        }),
    }
}

pub fn body_snippet(body: &str) -> String {
    let mut clean = body.replace('\n', " ");
    clean = clean.replace('\r', " ");
    clean = redact_secret_like(&clean);
    clean.chars().take(SNIPPET_LIMIT).collect()
}

fn redact_secret_like(value: &str) -> String {
    value
        .split_whitespace()
        .map(|part| {
            if part.starts_with("sk-")
                || part.starts_with("AIza")
                || part.to_ascii_lowercase().contains("api_key")
            {
                "<redacted>"
            } else {
                part
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_jsonl_without_prompt_payloads() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.jsonl");
        emit(
            Some(&path),
            json!({"event":"tool_call_raw","name":"Grep","arguments": argument_shape(&json!({"pattern":"secret"}))}),
        );
        let text = std::fs::read_to_string(path).unwrap();
        assert!(text.contains("\"event\":\"tool_call_raw\""));
        assert!(text.contains("\"argument_keys\":[\"pattern\"]"));
        assert!(!text.contains("secret"));
    }

    #[test]
    fn body_snippet_truncates_and_redacts_secret_like_values() {
        let snippet = body_snippet(&format!("api_key sk-test {}", "x".repeat(700)));
        assert!(snippet.contains("<redacted>"));
        assert!(snippet.chars().count() <= SNIPPET_LIMIT);
    }
}
