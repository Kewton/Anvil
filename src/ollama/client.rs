use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ollama::xml_fallback::{ToolCall, extract_tool_calls};
use crate::session::store::ConversationMessage;
use crate::tools::registry::ToolSpec;

#[derive(Debug, Clone)]
pub struct OllamaClient {
    base_url: String,
    http: Client,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssistantReply {
    pub content: String,
    pub tool_calls: Vec<ToolCall>,
}

impl OllamaClient {
    pub fn new(base_url: String) -> Result<Self, String> {
        let http = Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .map_err(|err| format!("failed to create HTTP client: {err}"))?;
        Ok(Self { base_url, http })
    }

    pub fn list_models(&self) -> Result<Vec<String>, String> {
        let response = self
            .http
            .get(format!("{}/api/tags", self.base_url))
            .send()
            .map_err(|err| format!("failed to contact Ollama: {err}"))?;
        if !response.status().is_success() {
            return Err(format!("Ollama /api/tags failed: {}", response.status()));
        }
        let body = response
            .text()
            .map_err(|err| format!("failed to decode Ollama tags response: {err}"))?;
        parse_tags_response(&body)
    }

    pub fn chat(
        &self,
        model: &str,
        messages: &[ConversationMessage],
        tools: &[ToolSpec],
    ) -> Result<AssistantReply, String> {
        #[derive(Serialize)]
        struct ChatRequest<'a> {
            model: &'a str,
            stream: bool,
            messages: &'a [ConversationMessage],
            tools: &'a [ToolSpec],
        }

        let response = self
            .http
            .post(format!("{}/api/chat", self.base_url))
            .json(&ChatRequest {
                model,
                stream: false,
                messages,
                tools,
            })
            .send()
            .map_err(|err| format!("failed to contact Ollama chat API: {err}"))?;

        if !response.status().is_success() {
            return Err(format!("Ollama /api/chat failed: {}", response.status()));
        }
        let body = response
            .text()
            .map_err(|err| format!("failed to decode Ollama chat response: {err}"))?;
        let tool_names = tools
            .iter()
            .map(|tool| tool.function.name.clone())
            .collect::<Vec<_>>();
        parse_chat_response(&body, &tool_names)
    }
}

#[derive(Deserialize)]
struct TagsResponse {
    models: Vec<ModelEntry>,
}

#[derive(Deserialize)]
struct ModelEntry {
    name: String,
}

#[derive(Deserialize)]
struct ChatResponse {
    message: ResponseMessage,
}

#[derive(Deserialize)]
struct ResponseMessage {
    #[serde(default)]
    content: String,
    #[serde(default)]
    tool_calls: Vec<ResponseToolCall>,
}

#[derive(Deserialize)]
struct ResponseToolCall {
    #[serde(default)]
    id: Option<String>,
    function: ResponseFunctionCall,
}

#[derive(Deserialize)]
struct ResponseFunctionCall {
    name: String,
    arguments: Value,
}

pub fn parse_tags_response(body: &str) -> Result<Vec<String>, String> {
    let parsed: TagsResponse = serde_json::from_str(body)
        .map_err(|err| format!("failed to decode Ollama tags response: {err}"))?;
    Ok(parsed.models.into_iter().map(|model| model.name).collect())
}

pub fn parse_chat_response(body: &str, tool_names: &[String]) -> Result<AssistantReply, String> {
    let parsed: ChatResponse = serde_json::from_str(body)
        .map_err(|err| format!("failed to decode Ollama chat response: {err}"))?;
    let content = parsed.message.content;
    let tool_calls = if parsed.message.tool_calls.is_empty() {
        extract_tool_calls(&content, tool_names).0
    } else {
        parsed
            .message
            .tool_calls
            .into_iter()
            .enumerate()
            .map(|(index, tool_call)| ToolCall {
                id: tool_call
                    .id
                    .unwrap_or_else(|| format!("native-{}", index + 1)),
                name: tool_call.function.name,
                arguments: tool_call.function.arguments,
            })
            .collect()
    };
    let (_, cleaned_content) = if tool_calls.is_empty() {
        (
            Vec::new(),
            crate::ollama::xml_fallback::strip_think_tags(&content),
        )
    } else {
        extract_tool_calls(&content, tool_names)
    };

    Ok(AssistantReply {
        content: cleaned_content,
        tool_calls,
    })
}
