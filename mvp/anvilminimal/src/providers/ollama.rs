use reqwest::blocking::Client;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::state::{ConversationMessage, ToolCall};
use crate::tools::args_recovery::recover_tool_arguments;
use crate::tools::registry::ToolSpec;

use super::parsing::tool_names;
use super::{AssistantReply, ChatClient};

#[derive(Debug, Clone)]
pub struct OllamaClient {
    base_url: String,
    http: Client,
    max_predict: usize,
    retries: usize,
}

impl OllamaClient {
    pub fn new(
        base_url: String,
        timeout_secs: u64,
        max_predict: usize,
        retries: usize,
    ) -> anyhow::Result<Self> {
        let http = Client::builder()
            .connect_timeout(std::time::Duration::from_secs(timeout_secs))
            .timeout(std::time::Duration::from_secs(timeout_secs))
            .build()?;
        Ok(Self {
            base_url,
            http,
            max_predict,
            retries,
        })
    }

    pub fn list_models(&self) -> anyhow::Result<Vec<String>> {
        let response = self
            .http
            .get(format!("{}/api/tags", self.base_url))
            .send()?;
        if !response.status().is_success() {
            anyhow::bail!("Ollama /api/tags failed: {}", response.status());
        }
        let body = response.text()?;
        parse_tags_response(&body)
    }
}

impl ChatClient for OllamaClient {
    fn label(&self) -> &str {
        "ollama"
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
        let mut body = json!({
            "model": model,
            "messages": ollama_messages(messages),
            "stream": false,
            "options": { "num_predict": self.max_predict },
        });
        if native_tools_enabled && !tools.is_empty() {
            body["tools"] = json!(tools);
        }
        for attempt in 0..=self.retries {
            match self
                .http
                .post(format!("{}/api/chat", self.base_url))
                .json(&body)
                .send()
            {
                Ok(response) if response.status().is_success() => {
                    return parse_chat_response(
                        &response.text()?,
                        &tool_names(tools),
                        !native_tools_enabled,
                    );
                }
                Ok(response) if attempt == self.retries => {
                    anyhow::bail!("Ollama /api/chat failed: {}", response.status());
                }
                Ok(_) => {}
                Err(err) if attempt == self.retries => return Err(err.into()),
                Err(_) => {}
            }
        }
        unreachable!("retry loop always returns or bails")
    }
}

fn ollama_messages(messages: &[ConversationMessage]) -> Vec<Value> {
    messages
        .iter()
        .map(|message| {
            let role = if message.role == "tool" {
                "tool"
            } else {
                message.role.as_str()
            };
            let mut value = json!({"role": role, "content": message.content});
            if let Some(name) = &message.name {
                value["name"] = json!(name);
            }
            value
        })
        .collect()
}

#[derive(Deserialize)]
struct TagsResponse {
    #[serde(default)]
    models: Vec<TagModel>,
}

#[derive(Deserialize)]
struct TagModel {
    name: String,
}

pub fn parse_tags_response(body: &str) -> anyhow::Result<Vec<String>> {
    let parsed: TagsResponse = serde_json::from_str(body)?;
    Ok(parsed.models.into_iter().map(|model| model.name).collect())
}

#[derive(Deserialize)]
struct ChatResponse {
    message: Option<ChatMessage>,
    #[serde(default)]
    prompt_eval_count: Option<u64>,
    #[serde(default)]
    eval_count: Option<u64>,
}

#[derive(Deserialize)]
struct ChatMessage {
    #[serde(default)]
    content: String,
    #[serde(default)]
    tool_calls: Vec<OllamaToolCall>,
}

#[derive(Deserialize)]
struct OllamaToolCall {
    function: OllamaFunctionCall,
}

#[derive(Deserialize)]
struct OllamaFunctionCall {
    name: String,
    #[serde(default)]
    arguments: Value,
}

pub fn parse_chat_response(
    body: &str,
    allowed_tools: &[String],
    xml_fallback: bool,
) -> anyhow::Result<AssistantReply> {
    let parsed: ChatResponse = serde_json::from_str(body)?;
    let message = parsed
        .message
        .ok_or_else(|| anyhow::anyhow!("Ollama response missing message"))?;
    if xml_fallback {
        let (tool_calls, content) =
            super::xml_fallback::extract_tool_calls(&message.content, allowed_tools)?;
        return Ok(AssistantReply {
            content,
            tool_calls,
            prompt_tokens: parsed.prompt_eval_count,
            completion_tokens: parsed.eval_count,
        });
    }
    let tool_calls = message
        .tool_calls
        .into_iter()
        .map(|call| {
            let arguments =
                recover_tool_arguments(&call.function.name, call.function.arguments).arguments;
            ToolCall::new(call.function.name, arguments)
        })
        .collect();
    Ok(AssistantReply {
        content: message.content,
        tool_calls,
        prompt_tokens: parsed.prompt_eval_count,
        completion_tokens: parsed.eval_count,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_tags() {
        assert_eq!(
            parse_tags_response(r#"{"models":[{"name":"m"}]}"#).unwrap(),
            vec!["m"]
        );
    }
}
