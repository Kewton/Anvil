use std::io::{BufRead, BufReader};

use serde::Deserialize;
use serde_json::json;

use crate::logging;
use crate::ollama::xml_fallback::{ToolCall, extract_tool_calls};
use crate::tools::registry::ToolSpec;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssistantReply {
    pub content: String,
    pub tool_calls: Vec<ToolCall>,
}

#[derive(Deserialize)]
struct TagsResponse {
    models: Vec<ModelEntry>,
}

#[derive(Deserialize)]
struct ModelEntry {
    name: String,
}

#[derive(Deserialize)]
struct GenerateResponse {
    #[serde(default)]
    response: String,
    #[serde(default)]
    done_reason: String,
}

#[derive(Deserialize)]
struct GenerateStreamChunk {
    #[serde(default)]
    done: bool,
    #[serde(default)]
    response: String,
    #[serde(default)]
    done_reason: String,
}

pub fn parse_tags_response(body: &str) -> Result<Vec<String>, String> {
    let parsed: TagsResponse = serde_json::from_str(body)
        .map_err(|err| format!("failed to decode Ollama tags response: {err}"))?;
    Ok(parsed.models.into_iter().map(|model| model.name).collect())
}

pub fn parse_generate_response(
    body: &str,
    tool_names: &[String],
) -> Result<AssistantReply, String> {
    let parsed: GenerateResponse = serde_json::from_str(body)
        .map_err(|err| format!("failed to decode Ollama generate response: {err}"))?;
    finalize_reply(parsed.response, tool_names, &parsed.done_reason)
}

pub(crate) fn parse_streaming_generate_response<F>(
    response: reqwest::blocking::Response,
    tool_names: &[String],
    on_chunk: &mut F,
) -> Result<AssistantReply, String>
where
    F: FnMut(&str),
{
    let reader = BufReader::new(response);
    let mut content = String::new();
    let mut raw_chunks = Vec::new();
    let mut done_reason = String::new();

    for line in reader.lines() {
        let line = line.map_err(|err| format!("failed to read streaming response: {err}"))?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        raw_chunks.push(truncate_for_log(trimmed, 20_000));
        let chunk: GenerateStreamChunk = serde_json::from_str(trimmed)
            .map_err(|err| format!("failed to parse streaming generate chunk: {err}"))?;
        if !chunk.response.is_empty() {
            on_chunk(&chunk.response);
            content.push_str(&chunk.response);
        }
        if chunk.done {
            done_reason = chunk.done_reason;
            break;
        }
    }

    logging::log_llm_event(
        "ollama.generate.response_raw",
        json!({
            "stream": true,
            "chunks": raw_chunks,
        }),
    );

    finalize_reply(content, tool_names, &done_reason)
}

pub(crate) fn tool_names(tools: &[ToolSpec]) -> Vec<String> {
    tools
        .iter()
        .map(|tool| tool.function.name.clone())
        .collect::<Vec<_>>()
}

pub(crate) fn truncate_for_log(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let truncated = text.chars().take(max_chars).collect::<String>();
    format!("{truncated}\n...[truncated]")
}

fn finalize_reply(
    content: String,
    tool_names: &[String],
    done_reason: &str,
) -> Result<AssistantReply, String> {
    let (tool_calls, cleaned_content) = extract_tool_calls(&content, tool_names);
    detect_malformed_tool_call(&content, &tool_calls, done_reason)?;
    let reply = AssistantReply {
        content: cleaned_content,
        tool_calls,
    };
    logging::log_llm_event(
        "ollama.generate.reply_final",
        json!({
            "content": truncate_for_log(&reply.content, 100_000),
            "tool_calls": reply.tool_calls,
        }),
    );
    Ok(reply)
}

fn detect_malformed_tool_call(
    content: &str,
    tool_calls: &[ToolCall],
    done_reason: &str,
) -> Result<(), String> {
    if !tool_calls.is_empty() {
        return Ok(());
    }

    let stripped = crate::ollama::xml_fallback::strip_think_tags(content);
    let has_tool_markup = stripped.contains("<anvil_tool_call")
        || stripped.contains("</anvil_tool_call>")
        || stripped.contains("<function_call")
        || stripped.contains("</function_call>")
        || stripped.contains("<function=")
        || stripped.contains("<function ");
    if !has_tool_markup {
        return Ok(());
    }

    let lower_done_reason = done_reason.to_ascii_lowercase();
    if lower_done_reason == "length" {
        return Err(
            "tool call parser failed: truncated tool call (generate response hit length limit)"
                .to_string(),
        );
    }

    if stripped.contains("<anvil_tool_call>") && !stripped.contains("</anvil_tool_call>") {
        return Err("tool call parser failed: unterminated <anvil_tool_call> block".to_string());
    }

    Err("tool call parser failed: malformed tool call markup".to_string())
}

#[cfg(test)]
mod tests {
    use super::parse_generate_response;

    #[test]
    fn parse_generate_response_rejects_truncated_tool_call() {
        let body = r#"{
          "response":"<anvil_tool_call>{\"name\":\"Write\",\"arguments\":{\"path\":\"src/app/page.tsx\"",
          "done_reason":"length"
        }"#;
        let err = parse_generate_response(body, &["Write".to_string()]).unwrap_err();
        assert!(err.contains("truncated tool call"), "got: {err}");
    }

    #[test]
    fn parse_generate_response_rejects_malformed_tool_call_markup() {
        let body = r#"{
          "response":"<anvil_tool_call>{\"arguments\":{\"command\":\"pwd\"},name\":\"Bash\"}</anvil_tool_call>",
          "done_reason":"stop"
        }"#;
        let err = parse_generate_response(body, &["Bash".to_string()]).unwrap_err();
        assert!(err.contains("malformed tool call"), "got: {err}");
    }
}
