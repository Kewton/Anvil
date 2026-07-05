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

const OPENAI_BASE_URL: &str = "https://api.openai.com";

#[derive(Debug, Clone)]
pub struct OpenAiClient {
    api_key: String,
    http: Client,
    timeout_secs: u64,
    max_predict: usize,
}

impl OpenAiClient {
    pub fn from_env(
        workspace_root: &Path,
        timeout_secs: u64,
        max_predict: usize,
    ) -> Result<Self, String> {
        let api_key = load_api_key(workspace_root, "OPENAI_API_KEY")?;
        Self::new(api_key, timeout_secs, max_predict)
    }

    pub fn new(api_key: String, timeout_secs: u64, max_predict: usize) -> Result<Self, String> {
        if api_key.trim().is_empty() {
            return Err("OPENAI_API_KEY is empty".to_string());
        }
        let http = provider_timeout::blocking_client(timeout_secs)
            .map_err(|err| format!("failed to create OpenAI HTTP client: {err}"))?;
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
        let tool_names_vec = tool_names(tools);
        let request_body = build_response_request(model, messages, self.max_predict);
        let prompt_metrics =
            logging::build_prompt_log_metrics(messages, render_openai_prompt(messages));

        logging::log_llm_event(
            "openai.responses.request",
            json!({
                "base_url": OPENAI_BASE_URL,
                "model": model,
                "tool_mode": tool_mode,
                "max_output_tokens": self.max_predict,
                "tools": tool_names_vec,
                "messages": messages,
                "prompt_metrics": prompt_metrics,
            }),
        );

        let response = self
            .http
            .post(format!("{OPENAI_BASE_URL}/v1/responses"))
            .bearer_auth(&self.api_key)
            .json(&request_body)
            .send()
            .map_err(|err| {
                logging::log_llm_event(
                    "openai.responses.error",
                    json!({
                        "model": model,
                        "kind": "transport",
                        "error": err.to_string(),
                    }),
                );
                self.map_transport_error("Responses API", err)
            })?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().unwrap_or_default();
            logging::log_llm_event(
                "openai.responses.error",
                json!({
                    "model": model,
                    "kind": "status",
                    "status": status.as_u16(),
                    "body": truncate_for_log(&body, 20_000),
                }),
            );
            return Err(format!(
                "OpenAI Responses API failed: {status}: {}",
                truncate_for_log(&body, 2_000)
            ));
        }

        let body = response
            .text()
            .map_err(|err| self.map_response_body_error("response", err))?;
        logging::log_llm_event(
            "openai.responses.response_raw",
            json!({
                "model": model,
                "body": truncate_for_log(&body, 200_000),
            }),
        );
        parse_openai_response(&body, &tool_names_vec)
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
                "OpenAI",
                operation,
                self.timeout_secs,
                err,
            )
        } else {
            format!("failed to contact OpenAI {operation}: {err}")
        }
    }

    fn map_response_body_error(&self, response_label: &str, err: reqwest::Error) -> String {
        if err.is_timeout() {
            provider_timeout::provider_turn_timeout_message(
                "OpenAI",
                response_label,
                self.timeout_secs,
                err,
            )
        } else {
            format!("failed to decode OpenAI {response_label}: {err}")
        }
    }
}

fn build_response_request(
    model: &str,
    messages: &[ConversationMessage],
    max_predict: usize,
) -> Value {
    json!({
        "model": model,
        "input": openai_input(messages),
        "max_output_tokens": max_predict,
    })
}

fn openai_input(messages: &[ConversationMessage]) -> Vec<Value> {
    let mut input = Vec::new();
    for message in messages {
        match message.role.as_str() {
            "system" | "developer" | "user" => {
                input.push(openai_message(&message.role, &message.content));
            }
            "assistant" => {
                input.push(openai_message("assistant", &assistant_content(message)));
            }
            "tool" => {
                let label = message.name.as_deref().unwrap_or("tool");
                input.push(openai_message(
                    "user",
                    &format!("[tool result {label}]\n{}", message.content),
                ));
            }
            _ => input.push(openai_message("user", &message.content)),
        }
    }
    if input.is_empty() {
        input.push(openai_message("user", ""));
    }
    input
}

