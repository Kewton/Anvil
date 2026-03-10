use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::time::Duration;

use crate::models::client::{ModelClient, ModelRequest, ModelResponse};

#[derive(Debug, Clone)]
pub struct OllamaClient {
    endpoint: String,
}

impl Default for OllamaClient {
    fn default() -> Self {
        Self {
            endpoint: "http://127.0.0.1:11434".to_string(),
        }
    }
}

impl ModelClient for OllamaClient {
    fn provider_name(&self) -> &'static str {
        "ollama"
    }

    fn can_handle(&self, model: &str) -> bool {
        !model.starts_with("lmstudio/")
    }

    fn complete(&self, request: &ModelRequest) -> anyhow::Result<ModelResponse> {
        let client = reqwest::blocking::Client::builder()
            .connect_timeout(Duration::from_secs(30))
            .timeout(Duration::from_secs(600))
            .build()
            .context("failed to build Ollama HTTP client")?;
        let response: OllamaGenerateResponse = client
            .post(format!(
                "{}/api/generate",
                self.endpoint.trim_end_matches('/')
            ))
            .json(&OllamaGenerateRequest {
                model: request.model.clone(),
                prompt: format!(
                    "{}\n\n{}",
                    request.system_prompt.trim(),
                    request.user_prompt.trim()
                ),
                stream: false,
            })
            .send()
            .with_context(|| format!("failed to reach Ollama endpoint {}", self.endpoint))?
            .error_for_status()
            .context("Ollama returned an error status")?
            .json()
            .context("failed to decode Ollama response")?;

        Ok(ModelResponse {
            provider: self.provider_name().to_string(),
            model: request.model.clone(),
            output: response.response.trim().to_string(),
        })
    }
}

#[derive(Debug, Clone, Serialize)]
struct OllamaGenerateRequest {
    model: String,
    prompt: String,
    stream: bool,
}

#[derive(Debug, Clone, Deserialize)]
struct OllamaGenerateResponse {
    response: String,
}
