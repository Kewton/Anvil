use anyhow::bail;

use crate::models::client::{ModelClient, ModelRequest, ModelResponse};
use crate::models::lm_studio::LmStudioClient;
use crate::models::ollama::OllamaClient;

pub struct ModelRouter {
    clients: Vec<Box<dyn ModelClient>>,
}

impl Default for ModelRouter {
    fn default() -> Self {
        Self {
            clients: vec![
                Box::new(LmStudioClient::default()),
                Box::new(OllamaClient::default()),
            ],
        }
    }
}

impl ModelRouter {
    pub fn new(clients: Vec<Box<dyn ModelClient>>) -> Self {
        Self { clients }
    }

    pub fn route<'a>(&'a self, model: &str) -> anyhow::Result<&'a dyn ModelClient> {
        self.clients
            .iter()
            .find(|client| client.can_handle(model))
            .map(|client| client.as_ref())
            .ok_or_else(|| anyhow::anyhow!("no model provider could handle model {model}"))
    }

    pub fn complete(&self, request: &ModelRequest) -> anyhow::Result<ModelResponse> {
        let client = self.route(&request.model)?;
        let response = client.complete(request)?;
        if response.model != request.model {
            bail!("model router returned a mismatched model response");
        }
        Ok(response)
    }

    pub fn stream_complete(
        &self,
        request: &ModelRequest,
        on_chunk: &mut dyn FnMut(&str),
    ) -> anyhow::Result<ModelResponse> {
        let client = self.route(&request.model)?;
        let response = client.stream_complete(request, on_chunk)?;
        if response.model != request.model {
            bail!("model router returned a mismatched model response");
        }
        Ok(response)
    }
}
