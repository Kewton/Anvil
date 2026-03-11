use std::time::Duration;

use futures_util::StreamExt;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::models::stream::NdjsonStreamParser;
use crate::models::tool_calling::{
    NativeModelResponse, NativeToolCall, NativeToolSpec, ToolUseOptions,
};

#[derive(Debug, Clone)]
pub struct OllamaClient {
    base_url: String,
    client: Client,
}

#[derive(Debug, Clone, Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    stream: bool,
    messages: Vec<Message<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<ToolSpec<'a>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    options: Option<OllamaOptions>,
    #[serde(skip_serializing_if = "Option::is_none")]
    keep_alive: Option<i32>,
}

#[derive(Debug, Clone, Serialize)]
struct Message<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(Debug, Clone, Deserialize)]
struct ChatResponse {
    message: Option<ChatMessage>,
}

#[derive(Debug, Clone, Deserialize)]
struct ChatMessage {
    content: String,
    #[serde(default)]
    tool_calls: Vec<OllamaToolCall>,
}

#[derive(Debug, Clone, Deserialize)]
struct StreamChunk {
    message: Option<ChatMessage>,
    done: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
struct ToolSpec<'a> {
    r#type: &'static str,
    function: ToolFunction<'a>,
}

#[derive(Debug, Clone, Serialize)]
struct ToolFunction<'a> {
    name: &'a str,
    description: &'a str,
    parameters: Value,
}

#[derive(Debug, Clone, Deserialize)]
struct OllamaToolCall {
    function: OllamaToolFunctionCall,
}

#[derive(Debug, Clone, Deserialize)]
struct OllamaToolFunctionCall {
    name: String,
    arguments: Value,
}

#[derive(Debug, Clone, Serialize)]
struct OllamaOptions {
    temperature: f32,
    num_ctx: usize,
}

impl OllamaClient {
    pub fn new(base_url: impl Into<String>) -> anyhow::Result<Self> {
        let client = Client::builder()
            .timeout(Duration::from_secs(300))
            .build()?;
        Ok(Self {
            base_url: base_url.into(),
            client,
        })
    }

    pub async fn health(&self) -> anyhow::Result<()> {
        let url = format!("{}/api/version", self.base_url);
        self.client.get(url).send().await?.error_for_status()?;
        Ok(())
    }

    pub async fn list_models(&self) -> anyhow::Result<Vec<String>> {
        let url = format!("{}/api/tags", self.base_url);
        let value: serde_json::Value = self
            .client
            .get(url)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        let models = value["models"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|m| m["name"].as_str().map(ToOwned::to_owned))
            .collect();
        Ok(models)
    }

    pub async fn chat(&self, model: &str, prompt: &str) -> anyhow::Result<String> {
        let url = format!("{}/api/chat", self.base_url);
        let req = ChatRequest {
            model,
            stream: false,
            messages: vec![Message {
                role: "user",
                content: prompt,
            }],
            tools: None,
            options: None,
            keep_alive: None,
        };
        let body: ChatResponse = self
            .client
            .post(url)
            .json(&req)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        Ok(body.message.map(|m| m.content).unwrap_or_default())
    }

    pub async fn chat_stream(&self, model: &str, prompt: &str) -> anyhow::Result<String> {
        let url = format!("{}/api/chat", self.base_url);
        let req = ChatRequest {
            model,
            stream: true,
            messages: vec![Message {
                role: "user",
                content: prompt,
            }],
            tools: None,
            options: None,
            keep_alive: None,
        };
        let mut resp_stream = self
            .client
            .post(url)
            .json(&req)
            .send()
            .await?
            .error_for_status()?
            .bytes_stream();

        let mut parser = NdjsonStreamParser::default();
        let mut out = String::new();
        while let Some(chunk) = resp_stream.next().await {
            let text = String::from_utf8(chunk?.to_vec())?;
            for parsed in parser.push::<StreamChunk>(&text) {
                let parsed = parsed?;
                if let Some(message) = parsed.message {
                    out.push_str(&message.content);
                }
                if parsed.done.unwrap_or(false) {
                    return Ok(out);
                }
            }
        }
        for parsed in parser.finish::<StreamChunk>() {
            let parsed = parsed?;
            if let Some(message) = parsed.message {
                out.push_str(&message.content);
            }
            if parsed.done.unwrap_or(false) {
                return Ok(out);
            }
        }
        Ok(out)
    }

    pub async fn chat_with_tools(
        &self,
        model: &str,
        prompt: &str,
        tools: &[NativeToolSpec],
        options: ToolUseOptions,
    ) -> anyhow::Result<NativeModelResponse> {
        let url = format!("{}/api/chat", self.base_url);
        let req = ChatRequest {
            model,
            stream: false,
            messages: vec![Message {
                role: "user",
                content: prompt,
            }],
            tools: Some(
                tools
                    .iter()
                    .map(|tool| ToolSpec {
                        r#type: "function",
                        function: ToolFunction {
                            name: tool.name,
                            description: tool.description,
                            parameters: tool.input_schema.clone(),
                        },
                    })
                    .collect(),
            ),
            options: Some(OllamaOptions {
                temperature: options.temperature,
                num_ctx: options.max_context_tokens,
            }),
            keep_alive: options.keep_alive.then_some(-1),
        };
        let body: ChatResponse = self
            .client
            .post(url)
            .json(&req)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        let message = body.message.unwrap_or(ChatMessage {
            content: String::new(),
            tool_calls: Vec::new(),
        });
        if !message.tool_calls.is_empty() {
            return Ok(NativeModelResponse::ToolCalls(
                message
                    .tool_calls
                    .into_iter()
                    .map(|call| NativeToolCall {
                        id: None,
                        name: call.function.name,
                        arguments: call.function.arguments,
                    })
                    .collect(),
            ));
        }
        Ok(NativeModelResponse::Message(message.content))
    }
}
