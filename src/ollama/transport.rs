use reqwest::blocking::{Client, Response};
use serde::Serialize;

use crate::session::store::ConversationMessage;
use crate::tools::registry::ToolSpec;

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

pub(crate) struct ChatTransport<'a> {
    base_url: &'a str,
    http: &'a Client,
    context_window: usize,
    max_predict: usize,
}

impl<'a> ChatTransport<'a> {
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

    pub(crate) fn send_chat_request(
        &self,
        model: &str,
        messages: &[ConversationMessage],
        tools: Option<&[ToolSpec]>,
        stream: bool,
        temperature: f32,
    ) -> Result<Response, reqwest::Error> {
        let request = ChatRequest {
            model,
            stream,
            messages,
            keep_alive: -1,
            options: self.request_options(temperature),
            tools,
        };
        self.http
            .post(format!("{}/api/chat", self.base_url))
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
}

pub fn should_use_native_tool_calls(model: &str) -> bool {
    !model.trim().is_empty()
}

pub(crate) fn is_native_tool_parse_failure(status: u16, body: &str) -> bool {
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