fn openai_message(role: &str, text: &str) -> Value {
    let content_type = if role == "assistant" {
        "output_text"
    } else {
        "input_text"
    };
    json!({
        "role": role,
        "content": [
            {
                "type": content_type,
                "text": text,
            }
        ],
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

fn render_openai_prompt(messages: &[ConversationMessage]) -> String {
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
struct OpenAiResponse {
    #[serde(default)]
    output_text: Option<String>,
    #[serde(default)]
    output: Vec<OpenAiOutputItem>,
    #[serde(default)]
    usage: Option<OpenAiUsage>,
}

#[derive(Default, Deserialize)]
struct OpenAiOutputItem {
    #[serde(default)]
    content: Vec<OpenAiContentPart>,
}

#[derive(Default, Deserialize)]
struct OpenAiContentPart {
    #[serde(default)]
    text: String,
}

#[derive(Default, Deserialize)]
struct OpenAiUsage {
    #[serde(default)]
    input_tokens: Option<u64>,
    #[serde(default)]
    output_tokens: Option<u64>,
}

fn parse_openai_response(body: &str, tool_names: &[String]) -> Result<AssistantReply, String> {
    let parsed: OpenAiResponse = serde_json::from_str(body)
        .map_err(|err| format!("failed to decode OpenAI response: {err}"))?;
    let content = parsed.output_text.unwrap_or_else(|| {
        parsed
            .output
            .iter()
            .flat_map(|item| item.content.iter())
            .map(|part| part.text.as_str())
            .collect::<String>()
    });
    if content.trim().is_empty() && parsed.output.is_empty() {
        return Err("OpenAI response did not include output text".to_string());
    }
    let (tool_calls, cleaned_content) = extract_tool_calls(&content, tool_names);
    detect_malformed_tool_call(&content, &tool_calls)?;
    let reply = AssistantReply {
        content: cleaned_content,
        tool_calls,
        prompt_tokens: parsed.usage.as_ref().and_then(|usage| usage.input_tokens),
        completion_tokens: parsed.usage.as_ref().and_then(|usage| usage.output_tokens),
    };
    logging::log_llm_event(
        "openai.responses.reply_final",
        json!({
            "content": truncate_for_log(&reply.content, 100_000),
            "tool_calls": reply.tool_calls,
            "prompt_tokens": reply.prompt_tokens,
            "completion_tokens": reply.completion_tokens,
        }),
    );
    Ok(reply)
}

fn detect_malformed_tool_call(content: &str, tool_calls: &[ToolCall]) -> Result<(), String> {
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

    if stripped.contains("<anvil_tool_call>") && !stripped.contains("</anvil_tool_call>") {
        return Err("tool call parser failed: unterminated <anvil_tool_call> block".to_string());
    }

    Err("tool call parser failed: malformed tool call markup".to_string())
}

#[cfg(test)]
mod tests {
    use super::{build_response_request, parse_openai_response};
    use crate::session::store::ConversationMessage;

    #[test]
    fn response_request_uses_output_text_for_assistant_history() {
        let body = build_response_request(
            "gpt-test",
            &[
                ConversationMessage::system("system".to_string()),
                ConversationMessage::user("user".to_string()),
                ConversationMessage::assistant("assistant".to_string(), Vec::new()),
            ],
            128,
        );

        assert_eq!(body["input"][0]["content"][0]["type"], "input_text");
        assert_eq!(body["input"][1]["content"][0]["type"], "input_text");
        assert_eq!(body["input"][2]["content"][0]["type"], "output_text");
    }

    #[test]
    fn parses_xml_tool_call_from_output_text() {
        let body = r#"{
          "output_text": "<anvil_tool_call>{\"name\":\"Write\",\"arguments\":{\"path\":\"hello.txt\",\"content\":\"hi\"}}</anvil_tool_call>",
          "usage": {"input_tokens": 11, "output_tokens": 22}
        }"#;
        let reply = parse_openai_response(body, &["Write".to_string()]).unwrap();
        assert_eq!(reply.tool_calls.len(), 1);
        assert_eq!(reply.tool_calls[0].name, "Write");
        assert_eq!(reply.prompt_tokens, Some(11));
        assert_eq!(reply.completion_tokens, Some(22));
    }

    #[test]
    fn parses_text_from_output_items() {
        let body = r#"{
          "output": [{
            "content": [{"type":"output_text","text":"done"}]
          }]
        }"#;
        let reply = parse_openai_response(body, &[]).unwrap();
        assert_eq!(reply.content, "done");
        assert!(reply.tool_calls.is_empty());
    }
}
