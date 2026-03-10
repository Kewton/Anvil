use std::time::Duration;

use futures_util::StreamExt;
use reqwest::Client;
use serde::{Deserialize, Serialize};

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
}

#[derive(Debug, Clone, Deserialize)]
struct StreamChunk {
    message: Option<ChatMessage>,
    done: Option<bool>,
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
        };
        let mut resp_stream = self
            .client
            .post(url)
            .json(&req)
            .send()
            .await?
            .error_for_status()?
            .bytes_stream();

        let mut buf = String::new();
        let mut out = String::new();
        while let Some(chunk) = resp_stream.next().await {
            let text = String::from_utf8(chunk?.to_vec())?;
            buf.push_str(&text);
            while let Some(pos) = buf.find('\n') {
                let line = buf[..pos].trim().to_string();
                buf = buf[pos + 1..].to_string();
                if line.is_empty() {
                    continue;
                }
                let parsed: StreamChunk = serde_json::from_str(&line)?;
                if let Some(message) = parsed.message {
                    out.push_str(&message.content);
                    print!("{}", message.content);
                }
                if parsed.done.unwrap_or(false) {
                    println!();
                    return Ok(out);
                }
            }
        }
        Ok(out)
    }
}
