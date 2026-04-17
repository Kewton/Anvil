use reqwest::blocking::Client;
use serde_json::json;

use crate::logging;
use crate::ollama::fallback;
use crate::ollama::parsing::{parse_streaming_chat_response, tool_names, truncate_for_log};
use crate::ollama::transport::{ChatTransport, is_native_tool_parse_failure};
use crate::session::store::ConversationMessage;
use crate::tools::registry::ToolSpec;

pub use crate::ollama::parsing::AssistantReply;
pub use crate::ollama::parsing::{parse_chat_response, parse_tags_response};
pub use crate::ollama::transport::should_use_native_tool_calls;

#[derive(Debug, Clone)]
pub struct OllamaClient {
    base_url: String,
    http: Client,
    context_window: usize,
    max_predict: usize,
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
        self.chat_impl(
            model,
            messages,
            Some(tools),
            false,
            should_use_native_tool_calls(model),
            |_| {},
        )
    }

    pub fn chat_with_mode(
        &self,
        model: &str,
        messages: &[ConversationMessage],
        tools: &[ToolSpec],
        native_tools_enabled: bool,
    ) -> Result<AssistantReply, String> {
        self.chat_impl(
            model,
            messages,
            Some(tools),
            false,
            native_tools_enabled,
            |_| {},
        )
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
        self.chat_impl(
            model,
            messages,
            Some(tools),
            true,
            should_use_native_tool_calls(model),
            on_chunk,
        )
    }

    pub fn chat_streaming_with_mode<F>(
        &self,
        model: &str,
        messages: &[ConversationMessage],
        tools: &[ToolSpec],
        native_tools_enabled: bool,
        on_chunk: F,
    ) -> Result<AssistantReply, String>
    where
        F: FnMut(&str),
    {
        self.chat_impl(
            model,
            messages,
            Some(tools),
            true,
            native_tools_enabled,
            on_chunk,
        )
    }

    pub fn chat_text(
        &self,
        model: &str,
        messages: &[ConversationMessage],
    ) -> Result<AssistantReply, String> {
        self.chat_impl(
            model,
            messages,
            None,
            false,
            should_use_native_tool_calls(model),
            |_| {},
        )
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
        native_tools_enabled: bool,
        mut on_chunk: F,
    ) -> Result<AssistantReply, String>
    where
        F: FnMut(&str),
    {
        let tool_mode = tools.is_some_and(|tool_specs| !tool_specs.is_empty());
        let serialized_tools = if native_tools_enabled { tools } else { None };
        let temperature = if tool_mode { 0.3 } else { 0.7 };
        let tool_names = tool_names(tools.unwrap_or(&[]));
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
                "tools": tool_names,
                "messages": messages,
            }),
        );

        let transport = ChatTransport::new(
            &self.base_url,
            &self.http,
            self.context_window,
            self.max_predict,
        );
        let response = transport
            .send_chat_request(model, messages, serialized_tools, stream, temperature)
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
                && let Ok(reply) = fallback::salvage_malformed_tool_call(
                    &transport,
                    model,
                    messages,
                    tools.unwrap_or(&[]),
                )
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
            if native_tools_enabled
                && tool_mode
                && is_native_tool_parse_failure(status.as_u16(), &body)
            {
                return Err(format!(
                    "native tool parser failed: {}",
                    truncate_for_log(&body, 500)
                ));
            }
            return Err(format!("Ollama /api/chat failed: {status}"));
        }

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
