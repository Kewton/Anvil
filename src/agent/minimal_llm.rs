use crate::gemini::GeminiClient;
use crate::ollama::client::OllamaClient;
use crate::ollama::parsing::AssistantReply;
use crate::openai::OpenAiClient;
use crate::provider_call::{self, ProviderCallScope};
use crate::session::store::ConversationMessage;
use crate::tools::registry::ToolSpec;

use super::minimal_loop::MinimalChatClient;

#[derive(Debug, Clone)]
pub enum MinimalLlmClient {
    Ollama(OllamaClient),
    Gemini(GeminiClient),
    Openai(OpenAiClient),
}

impl MinimalChatClient for MinimalLlmClient {
    fn supports_native_tools(&self, model: &str) -> bool {
        match self {
            Self::Ollama(client) => client.supports_native_tools(model),
            Self::Gemini(_) => false,
            Self::Openai(_) => false,
        }
    }

    fn chat(
        &mut self,
        model: &str,
        messages: &[ConversationMessage],
        tools: &[ToolSpec],
        native_tools_enabled: bool,
    ) -> Result<AssistantReply, String> {
        self.chat_scoped(
            ProviderCallScope::Executor,
            model,
            messages,
            tools,
            native_tools_enabled,
        )
    }

    fn chat_scoped(
        &mut self,
        scope: ProviderCallScope,
        model: &str,
        messages: &[ConversationMessage],
        tools: &[ToolSpec],
        native_tools_enabled: bool,
    ) -> Result<AssistantReply, String> {
        match self {
            Self::Ollama(client) => provider_call::chat_ollama_with_mode(
                scope,
                client,
                model,
                messages,
                tools,
                native_tools_enabled,
            ),
            Self::Gemini(client) => {
                let _ = native_tools_enabled;
                provider_call::chat_gemini(scope, client, model, messages, tools)
            }
            Self::Openai(client) => {
                let _ = native_tools_enabled;
                provider_call::chat_openai(scope, client, model, messages, tools)
            }
        }
    }
}
