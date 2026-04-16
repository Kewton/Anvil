use std::io::{BufRead, BufReader};

use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::logging;
use crate::ollama::xml_fallback::{ToolCall, extract_tool_calls};
use crate::session::store::ConversationMessage;
use crate::tools::registry::ToolSpec;

#[derive(Debug, Clone)]
pub struct OllamaClient {
    base_url: String,
    http: Client,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssistantReply {
    pub content: String,
    pub tool_calls: Vec<ToolCall>,
}

impl OllamaClient {
    pub fn new(base_url: String) -> Result<Self, String> {
        Self::new_with_timeout(base_url, 120)
    }

    pub fn new_with_timeout(base_url: String, timeout_secs: u64) -> Result<Self, String> {
        let http = Client::builder()
            .timeout(std::time::Duration::from_secs(timeout_secs))
            .build()
            .map_err(|err| format!("failed to create HTTP client: {err}"))?;
        Ok(Self { base_url, http })
    }

    pub fn list_models(&self) -> Result<Vec<String>, String> {
        let response = self
            .http
            .get(format!("{}/api/tags", self.base_url))
            .send()
            .map_err(|err| format!("failed to contact Ollama: {err}"))?;
        if !response.status().is_success() {
            return Err(format!("Ollama /api/tags failed: {}", response.status()));
        }
        let body = response
            .text()
            .map_err(|err| format!("failed to decode Ollama tags response: {err}"))?;
        logging::log_llm_event(
            "ollama.tags.response",
            json!({
                "base_url": self.base_url,
                "body": truncate_for_log(&body, 20_000),
            }),
        );
        parse_tags_response(&body)
    }

    pub fn chat(
        &self,
        model: &str,
        messages: &[ConversationMessage],
        tools: &[ToolSpec],
    ) -> Result<AssistantReply, String> {
        self.chat_impl(model, messages, Some(tools), false, |_| {})
    }

    pub fn chat_streaming<F>(
        &self,
        model: &str,
        messages: &[ConversationMessage],
        tools: &[ToolSpec],
        on_chunk: F,
    ) -> Result<AssistantReply, String>
    where
        F: FnMut(&str),
    {
        self.chat_impl(model, messages, Some(tools), true, on_chunk)
    }

    pub fn chat_text(
        &self,
        model: &str,
        messages: &[ConversationMessage],
    ) -> Result<AssistantReply, String> {
        self.chat_impl(model, messages, None, false, |_| {})
    }

    pub fn summarize_conversation(
        &self,
        model: &str,
        messages: &[ConversationMessage],
    ) -> Result<String, String> {
        let transcript = crate::session::compact::render_messages_for_summary(messages, 16_000);
        let summary_messages = vec![
            ConversationMessage::system(
                "You compress earlier conversation for a local coding agent. Summarize the user goal, repository facts learned, files already changed, current plan status, open risks, and next actions. Keep it concise, factual, and under 220 words. Reply only with the summary.".to_string(),
            ),
            ConversationMessage::user(transcript),
        ];
        let reply = self.chat_text(model, &summary_messages)?;
        if !reply.tool_calls.is_empty() {
            return Err("sidecar summary unexpectedly requested tools".to_string());
        }
        let summary = reply.content.trim();
        if summary.is_empty() {
            return Err("sidecar summary was empty".to_string());
        }
        Ok(summary.to_string())
    }

    fn chat_impl<F>(
        &self,
        model: &str,
        messages: &[ConversationMessage],
        tools: Option<&[ToolSpec]>,
        stream: bool,
        mut on_chunk: F,
    ) -> Result<AssistantReply, String>
    where
        F: FnMut(&str),
    {
        #[derive(Serialize)]
        struct ChatRequest<'a> {
            model: &'a str,
            stream: bool,
            messages: &'a [ConversationMessage],
            #[serde(skip_serializing_if = "Option::is_none")]
            tools: Option<&'a [ToolSpec]>,
        }

        let native_tools_enabled = should_use_native_tool_calls(model);
        let serialized_tools = if native_tools_enabled { tools } else { None };
        logging::log_llm_event(
            "ollama.chat.request",
            json!({
                "base_url": self.base_url,
                "model": model,
                "stream": stream,
                "native_tools_enabled": native_tools_enabled,
                "tools": tools.unwrap_or(&[]).iter().map(|tool| tool.function.name.clone()).collect::<Vec<_>>(),
                "messages": messages,
            }),
        );

        let response = self
            .http
            .post(format!("{}/api/chat", self.base_url))
            .json(&ChatRequest {
                model,
                stream,
                messages,
                tools: serialized_tools,
            })
            .send()
            .map_err(|err| {
                logging::log_llm_event(
                    "ollama.chat.error",
                    json!({
                        "model": model,
                        "stream": stream,
                        "kind": "transport",
                        "error": err.to_string(),
                    }),
                );
                format!("failed to contact Ollama chat API: {err}")
            })?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().unwrap_or_default();
            logging::log_llm_event(
                "ollama.chat.error",
                json!({
                    "model": model,
                    "stream": stream,
                    "kind": "status",
                    "status": status.as_u16(),
                    "body": truncate_for_log(&body, 20_000),
                }),
            );
            return Err(format!("Ollama /api/chat failed: {status}"));
        }

        let tool_names = tools
            .unwrap_or(&[])
            .iter()
            .map(|tool| tool.function.name.clone())
            .collect::<Vec<_>>();

        if stream {
            parse_streaming_chat_response(response, &tool_names, &mut on_chunk)
        } else {
            let body = response
                .text()
                .map_err(|err| format!("failed to decode Ollama chat response: {err}"))?;
            logging::log_llm_event(
                "ollama.chat.response_raw",
                json!({
                    "model": model,
                    "stream": false,
                    "body": truncate_for_log(&body, 200_000),
                }),
            );
            parse_chat_response(&body, &tool_names)
        }
    }
}

pub fn should_use_native_tool_calls(model: &str) -> bool {
    let normalized = model.to_ascii_lowercase();
    !(normalized.contains("qwen3.5") || normalized.starts_with("qwen3:"))
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

fn parse_streaming_chat_response<F>(
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
                name: tool_call.function.name,
                arguments: tool_call.function.arguments,
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

fn truncate_for_log(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let truncated = text.chars().take(max_chars).collect::<String>();
    format!("{truncated}\n...[truncated]")
}
