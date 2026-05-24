use reqwest::blocking::{Client, Response};
use serde::Serialize;

use crate::model_capabilities::model_capabilities;
use crate::session::store::ConversationMessage;
use crate::tools::registry::ToolSpec;

#[derive(Serialize)]
struct RequestOptions {
    temperature: f32,
    num_ctx: usize,
    num_predict: usize,
}

#[derive(Serialize)]
struct GenerateRequest<'a> {
    model: &'a str,
    prompt: &'a str,
    raw: bool,
    stream: bool,
    keep_alive: i32,
    options: RequestOptions,
}

#[derive(Serialize)]
struct ChatMessage {
    role: String,
    content: String,
}

#[derive(Serialize)]
struct ChatToolDefinition {
    #[serde(rename = "type")]
    kind: String,
    function: ChatToolFunction,
}

#[derive(Serialize)]
struct ChatToolFunction {
    name: String,
    description: String,
    parameters: serde_json::Value,
}

#[derive(Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: Vec<ChatMessage>,
    stream: bool,
    think: bool,
    keep_alive: i32,
    options: RequestOptions,
    #[serde(skip_serializing_if = "Option::is_none")]
    format: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<ChatToolDefinition>>,
}

pub(crate) struct GenerateTransport<'a> {
    base_url: &'a str,
    http: &'a Client,
    context_window: usize,
    max_predict: usize,
}

impl<'a> GenerateTransport<'a> {
    pub(crate) fn new(
        base_url: &'a str,
        http: &'a Client,
        context_window: usize,
        max_predict: usize,
    ) -> Self {
        Self {
            base_url,
            http,
            context_window,
            max_predict,
        }
    }

    pub(crate) fn send_generate_request(
        &self,
        model: &str,
        prompt: &str,
        stream: bool,
        temperature: f32,
    ) -> Result<Response, reqwest::Error> {
        let request = GenerateRequest {
            model,
            prompt,
            raw: true,
            stream,
            keep_alive: -1,
            options: self.request_options(temperature),
        };
        self.http
            .post(format!("{}/api/generate", self.base_url))
            .json(&request)
            .send()
    }

    fn request_options(&self, temperature: f32) -> RequestOptions {
        RequestOptions {
            temperature,
            num_ctx: self.context_window,
            num_predict: self.max_predict,
        }
    }

    pub(crate) fn send_chat_request_with_format(
        &self,
        model: &str,
        messages: &[ConversationMessage],
        tools: &[ToolSpec],
        stream: bool,
        temperature: f32,
        response_format: Option<&str>,
    ) -> Result<Response, reqwest::Error> {
        let request = ChatRequest {
            model,
            messages: to_chat_messages(messages),
            stream,
            think: false,
            keep_alive: -1,
            options: self.request_options(temperature),
            format: response_format,
            tools: (!tools.is_empty()).then(|| to_chat_tool_definitions(tools)),
        };
        self.http
            .post(format!("{}/api/chat", self.base_url))
            .json(&request)
            .send()
    }
}

fn to_chat_messages(messages: &[ConversationMessage]) -> Vec<ChatMessage> {
    let mut chat_messages = Vec::with_capacity(messages.len());
    for message in messages {
        match message.role.as_str() {
            "system" | "user" | "assistant" => chat_messages.push(ChatMessage {
                role: message.role.clone(),
                content: message.content.clone(),
            }),
            "tool" => {
                let label = message.name.as_deref().unwrap_or("tool");
                chat_messages.push(ChatMessage {
                    role: "user".to_string(),
                    content: format!("[tool result {label}]\n{}", message.content),
                });
            }
            _ => chat_messages.push(ChatMessage {
                role: "user".to_string(),
                content: message.content.clone(),
            }),
        }
    }
    chat_messages
}

fn to_chat_tool_definitions(tools: &[ToolSpec]) -> Vec<ChatToolDefinition> {
    tools
        .iter()
        .map(|tool| ChatToolDefinition {
            kind: tool.kind.clone(),
            function: ChatToolFunction {
                name: tool.function.name.clone(),
                description: tool.function.description.clone(),
                parameters: tool.function.parameters.clone(),
            },
        })
        .collect()
}

pub fn should_use_native_tool_calls(model: &str) -> bool {
    model_capabilities(model).native_tool_calls
}

#[cfg(test)]
mod tests {
    use super::should_use_native_tool_calls;

    #[test]
    fn native_tool_allowlist_is_narrow() {
        assert!(should_use_native_tool_calls("qwen3.6:27b-coding-nvfp4"));
        assert!(!should_use_native_tool_calls("qwen3.5:122b"));
        assert!(!should_use_native_tool_calls("qwen3.5:9b"));
    }
}
