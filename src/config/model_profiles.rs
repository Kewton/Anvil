#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelProfile {
    pub name: String,
    pub max_context_tokens: usize,
    pub summary_trigger_tokens: usize,
    pub subagent_trigger_tokens: usize,
}

impl ModelProfile {
    pub fn default_for(name: &str) -> Self {
        Self {
            name: name.to_string(),
            max_context_tokens: 200_000,
            summary_trigger_tokens: 48_000,
            subagent_trigger_tokens: 96_000,
        }
    }
}

pub fn profile_for_model(name: &str) -> ModelProfile {
    match name {
        "qwen3.5:35b" => ModelProfile {
            name: name.to_string(),
            max_context_tokens: 200_000,
            summary_trigger_tokens: 48_000,
            subagent_trigger_tokens: 96_000,
        },
        _ => ModelProfile::default_for(name),
    }
}
