pub mod looping;
pub mod plan;
pub mod subagent;

use std::collections::BTreeMap;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, anyhow};
use serde::Deserialize;

use crate::agent::looping::{LoopConfig, LoopDriver, LoopEvent, ModelExchange};
use crate::agent::plan::{PlanDocument, PlanState};
use crate::agent::subagent::{SubagentRequest, SubagentRunner};
use crate::config::model_profiles::profile_for_model;
use crate::config::{AppConfig, ProviderKind};
use crate::instructions::{LoadedInstructions, load_instructions};
use crate::models::lm_studio::LmStudioClient;
use crate::models::ollama::OllamaClient;
use crate::policy::permissions::{PermissionCategory, PermissionPolicy};
use crate::slash::builtins::BuiltinCommand;
use crate::slash::custom::CustomExecutionContext;
use crate::slash::registry::{ResolvedSlashCommand, SlashRegistry};
use crate::state::audit::{
    AuditActor, AuditEvent, AuditEventData, AuditLog, AuditMetadata, AuditSource, ToolResultStatus,
    redact_map,
};
use crate::state::memory::MemoryStore;
use crate::state::session::Session;
use crate::state::summary::{SummaryController, SummaryInput, SummaryPolicy};
use crate::tools::{GeneratedFile, write_files};
use crate::ui::interactive::{FooterState, InteractiveFrame, SpinnerHandle, UiEvent};
use crate::ui::render::{render_banner, render_frame};

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
        println!("{}", render_banner());
        println!("Anvil interactive mode");
        println!("provider: {:?}", self.config.provider);
        println!("model: {}", self.config.model);
        println!("cwd: {}", self.config.cwd.display());
        println!("type /exit to quit, /memory add|show|edit, /plan, /act, /subagent");

        let memory = MemoryStore::new(self.config.cwd.join("ANVIL-MEMORY.md"));
        let registry = SlashRegistry::load(&self.config.cwd)?;
        let mut plan_state = PlanState::default();
        let profile = profile_for_model(&self.config.model);
        let summary = SummaryController::new(SummaryPolicy::default());
        let mut transcript = Vec::new();
        let loop_driver = LoopDriver::new(LoopConfig::default());
        let mut loop_turns = Vec::new();
        loop {
            let frame = InteractiveFrame {
                title: "Anvil".to_string(),
                provider: format!("{:?}", self.config.provider).to_lowercase(),
                model: self.config.model.clone(),
                cwd: self.config.cwd.display().to_string(),
                transcript: transcript.clone(),
                footer: FooterState {
                    mode: format!("{:?}", plan_state.mode).to_lowercase(),
                    pending_hint: "/memory show".to_string(),
                    token_status: format!(
                        "{}/{}",
                        transcript.len() * 1200,
                        profile.max_context_tokens
                    ),
                },
            };
            println!("{}", render_frame(&frame));
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
            transcript.push(UiEvent::UserInput(input.to_string()));
            if let Some(command) = registry.resolve(input)? {
                let spinner = SpinnerHandle::start("slash");
                let output = self
                    .execute_slash(command, memory.path(), &mut plan_state)
                    .await?;
                spinner.stop("slash command finished");
                transcript.push(UiEvent::AgentText(output.clone()));
                println!("{output}");
                continue;
            }

            let policy =
                PermissionPolicy::from_mode(self.config.permission_mode, PermissionCategory::Read);
            println!("permission policy: {:?}", policy.base_requirement());
            let prompt = if let Some(injection) = plan_state.injection() {
                format!("{injection}\n\nUser task:\n{input}")
            } else {
                input.to_string()
            };
            if summary.should_summarize(SummaryInput {
                tokens: transcript.len() * 1200,
                turns: transcript.len() / 2,
            }) {
                transcript.push(UiEvent::AgentText(
                    summary.summarize_history(
                        &transcript
                            .iter()
                            .map(|event| match event {
                                UiEvent::UserInput(text)
                                | UiEvent::AgentText(text)
                                | UiEvent::ToolCall(text) => text.clone(),
                            })
                            .collect::<Vec<_>>(),
                    ),
                ));
            }
            let spinner = SpinnerHandle::start("model");
            let reply = loop_driver
                .run_with_observer(
                    self,
                    &self.config.cwd,
                    &prompt,
                    loop_turns.clone(),
                    |event| print_loop_event(&event),
                )
                .await;
            spinner.stop("model response received");
            let reply = reply.map_err(|err| anyhow!(err.to_string()))?;
            loop_turns = reply.turns.clone();
            transcript.push(UiEvent::AgentText(
                summary.truncate_tool_output(&reply.final_text, 800),
            ));
            println!("\n---\n{}", truncate(&reply.final_text, 240));
        }
        Ok(())
    }

    async fn chat(&self, prompt: &str) -> anyhow::Result<String> {
        match &self.model {
            ModelBackend::Ollama(client) => client.chat(&self.config.model, prompt).await,
            ModelBackend::LmStudio(client) => client.chat(&self.config.model, prompt).await,
        }
    }

    async fn execute_slash(
        &self,
        command: ResolvedSlashCommand,
        memory_path: &Path,
        plan_state: &mut PlanState,
    ) -> anyhow::Result<String> {
        match command {
            ResolvedSlashCommand::Builtin(command) => match command {
                BuiltinCommand::MemoryAdd { .. }
                | BuiltinCommand::MemoryShow
                | BuiltinCommand::MemoryEdit { .. } => command.execute(memory_path),
                BuiltinCommand::PlanCreate { slug, text } => {
                    let doc = PlanState::create_plan(&self.config.cwd, &slug, &text)?;
                    *plan_state = PlanState::enter_plan(doc);
                    Ok(format!(
                        "plan created: {}",
                        plan_state.active_path_display()
                    ))
                }
                BuiltinCommand::PlanShow => Ok(plan_state
                    .show()
                    .unwrap_or_else(|| "no active plan".to_string())),
                BuiltinCommand::Act { path } => {
                    let doc = if let Some(path) = path {
                        PlanDocument::load(&self.config.cwd.join(path))?
                    } else if let Some(doc) = plan_state.active_document().cloned() {
                        doc
                    } else {
                        anyhow::bail!("no plan available to activate");
                    };
                    *plan_state = PlanState::activate(doc);
                    Ok("mode switched to act".to_string())
                }
                BuiltinCommand::SubagentRun { task } => {
                    let audit = AuditLog::new(self.config.state_dir.join("audit.log.jsonl"));
                    let runner = SubagentRunner::new(&self.config.cwd, &self.config.state_dir);
                    let report = runner.run(
                        "sess_interactive",
                        &audit,
                        SubagentRequest {
                            task,
                            granted_permissions: vec![PermissionCategory::SubagentRead],
                        },
                    )?;
                    Ok(report.summary)
                }
            },
            ResolvedSlashCommand::Custom(invocation) => {
                invocation.execute(&CustomExecutionContext {
                    memory_path: memory_path.to_path_buf(),
                })
            }
        }
    }
}

