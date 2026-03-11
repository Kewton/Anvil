pub mod looping;
pub mod plan;
pub mod subagent;

use std::collections::{BTreeMap, BTreeSet};
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
use crate::state::summary::{CarryoverState, SummaryController, SummaryInput, SummaryPolicy};
use crate::ui::interactive::{FooterState, InteractiveFrame, LineEditor, SpinnerHandle, UiEvent};
use crate::ui::render::{render_banner, render_frame, render_result_block, render_startup_help};
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
        println!("{}", render_startup_help());

        let memory = MemoryStore::new(self.config.cwd.join("ANVIL-MEMORY.md"));
        let registry = SlashRegistry::load(&self.config.cwd)?;
        let session_path = self.config.state_dir.join("session.json");
        let mut session = if session_path.exists() {
            Session::load(&session_path).unwrap_or_else(|_| Session::new(&self.config.cwd))
        } else {
            Session::new(&self.config.cwd)
        };
        session.root = self.config.cwd.clone();
        let audit = AuditLog::new(self.config.state_dir.join("audit.log.jsonl"));
        if !session_path.exists() {
            audit.append(&AuditEvent {
                meta: AuditMetadata::new(
                    &session.id,
                    AuditActor::MainAgent,
                    AuditSource::Interactive,
                    &self.config.cwd,
                ),
                data: AuditEventData::SessionStarted {
                    model: self.config.model.clone(),
                    permission_mode: self.config.permission_mode,
                },
            })?;
        }
        session.save(&session_path)?;
        let mut carryover = CarryoverState {
            rolling_summary: session.rolling_summary.clone(),
            summarized_events: session.summarized_events,
        };
        let mut plan_state = PlanState::default();
        let profile = profile_for_model(&self.config.model);
        let summary = SummaryController::new(SummaryPolicy::default());
        let mut transcript = Vec::new();
        let loop_driver = LoopDriver::new(LoopConfig::default());
        let mut loop_turns = Vec::new();
        let mut line_editor = LineEditor::new()?;
        if let Some(existing_summary) = carryover.rolling_summary.as_deref()
            && !existing_summary.trim().is_empty()
        {
            transcript.push(UiEvent::AgentText(format!(
                "🧠 Restored session summary ({})",
                truncate(existing_summary, 120)
            )));
        }
        loop {
            let transcript_strings = transcript
                .iter()
                .map(transcript_event_text)
                .collect::<Vec<_>>();
            let estimated_tokens =
                summary.estimate_tokens(&transcript_strings, carryover.rolling_summary.as_deref());
            let frame = InteractiveFrame {
                title: "Anvil".to_string(),
                provider: format!("{:?}", self.config.provider).to_lowercase(),
                model: self.config.model.clone(),
                cwd: self.config.cwd.display().to_string(),
                transcript: transcript.clone(),
                footer: FooterState {
                    mode: format!("{:?}", plan_state.mode).to_lowercase(),
                    pending_hint: "/memory show".to_string(),
                    token_status: format!("{estimated_tokens}/{}", profile.max_context_tokens),
                },
            };
            println!("{}", render_frame(&frame));
            let input = line_editor.read_command("anvil> ")?;
            if input.is_empty() {
                continue;
            }
            if input == "/exit" || input == "/quit" {
                break;
            }
            transcript.push(UiEvent::UserInput(input.to_string()));
            if let Some(command) = registry.resolve(&input)? {
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
            let mut prompt = if let Some(injection) = plan_state.injection() {
                format!("{injection}\n\nUser task:\n{input}")
            } else {
                input.to_string()
            };
            if summary.should_summarize(SummaryInput {
                tokens: estimated_tokens,
                turns: transcript.len() / 2,
            }) && let Some(outcome) =
                summary.compact_history(carryover.rolling_summary.as_deref(), &transcript_strings)
            {
                carryover.rolling_summary = Some(outcome.rolling_summary.clone());
                carryover.summarized_events += outcome.summarized_events;
                prune_transcript(&mut transcript, outcome.retained_events);
                transcript.push(UiEvent::AgentText(format!(
                    "🧠 Context compacted: summarized {} prior events",
                    outcome.summarized_events
                )));
                session.update_summary(
                    carryover.rolling_summary.clone(),
                    carryover.summarized_events,
                );
                session.save(&session_path)?;
                audit.append(&AuditEvent {
                    meta: AuditMetadata::new(
                        &session.id,
                        AuditActor::System,
                        AuditSource::Interactive,
                        &self.config.cwd,
                    ),
                    data: AuditEventData::SessionCompacted {
                        summary: outcome.rolling_summary,
                        summarized_events: outcome.summarized_events,
                        retained_events: outcome.retained_events,
                    },
                })?;
            }
            if let Some(prefix) = summary.prompt_prefix(&carryover) {
                prompt = format!("{prefix}\n{prompt}");
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
            session.update_summary(
                carryover.rolling_summary.clone(),
                carryover.summarized_events,
            );
            session.save(&session_path)?;
            let result_details = collect_result_details(&reply.final_text, &self.config.cwd);
            println!(
                "\n{}",
                render_result_block(&truncate(&reply.final_text, 240), &result_details)
            );
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

fn prune_transcript(transcript: &mut Vec<UiEvent>, retained_events: usize) {
    if transcript.len() <= retained_events {
        return;
    }
    let start = transcript.len() - retained_events;
    transcript.drain(..start);
}

fn render_workflow(workflow: &[String], current_index: usize) -> String {
    workflow
        .iter()
        .enumerate()
        .map(|(idx, phase)| {
            let position = idx + 1;
            if position == current_index {
                format!("[{position}:{phase}]")
            } else {
                format!("{position}:{phase}")
            }
        })
        .collect::<Vec<_>>()
        .join(" -> ")
}

fn transcript_event_text(event: &UiEvent) -> String {
    match event {
        UiEvent::UserInput(text) => format!("User: {text}"),
        UiEvent::AgentText(text) => format!("Agent: {text}"),
        UiEvent::ToolCall(text) => format!("Tool: {text}"),
    }
}

fn collect_result_details(text: &str, cwd: &Path) -> Vec<String> {
    let mut details = Vec::new();
    for token in text.split_whitespace() {
        let candidate = token.trim_matches(|ch: char| {
            matches!(ch, '`' | '"' | '\'' | ',' | '.' | ')' | '(' | ':' | ';')
        });
        if !(candidate.starts_with("./") || candidate.starts_with('/')) {
            continue;
        }
        let path = PathBuf::from(candidate);
        let normalized = if path.is_absolute() {
            path
        } else {
            cwd.join(path)
        };
        let display = if normalized.starts_with(cwd) {
            match normalized.strip_prefix(cwd) {
                Ok(relative) => format!("./{}", relative.display()),
                Err(_) => normalized.display().to_string(),
            }
        } else {
            normalized.display().to_string()
        };
        if !details.contains(&display) {
            details.push(display);
        }
    }
    details
}

fn print_loop_event(event: &LoopEvent) {
    match event {
        LoopEvent::StepStarted {
            step,
            purpose,
            brief,
            phase,
            plan,
            workflow,
            phase_index,
            phase_total,
            remaining_requirements,
            progress_class,
            stall_count,
            remaining_budget,
        } => {
            println!("\n▶ Agent Step {step}: {purpose}");
            println!(
                "  🧭 Workflow {phase_index}/{phase_total}: {}",
                render_workflow(workflow, *phase_index)
            );
            println!("  📝 Agent brief: {}", truncate(brief, 160));
            println!("  📍 Current phase: {phase}");
            println!("  📈 Progress: {progress_class}");
            println!("  ⏱ Remaining budget: {remaining_budget} steps");
            println!("  🧱 Stall count: {stall_count}");
            if !remaining_requirements.is_empty() {
                println!(
                    "  ✅ Remaining requirements: {}",
                    remaining_requirements.join(", ")
                );
            }
            for item in plan.iter().take(6) {
                println!("  📋 {item}");
            }
        }
        LoopEvent::ModelResponseReceived { bytes, elapsed_ms } => {
            println!("  ◦ Model chunk ({bytes} bytes, {elapsed_ms} ms)")
        }
        LoopEvent::ModelResponsePreview { preview } => {
            println!("  ◦ Model raw: {}", truncate(preview, 200))
        }
        LoopEvent::ProtocolRetry {
            error_kind,
            message,
            retry,
            max_retries,
        } => println!(
            "  ! Recovery protocol [{error_kind}] retry {retry}/{max_retries}: {}",
            truncate(message, 120)
        ),
        LoopEvent::FinalRejected {
            reason,
            retry,
            max_retries,
        } => println!(
            "  ! Recovery final rejected {retry}/{max_retries}: {}",
            truncate(reason, 120)
        ),
        LoopEvent::ToolSchemaRetry {
            tool,
            message,
            retry,
            max_retries,
        } => println!(
            "  ! Recovery schema [{tool}] {retry}/{max_retries}: {}",
            truncate(message, 120)
        ),
        LoopEvent::ToolExecutionRetry {
            tool,
            message,
            retry,
            max_retries,
        } => println!(
            "  ! Recovery execution [{tool}] {retry}/{max_retries}: {}",
            truncate(message, 120)
        ),
        LoopEvent::ToolExecutionStarted { tool, summary } => {
            println!("  ⚙ Tool start [{tool}] {summary}")
        }
        LoopEvent::ToolCallValidated { tool, normalized } => {
            println!("  ✓ Tool validated [{tool}] {}", truncate(normalized, 200))
        }
        LoopEvent::ToolExecutionFinished { tool, elapsed_ms } => {
            println!("  ✓ Tool done [{tool}] ({elapsed_ms} ms)")
        }
        LoopEvent::ToolResultPreview { tool, preview } => {
            println!("  ◦ Tool result [{tool}] {}", truncate(preview, 180))
        }
        LoopEvent::ToolResultReused { tool, reuse_count } => {
            println!("  ◦ Tool reused [{tool}] (reuse #{reuse_count})")
        }
        LoopEvent::ToolErrorRecorded {
            tool,
            error_kind,
            message,
        } => println!(
            "  ! Recovery tool [{tool}] {error_kind}: {}",
            truncate(message, 120)
        ),
        LoopEvent::StepFinished { step, elapsed_ms } => {
            println!("  ✓ Agent step {step} finished ({elapsed_ms} ms)")
        }
        LoopEvent::FinalReady => println!("✔ Agent result ready"),
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
