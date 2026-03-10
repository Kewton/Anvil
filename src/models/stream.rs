use anyhow::{Context, anyhow};
use serde::de::DeserializeOwned;

#[derive(Debug, Default, Clone)]
pub struct NdjsonStreamParser {
    buffer: String,
}

impl NdjsonStreamParser {
    pub fn push<T>(&mut self, chunk: &str) -> Vec<anyhow::Result<T>>
    where
        T: DeserializeOwned,
    {
        self.buffer.push_str(chunk);
        self.drain_ready()
    }

    pub fn finish<T>(&mut self) -> Vec<anyhow::Result<T>>
    where
        T: DeserializeOwned,
    {
        if self.buffer.trim().is_empty() {
            return Vec::new();
        }

        let tail = std::mem::take(&mut self.buffer);
        vec![
            serde_json::from_str::<T>(tail.trim())
                .map_err(|err| anyhow!(err.to_string()))
                .context("invalid NDJSON tail"),
        ]
    }

    fn drain_ready<T>(&mut self) -> Vec<anyhow::Result<T>>
    where
        T: DeserializeOwned,
    {
        let mut parsed = Vec::new();
        while let Some(pos) = self.buffer.find('\n') {
            let line = self.buffer[..pos].trim().to_string();
            self.buffer = self.buffer[pos + 1..].to_string();
            if line.is_empty() {
                continue;
            }
            parsed.push(
                serde_json::from_str::<T>(&line)
                    .map_err(|err| anyhow!(err.to_string()))
                    .context("invalid NDJSON line"),
            );
        }
        parsed
    }
}

pub fn parse_ndjson_events<T>(input: &str) -> anyhow::Result<Vec<T>>
where
    T: DeserializeOwned,
{
    let mut parser = NdjsonStreamParser::default();
    let mut out = Vec::new();
    for item in parser.push::<T>(input) {
        out.push(item?);
    }
    for item in parser.finish::<T>() {
        out.push(item?);
    }
    Ok(out)
}

#[derive(Debug, Default, Clone)]
pub struct SseStreamParser {
    buffer: String,
}

impl SseStreamParser {
    pub fn push<T>(&mut self, chunk: &str) -> Vec<anyhow::Result<T>>
    where
        T: DeserializeOwned,
    {
        self.buffer.push_str(chunk);
        self.drain_ready()
    }

    pub fn finish<T>(&mut self) -> Vec<anyhow::Result<T>>
    where
        T: DeserializeOwned,
    {
        if self.buffer.trim().is_empty() {
            return Vec::new();
        }
        self.drain_ready()
    }

    fn drain_ready<T>(&mut self) -> Vec<anyhow::Result<T>>
    where
        T: DeserializeOwned,
    {
        let mut parsed = Vec::new();
        while let Some(pos) = self.buffer.find("\n\n") {
            let event = self.buffer[..pos].trim().to_string();
            self.buffer = self.buffer[pos + 2..].to_string();
            if event.is_empty() {
                continue;
            }
            let Some(payload) = event.strip_prefix("data:") else {
                parsed.push(Err(anyhow!("invalid SSE event")));
                continue;
            };
            let payload = payload.trim();
            if payload == "[DONE]" {
                continue;
            }
            parsed.push(
                serde_json::from_str::<T>(payload)
                    .map_err(|err| anyhow!(err.to_string()))
                    .context("invalid SSE data event"),
            );
        }
        parsed
    }
}
