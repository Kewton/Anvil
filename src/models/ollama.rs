use crate::models::client::ModelClient;

#[derive(Debug, Default)]
pub struct OllamaClient;

impl ModelClient for OllamaClient {
    fn provider_name(&self) -> &'static str {
        "ollama"
    }
}
