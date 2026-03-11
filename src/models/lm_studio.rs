use std::time::Duration;

use futures_util::StreamExt;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::models::stream::SseStreamParser;
use crate::models::tool_calling::{
    NativeModelResponse, NativeToolCall, NativeToolSpec, ToolUseOptions,
};

#[derive(Debug, Clone)]
pub struct LmStudioClient {
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
    temperature: Option<f32>,
}

#[derive(Debug, Clone, Serialize)]
struct Message<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(Debug, Deserialize)]
struct ModelsResponse {
    data: Vec<ModelItem>,
}

#[derive(Debug, Deserialize)]
struct ModelItem {
    id: String,
}

#[derive(Debug, Deserialize)]
struct ChatResponse {
    choices: Vec<Choice>,
}

#[derive(Debug, Deserialize)]
struct Choice {
    message: Option<AssistantMessage>,
    delta: Option<AssistantMessage>,
}

#[derive(Debug, Clone, Deserialize)]
struct AssistantMessage {
    content: Option<String>,
    #[serde(default)]
    tool_calls: Vec<OpenAiToolCall>,
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
struct OpenAiToolCall {
    id: Option<String>,
    function: OpenAiFunctionCall,
}

#[derive(Debug, Clone, Deserialize)]
struct OpenAiFunctionCall {
    name: String,
    arguments: Value,
}

impl LmStudioClient {
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
        self.client
            .get(format!("{}/v1/models", self.base_url))
            .send()
            .await?
            .error_for_status()?;
        Ok(())
    }

    pub async fn list_models(&self) -> anyhow::Result<Vec<String>> {
        let body: ModelsResponse = self
            .client
            .get(format!("{}/v1/models", self.base_url))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        Ok(body.data.into_iter().map(|m| m.id).collect())
    }

    pub async fn chat(&self, model: &str, prompt: &str) -> anyhow::Result<String> {
        let body: ChatResponse = self
            .client
            .post(format!("{}/v1/chat/completions", self.base_url))
            .json(&ChatRequest {
                model,
                stream: false,
                messages: vec![Message {
                    role: "user",
                    content: prompt,
                }],
                tools: None,
                temperature: None,
            })
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        Ok(body
            .choices
            .into_iter()
            .find_map(|choice| choice.message.and_then(|msg| msg.content))
            .unwrap_or_default())
    }

    pub async fn chat_stream(&self, model: &str, prompt: &str) -> anyhow::Result<String> {
        let mut stream = self
            .client
            .post(format!("{}/v1/chat/completions", self.base_url))
            .json(&ChatRequest {
                model,
                stream: true,
                messages: vec![Message {
                    role: "user",
                    content: prompt,
                }],
                tools: None,
                temperature: None,
            })
            .send()
            .await?
            .error_for_status()?
            .bytes_stream();
        let mut parser = SseStreamParser::default();
        let mut out = String::new();
        while let Some(chunk) = stream.next().await {
            let text = String::from_utf8(chunk?.to_vec())?;
            for event in parser.push::<ChatResponse>(&text) {
                let event = event?;
                for choice in event.choices {
                    if let Some(content) = choice.delta.and_then(|msg| msg.content) {
                        out.push_str(&content);
                    }
                }
            }
        }
        for event in parser.finish::<ChatResponse>() {
            let event = event?;
            for choice in event.choices {
                if let Some(content) = choice.delta.and_then(|msg| msg.content) {
                    out.push_str(&content);
                }
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
        let body: ChatResponse = self
            .client
            .post(format!("{}/v1/chat/completions", self.base_url))
            .json(&ChatRequest {
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
                temperature: Some(options.temperature),
            })
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        let Some(choice) = body.choices.into_iter().next() else {
            return Ok(NativeModelResponse::Message(String::new()));
        };
        let message = choice.message.or(choice.delta).unwrap_or(AssistantMessage {
            content: None,
            tool_calls: Vec::new(),
        });
        if !message.tool_calls.is_empty() {
            return Ok(NativeModelResponse::ToolCalls(
                message
                    .tool_calls
                    .into_iter()
                    .map(|call| NativeToolCall {
                        id: call.id,
                        name: call.function.name,
                        arguments: call.function.arguments,
                    })
                    .collect(),
            ));
        }
        Ok(NativeModelResponse::Message(
            message.content.unwrap_or_default(),
        ))
    }
}
