use reqwest::blocking::Client;
use serde_json::json;

use crate::logging;
use crate::ollama::parsing::{
    parse_streaming_chat_response, parse_streaming_generate_response, tool_names, truncate_for_log,
};
use crate::ollama::transport::GenerateTransport;
use crate::session::store::ConversationMessage;
use crate::tools::registry::ToolSpec;

pub use crate::ollama::parsing::AssistantReply;
pub use crate::ollama::parsing::{
    parse_chat_response, parse_generate_response, parse_tags_response,
};
pub use crate::ollama::transport::should_use_native_tool_calls;

#[derive(Debug, Clone)]
pub struct OllamaClient {
    base_url: String,
    http: Client,
    context_window: usize,
    max_predict: usize,
    timeout_secs: u64,
}

pub(crate) const SIDECAR_SUMMARY_TIMEOUT_SECS: u64 = 8;
const SIDECAR_SUMMARY_MAX_PREDICT: usize = 384;
const CLASSIFIER_TIMEOUT_SECS: u64 = 20;
const CLASSIFIER_MAX_PREDICT: usize = 160;
const STAGE_THREE_FALLBACK_TIMEOUT_SECS: u64 = 6;
const STAGE_THREE_FALLBACK_MAX_PREDICT: usize = 96;

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
            .connect_timeout(std::time::Duration::from_secs(timeout_secs))
            .timeout(std::time::Duration::from_secs(timeout_secs))
            .build()
            .map_err(|err| format!("failed to create HTTP client: {err}"))?;
        Ok(Self {
            base_url,
            http,
            context_window,
            max_predict,
            timeout_secs,
        })
    }

    pub fn timeout_secs(&self) -> u64 {
        self.timeout_secs
    }

    pub fn clone_with_overrides(
        &self,
        timeout_secs: u64,
        max_predict: usize,
    ) -> Result<Self, String> {
        Self::new_with_timeout_and_options(
            self.base_url.clone(),
            timeout_secs,
            self.context_window,
            max_predict,
        )
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
        self.generate_impl(model, messages, Some(tools), false, |_| Ok(()))
    }

    pub fn chat_with_mode(
        &self,
        model: &str,
        messages: &[ConversationMessage],
        tools: &[ToolSpec],
        native_tools_enabled: bool,
    ) -> Result<AssistantReply, String> {
        if native_tools_enabled {
            self.chat_impl(model, messages, tools, false, |_| Ok(()))
        } else {
            self.generate_impl(model, messages, Some(tools), false, |_| Ok(()))
        }
    }

    pub fn chat_streaming<F>(
        &self,
        model: &str,
        messages: &[ConversationMessage],
        tools: &[ToolSpec],
        on_chunk: F,
    ) -> Result<AssistantReply, String>
    where
        F: FnMut(&str) -> Result<(), String>,
    {
        self.generate_impl(model, messages, Some(tools), true, on_chunk)
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
        F: FnMut(&str) -> Result<(), String>,
    {
        if native_tools_enabled {
            self.chat_impl(model, messages, tools, true, on_chunk)
        } else {
            self.generate_impl(model, messages, Some(tools), true, on_chunk)
        }
    }

    pub fn chat_text(
        &self,
        model: &str,
        messages: &[ConversationMessage],
    ) -> Result<AssistantReply, String> {
        self.generate_impl(model, messages, None, false, |_| Ok(()))
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
        let summary_client = Self::new_with_timeout_and_options(
            self.base_url.clone(),
            SIDECAR_SUMMARY_TIMEOUT_SECS,
            self.context_window,
            SIDECAR_SUMMARY_MAX_PREDICT,
        )?;
        let reply = summary_client.chat_text(model, &summary_messages)?;
        if !reply.tool_calls.is_empty() {
            return Err("sidecar summary unexpectedly requested tools".to_string());
        }
        let summary = reply.content.trim();
        if summary.is_empty() {
            return Err("sidecar summary was empty".to_string());
        }
        Ok(summary.to_string())
    }

    pub fn classify_task_request(
        &self,
        model: &str,
        messages: &[ConversationMessage],
    ) -> Result<AssistantReply, String> {
        let classifier_client = Self::new_with_timeout_and_options(
            self.base_url.clone(),
            CLASSIFIER_TIMEOUT_SECS,
            self.context_window,
            CLASSIFIER_MAX_PREDICT,
        )?;
        classifier_client.chat_text(model, messages)
    }

    pub fn classify_stage_three_fallback(
        &self,
        model: &str,
        messages: &[ConversationMessage],
    ) -> Result<AssistantReply, String> {
        let fallback_client = Self::new_with_timeout_and_options(
            self.base_url.clone(),
            STAGE_THREE_FALLBACK_TIMEOUT_SECS,
            self.context_window,
            STAGE_THREE_FALLBACK_MAX_PREDICT,
        )?;
        fallback_client.chat_text(model, messages)
    }

    fn generate_impl<F>(
        &self,
        model: &str,
        messages: &[ConversationMessage],
        tools: Option<&[ToolSpec]>,
        stream: bool,
        mut on_chunk: F,
    ) -> Result<AssistantReply, String>
    where
        F: FnMut(&str) -> Result<(), String>,
    {
        let tool_mode = tools.is_some_and(|tool_specs| !tool_specs.is_empty());
        let temperature = if tool_mode { 0.3 } else { 0.7 };
        let tool_names_vec = tool_names(tools.unwrap_or(&[]));
        let prompt = flatten_messages_to_chatml(messages);

        logging::log_llm_event(
            "ollama.generate.request",
            json!({
                "base_url": self.base_url,
                "model": model,
                "stream": stream,
                "tool_mode": tool_mode,
                "temperature": temperature,
                "num_ctx": self.context_window,
                "num_predict": self.max_predict,
                "tools": tool_names_vec,
                "messages": messages,
            }),
        );

        let transport = GenerateTransport::new(
            &self.base_url,
            &self.http,
            self.context_window,
            self.max_predict,
        );
        let response = transport
            .send_generate_request(model, &prompt, stream, temperature)
            .map_err(|err| {
                logging::log_llm_event(
                    "ollama.generate.error",
                    json!({
                        "model": model,
                        "stream": stream,
                        "kind": "transport",
                        "error": err.to_string(),
                    }),
                );
                format!("failed to contact Ollama generate API: {err}")
            })?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().unwrap_or_default();
            logging::log_llm_event(
                "ollama.generate.error",
                json!({
                    "model": model,
                    "stream": stream,
                    "kind": "status",
                    "status": status.as_u16(),
                    "body": truncate_for_log(&body, 20_000),
                }),
            );
            return Err(format!("Ollama /api/generate failed: {status}"));
        }

        if stream {
            parse_streaming_generate_response(response, &tool_names_vec, &mut on_chunk)
        } else {
            let body = response
                .text()
                .map_err(|err| format!("failed to decode Ollama generate response: {err}"))?;
            logging::log_llm_event(
                "ollama.generate.response_raw",
                json!({
                    "model": model,
                    "stream": false,
                    "body": truncate_for_log(&body, 200_000),
                }),
            );
            parse_generate_response(&body, &tool_names_vec)
        }
    }

    fn chat_impl<F>(
        &self,
        model: &str,
        messages: &[ConversationMessage],
        tools: &[ToolSpec],
        stream: bool,
        mut on_chunk: F,
    ) -> Result<AssistantReply, String>
    where
        F: FnMut(&str) -> Result<(), String>,
    {
        let temperature = 0.3;
        let tool_names_vec = tool_names(tools);

        logging::log_llm_event(
            "ollama.chat.request",
            json!({
                "base_url": self.base_url,
                "model": model,
                "stream": stream,
                "temperature": temperature,
                "num_ctx": self.context_window,
                "num_predict": self.max_predict,
                "tools": tool_names_vec,
                "messages": messages,
            }),
        );

        let transport = GenerateTransport::new(
            &self.base_url,
            &self.http,
            self.context_window,
            self.max_predict,
        );
        let response = transport
            .send_chat_request(model, messages, tools, stream, temperature)
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

        if stream {
            parse_streaming_chat_response(response, &tool_names_vec, &mut on_chunk)
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
            parse_chat_response(&body, &tool_names_vec)
        }
    }
}

