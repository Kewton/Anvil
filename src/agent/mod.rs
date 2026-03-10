use std::collections::BTreeMap;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, anyhow};
use serde::Deserialize;

use crate::config::AppConfig;
use crate::instructions::{LoadedInstructions, load_instructions};
use crate::models::ollama::OllamaClient;
use crate::policy::permissions::{PermissionCategory, PermissionPolicy};
use crate::state::audit::{
    AuditActor, AuditEvent, AuditEventData, AuditLog, AuditMetadata, AuditSource, ToolResultStatus,
};
use crate::state::memory::MemoryStore;
use crate::state::session::Session;
use crate::tools::{GeneratedFile, write_files};

#[derive(Debug, Clone)]
pub struct Agent {
    config: AppConfig,
    model: OllamaClient,
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

impl Agent {
    pub async fn new(config: AppConfig) -> anyhow::Result<Self> {
        let model = OllamaClient::new(config.ollama_host.clone())?;
        Ok(Self { config, model })
    }

    pub async fn run_one_shot(&self, req: OneShotRequest) -> anyhow::Result<OneShotOutput> {
        let session = Session::new(&self.config.state_dir);
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
        let text = self.model.chat(&self.config.model, &prompt).await?;
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
                args_summary: summary_map,
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
        println!("model: {}", self.config.model);
        println!("cwd: {}", self.config.cwd.display());
        println!("type /exit to quit, /memory add <text> to update memory");

        let memory = MemoryStore::new(self.config.cwd.join("ANVIL-MEMORY.md"));
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
            if let Some(rest) = input.strip_prefix("/memory add ") {
                memory.add_entry(rest)?;
                println!("memory updated");
                continue;
            }

            let policy =
                PermissionPolicy::from_mode(self.config.permission_mode, PermissionCategory::Read);
            println!("permission policy: {:?}", policy.base_requirement());
            let reply = self.model.chat_stream(&self.config.model, input).await?;
            println!("\n---\n{}", truncate(&reply, 240));
        }
        Ok(())
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
    Err(anyhow!("model response was not valid JSON manifest")).context(truncate(text, 400))
}

fn truncate(text: &str, max_chars: usize) -> String {
    text.chars().take(max_chars).collect()
}
