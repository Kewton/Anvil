use crate::models::client::{ModelClient, ModelRequest, ModelResponse};

#[derive(Debug, Clone)]
pub struct LmStudioClient {
    endpoint: String,
}

impl Default for LmStudioClient {
    fn default() -> Self {
        Self {
            endpoint: "http://127.0.0.1:1234".to_string(),
        }
    }
}

impl ModelClient for LmStudioClient {
    fn provider_name(&self) -> &'static str {
        "lm_studio"
    }

    fn can_handle(&self, model: &str) -> bool {
        model.starts_with("lmstudio/")
    }

    fn complete(&self, request: &ModelRequest) -> anyhow::Result<ModelResponse> {
        Ok(ModelResponse {
            provider: self.provider_name().to_string(),
            model: request.model.clone(),
            output: format!(
                "lm-studio dry-run via {} for model {}",
                self.endpoint, request.model
            ),
        })
    }
}
