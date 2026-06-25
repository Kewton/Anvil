use crate::config::{Config, Provider};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UiStatus {
    pub mode: String,
    pub provider: String,
    pub model: String,
    pub context_budget: usize,
    pub yes: bool,
    pub prompt_tokens: Option<u64>,
    pub completion_tokens: Option<u64>,
}

impl UiStatus {
    pub fn from_config(config: &Config) -> Self {
        Self {
            mode: "act".to_string(),
            provider: provider_label(config.provider).to_string(),
            model: config.model.clone(),
            context_budget: config.context_budget,
            yes: config.yes,
            prompt_tokens: None,
            completion_tokens: None,
        }
    }

    pub fn for_model_reply(
        config: &Config,
        model: &str,
        provider: &str,
        prompt: Option<u64>,
        completion: Option<u64>,
    ) -> Self {
        Self {
            mode: "act".to_string(),
            provider: provider.to_string(),
            model: model.to_string(),
            context_budget: config.context_budget,
            yes: config.yes,
            prompt_tokens: prompt,
            completion_tokens: completion,
        }
    }

    pub fn token_total(&self) -> Option<u64> {
        match (self.prompt_tokens, self.completion_tokens) {
            (None, None) => None,
            (prompt, completion) => Some(prompt.unwrap_or(0) + completion.unwrap_or(0)),
        }
    }
}

fn provider_label(provider: Provider) -> &'static str {
    match provider {
        Provider::Ollama => "ollama",
        Provider::Openai => "openai",
        Provider::Gemini => "gemini",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_token_total_unknown_stays_none() {
        let status = UiStatus {
            mode: "act".to_string(),
            provider: "gemini".to_string(),
            model: "m".to_string(),
            context_budget: 1000,
            yes: true,
            prompt_tokens: None,
            completion_tokens: None,
        };
        assert_eq!(status.token_total(), None);
    }

    #[test]
    fn status_token_total_uses_known_values_only() {
        let status = UiStatus {
            mode: "act".to_string(),
            provider: "ollama".to_string(),
            model: "m".to_string(),
            context_budget: 1000,
            yes: true,
            prompt_tokens: Some(10),
            completion_tokens: None,
        };
        assert_eq!(status.token_total(), Some(10));
    }
}
