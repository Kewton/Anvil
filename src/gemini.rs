use std::path::Path;

use reqwest::blocking::Client;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::api_keys::load_api_key;
use crate::logging;
use crate::ollama::parsing::{AssistantReply, tool_names, truncate_for_log};
use crate::ollama::xml_fallback::{ToolCall, extract_tool_calls, strip_think_tags};
use crate::provider_timeout;
use crate::session::store::ConversationMessage;
use crate::tools::registry::ToolSpec;

const GEMINI_BASE_URL: &str = "https://generativelanguage.googleapis.com";

#[derive(Debug, Clone)]
pub struct GeminiClient {
    api_key: String,
    http: Client,
    timeout_secs: u64,
    max_predict: usize,
}

impl GeminiClient {
    pub fn from_env(
        workspace_root: &Path,
        timeout_secs: u64,
        max_predict: usize,
    ) -> Result<Self, String> {
        let api_key = load_api_key(workspace_root, "GEMINI_API_KEY")?;
        Self::new(api_key, timeout_secs, max_predict)
    }

    pub fn new(api_key: String, timeout_secs: u64, max_predict: usize) -> Result<Self, String> {
        if api_key.trim().is_empty() {
            return Err("GEMINI_API_KEY is empty".to_string());
        }
        let http = provider_timeout::blocking_client(timeout_secs)
            .map_err(|err| format!("failed to create Gemini HTTP client: {err}"))?;
        Ok(Self {
            api_key,
            http,
            timeout_secs,
            max_predict,
        })
    }

    pub fn chat(
        &self,
        model: &str,
        messages: &[ConversationMessage],
        tools: &[ToolSpec],
    ) -> Result<AssistantReply, String> {
        let tool_mode = !tools.is_empty();
        let temperature = if tool_mode { 0.3 } else { 0.7 };
        let tool_names_vec = tool_names(tools);
        let request_body = build_generate_request(messages, temperature, self.max_predict);
        let prompt_metrics =
            logging::build_prompt_log_metrics(messages, render_gemini_prompt(messages));

        logging::log_llm_event(
            "gemini.generate.request",
            json!({
                "base_url": GEMINI_BASE_URL,
                "model": model,
                "tool_mode": tool_mode,
                "temperature": temperature,
                "max_output_tokens": self.max_predict,
                "tools": tool_names_vec,
                "messages": messages,
                "prompt_metrics": prompt_metrics,
            }),
        );

        let url = format!(
            "{}/v1beta/{}:generateContent",
            GEMINI_BASE_URL,
            normalize_model_path(model)
        );
        let response = self
            .http
            .post(url)
            .query(&[("key", self.api_key.as_str())])
            .json(&request_body)
            .send()
            .map_err(|err| {
                logging::log_llm_event(
                    "gemini.generate.error",
                    json!({
                        "model": model,
                        "kind": "transport",
                        "error": err.to_string(),
                    }),
                );
                self.map_transport_error("generateContent API", err)
            })?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().unwrap_or_default();
            logging::log_llm_event(
                "gemini.generate.error",
                json!({
                    "model": model,
                    "kind": "status",
                    "status": status.as_u16(),
                    "body": truncate_for_log(&body, 20_000),
                }),
            );
            return Err(format!(
                "Gemini generateContent failed: {status}: {}",
                truncate_for_log(&body, 2_000)
            ));
        }

        let body = response
            .text()
            .map_err(|err| self.map_response_body_error("response", err))?;
        logging::log_llm_event(
            "gemini.generate.response_raw",
            json!({
                "model": model,
                "body": truncate_for_log(&body, 200_000),
            }),
        );
        parse_gemini_response(&body, &tool_names_vec)
    }

    pub fn chat_text(
        &self,
        model: &str,
        messages: &[ConversationMessage],
    ) -> Result<AssistantReply, String> {
        self.chat(model, messages, &[])
    }

    fn map_transport_error(&self, operation: &str, err: reqwest::Error) -> String {
        if err.is_timeout() {
            provider_timeout::provider_turn_timeout_message(
                "Gemini",
                operation,
                self.timeout_secs,
                err,
            )
        } else {
            format!("failed to contact Gemini {operation}: {err}")
        }
    }

    fn map_response_body_error(&self, response_label: &str, err: reqwest::Error) -> String {
        if err.is_timeout() {
            provider_timeout::provider_turn_timeout_message(
                "Gemini",
                response_label,
                self.timeout_secs,
                err,
            )
        } else {
            format!("failed to decode Gemini {response_label}: {err}")
        }
    }
}

fn normalize_model_path(model: &str) -> String {
    let trimmed = model.trim();
    if trimmed.starts_with("models/") {
        trimmed.to_string()
    } else {
        format!("models/{trimmed}")
    }
}

fn build_generate_request(
    messages: &[ConversationMessage],
    temperature: f32,
    max_predict: usize,
) -> Value {
    let mut system_parts = Vec::new();
    let mut contents = Vec::new();

    for message in messages {
        match message.role.as_str() {
            "system" => {
                if !message.content.trim().is_empty() {
                    system_parts.push(json!({ "text": message.content }));
                }
            }
            "assistant" => contents.push(gemini_content("model", &assistant_content(message))),
            "tool" => {
                let label = message.name.as_deref().unwrap_or("tool");
                contents.push(gemini_content(
                    "user",
                    &format!("[tool result {label}]\n{}", message.content),
                ));
            }
            "user" => contents.push(gemini_content("user", &message.content)),
            _ => contents.push(gemini_content("user", &message.content)),
        }
    }

    if contents.is_empty() {
        contents.push(gemini_content("user", ""));
    }

    let mut body = json!({
        "contents": contents,
        "generationConfig": {
            "temperature": temperature,
            "maxOutputTokens": max_predict,
        },
    });

    if !system_parts.is_empty() {
        body["systemInstruction"] = json!({ "parts": system_parts });
    }

    body
}

