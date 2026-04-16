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
    context_window: usize,
    max_predict: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssistantReply {
    pub content: String,
    pub tool_calls: Vec<ToolCall>,
}

impl OllamaClient {
    pub fn new(base_url: String) -> Result<Self, String> {
        Self::new_with_timeout_and_options(base_url, 120, 24_000, 2_048)
    }

    pub fn new_with_timeout(base_url: String, timeout_secs: u64) -> Result<Self, String> {
        Self::new_with_timeout_and_options(base_url, timeout_secs, 24_000, 2_048)
    }

    pub fn new_with_timeout_and_options(
        base_url: String,
        timeout_secs: u64,
        context_window: usize,
        max_predict: usize,
    ) -> Result<Self, String> {
        let http = Client::builder()
            .timeout(std::time::Duration::from_secs(timeout_secs))
            .build()
            .map_err(|err| format!("failed to create HTTP client: {err}"))?;
        Ok(Self {
            base_url,
            http,
            context_window,
            max_predict,
        })
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
        struct RequestOptions {
            temperature: f32,
            num_ctx: usize,
            num_predict: usize,
        }

        #[derive(Serialize)]
        struct ChatRequest<'a> {
            model: &'a str,
            stream: bool,
            messages: &'a [ConversationMessage],
            keep_alive: i32,
            options: RequestOptions,
            #[serde(skip_serializing_if = "Option::is_none")]
            tools: Option<&'a [ToolSpec]>,
        }

        let native_tools_enabled = should_use_native_tool_calls(model);
        let tool_mode = tools.is_some_and(|tool_specs| !tool_specs.is_empty());
        let serialized_tools = if native_tools_enabled { tools } else { None };
        let temperature = if tool_mode { 0.3 } else { 0.7 };
        logging::log_llm_event(
            "ollama.chat.request",
            json!({
                "base_url": self.base_url,
                "model": model,
                "stream": stream,
                "tool_mode": tool_mode,
                "native_tools_enabled": native_tools_enabled,
                "temperature": temperature,
                "num_ctx": self.context_window,
                "num_predict": self.max_predict,
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
                keep_alive: -1,
                options: RequestOptions {
                    temperature,
                    num_ctx: self.context_window,
                    num_predict: self.max_predict,
                },
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
            if native_tools_enabled
                && tool_mode
                && is_native_tool_parse_failure(status.as_u16(), &body)
                && let Ok(reply) =
                    self.salvage_malformed_tool_call(model, messages, tools.unwrap_or(&[]))
            {
                logging::log_llm_event(
                    "ollama.chat.salvage_success",
                    json!({
                        "model": model,
                        "status": status.as_u16(),
                    }),
                );
                return Ok(reply);
            }
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

    fn salvage_malformed_tool_call(
        &self,
        model: &str,
        messages: &[ConversationMessage],
        tools: &[ToolSpec],
    ) -> Result<AssistantReply, String> {
        #[derive(Serialize)]
        struct RequestOptions {
            temperature: f32,
            num_ctx: usize,
            num_predict: usize,
        }

        #[derive(Serialize)]
        struct ChatRequest<'a> {
            model: &'a str,
            stream: bool,
            messages: &'a [ConversationMessage],
            keep_alive: i32,
            options: RequestOptions,
        }

        let mut salvage_messages = messages.to_vec();
        salvage_messages.push(ConversationMessage::system(
            "The previous native tool call response was malformed and rejected by the runtime parser. Retry immediately. Return exactly one valid next action. If native tool calling fails again, emit a single <function name=\"Tool\">{\"key\":\"value\"}</function> block with valid JSON arguments and no extra prose."
                .to_string(),
        ));

        logging::log_llm_event(
            "ollama.chat.salvage_request",
            json!({
                "model": model,
                "tools": tools.iter().map(|tool| tool.function.name.clone()).collect::<Vec<_>>(),
            }),
        );

        let response = self
            .http
            .post(format!("{}/api/chat", self.base_url))
            .json(&ChatRequest {
                model,
                stream: false,
                messages: &salvage_messages,
                keep_alive: -1,
                options: RequestOptions {
                    temperature: 0.2,
                    num_ctx: self.context_window,
                    num_predict: self.max_predict,
                },
            })
            .send()
            .map_err(|err| format!("failed salvage chat retry: {err}"))?;

        if !response.status().is_success() {
            return Err(format!(
                "salvage Ollama /api/chat failed: {}",
                response.status()
            ));
        }

        let body = response
            .text()
            .map_err(|err| format!("failed to decode salvage chat response: {err}"))?;
        logging::log_llm_event(
            "ollama.chat.salvage_response_raw",
            json!({
                "model": model,
                "body": truncate_for_log(&body, 200_000),
            }),
        );
        let tool_names = tools
            .iter()
            .map(|tool| tool.function.name.clone())
            .collect::<Vec<_>>();
        parse_chat_response(&body, &tool_names)
    }
}

pub fn should_use_native_tool_calls(model: &str) -> bool {
    !model.trim().is_empty()
}

fn is_native_tool_parse_failure(status: u16, body: &str) -> bool {
    if status != 500 && status != 400 {
        return false;
    }
    let lower = body.to_ascii_lowercase();
    lower.contains("xml syntax error")
        || lower.contains("unexpected end element")
        || lower.contains("unexpected eof")
        || lower.contains("</function>")
        || lower.contains("tool call")
        || lower.contains("function")
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

fn truncate_for_log(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let truncated = text.chars().take(max_chars).collect::<String>();
    format!("{truncated}\n...[truncated]")
}
