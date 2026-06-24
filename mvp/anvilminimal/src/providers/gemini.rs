use reqwest::blocking::Client;

use crate::config::{Config, load_api_key};
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
        })
    }
}

impl ChatClient for GeminiClient {
    fn label(&self) -> &str {
        "gemini"
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
        let body = super::gemini_function_calling::build_interactions_request(
            model,
            messages,
            tools,
            self.max_predict,
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
                    return super::gemini_function_calling::parse_interactions_response(
                        &response.text()?,
                    );
                }
                Ok(response) if attempt == self.retries => {
                    anyhow::bail!("Gemini interactions API failed: {}", response.status());
                }
                Ok(_) => {}
                Err(err) if attempt == self.retries => return Err(err.into()),
                Err(_) => {}
            }
        }
        unreachable!("retry loop always returns or bails")
    }
}
