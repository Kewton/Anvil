use std::time::Duration;

use futures_util::StreamExt;
use reqwest::Client;
use serde::{Deserialize, Serialize};

use crate::models::stream::SseStreamParser;

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
}