fn gemini_content(role: &str, text: &str) -> Value {
    json!({
        "role": role,
        "parts": [{ "text": text }],
    })
}

fn assistant_content(message: &ConversationMessage) -> String {
    let mut text = message.content.clone();
    for tool_call in &message.tool_calls {
        text.push_str("\n<anvil_tool_call>");
        text.push_str(
            &serde_json::to_string(&json!({
                "name": tool_call.name,
                "arguments": tool_call.arguments,
            }))
            .unwrap_or_default(),
        );
        text.push_str("</anvil_tool_call>");
    }
    text
}

fn render_gemini_prompt(messages: &[ConversationMessage]) -> String {
    messages
        .iter()
        .map(|message| match message.role.as_str() {
            "assistant" => format!("assistant: {}", assistant_content(message)),
            "tool" => {
                let label = message.name.as_deref().unwrap_or("tool");
                format!("tool {label}: {}", message.content)
            }
            role => format!("{role}: {}", message.content),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[derive(Deserialize)]
struct GeminiResponse {
    #[serde(default)]
    candidates: Vec<GeminiCandidate>,
    #[serde(default, rename = "usageMetadata")]
    usage_metadata: Option<GeminiUsageMetadata>,
}

#[derive(Default, Deserialize)]
struct GeminiCandidate {
    #[serde(default)]
    content: GeminiContent,
    #[serde(default, rename = "finishReason")]
    finish_reason: String,
}

#[derive(Default, Deserialize)]
struct GeminiContent {
    #[serde(default)]
    parts: Vec<GeminiPart>,
}

#[derive(Default, Deserialize)]
struct GeminiPart {
    #[serde(default)]
    text: String,
}

#[derive(Default, Deserialize)]
struct GeminiUsageMetadata {
    #[serde(default, rename = "promptTokenCount")]
    prompt_token_count: Option<u64>,
    #[serde(default, rename = "candidatesTokenCount")]
    candidates_token_count: Option<u64>,
}

fn parse_gemini_response(body: &str, tool_names: &[String]) -> Result<AssistantReply, String> {
    let parsed: GeminiResponse = serde_json::from_str(body)
        .map_err(|err| format!("failed to decode Gemini response: {err}"))?;
    let candidate = parsed
        .candidates
        .first()
        .ok_or_else(|| "Gemini response did not include a candidate".to_string())?;
    let content = candidate
        .content
        .parts
        .iter()
        .map(|part| part.text.as_str())
        .collect::<String>();
    let (tool_calls, cleaned_content) = extract_tool_calls(&content, tool_names);
    detect_malformed_tool_call(&content, &tool_calls, &candidate.finish_reason)?;
    let reply = AssistantReply {
        content: cleaned_content,
        tool_calls,
        prompt_tokens: parsed
            .usage_metadata
            .as_ref()
            .and_then(|usage| usage.prompt_token_count),
        completion_tokens: parsed
            .usage_metadata
            .as_ref()
            .and_then(|usage| usage.candidates_token_count),
    };
    logging::log_llm_event(
        "gemini.generate.reply_final",
        json!({
            "content": truncate_for_log(&reply.content, 100_000),
            "tool_calls": reply.tool_calls,
            "prompt_tokens": reply.prompt_tokens,
            "completion_tokens": reply.completion_tokens,
        }),
    );
    Ok(reply)
}

fn detect_malformed_tool_call(
    content: &str,
    tool_calls: &[ToolCall],
    finish_reason: &str,
) -> Result<(), String> {
    if !tool_calls.is_empty() {
        return Ok(());
    }

    let stripped = strip_think_tags(content);
    let has_tool_markup = stripped.contains("<anvil_tool_call")
        || stripped.contains("</anvil_tool_call>")
        || stripped.contains("<function_call")
        || stripped.contains("</function_call>")
        || stripped.contains("<function=")
        || stripped.contains("<function ");
    if !has_tool_markup {
        return Ok(());
    }

    let lower_finish_reason = finish_reason.to_ascii_lowercase();
    if matches!(lower_finish_reason.as_str(), "max_tokens" | "length") {
        return Err(
            "tool call parser failed: truncated tool call (Gemini response hit output token limit)"
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
    use super::parse_gemini_response;

    #[test]
    fn parses_xml_tool_call_from_gemini_response() {
        let body = r#"{
          "candidates": [{
            "content": {
              "parts": [{
                "text": "<anvil_tool_call>{\"name\":\"Write\",\"arguments\":{\"path\":\"hello.txt\",\"content\":\"hi\"}}</anvil_tool_call>"
              }]
            },
            "finishReason": "STOP"
          }],
          "usageMetadata": {
            "promptTokenCount": 10,
            "candidatesTokenCount": 20
          }
        }"#;
        let reply = parse_gemini_response(body, &["Write".to_string()]).unwrap();
        assert_eq!(reply.tool_calls.len(), 1);
        assert_eq!(reply.tool_calls[0].name, "Write");
        assert_eq!(reply.prompt_tokens, Some(10));
        assert_eq!(reply.completion_tokens, Some(20));
    }

    #[test]
    fn rejects_malformed_tool_markup() {
        let body = r#"{
          "candidates": [{
            "content": { "parts": [{ "text": "<anvil_tool_call>{\"name\":\"Write\"" }] },
            "finishReason": "MAX_TOKENS"
          }]
        }"#;
        let err = parse_gemini_response(body, &["Write".to_string()]).unwrap_err();
        assert!(err.contains("truncated tool call"), "got: {err}");
    }
}
