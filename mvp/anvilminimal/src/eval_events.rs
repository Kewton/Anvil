use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

const SNIPPET_LIMIT: usize = 500;
const SUMMARY_LIMIT: usize = 8_000;

pub fn path_from_env() -> Option<PathBuf> {
    std::env::var_os("ANVIL_EVAL_EVENTS")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

pub fn path_from_env_or_default(root: &Path) -> Option<PathBuf> {
    if let Some(path) = path_from_env() {
        return Some(path);
    }
    if std::env::var_os("ANVIL_NO_RUN_LOG").is_some_and(|value| value == "1" || value == "true") {
        return None;
    }
    Some(default_run_events_path(root))
}

pub fn default_run_events_path(root: &Path) -> PathBuf {
    root.join(".anvil")
        .join("runs")
        .join(uuid::Uuid::now_v7().to_string())
        .join("events.jsonl")
}

pub fn is_eval_events_override() -> bool {
    path_from_env().is_some()
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

pub fn write_run_summary(path: Option<&Path>, text: &str) {
    let Some(path) = path else {
        return;
    };
    let Some(parent) = path.parent() else {
        return;
    };
    let summary = parent.join("summary.md");
    let content = summary_body(text);
    if let Err(err) = std::fs::create_dir_all(parent) {
        eprintln!("warning: failed to create run summary directory: {err}");
        return;
    }
    if let Err(err) = std::fs::write(summary, format!("{content}\n")) {
        eprintln!("warning: failed to write run summary: {err}");
    }
}

pub fn append_run_summary(path: Option<&Path>, text: &str) {
    let Some(path) = path else {
        return;
    };
    let Some(parent) = path.parent() else {
        return;
    };
    let summary = parent.join("summary.md");
    let content = summary_body(text);
    if let Err(err) = std::fs::create_dir_all(parent) {
        eprintln!("warning: failed to create run summary directory: {err}");
        return;
    }
    let existing = std::fs::read_to_string(&summary).unwrap_or_default();
    let combined = if existing.trim().is_empty() {
        format!("{content}\n")
    } else {
        format!("{}\n---\n\n{content}\n", existing.trim_end())
    };
    if let Err(err) = std::fs::write(summary, combined) {
        eprintln!("warning: failed to append run summary: {err}");
    }
}

pub fn argument_shape(arguments: &Value) -> Value {
    match arguments {
        Value::Object(map) => {
            let mut keys = map.keys().cloned().collect::<Vec<_>>();
            keys.sort();
            let mut summaries = serde_json::Map::new();
            for key in &keys {
                if let Some(value) = map.get(key) {
                    summaries.insert(key.clone(), argument_value_summary(key, value));
                }
            }
            json!({
                "arguments_type": "object",
                "argument_keys": keys,
                "argument_summaries": summaries,
            })
        }
        Value::String(value) => json!({
            "arguments_type": "string",
            "argument_len": value.chars().count(),
            "argument_preview": safe_preview(value),
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
    clean = redact_home_paths(&clean);
    clean.chars().take(SNIPPET_LIMIT).collect()
}

fn summary_body(body: &str) -> String {
    let clean = body.replace("\r\n", "\n").replace('\r', "\n");
    let clean = redact_home_paths(&clean);
    clean
        .lines()
        .map(redact_secret_like)
        .collect::<Vec<_>>()
        .join("\n")
        .chars()
        .take(SUMMARY_LIMIT)
        .collect()
}

fn argument_value_summary(key: &str, value: &Value) -> Value {
    match value {
        Value::String(text) => {
            if key == "content" {
                json!({
                    "type": "string",
                    "string_len": text.chars().count(),
                    "preview": "<omitted>",
                })
            } else if matches!(key, "path" | "pattern" | "glob" | "command") {
                json!({
                    "type": "string",
                    "string_len": text.chars().count(),
                    "preview": safe_preview(text),
                })
            } else {
                json!({
                    "type": "string",
                    "string_len": text.chars().count(),
                })
            }
        }
        Value::Array(values) => json!({"type": "array", "len": values.len()}),
        Value::Object(map) => json!({"type": "object", "keys": map.len()}),
        Value::Bool(_) => json!({"type": "bool"}),
        Value::Number(_) => json!({"type": "number"}),
        Value::Null => json!({"type": "null"}),
    }
}

fn safe_preview(value: &str) -> String {
    let mut clean = value.replace('\n', "\\n").replace('\r', "\\r");
    clean = redact_secret_like(&clean);
    clean = redact_home_paths(&clean);
    clean.chars().take(120).collect()
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

fn redact_home_paths(value: &str) -> String {
    let mut out = value.to_string();
    for prefix in ["/Users/", "/home/"] {
        let mut search_from = 0usize;
        loop {
            let Some(relative_start) = out[search_from..].find(prefix) else {
                break;
            };
            let start = search_from + relative_start;
            let name_start = start + prefix.len();
            let Some(rest_end) = out[name_start..].find('/') else {
                break;
            };
            let name_end = name_start + rest_end;
            if &out[name_start..name_end] == "<user>" {
                search_from = name_end;
                continue;
            }
            out.replace_range(name_start..name_end, "<user>");
            search_from = name_start + "<user>".len();
        }
    }
    out
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
            json!({"event":"tool_call_raw","name":"Grep","arguments": argument_shape(&json!({"pattern":"sk-test","content":"do not persist"}))}),
        );
        let text = std::fs::read_to_string(path).unwrap();
        assert!(text.contains("\"event\":\"tool_call_raw\""));
        assert!(text.contains("\"argument_keys\":[\"content\",\"pattern\"]"));
        assert!(text.contains("<redacted>"));
        assert!(!text.contains("do not persist"));
    }

    #[test]
    fn body_snippet_truncates_and_redacts_secret_like_values() {
        let snippet = body_snippet(&format!(
            "api_key sk-test /Users/example/project {}",
            "x".repeat(700)
        ));
        assert!(snippet.contains("<redacted>"));
        assert!(snippet.contains("/Users/<user>/project"));
        assert!(snippet.chars().count() <= SNIPPET_LIMIT);
    }

    #[test]
    fn default_run_events_path_uses_anvil_runs_events_jsonl() {
        let dir = tempfile::tempdir().unwrap();
        let path = default_run_events_path(dir.path());
        assert!(path.starts_with(dir.path().join(".anvil").join("runs")));
        assert_eq!(path.file_name().unwrap(), "events.jsonl");
    }

    #[test]
    fn run_summary_preserves_human_readable_sections_and_appends() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".anvil/runs/test/events.jsonl");
        write_run_summary(
            Some(&path),
            "Status: incomplete\nCompleted phases:\n- scaffold\napi_key sk-test",
        );
        append_run_summary(Some(&path), "TUI command failed: phase failed");
        let summary = std::fs::read_to_string(path.parent().unwrap().join("summary.md")).unwrap();
        assert!(summary.contains("Status: incomplete\nCompleted phases:\n- scaffold"));
        assert!(summary.contains("---\n\nTUI command failed: phase failed"));
        assert!(summary.contains("<redacted>"));
        assert!(!summary.contains("sk-test"));
    }
}
