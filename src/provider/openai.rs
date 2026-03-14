/// OpenAI-compatible provider client.
///
/// Works with the standard `/v1/chat/completions` endpoint used by
/// OpenAI, Azure OpenAI, LM Studio, and other compatible servers.

use super::{
    AgentEvent, CurlHttpTransport, HttpTransport, ProviderClient, ProviderEvent,
    ProviderTurnError, ProviderTurnRequest,
};
use crate::config::EffectiveConfig;
use serde::{Deserialize, Serialize};

/// Client for OpenAI-compatible chat completion APIs.
///
/// Generic over [`HttpTransport`] for testability.
pub struct OpenAiCompatibleProviderClient<T = CurlHttpTransport> {
    base_url: String,
    api_key: Option<String>,
    transport: T,
}

#[derive(Debug, Clone, Serialize)]
struct OpenAiChatRequest {
    model: String,
    messages: Vec<OpenAiChatMessage>,
    stream: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct OpenAiChatMessage {
    role: String,
    content: String,
}

#[derive(Debug, Clone, Deserialize)]
struct OpenAiChatResponse {
    choices: Vec<OpenAiChoice>,
}

#[derive(Debug, Clone, Deserialize)]
struct OpenAiChoice {
    message: OpenAiChatMessage,
}

impl OpenAiCompatibleProviderClient {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            api_key: None,
            transport: CurlHttpTransport,
        }
    }

    pub fn from_config(config: &EffectiveConfig) -> Self {
        Self {
            base_url: config.runtime.provider_url.clone(),
            api_key: config.runtime.api_key.clone(),
            transport: CurlHttpTransport,
        }
    }
}

impl<T> OpenAiCompatibleProviderClient<T> {
    pub fn with_transport(base_url: impl Into<String>, transport: T) -> Self {
        Self {
            base_url: base_url.into(),
            api_key: None,
            transport,
        }
    }

    pub fn with_api_key(mut self, key: impl Into<String>) -> Self {
        self.api_key = Some(key.into());
        self
    }
}

impl<T: HttpTransport> OpenAiCompatibleProviderClient<T> {
    fn send_chat_request(
        &self,
        request: &ProviderTurnRequest,
    ) -> Result<String, ProviderTurnError> {
        let chat_request = OpenAiChatRequest {
            model: request.model.clone(),
            messages: request
                .messages
                .iter()
                .map(|m| OpenAiChatMessage {
                    role: match m.role {
                        super::ProviderMessageRole::System => "system".to_string(),
                        super::ProviderMessageRole::User => "user".to_string(),
                        super::ProviderMessageRole::Assistant => "assistant".to_string(),
                        super::ProviderMessageRole::Tool => "tool".to_string(),
                    },
                    content: m.content.clone(),
                })
                .collect(),
            stream: false,
        };

        let request_body = serde_json::to_vec(&chat_request).map_err(|err| {
            ProviderTurnError::Backend(format!("failed to encode openai request: {err}"))
        })?;
        let url = format!(
            "{}/v1/chat/completions",
            self.base_url.trim_end_matches('/')
        );

        let response = self.transport.post_json(&url, &request_body)?;
        if response.status_code != 200 {
            let body_text = String::from_utf8_lossy(&response.body);
            return Err(ProviderTurnError::Backend(format!(
                "openai request failed with status {}: {}",
                response.status_code,
                body_text.trim()
            )));
        }

        let parsed: OpenAiChatResponse =
            serde_json::from_slice(&response.body).map_err(|err| {
                ProviderTurnError::Backend(format!("invalid openai response: {err}"))
            })?;

        parsed
            .choices
            .first()
            .map(|choice| choice.message.content.clone())
            .ok_or_else(|| {
                ProviderTurnError::Backend("openai response contained no choices".to_string())
            })
    }
}

impl<T: HttpTransport> ProviderClient for OpenAiCompatibleProviderClient<T> {
    fn stream_turn(
        &self,
        request: &ProviderTurnRequest,
        emit: &mut dyn FnMut(ProviderEvent),
    ) -> Result<(), ProviderTurnError> {
        let content = self.send_chat_request(request)?;

        emit(ProviderEvent::TokenDelta(content.clone()));
        emit(ProviderEvent::Agent(AgentEvent::Done {
            status: "Done. session saved".to_string(),
            assistant_message: content,
            completion_summary: "Provider turn finished successfully.".to_string(),
            saved_status: "session saved".to_string(),
            tool_logs: Vec::new(),
            elapsed_ms: 0,
        }));

        Ok(())
    }
}
