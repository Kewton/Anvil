pub mod gemini;
pub mod gemini_function_calling;
pub mod ollama;
pub mod openai;
pub mod parsing;
pub mod xml_fallback;

use anyhow::bail;
use serde::{Deserialize, Serialize};

use crate::config::{Config, Provider};
use crate::state::{ConversationMessage, ToolCall};
use crate::tools::registry::ToolSpec;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssistantReply {
    pub content: String,
    #[serde(default)]
    pub tool_calls: Vec<ToolCall>,
    #[serde(default)]
    pub prompt_tokens: Option<u64>,
    #[serde(default)]
    pub completion_tokens: Option<u64>,
}

impl AssistantReply {
    pub fn text(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            tool_calls: Vec::new(),
            prompt_tokens: None,
            completion_tokens: None,
        }
    }
}

pub trait ChatClient {
    fn label(&self) -> &str;
    fn supports_native_tools(&self, _model: &str) -> bool {
        false
    }
    fn allows_xml_fallback(&self) -> bool {
        false
    }
    fn chat(
        &mut self,
        model: &str,
        messages: &[ConversationMessage],
        tools: &[ToolSpec],
        native_tools_enabled: bool,
    ) -> anyhow::Result<AssistantReply>;
}

pub fn client_from_config(config: &Config, planner: bool) -> anyhow::Result<Box<dyn ChatClient>> {
    let provider = if planner {
        config.planner_provider
    } else {
        config.provider
    };
    match provider {
        Provider::Ollama => Ok(Box::new(ollama::OllamaClient::new(
            config.ollama_host.clone(),
            config.chat_timeout_secs,
            config.num_predict,
            config.chat_retries,
        )?)),
        Provider::Openai => Ok(Box::new(openai::OpenAiClient::from_env(config)?)),
        Provider::Gemini => Ok(Box::new(gemini::GeminiClient::from_env(config)?)),
    }
}

pub fn model_for(config: &Config, planner: bool) -> &str {
    if planner {
        &config.planner_model
    } else {
        &config.model
    }
}

pub fn ensure_known_tool(tool_name: &str, tools: &[ToolSpec]) -> anyhow::Result<()> {
    if tools.iter().any(|tool| tool.function.name == tool_name) {
        return Ok(());
    }
    bail!("unknown tool requested by provider: {tool_name}")
}
