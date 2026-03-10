use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::time::Duration;

use crate::models::client::{ModelClient, ModelRequest, ModelResponse};

#[derive(Debug, Clone)]
pub struct LmStudioClient {
    endpoint: String,
}

impl Default for LmStudioClient {
    fn default() -> Self {
        Self {
            endpoint: "http://127.0.0.1:1234".to_string(),
        }
    }
}

impl ModelClient for LmStudioClient {
    fn provider_name(&self) -> &'static str {
        "lm_studio"
    }

    fn can_handle(&self, model: &str) -> bool {
        model.starts_with("lmstudio/")
    }

    fn complete(&self, request: &ModelRequest) -> anyhow::Result<ModelResponse> {
        let client = reqwest::blocking::Client::builder()
            .connect_timeout(Duration::from_secs(30))
            .timeout(Duration::from_secs(600))
            .build()
            .context("failed to build LM Studio HTTP client")?;
        let response: LmStudioChatResponse = client
            .post(format!(
                "{}/v1/chat/completions",
                self.endpoint.trim_end_matches('/')
            ))
            .json(&LmStudioChatRequest {
                model: external_model_name(&request.model).to_string(),
                messages: vec![
                    ChatMessage {
                        role: "system".to_string(),
                        content: request.system_prompt.trim().to_string(),
                    },
                    ChatMessage {
                        role: "user".to_string(),
                        content: request.user_prompt.trim().to_string(),
                    },
                ],
                stream: false,
            })
            .send()
            .with_context(|| format!("failed to reach LM Studio endpoint {}", self.endpoint))?
            .error_for_status()
            .context("LM Studio returned an error status")?
            .json()
            .context("failed to decode LM Studio response")?;

        let output = response
            .choices
            .into_iter()
            .next()
            .map(|choice| choice.message.content.trim().to_string())
            .filter(|content| !content.is_empty())
            .context("LM Studio response did not contain a message")?;

        Ok(ModelResponse {
            provider: self.provider_name().to_string(),
            model: request.model.clone(),
            output,
        })
    }
}

fn external_model_name(model: &str) -> &str {
    model.strip_prefix("lmstudio/").unwrap_or(model)
}

#[derive(Debug, Clone, Serialize)]
struct LmStudioChatRequest {
    model: String,
    messages: Vec<ChatMessage>,
    stream: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ChatMessage {
    role: String,
    content: String,
}

#[derive(Debug, Clone, Deserialize)]
struct LmStudioChatResponse {
    choices: Vec<LmStudioChoice>,
}

#[derive(Debug, Clone, Deserialize)]
struct LmStudioChoice {
    message: ChatMessage,
}

#[cfg(test)]
mod tests {
    use super::external_model_name;

    #[test]
    fn external_model_name_strips_lmstudio_prefix() {
        assert_eq!(
            external_model_name("lmstudio/qwen2.5-coder"),
            "qwen2.5-coder"
        );
        assert_eq!(external_model_name("plain-model"), "plain-model");
    }
}
