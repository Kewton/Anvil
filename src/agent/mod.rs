pub mod looping;
pub mod plan;
pub mod subagent;

use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use crate::agent::looping::{LoopConfig, LoopDriver, LoopEvent, ModelExchange};
use crate::agent::plan::{PlanDocument, PlanState};
use crate::agent::subagent::{SubagentRequest, SubagentRunner};
use crate::config::model_profiles::profile_for_model;
use crate::config::{AppConfig, ProviderKind};
use crate::instructions::load_instructions;
use crate::models::lm_studio::LmStudioClient;
use crate::models::ollama::OllamaClient;
use crate::models::tool_calling::{NativeModelResponse, NativeToolSpec};
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
use crate::ui::interactive::{FooterState, InteractiveFrame, SpinnerHandle, UiEvent};
use crate::ui::render::{render_banner, render_frame};
use anyhow::anyhow;

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
        let before_files = collect_files_recursive(&req.target_dir)?;
        let loop_driver = LoopDriver::new(LoopConfig::default());

        let mut summary_map = BTreeMap::new();
        summary_map.insert("prompt".to_string(), truncate(&req.prompt, 120));
        summary_map.insert(
            "target_dir".to_string(),
            truncate(&req.target_dir.display().to_string(), 120),
        );
        audit.append(&AuditEvent {
            meta: AuditMetadata::new(
                &session.id,
                AuditActor::MainAgent,
                AuditSource::OneShot,
                &self.config.cwd,
            ),
            data: AuditEventData::ToolExecution {
                tool_name: "agent_loop".to_string(),
                args_summary: redact_map(&summary_map),
            },
        })?;

        let output = loop_driver
            .run_with_observer(self, &self.config.cwd, &req.prompt, Vec::new(), |event| {
                print_loop_event(&event)
            })
            .await?;
        let after_files = collect_files_recursive(&req.target_dir)?;
        let written_files = diff_new_or_changed_files(&before_files, &after_files);
        audit.append(&AuditEvent {
            meta: AuditMetadata::new(
                &session.id,
                AuditActor::System,
                AuditSource::OneShot,
                &self.config.cwd,
            ),
            data: AuditEventData::ToolResult {
                tool_name: "agent_loop".to_string(),
                status: ToolResultStatus::Ok,
                changed_files: written_files.clone(),
            },
        })?;

        Ok(OneShotOutput {
            final_message: output.final_text,
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
        LoopEvent::ToolErrorRecorded {
            tool,
            error_kind,
            message,
        } => println!(
            "- tool error [{tool}] {error_kind}: {}",
            truncate(message, 120)
        ),
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

    async fn complete_with_tools(
        &self,
        prompt: &str,
        tools: &[NativeToolSpec],
    ) -> anyhow::Result<Option<NativeModelResponse>> {
        let prompt = format!(
            "Project instructions:\n{}\n\nMemory:\n{}\n\n{}",
            load_instructions(&self.config.cwd)?
                .project_text
                .unwrap_or_else(|| "(none)".to_string()),
            load_instructions(&self.config.cwd)?.memory_text,
            prompt
        );
        let response = match &self.model {
            ModelBackend::Ollama(client) => {
                client
                    .chat_with_tools(
                        &self.config.model,
                        &prompt,
                        tools,
                        crate::models::tool_calling::ToolUseOptions {
                            temperature: profile_for_model(&self.config.model).tool_temperature,
                            max_context_tokens: profile_for_model(&self.config.model)
                                .tool_context_tokens,
                            keep_alive: true,
                        },
                    )
                    .await?
            }
            ModelBackend::LmStudio(client) => {
                client
                    .chat_with_tools(
                        &self.config.model,
                        &prompt,
                        tools,
                        crate::models::tool_calling::ToolUseOptions {
                            temperature: profile_for_model(&self.config.model).tool_temperature,
                            max_context_tokens: profile_for_model(&self.config.model)
                                .tool_context_tokens,
                            keep_alive: true,
                        },
                    )
                    .await?
            }
        };
        Ok(Some(response))
    }
}

fn collect_files_recursive(base: &Path) -> anyhow::Result<BTreeSet<PathBuf>> {
    let mut files = BTreeSet::new();
    if !base.exists() {
        return Ok(files);
    }
    if base.is_file() {
        files.insert(base.to_path_buf());
        return Ok(files);
    }

    for entry in walkdir::WalkDir::new(base) {
        let entry = entry?;
        if entry.file_type().is_file() {
            files.insert(entry.into_path());
        }
    }
    Ok(files)
}

fn diff_new_or_changed_files(
    before: &BTreeSet<PathBuf>,
    after: &BTreeSet<PathBuf>,
) -> Vec<PathBuf> {
    after
        .iter()
        .filter(|path| !before.contains(*path))
        .cloned()
        .collect()
}

fn truncate(text: &str, max_chars: usize) -> String {
    text.chars().take(max_chars).collect()
}
