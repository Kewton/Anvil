use reqwest::blocking::Client;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::config::{Config, load_api_key};
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
        for attempt in 0..=self.retries {
            match self
                .http
                .post(format!("{OPENAI_BASE_URL}/v1/responses"))
                .bearer_auth(&self.api_key)
                .json(&body)
                .send()
            {
                Ok(response) if response.status().is_success() => {
                    return parse_openai_response(&response.text()?);
                }
                Ok(response) if attempt == self.retries => {
                    anyhow::bail!("OpenAI Responses API failed: {}", response.status());
                }
                Ok(_) => {}
                Err(err) if attempt == self.retries => return Err(err.into()),
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
            let arguments = item.arguments.unwrap_or(Value::Null);
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
    fn parses_function_call() {
        let reply = parse_openai_response(
            r#"{"output":[{"type":"function_call","name":"Read","call_id":"c1","arguments":{"path":"a"}}]}"#,
        )
        .unwrap();
        assert_eq!(reply.tool_calls[0].name, "Read");
    }
}
