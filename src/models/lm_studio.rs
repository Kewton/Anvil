use crate::models::client::ModelClient;

#[derive(Debug, Default)]
pub struct LmStudioClient;

impl ModelClient for LmStudioClient {
    fn provider_name(&self) -> &'static str {
        "lm_studio"
    }
}
