#[derive(Debug, Clone)]
pub struct ModelRequest {
    pub model: String,
    pub system_prompt: String,
    pub user_prompt: String,
}

#[derive(Debug, Clone)]
pub struct ModelResponse {
    pub provider: String,
    pub model: String,
    pub output: String,
}

pub trait ModelClient {
    fn provider_name(&self) -> &'static str;
    fn can_handle(&self, model: &str) -> bool;
    fn complete(&self, request: &ModelRequest) -> anyhow::Result<ModelResponse>;

    fn stream_complete(
        &self,
        request: &ModelRequest,
        on_chunk: &mut dyn FnMut(&str),
    ) -> anyhow::Result<ModelResponse> {
        let response = self.complete(request)?;
        on_chunk(&response.output);
        Ok(response)
    }
}
