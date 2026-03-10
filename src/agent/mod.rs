use std::collections::BTreeMap;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, anyhow};
use serde::Deserialize;

use crate::config::{AppConfig, ProviderKind};
use crate::instructions::{LoadedInstructions, load_instructions};
use crate::models::lm_studio::LmStudioClient;
use crate::models::ollama::OllamaClient;
use crate::policy::permissions::{PermissionCategory, PermissionPolicy};
use crate::slash::registry::SlashRegistry;
use crate::state::audit::{
    AuditActor, AuditEvent, AuditEventData, AuditLog, AuditMetadata, AuditSource, ToolResultStatus,
    redact_map,
};
use crate::state::memory::MemoryStore;
use crate::state::session::Session;
use crate::tools::{GeneratedFile, write_files};

#[derive(Debug, Clone)]
pub struct Agent {
    config: AppConfig,
    model: ModelBackend,
}

#[derive(Debug, Clone)]
pub struct OneShotRequest {
    pub prompt: String,
    pub target_dir: PathBuf,
}

#[derive(Debug, Clone)]
pub struct OneShotOutput {
    pub final_message: String,
    pub written_files: Vec<PathBuf>,
}

#[derive(Debug, Deserialize)]
struct ModelFile {
    path: String,
    content: String,
}

#[derive(Debug, Deserialize)]
struct ModelPlan {
    summary: String,
    files: Vec<ModelFile>,
}

#[derive(Debug, Clone)]
enum ModelBackend {
    Ollama(OllamaClient),
    LmStudio(LmStudioClient),
}

impl Agent {
    pub async fn new(config: AppConfig) -> anyhow::Result<Self> {
        let model = match config.provider {
            ProviderKind::Ollama => ModelBackend::Ollama(OllamaClient::new(config.host.clone())?),
            ProviderKind::LmStudio => {
                ModelBackend::LmStudio(LmStudioClient::new(config.host.clone())?)
            }
        };
        Ok(Self { config, model })
    }

    pub async fn run_one_shot(&self, req: OneShotRequest) -> anyhow::Result<OneShotOutput> {
        let session = Session::new(&self.config.state_dir);
        session.save(&self.config.state_dir.join("session.json"))?;
        let audit = AuditLog::new(self.config.state_dir.join("audit.log.jsonl"));
        let instructions = load_instructions(&self.config.cwd)?;
        audit.append(&AuditEvent {
            meta: AuditMetadata::new(
                &session.id,
                AuditActor::MainAgent,
                AuditSource::OneShot,
                &self.config.cwd,
            ),
            data: AuditEventData::SessionStarted {
                model: self.config.model.clone(),
                permission_mode: self.config.permission_mode,
            },
        })?;

        let prompt = build_generation_prompt(&instructions, &req.prompt, &req.target_dir);
        let text = self.chat(&prompt).await?;
        let plan = parse_model_plan(&text)?;
        let generated = plan
            .files
            .into_iter()
            .map(|f| GeneratedFile {
                path: PathBuf::from(f.path),
                content: f.content,
            })
            .collect::<Vec<_>>();

        let mut summary_map = BTreeMap::new();
        summary_map.insert("prompt".to_string(), truncate(&req.prompt, 120));
        audit.append(&AuditEvent {
            meta: AuditMetadata::new(
                &session.id,
                AuditActor::MainAgent,
                AuditSource::OneShot,
                &self.config.cwd,
            ),
            data: AuditEventData::ToolExecution {
                tool_name: "write_files".to_string(),
                args_summary: redact_map(&summary_map),
            },
        })?;

        let written_files = write_files(&req.target_dir, &generated)?;
        audit.append(&AuditEvent {
            meta: AuditMetadata::new(
                &session.id,
                AuditActor::System,
                AuditSource::OneShot,
                &self.config.cwd,
            ),
            data: AuditEventData::ToolResult {
                tool_name: "write_files".to_string(),
                status: ToolResultStatus::Ok,
                changed_files: written_files.clone(),
            },
        })?;

        Ok(OneShotOutput {
            final_message: plan.summary,
            written_files,
        })
    }

