use std::path::PathBuf;

use reqwest::blocking::Client;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::config::{Config, load_api_key};
use crate::eval_events;
use crate::state::{ConversationMessage, ToolCall};
use crate::tools::registry::ToolSpec;

use super::parsing::sanitized_tool_schema;
use super::{AssistantReply, ChatClient};

const OPENAI_BASE_URL: &str = "https://api.openai.com";

#[derive(Debug, Clone)]
pub struct OpenAiClient {
    api_key: String,
    http: Client,
    max_predict: usize,
    retries: usize,
    eval_events_path: Option<PathBuf>,
}

impl OpenAiClient {
    pub fn from_env(config: &Config) -> anyhow::Result<Self> {
        let api_key = load_api_key(&config.workspace_root, "OPENAI_API_KEY")?;
        let http = Client::builder()
            .connect_timeout(std::time::Duration::from_secs(config.chat_timeout_secs))
            .timeout(std::time::Duration::from_secs(config.chat_timeout_secs))
            .build()?;
        Ok(Self {
            api_key,
            http,
            max_predict: config.num_predict,
            retries: config.chat_retries,
            eval_events_path: config.eval_events_path.clone(),
        })
    }
}

impl ChatClient for OpenAiClient {
    fn label(&self) -> &str {
        "openai"
    }

    fn supports_native_tools(&self, _model: &str) -> bool {
        true
    }

    fn allows_xml_fallback(&self) -> bool {
        true
    }

    fn chat(
        &mut self,
        model: &str,
        messages: &[ConversationMessage],
        tools: &[ToolSpec],
        native_tools_enabled: bool,
    ) -> anyhow::Result<AssistantReply> {
        let body = build_response_request(
            model,
            messages,
            tools,
            native_tools_enabled,
            self.max_predict,
        );
        eval_events::emit(
            self.eval_events_path.as_deref(),
            json!({
                "event": "provider_request",
                "provider": "openai",
                "model": model,
                "tools": if native_tools_enabled { tools.len() } else { 0 },
            }),
        );
        for attempt in 0..=self.retries {
            match self
                .http
                .post(format!("{OPENAI_BASE_URL}/v1/responses"))
                .bearer_auth(&self.api_key)
                .json(&body)
                .send()
            {
                Ok(response) if response.status().is_success() => {
                    let body = response.text()?;
                    let parsed = parse_openai_response(&body);
                    match parsed {
                        Ok(reply) => {
                            eval_events::emit(
                                self.eval_events_path.as_deref(),
                                json!({
                                    "event": "provider_response",
                                    "provider": "openai",
                                    "model": model,
                                    "tool_calls": reply.tool_calls.len(),
                                }),
                            );
                            return Ok(reply);
                        }
                        Err(err) => {
                            eval_events::emit(
                                self.eval_events_path.as_deref(),
                                json!({
                                    "event": "provider_parse_error",
                                    "provider": "openai",
                                    "model": model,
                                    "error_kind": "provider_parse_error",
                                    "message": eval_events::body_snippet(&err.to_string()),
                                }),
                            );
                            return Err(err);
                        }
                    }
                }
                Ok(response) if attempt == self.retries => {
                    let status = response.status();
                    let body = response.text().unwrap_or_default();
                    eval_events::emit(
                        self.eval_events_path.as_deref(),
                        json!({
                            "event": "provider_error",
                            "provider": "openai",
                            "model": model,
                            "status": status.as_u16(),
                            "error_kind": "http_status",
                            "body_snippet": eval_events::body_snippet(&body),
                        }),
                    );
                    anyhow::bail!("OpenAI Responses API failed: {}", status);
                }
                Ok(_) => {}
                Err(err) if attempt == self.retries => {
                    eval_events::emit(
                        self.eval_events_path.as_deref(),
                        json!({
                            "event": "provider_error",
                            "provider": "openai",
                            "model": model,
                            "error_kind": "network",
                            "message": eval_events::body_snippet(&err.to_string()),
                        }),
                    );
                    return Err(err.into());
                }
                Err(_) => {}
            }
        }
        unreachable!("retry loop always returns or bails")
    }
}

pub fn build_response_request(
    model: &str,
    messages: &[ConversationMessage],
    tools: &[ToolSpec],
    native_tools_enabled: bool,
    max_predict: usize,
) -> Value {
    let mut body = json!({
        "model": model,
        "input": openai_input(messages),
        "max_output_tokens": max_predict,
    });
    if native_tools_enabled && !tools.is_empty() {
        body["tools"] = Value::Array(tools.iter().map(sanitized_tool_schema).collect());
    }
    body
}