fn flatten_messages_to_chatml(messages: &[ConversationMessage]) -> String {
    let mut buffer = String::new();
    for message in messages {
        match message.role.as_str() {
            "system" => {
                buffer.push_str("<|im_start|>system\n");
                buffer.push_str(&message.content);
                buffer.push_str("<|im_end|>\n");
            }
            "user" => {
                buffer.push_str("<|im_start|>user\n");
                buffer.push_str(&message.content);
                buffer.push_str("<|im_end|>\n");
            }
            "assistant" => {
                buffer.push_str("<|im_start|>assistant\n");
                buffer.push_str(&message.content);
                if !message.tool_calls.is_empty() {
                    for tool_call in &message.tool_calls {
                        buffer.push_str("\n<anvil_tool_call>");
                        buffer.push_str(
                            &serde_json::to_string(&serde_json::json!({
                                "name": tool_call.name,
                                "arguments": tool_call.arguments,
                            }))
                            .unwrap_or_default(),
                        );
                        buffer.push_str("</anvil_tool_call>");
                    }
                }
                buffer.push_str("<|im_end|>\n");
            }
            "tool" => {
                let label = message.name.as_deref().unwrap_or("tool");
                buffer.push_str("<|im_start|>user\n");
                buffer.push_str(&format!("[tool result {label}]\n"));
                buffer.push_str(&message.content);
                buffer.push_str("<|im_end|>\n");
            }
            _ => {
                buffer.push_str("<|im_start|>user\n");
                buffer.push_str(&message.content);
                buffer.push_str("<|im_end|>\n");
            }
        }
    }
    buffer.push_str("<|im_start|>assistant\n");
    buffer
}
