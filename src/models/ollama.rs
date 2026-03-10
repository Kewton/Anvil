use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::env;
use std::io::{BufRead, BufReader};
use std::time::Duration;

use crate::models::client::{ModelClient, ModelRequest, ModelResponse};

#[derive(Debug, Clone)]
pub struct OllamaClient {
    endpoint: String,
}

impl Default for OllamaClient {
    fn default() -> Self {
        Self {
            endpoint: env::var("ANVIL_OLLAMA_ENDPOINT")
                .unwrap_or_else(|_| "http://127.0.0.1:11434".to_string()),
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

    fn stream_complete(
        &self,
        request: &ModelRequest,
        on_chunk: &mut dyn FnMut(&str),
    ) -> anyhow::Result<ModelResponse> {
        let client = reqwest::blocking::Client::builder()
            .connect_timeout(Duration::from_secs(30))
            .timeout(Duration::from_secs(600))
            .build()
            .context("failed to build Ollama HTTP client")?;
        let response = client
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
                stream: true,
            })
            .send()
            .with_context(|| format!("failed to reach Ollama endpoint {}", self.endpoint))?
            .error_for_status()
            .context("Ollama returned an error status")?;

        let mut output = String::new();
        for line in BufReader::new(response).lines() {
            let line = line.context("failed to read streamed Ollama response")?;
            if line.trim().is_empty() {
                continue;
            }
            let chunk: OllamaStreamChunk =
                serde_json::from_str(&line).context("failed to decode streamed Ollama chunk")?;
            if !chunk.response.is_empty() {
                on_chunk(&chunk.response);
                output.push_str(&chunk.response);
            }
        }

        Ok(ModelResponse {
            provider: self.provider_name().to_string(),
            model: request.model.clone(),
            output: output.trim().to_string(),
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

#[derive(Debug, Clone, Deserialize)]
struct OllamaStreamChunk {
    #[serde(default)]
    response: String,
}

#[cfg(test)]
mod tests {
    use super::OllamaClient;
    use std::env;

    #[test]
    fn default_client_uses_endpoint_override_when_present() {
        env::set_var("ANVIL_OLLAMA_ENDPOINT", "http://192.168.11.7:11434");
        let client = OllamaClient::default();
        assert_eq!(client.endpoint, "http://192.168.11.7:11434");
        env::remove_var("ANVIL_OLLAMA_ENDPOINT");
    }
}
