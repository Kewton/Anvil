use std::io::{BufRead, BufReader};

use serde::Deserialize;
use serde_json::{Value, json};

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
struct ChatResponse {
    message: ResponseMessage,
}

#[derive(Deserialize)]
struct ResponseMessage {
    #[serde(default)]
    content: String,
    #[serde(default)]
    tool_calls: Vec<ResponseToolCall>,
}

#[derive(Deserialize, Clone)]
struct ResponseToolCall {
    #[serde(default)]
    id: Option<String>,
    function: ResponseFunctionCall,
}

#[derive(Deserialize, Clone)]
struct ResponseFunctionCall {
    name: String,
    arguments: Value,
}

#[derive(Deserialize)]
struct StreamChunk {
    #[serde(default)]
    done: bool,
    #[serde(default)]
    message: Option<ResponseMessage>,
}

pub fn parse_tags_response(body: &str) -> Result<Vec<String>, String> {
    let parsed: TagsResponse = serde_json::from_str(body)
        .map_err(|err| format!("failed to decode Ollama tags response: {err}"))?;
    Ok(parsed.models.into_iter().map(|model| model.name).collect())
}

pub fn parse_chat_response(body: &str, tool_names: &[String]) -> Result<AssistantReply, String> {
    let parsed: ChatResponse = serde_json::from_str(body)
        .map_err(|err| format!("failed to decode Ollama chat response: {err}"))?;
    finalize_reply(
        parsed.message.content,
        parsed.message.tool_calls,
        tool_names,
    )
}

pub(crate) fn parse_streaming_chat_response<F>(
    response: reqwest::blocking::Response,
    tool_names: &[String],
    on_chunk: &mut F,
) -> Result<AssistantReply, String>
where
    F: FnMut(&str),
{
    let reader = BufReader::new(response);
    let mut content = String::new();
    let mut native_tool_calls = Vec::new();
    let mut raw_chunks = Vec::new();

    for line in reader.lines() {
        let line = line.map_err(|err| format!("failed to read streaming response: {err}"))?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        raw_chunks.push(truncate_for_log(trimmed, 20_000));
        let chunk: StreamChunk = serde_json::from_str(trimmed)
            .map_err(|err| format!("failed to parse streaming chat chunk: {err}"))?;
        if let Some(message) = chunk.message {
            if !message.content.is_empty() {
                on_chunk(&message.content);
                content.push_str(&message.content);
            }
            native_tool_calls.extend(message.tool_calls);
        }
        if chunk.done {
            break;
        }
    }

    logging::log_llm_event(
        "ollama.chat.response_raw",
        json!({
            "stream": true,
            "chunks": raw_chunks,
        }),
    );

    finalize_reply(content, native_tool_calls, tool_names)
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
    native_tool_calls: Vec<ResponseToolCall>,
    tool_names: &[String],
) -> Result<AssistantReply, String> {
    let tool_calls = if native_tool_calls.is_empty() {
        extract_tool_calls(&content, tool_names).0
    } else {
        native_tool_calls
            .into_iter()
            .enumerate()
            .map(|(index, tool_call)| ToolCall {
                id: tool_call
                    .id
                    .unwrap_or_else(|| format!("native-{}", index + 1)),
                name: normalize_tool_name(&tool_call.function.name, tool_names),
                arguments: normalize_native_arguments(tool_call.function.arguments),
            })
            .collect()
    };
    let (_, cleaned_content) = if tool_calls.is_empty() {
        (
            Vec::new(),
            crate::ollama::xml_fallback::strip_think_tags(&content),
        )
    } else {
        extract_tool_calls(&content, tool_names)
    };

    let reply = AssistantReply {
        content: cleaned_content,
        tool_calls,
    };
    logging::log_llm_event(
        "ollama.chat.reply_final",
        json!({
            "content": truncate_for_log(&reply.content, 100_000),
            "tool_calls": reply.tool_calls,
        }),
    );
    Ok(reply)
}

fn normalize_tool_name(name: &str, allowed_tools: &[String]) -> String {
    allowed_tools
        .iter()
        .find(|candidate| candidate.eq_ignore_ascii_case(name))
        .cloned()
        .unwrap_or_else(|| name.to_string())
}

fn normalize_native_arguments(arguments: Value) -> Value {
    match arguments {
        Value::String(raw) => {
            extract_tool_calls(&format!("<function name=\"noop\">{raw}</function>"), &[])
                .0
                .into_iter()
                .next()
                .map(|call| call.arguments)
                .unwrap_or_else(|| {
                    serde_json::from_str::<Value>(&raw).unwrap_or(Value::String(raw))
                })
        }
        other => other,
    }
}
