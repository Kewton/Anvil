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
}

#[derive(Deserialize)]
struct GenerateStreamChunk {
    #[serde(default)]
    done: bool,
    #[serde(default)]
    response: String,
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
    finalize_reply(parsed.response, tool_names)
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

    finalize_reply(content, tool_names)
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

fn finalize_reply(content: String, tool_names: &[String]) -> Result<AssistantReply, String> {
    let (tool_calls, cleaned_content) = extract_tool_calls(&content, tool_names);
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
