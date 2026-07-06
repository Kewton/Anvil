use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use reqwest::blocking::Client;
use serde_json::json;

use crate::config::{Config, load_api_key};
use crate::eval_events;
use crate::state::ConversationMessage;
use crate::tools::registry::ToolSpec;

use super::{AssistantReply, ChatClient};

const GEMINI_BASE_URL: &str = "https://generativelanguage.googleapis.com";

#[derive(Debug, Clone)]
pub struct GeminiClient {
    api_key: String,
    http: Client,
    max_predict: usize,
    retries: usize,
    eval_events_path: Option<PathBuf>,
    previous_interaction_id: Arc<Mutex<Option<String>>>,
}

impl GeminiClient {
    pub fn from_env(config: &Config) -> anyhow::Result<Self> {
        let api_key = load_api_key(&config.workspace_root, "GEMINI_API_KEY")?;
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
            previous_interaction_id: Arc::new(Mutex::new(None)),
        })
    }
}

impl ChatClient for GeminiClient {
    fn label(&self) -> &str {
        "gemini"
    }

    fn boxed_clone(&self) -> Option<Box<dyn ChatClient>> {
        Some(Box::new(self.clone()))
    }

    fn supports_native_tools(&self, _model: &str) -> bool {
        true
    }

    fn allows_xml_fallback(&self) -> bool {
        false
    }

    fn chat(
        &mut self,
        model: &str,
        messages: &[ConversationMessage],
        tools: &[ToolSpec],
        _native_tools_enabled: bool,
    ) -> anyhow::Result<AssistantReply> {
        let continues_existing_interaction = messages
            .iter()
            .any(|message| matches!(message.role.as_str(), "assistant" | "tool"));
        let previous_interaction_id = if continues_existing_interaction {
            self.previous_interaction_id
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone()
        } else {
            None
        };
        let body = super::gemini_function_calling::build_interactions_request_with_previous(
            model,
            messages,
            tools,
            self.max_predict,
            previous_interaction_id.as_deref(),
        );
        eval_events::emit(
            self.eval_events_path.as_deref(),
            json!({
                "event": "provider_request",
                "provider": "gemini",
                "model": model,
                "tools": tools.len(),
                "previous_interaction": previous_interaction_id.is_some(),
            }),
        );
        for attempt in 0..=self.retries {
            match self
                .http
                .post(format!("{GEMINI_BASE_URL}/v1beta/interactions"))
                .header("x-goog-api-key", &self.api_key)
                .header("Content-Type", "application/json")
                .json(&body)
                .send()
            {
                Ok(response) if response.status().is_success() => {
                    let body = response.text()?;
                    let parsed = super::gemini_function_calling::parse_interactions_response(&body);
                    match parsed {
                        Ok(reply) => {
                            if let Some(id) = super::gemini_function_calling::interaction_id(&body)
                            {
                                *self
                                    .previous_interaction_id
                                    .lock()
                                    .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(id);
                            }
                            eval_events::emit(
                                self.eval_events_path.as_deref(),
                                json!({
                                    "event": "provider_response",
                                    "provider": "gemini",
                                    "model": model,
                                    "attempt": attempt + 1,
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
                                    "provider": "gemini",
                                    "model": model,
                                    "error_kind": "provider_parse_error",
                                    "message": eval_events::body_snippet(&err.to_string()),
                                }),
                            );
                            return Err(err);
                        }
                    }
                }
                Ok(response)
                    if attempt == self.retries
                        || !is_retryable_status(response.status().as_u16()) =>
                {
                    let status = response.status();
                    let body = response.text().unwrap_or_default();
                    eval_events::emit(
                        self.eval_events_path.as_deref(),
                        json!({
                            "event": "provider_error",
                            "provider": "gemini",
                            "model": model,
                            "status": status.as_u16(),
                            "error_kind": "http_status",
                            "attempt": attempt + 1,
                            "retry_exhausted": attempt == self.retries,
                            "retryable": is_retryable_status(status.as_u16()),
                            "body_snippet": eval_events::body_snippet(&body),
                        }),
                    );
                    anyhow::bail!("Gemini interactions API failed: {}", status);
                }
                Ok(response) => {
                    let status = response.status();
                    eval_events::emit(
                        self.eval_events_path.as_deref(),
                        json!({
                            "event": "provider_retry",
                            "provider": "gemini",
                            "model": model,
                            "status": status.as_u16(),
                            "error_kind": "http_status",
                            "attempt": attempt + 1,
                            "retryable": is_retryable_status(status.as_u16()),
                        }),
                    );
                }
                Err(err) if attempt == self.retries => {
                    eval_events::emit(
                        self.eval_events_path.as_deref(),
                        json!({
                            "event": "provider_error",
                            "provider": "gemini",
                            "model": model,
                            "error_kind": "network",
                            "attempt": attempt + 1,
                            "retry_exhausted": true,
                            "message": eval_events::body_snippet(&err.to_string()),
                        }),
                    );
                    return Err(err.into());
                }
                Err(err) => {
                    eval_events::emit(
                        self.eval_events_path.as_deref(),
                        json!({
                            "event": "provider_retry",
                            "provider": "gemini",
                            "model": model,
                            "error_kind": "network",
                            "attempt": attempt + 1,
                            "message": eval_events::body_snippet(&err.to_string()),
                        }),
                    );
                }
            }
        }
        unreachable!("retry loop always returns or bails")
    }
}

fn is_retryable_status(status: u16) -> bool {
    matches!(status, 429 | 500 | 502 | 503 | 504)
}