    pub async fn run_interactive(&self) -> anyhow::Result<()> {
        println!("Anvil interactive mode");
        println!("provider: {:?}", self.config.provider);
        println!("model: {}", self.config.model);
        println!("cwd: {}", self.config.cwd.display());
        println!("type /exit to quit, /memory add|show|edit to manage memory");

        let memory = MemoryStore::new(self.config.cwd.join("ANVIL-MEMORY.md"));
        let registry = SlashRegistry;
        loop {
            print!("anvil> ");
            io::stdout().flush()?;
            let mut line = String::new();
            io::stdin().read_line(&mut line)?;
            let input = line.trim();
            if input.is_empty() {
                continue;
            }
            if input == "/exit" || input == "/quit" {
                break;
            }
            if let Some(command) = registry.resolve(input) {
                let output = command.execute(memory.path())?;
                println!("{output}");
                continue;
            }

            let policy =
                PermissionPolicy::from_mode(self.config.permission_mode, PermissionCategory::Read);
            println!("permission policy: {:?}", policy.base_requirement());
            let reply = self.chat_stream(input).await?;
            println!("\n---\n{}", truncate(&reply, 240));
        }
        Ok(())
    }

    async fn chat(&self, prompt: &str) -> anyhow::Result<String> {
        match &self.model {
            ModelBackend::Ollama(client) => client.chat(&self.config.model, prompt).await,
            ModelBackend::LmStudio(client) => client.chat(&self.config.model, prompt).await,
        }
    }

    async fn chat_stream(&self, prompt: &str) -> anyhow::Result<String> {
        match &self.model {
            ModelBackend::Ollama(client) => client.chat_stream(&self.config.model, prompt).await,
            ModelBackend::LmStudio(client) => client.chat_stream(&self.config.model, prompt).await,
        }
    }
}

fn build_generation_prompt(
    instructions: &LoadedInstructions,
    user_prompt: &str,
    target_dir: &Path,
) -> String {
    format!(
        "You are Anvil, a local coding agent.\n\
Return only valid JSON with this shape:\n\
{{\"summary\":\"...\",\"files\":[{{\"path\":\"relative/path\",\"content\":\"file text\"}}]}}\n\
Do not use markdown fences.\n\
Target directory: {}\n\
Project instructions:\n{}\n\
Memory:\n{}\n\
Task:\n{}",
        target_dir.display(),
        instructions.project_text.as_deref().unwrap_or("(none)"),
        instructions.memory_text,
        user_prompt
    )
}

fn parse_model_plan(text: &str) -> anyhow::Result<ModelPlan> {
    if let Ok(plan) = serde_json::from_str::<ModelPlan>(text) {
        return Ok(plan);
    }
    if let (Some(start), Some(end)) = (text.find('{'), text.rfind('}')) {
        let candidate = &text[start..=end];
        if let Ok(plan) = serde_json::from_str::<ModelPlan>(candidate) {
            return Ok(plan);
        }
    }
    Err(anyhow!(truncate(text, 400))).context("model response was not valid JSON manifest")
}

fn truncate(text: &str, max_chars: usize) -> String {
    text.chars().take(max_chars).collect()
}

#[cfg(test)]
mod tests {
    use super::parse_model_plan;

    #[test]
    fn parse_model_plan_accepts_embedded_json() {
        let plan = parse_model_plan(
            "preface {\"summary\":\"ok\",\"files\":[{\"path\":\"index.html\",\"content\":\"hi\"}]} suffix",
        )
        .unwrap();

        assert_eq!(plan.summary, "ok");
        assert_eq!(plan.files.len(), 1);
    }

    #[test]
    fn parse_model_plan_fails_closed_on_invalid_json() {
        let err = parse_model_plan("definitely not valid").unwrap_err();
        assert!(format!("{err}").contains("model response was not valid JSON manifest"));
    }
}
