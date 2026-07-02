use std::fmt;
use std::str::FromStr;

use clap::ValueEnum;

use crate::gemini::GeminiClient;
use crate::ollama::client::{AssistantReply, OllamaClient};
use crate::openai::OpenAiClient;
use crate::session::store::ConversationMessage;

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum PlannerProvider {
    Ollama,
    Gemini,
    #[value(alias = "gpt")]
    Openai,
}

impl fmt::Display for PlannerProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = match self {
            Self::Ollama => "ollama",
            Self::Gemini => "gemini",
            Self::Openai => "openai",
        };
        write!(f, "{value}")
    }
}

impl FromStr for PlannerProvider {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().replace('_', "-").as_str() {
            "ollama" => Ok(Self::Ollama),
            "gemini" => Ok(Self::Gemini),
            "openai" | "gpt" => Ok(Self::Openai),
            other => Err(format!(
                "unknown planner provider `{other}`; expected ollama, gemini, or openai"
            )),
        }
    }
}

pub trait PlannerLlm {
    fn chat_plan(&mut self, messages: &[ConversationMessage]) -> Result<AssistantReply, String>;
    fn label(&self) -> String;
}

pub struct OllamaPlannerLlm {
    client: OllamaClient,
    model: String,
}

impl OllamaPlannerLlm {
    pub fn new(client: OllamaClient, model: String) -> Self {
        Self { client, model }
    }
}

impl PlannerLlm for OllamaPlannerLlm {
    fn chat_plan(&mut self, messages: &[ConversationMessage]) -> Result<AssistantReply, String> {
        self.client
            .chat_with_mode(&self.model, messages, &[], false)
    }

    fn label(&self) -> String {
        format!("ollama:{}", self.model)
    }
}

pub struct GeminiPlannerLlm {
    client: GeminiClient,
    model: String,
}

impl GeminiPlannerLlm {
    pub fn new(client: GeminiClient, model: String) -> Self {
        Self { client, model }
    }
}

impl PlannerLlm for GeminiPlannerLlm {
    fn chat_plan(&mut self, messages: &[ConversationMessage]) -> Result<AssistantReply, String> {
        self.client.chat_text(&self.model, messages)
    }

    fn label(&self) -> String {
        format!("gemini:{}", self.model)
    }
}

pub struct OpenAiPlannerLlm {
    client: OpenAiClient,
    model: String,
}

impl OpenAiPlannerLlm {
    pub fn new(client: OpenAiClient, model: String) -> Self {
        Self { client, model }
    }
}

impl PlannerLlm for OpenAiPlannerLlm {
    fn chat_plan(&mut self, messages: &[ConversationMessage]) -> Result<AssistantReply, String> {
        self.client.chat_text(&self.model, messages)
    }

    fn label(&self) -> String {
        format!("openai:{}", self.model)
    }
}

pub fn build_planner_llm(
    provider: PlannerProvider,
    ollama_client: Option<&OllamaClient>,
    gemini_client: Option<&GeminiClient>,
    openai_client: Option<&OpenAiClient>,
    model: String,
) -> Result<Box<dyn PlannerLlm>, String> {
    match provider {
        PlannerProvider::Ollama => Ok(Box::new(OllamaPlannerLlm::new(
            ollama_client
                .ok_or_else(|| "planner provider `ollama` requires Ollama client".to_string())?
                .clone(),
            model,
        ))),
        PlannerProvider::Gemini => Ok(Box::new(GeminiPlannerLlm::new(
            gemini_client
                .ok_or_else(|| "planner provider `gemini` requires Gemini client".to_string())?
                .clone(),
            model,
        ))),
        PlannerProvider::Openai => Ok(Box::new(OpenAiPlannerLlm::new(
            openai_client
                .ok_or_else(|| "planner provider `openai` requires OpenAI client".to_string())?
                .clone(),
            model,
        ))),
    }
}