fn openai_input(messages: &[ConversationMessage]) -> Vec<Value> {
    messages
        .iter()
        .map(|message| {
            let content_type = if message.role == "assistant" {
                "output_text"
            } else {
                "input_text"
            };
            json!({
                "role": if message.role == "tool" { "user" } else { message.role.as_str() },
                "content": [{"type": content_type, "text": message.content}]
            })
        })
        .collect()
}

#[derive(Deserialize)]
struct OpenAiResponse {
    #[serde(default)]
    output_text: Option<String>,
    #[serde(default)]
    output: Vec<OpenAiOutput>,
    #[serde(default)]
    usage: Option<OpenAiUsage>,
}

#[derive(Deserialize)]
struct OpenAiOutput {
    #[serde(rename = "type", default)]
    kind: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    arguments: Option<Value>,
    #[serde(default)]
    call_id: Option<String>,
    #[serde(default)]
    content: Vec<OpenAiContent>,
}

#[derive(Deserialize)]
struct OpenAiContent {
    #[serde(default)]
    text: String,
}

#[derive(Deserialize)]
struct OpenAiUsage {
    #[serde(default)]
    input_tokens: Option<u64>,
    #[serde(default)]
    output_tokens: Option<u64>,
}

pub fn parse_openai_response(body: &str) -> anyhow::Result<AssistantReply> {
    let parsed: OpenAiResponse = serde_json::from_str(body)?;
    let mut text = parsed.output_text.unwrap_or_default();
    let mut tool_calls = Vec::new();
    for item in parsed.output {
        if item.kind == "function_call" {
            let name = item
                .name
                .ok_or_else(|| anyhow::anyhow!("OpenAI function_call missing name"))?;
            let arguments = normalize_function_arguments(item.arguments)?;
            tool_calls.push(ToolCall {
                id: item
                    .call_id
                    .unwrap_or_else(|| uuid::Uuid::now_v7().to_string()),
                name,
                arguments,
            });
        }
        for part in item.content {
            text.push_str(&part.text);
        }
    }
    Ok(AssistantReply {
        content: text,
        tool_calls,
        prompt_tokens: parsed.usage.as_ref().and_then(|usage| usage.input_tokens),
        completion_tokens: parsed.usage.and_then(|usage| usage.output_tokens),
    })
}

fn normalize_function_arguments(value: Option<Value>) -> anyhow::Result<Value> {
    match value {
        Some(object @ Value::Object(_)) => Ok(object),
        Some(Value::String(raw)) => {
            let decoded: Value = serde_json::from_str(&raw).map_err(|err| {
                anyhow::anyhow!("OpenAI function_call arguments are not valid JSON: {err}")
            })?;
            match decoded {
                Value::Object(_) => Ok(decoded),
                other => Err(anyhow::anyhow!(
                    "OpenAI function_call arguments must decode to object, got {}",
                    json_type(&other)
                )),
            }
        }
        Some(Value::Null) | None => Err(anyhow::anyhow!("OpenAI function_call missing arguments")),
        Some(other) => Err(anyhow::anyhow!(
            "OpenAI function_call arguments must be object or JSON string, got {}",
            json_type(&other)
        )),
    }
}

fn json_type(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::registry::ToolRegistry;

    #[test]
    fn request_keeps_model_string() {
        let body = build_response_request(
            "gpt-5.4-mini",
            &[],
            ToolRegistry::default().specs(),
            true,
            100,
        );
        assert_eq!(body["model"], "gpt-5.4-mini");
    }

    #[test]
    fn parses_function_call_arguments_object_for_compat() {
        let reply = parse_openai_response(
            r#"{"output":[{"type":"function_call","name":"Read","call_id":"c1","arguments":{"path":"a"}}]}"#,
        )
        .unwrap();
        assert_eq!(reply.tool_calls[0].name, "Read");
        assert_eq!(reply.tool_calls[0].arguments["path"], "a");
    }

    #[test]
    fn parses_function_call_arguments_json_string() {
        let reply = parse_openai_response(
            r#"{"output":[{"type":"function_call","name":"Grep","call_id":"c1","arguments":"{\"pattern\":\"TODO\"}"}]}"#,
        )
        .unwrap();
        assert_eq!(reply.tool_calls[0].arguments["pattern"], "TODO");
    }

    #[test]
    fn rejects_malformed_function_call_arguments_string() {
        let err = parse_openai_response(
            r#"{"output":[{"type":"function_call","name":"Grep","arguments":"{\"pattern\""}]}"#,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("not valid JSON"));
    }

    #[test]
    fn rejects_non_object_function_call_arguments() {
        let err = parse_openai_response(
            r#"{"output":[{"type":"function_call","name":"Grep","arguments":"[\"TODO\"]"}]}"#,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("must decode to object"));
    }
}