fn print_loop_event(event: &LoopEvent) {
    match event {
        LoopEvent::StepStarted { step } => println!("\n- agent loop step {step}"),
        LoopEvent::ModelResponseReceived { bytes, elapsed_ms } => {
            println!("- model response chunk received ({bytes} bytes, {elapsed_ms} ms)")
        }
        LoopEvent::ModelResponsePreview { preview } => {
            println!("- model raw: {}", truncate(preview, 200))
        }
        LoopEvent::ProtocolRetry {
            error_kind,
            message,
            retry,
            max_retries,
        } => println!(
            "- protocol error [{error_kind}] retrying {retry}/{max_retries}: {}",
            truncate(message, 120)
        ),
        LoopEvent::FinalRejected {
            reason,
            retry,
            max_retries,
        } => println!(
            "- final rejected retrying {retry}/{max_retries}: {}",
            truncate(reason, 120)
        ),
        LoopEvent::ToolSchemaRetry {
            tool,
            message,
            retry,
            max_retries,
        } => println!(
            "- tool schema retry [{tool}] {retry}/{max_retries}: {}",
            truncate(message, 120)
        ),
        LoopEvent::ToolExecutionRetry {
            tool,
            message,
            retry,
            max_retries,
        } => println!(
            "- tool execution retry [{tool}] {retry}/{max_retries}: {}",
            truncate(message, 120)
        ),
        LoopEvent::ToolExecutionStarted { tool, summary } => {
            println!("- tool start [{tool}] {summary}")
        }
        LoopEvent::ToolCallValidated { tool, normalized } => {
            println!("- tool validated [{tool}] {}", truncate(normalized, 200))
        }
        LoopEvent::ToolExecutionFinished { tool, elapsed_ms } => {
            println!("- tool done [{tool}] ({elapsed_ms} ms)")
        }
        LoopEvent::ToolResultPreview { tool, preview } => {
            println!("- tool result [{tool}] {}", truncate(preview, 180))
        }
        LoopEvent::ToolResultReused { tool, reuse_count } => {
            println!("- tool result reused [{tool}] (reuse #{reuse_count})")
        }
        LoopEvent::StepFinished { step, elapsed_ms } => {
            println!("- step {step} finished ({elapsed_ms} ms)")
        }
        LoopEvent::FinalReady => println!("- final response ready"),
    }
}

#[async_trait::async_trait]
impl ModelExchange for Agent {
    async fn complete(&self, prompt: &str) -> anyhow::Result<String> {
        self.chat(&format!(
            "Project instructions:\n{}\n\nMemory:\n{}\n\n{}",
            load_instructions(&self.config.cwd)?
                .project_text
                .unwrap_or_else(|| "(none)".to_string()),
            load_instructions(&self.config.cwd)?.memory_text,
            prompt
        ))
        .await
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
