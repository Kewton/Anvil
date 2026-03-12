use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::anyhow;
use serde::Deserialize;
use serde_json::{Map, Value};

use crate::models::tool_calling::{NativeModelResponse, NativeToolCall, NativeToolSpec};
use crate::tools::{
    edit_file, exec_in_dir, glob_paths, list_dir, mkdir_p, path_exists, read_file, search_in_files,
    stat_path, unified_diff, write_file,
};

#[async_trait::async_trait]
pub trait ModelExchange: Send + Sync {
    async fn complete(&self, prompt: &str) -> anyhow::Result<String>;

    async fn complete_with_tools(
        &self,
        _prompt: &str,
        _tools: &[NativeToolSpec],
    ) -> anyhow::Result<Option<NativeModelResponse>> {
        Ok(None)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelTurn {
    ToolResult {
        tool: String,
        output: String,
    },
    ToolError {
        tool: String,
        error_kind: String,
        message: String,
        hint: String,
    },
    ProtocolError {
        error_kind: String,
        message: String,
        hint: String,
    },
    AssistantFinal(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoopConfig {
    pub max_steps: usize,
    pub create_task_base_budget: usize,
    pub inspect_task_base_budget: usize,
    pub finalize_phase_budget: usize,
    pub max_tool_output_chars: usize,
    pub max_schema_retries: usize,
    pub max_cached_reuses_per_call: usize,
}

impl Default for LoopConfig {
    fn default() -> Self {
        Self {
            max_steps: 24,
            create_task_base_budget: 14,
            inspect_task_base_budget: 8,
            finalize_phase_budget: 3,
            max_tool_output_chars: 1_200,
            max_schema_retries: 1,
            max_cached_reuses_per_call: 2,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoopOutput {
    pub final_text: String,
    pub turns: Vec<ModelTurn>,
}

#[derive(Debug, thiserror::Error)]
pub enum LoopError {
    #[error("invalid tool call: {0}")]
    InvalidToolCall(String),
    #[error("duplicate tool call detected: {0}")]
    DuplicateToolCall(String),
    #[error("max loop steps reached")]
    MaxStepsReached,
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

#[derive(Debug, Clone)]
pub struct LoopDriver {
    config: LoopConfig,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoopEvent {
    StepStarted {
        step: usize,
        purpose: String,
        brief: String,
        phase: String,
        plan: Vec<String>,
        workflow: Vec<String>,
        phase_index: usize,
        phase_total: usize,
        remaining_requirements: Vec<String>,
        progress_class: String,
        stall_count: usize,
        remaining_budget: usize,
    },
    ModelResponseReceived {
        bytes: usize,
        elapsed_ms: u128,
    },
    ModelResponsePreview {
        preview: String,
    },
    ProtocolRetry {
        error_kind: String,
        message: String,
        retry: usize,
        max_retries: usize,
    },
    FinalRejected {
        reason: String,
        retry: usize,
        max_retries: usize,
    },
    ToolSchemaRetry {
        tool: String,
        message: String,
        retry: usize,
        max_retries: usize,
    },
    ToolExecutionRetry {
        tool: String,
        message: String,
        retry: usize,
        max_retries: usize,
    },
    ToolExecutionStarted {
        tool: String,
        summary: String,
    },
    ToolCallValidated {
        tool: String,
        normalized: String,
    },
    ToolExecutionFinished {
        tool: String,
        elapsed_ms: u128,
    },
    ToolResultPreview {
        tool: String,
        preview: String,
    },
    ToolResultReused {
        tool: String,
        reuse_count: usize,
    },
    ToolErrorRecorded {
        tool: String,
        error_kind: String,
        message: String,
    },
    StepFinished {
        step: usize,
        elapsed_ms: u128,
    },
    FinalReady,
}

impl LoopDriver {
    pub fn new(config: LoopConfig) -> Self {
        Self { config }
    }

    pub async fn run<M: ModelExchange>(
        &self,
        model: &M,
        cwd: &Path,
        task: &str,
        prior_turns: Vec<ModelTurn>,
    ) -> Result<LoopOutput, LoopError> {
        self.run_with_observer(model, cwd, task, prior_turns, |_| {})
            .await
    }

    pub async fn run_with_observer<M, F>(
        &self,
        model: &M,
        cwd: &Path,
        task: &str,
        prior_turns: Vec<ModelTurn>,
        mut observer: F,
    ) -> Result<LoopOutput, LoopError>
    where
        M: ModelExchange,
        F: FnMut(LoopEvent),
    {
        let mut turns = prior_turns;
        let mut seen_read_only = BTreeSet::new();
        let mut seen_mutating = BTreeSet::new();
        let mut cached_results = BTreeMap::<String, (String, usize)>::new();
        let mut protocol_retry_state: Option<(String, usize)> = None;
        let mut tool_retry_state: Option<(String, usize)> = None;
        let mut tool_exec_retry_state: Option<(String, usize)> = None;
        let contract = extract_task_contract(task);
        let task_requires_write = contract.requires_write;
        let expected_output_root = contract.output_root.clone();
        let mut requirement_state = RequirementState::new(&contract, self.config);

        for step in 0..self.config.max_steps {
            if requirement_state.should_stop(step) {
                return Err(LoopError::MaxStepsReached);
            }
            let step_number = step + 1;
            let step_started_at = Instant::now();
            let phase = requirement_state.current_phase();
            let plan = step_plan(&contract, &requirement_state, task, &turns);
            requirement_state.note_phase(step_number, phase);
            observer(LoopEvent::StepStarted {
                step: step_number,
                purpose: step_purpose(task, &turns, phase, &requirement_state),
                brief: step_instruction(task, &contract, phase, &requirement_state),
                phase: plan.phase.clone(),
                plan: plan.items,
                workflow: plan.workflow,
                phase_index: plan.phase_index,
                phase_total: plan.phase_total,
                remaining_requirements: requirement_state.remaining_labels(),
                progress_class: requirement_state.last_progress_label().to_string(),
                stall_count: requirement_state.stall_count,
                remaining_budget: requirement_state.remaining_budget(step_number),
            });
            let prompt = build_loop_prompt(task, &contract, &turns, &requirement_state);
            let model_started_at = Instant::now();
            let response = match model.complete_with_tools(&prompt, &tool_specs()).await? {
                Some(native) => {
                    observer(LoopEvent::ModelResponseReceived {
                        bytes: estimate_native_response_size(&native),
                        elapsed_ms: model_started_at.elapsed().as_millis(),
                    });
                    observer(LoopEvent::ModelResponsePreview {
                        preview: truncate(&format!("{native:?}"), 240),
                    });
                    protocol_retry_state = None;
                    native_to_model_response(native)?
                }
                None => {
                    let raw = model.complete(&prompt).await?;
                    observer(LoopEvent::ModelResponseReceived {
                        bytes: raw.len(),
                        elapsed_ms: model_started_at.elapsed().as_millis(),
                    });
                    observer(LoopEvent::ModelResponsePreview {
                        preview: truncate(&raw.replace('\n', "\\n"), 240),
                    });
                    match parse_model_response(&raw) {
                        Ok(response) => {
                            protocol_retry_state = None;
                            response
                        }
                        Err(LoopError::InvalidToolCall(message)) => {
                            let retry = next_retry_count(
                                &mut protocol_retry_state,
                                format!("invalid_json:{message}"),
                            );
                            if retry > self.config.max_schema_retries {
                                return Err(LoopError::InvalidToolCall(message));
                            }
                            observer(LoopEvent::ProtocolRetry {
                                error_kind: "invalid_json".to_string(),
                                message: message.clone(),
                                retry,
                                max_retries: self.config.max_schema_retries,
                            });
                            let turn = ModelTurn::ProtocolError {
                                error_kind: "invalid_json".to_string(),
                                message,
                                hint: "return one complete JSON object only; do not emit partial or truncated JSON".to_string(),
                            };
                            push_turn(&mut turns, turn, &mut observer);
                            observer(LoopEvent::StepFinished {
                                step: step_number,
                                elapsed_ms: step_started_at.elapsed().as_millis(),
                            });
                            continue;
                        }
                        Err(other) => return Err(other),
                    }
                }
            };

            match response {
                ModelResponse::Final { content } => {
                    if task_requires_write && !has_write_evidence(&turns) {
                        let reason =
                            "task requires file changes but no successful write_file/edit_file has occurred yet"
                                .to_string();
                        let retry = next_retry_count(
                            &mut protocol_retry_state,
                            format!("final_without_action:{reason}"),
                        );
                        if retry > self.config.max_schema_retries {
                            return Err(LoopError::InvalidToolCall(reason));
                        }
                        observer(LoopEvent::FinalRejected {
                            reason: reason.clone(),
                            retry,
                            max_retries: self.config.max_schema_retries,
                        });
                        let turn = ModelTurn::ProtocolError {
                            error_kind: "final_without_action".to_string(),
                            message: reason,
                            hint: "if the task requires creating or updating files, use write_file or edit_file before returning final".to_string(),
                        };
                        push_turn(&mut turns, turn, &mut observer);
                        observer(LoopEvent::StepFinished {
                            step: step_number,
                            elapsed_ms: step_started_at.elapsed().as_millis(),
                        });
                        continue;
                    }
                    observer(LoopEvent::FinalReady);
                    observer(LoopEvent::StepFinished {
                        step: step_number,
                        elapsed_ms: step_started_at.elapsed().as_millis(),
                    });
                    turns.push(ModelTurn::AssistantFinal(content.clone()));
                    return Ok(LoopOutput {
                        final_text: content,
                        turns,
                    });
                }
                ModelResponse::ToolCalls { calls } => {
                    for call in calls {
                        let validated = match validate_tool_call(&call) {
                            Ok(validated) => {
                                tool_retry_state = None;
                                validated
                            }
                            Err(LoopError::InvalidToolCall(message)) => {
                                let retry = next_retry_count(
                                    &mut tool_retry_state,
                                    format!("{}:{message}", call.tool),
                                );
                                if retry > self.config.max_schema_retries {
                                    return Err(LoopError::InvalidToolCall(message));
                                }
                                observer(LoopEvent::ToolSchemaRetry {
                                    tool: call.tool.clone(),
                                    message: message.clone(),
                                    retry,
                                    max_retries: self.config.max_schema_retries,
                                });
                                let turn = ModelTurn::ToolError {
                                    tool: call.tool.clone(),
                                    error_kind: "schema_validation".to_string(),
                                    message: message.clone(),
                                    hint: tool_error_hint(&call.tool),
                                };
                                push_turn(&mut turns, turn, &mut observer);
                                continue;
                            }
                            Err(other) => return Err(other),
                        };
                        if let Some(expected_root) = expected_output_root.as_deref()
                            && let Some(mismatch_message) =
                                validate_expected_output_path(task, expected_root, &validated)
                        {
                            let turn = ModelTurn::ToolError {
                                tool: validated.tool_name().to_string(),
                                error_kind: "path_mismatch".to_string(),
                                message: mismatch_message,
                                hint: format!(
                                    "use the exact requested output path under {}",
                                    expected_root.display()
                                ),
                            };
                            push_turn(&mut turns, turn, &mut observer);
                            continue;
                        }
                        let normalized = serde_json::to_string(&validated)
                            .map_err(|err| LoopError::InvalidToolCall(err.to_string()))?;
                        observer(LoopEvent::ToolCallValidated {
                            tool: validated.tool_name().to_string(),
                            normalized: truncate(&normalized, 240),
                        });
                        let create_phase = requirement_state.current_phase();
                        if should_block_pre_write_inspection(task, create_phase, &validated) {
                            let turn = ModelTurn::ToolError {
                                tool: validated.tool_name().to_string(),
                                error_kind: "stalled_pre_write_inspection".to_string(),
                                message: "the output directory is already ready; more directory inspection is not useful"
                                    .to_string(),
                                hint: "use write_file now to create the deliverable file instead of inspecting the empty directory again".to_string(),
                            };
                            push_turn(&mut turns, turn, &mut observer);
                            continue;
                        }
                        let seen_bucket = if can_reuse_cached_result(&validated) {
                            &mut seen_read_only
                        } else {
                            &mut seen_mutating
                        };
                        if !seen_bucket.insert(normalized.clone()) {
                            if can_reuse_cached_result(&validated) {
                                let Some((cached_output, reuse_count)) =
                                    cached_results.get_mut(&normalized)
                                else {
                                    let turn = duplicate_empty_result_turn(
                                        task,
                                        expected_output_root.as_deref(),
                                        &validated,
                                    );
                                    push_turn(&mut turns, turn, &mut observer);
                                    continue;
                                };
                                *reuse_count += 1;
                                let mut trial_state = requirement_state.clone();
                                trial_state.record_evidence(task, &validated, cached_output);
                                let advanced_requirements =
                                    trial_state.remaining.len() < requirement_state.remaining.len();
                                if *reuse_count > self.config.max_cached_reuses_per_call
                                    && !advanced_requirements
                                {
                                    let turn = ModelTurn::ToolError {
                                        tool: validated.tool_name().to_string(),
                                        error_kind: "duplicate_reuse_limit".to_string(),
                                        message: "identical read-only call was repeated too many times"
                                            .to_string(),
                                        hint: "use the existing tool results to answer, or choose a different tool instead of repeating the same read-only call".to_string(),
                                    };
                                    push_turn(&mut turns, turn, &mut observer);
                                    continue;
                                }
                                observer(LoopEvent::ToolResultReused {
                                    tool: validated.tool_name().to_string(),
                                    reuse_count: *reuse_count,
                                });
                                observer(LoopEvent::ToolResultPreview {
                                    tool: validated.tool_name().to_string(),
                                    preview: truncate(&cached_output.replace('\n', "\\n"), 220),
                                });
                                requirement_state = trial_state;
                                turns.push(ModelTurn::ToolResult {
                                    tool: validated.tool_name().to_string(),
                                    output: cached_output.clone(),
                                });
                                continue;
                            }
                            return Err(LoopError::DuplicateToolCall(normalized));
                        }
                        observer(LoopEvent::ToolExecutionStarted {
                            tool: validated.tool_name().to_string(),
                            summary: tool_call_summary(&validated),
                        });
                        let tool_started_at = Instant::now();
                        let result = execute_validated_tool_call(
                            cwd,
                            &validated,
                            self.config.max_tool_output_chars,
                        );
                        let result = match result {
                            Ok(result) => {
                                tool_exec_retry_state = None;
                                result
                            }
                            Err(err) => {
                                let message = err.to_string();
                                let retry = next_retry_count(
                                    &mut tool_exec_retry_state,
                                    format!("{}:{message}", validated.tool_name()),
                                );
                                if retry > self.config.max_schema_retries {
                                    return Err(LoopError::Other(
                                        err.context(format!("tool failed: {:?}", call)),
                                    ));
                                }
                                observer(LoopEvent::ToolExecutionRetry {
                                    tool: validated.tool_name().to_string(),
                                    message: message.clone(),
                                    retry,
                                    max_retries: self.config.max_schema_retries,
                                });
                                let turn = ModelTurn::ToolError {
                                    tool: validated.tool_name().to_string(),
                                    error_kind: "execution_error".to_string(),
                                    message,
                                    hint: tool_execution_error_hint(
                                        task,
                                        expected_output_root.as_deref(),
                                        &validated,
                                    ),
                                };
                                push_turn(&mut turns, turn, &mut observer);
                                continue;
                            }
                        };
                        observer(LoopEvent::ToolExecutionFinished {
                            tool: validated.tool_name().to_string(),
                            elapsed_ms: tool_started_at.elapsed().as_millis(),
                        });
                        observer(LoopEvent::ToolResultPreview {
                            tool: validated.tool_name().to_string(),
                            preview: truncate(&result.replace('\n', "\\n"), 220),
                        });
                        requirement_state.record_evidence(task, &validated, &result);
                        if invalidates_read_only_cache(&validated) {
                            seen_read_only.clear();
                            cached_results.clear();
                        }
                        if can_reuse_cached_result(&validated) && !result.trim().is_empty() {
                            cached_results.insert(normalized, (result.clone(), 0));
                        }
                        push_turn(
                            &mut turns,
                            ModelTurn::ToolResult {
                                tool: validated.tool_name().to_string(),
                                output: result,
                            },
                            &mut observer,
                        );
                    }
                    observer(LoopEvent::StepFinished {
                        step: step_number,
                        elapsed_ms: step_started_at.elapsed().as_millis(),
                    });
                }
            }
        }

        Err(LoopError::MaxStepsReached)
    }
}

fn next_retry_count(state: &mut Option<(String, usize)>, key: String) -> usize {
    match state {
        Some((current_key, retries)) if *current_key == key => {
            *retries += 1;
            *retries
        }
        _ => {
            *state = Some((key, 1));
            1
        }
    }
}

fn push_turn<F>(turns: &mut Vec<ModelTurn>, turn: ModelTurn, observer: &mut F)
where
    F: FnMut(LoopEvent),
{
    if let ModelTurn::ToolError {
        tool,
        error_kind,
        message,
        ..
    } = &turn
    {
        observer(LoopEvent::ToolErrorRecorded {
            tool: tool.clone(),
            error_kind: error_kind.clone(),
            message: message.clone(),
        });
    }
    turns.push(turn);
}

fn task_requires_write_action(task: &str) -> bool {
    extract_task_contract(task).requires_write
}

fn extract_task_contract(task: &str) -> TaskContract {
    let lower = task.to_lowercase();
    let requires_write = [
        "create",
        "write",
        "save",
        "generate",
        "output",
        "edit",
        "modify",
        "implement",
        "fix",
        "作成",
        "出力",
        "保存",
        "生成",
        "修正",
        "更新",
        "実装",
    ];
    let browser_runnable = [
        "browser",
        "html",
        "directly runnable",
        "ブラウザ",
        "直接実行可能",
    ]
    .iter()
    .any(|marker| lower.contains(marker));
    let must_review = task_requires_review(task);
    let deliverable_kind = if browser_runnable || lower.contains("html") {
        DeliverableKind::HtmlApp
    } else if lower.contains("rust") || lower.contains(".rs") {
        DeliverableKind::RustCode
    } else {
        DeliverableKind::GenericFile
    };
    let polished_request = [
        "cool",
        "polished",
        "fancy",
        "stylish",
        "awesome",
        "いけてる",
        "カッコ",
        "かっこ",
        "凝った",
    ]
    .iter()
    .any(|marker| lower.contains(marker));
    let game_like = ["game", "invader", "arcade", "ゲーム", "インベーダー"]
        .iter()
        .any(|marker| lower.contains(marker));
    let creative_mode = if !requires_write.iter().any(|marker| lower.contains(marker)) {
        CreativeMode::Disabled
    } else if polished_request || (browser_runnable && game_like) {
        CreativeMode::Enhanced
    } else if browser_runnable {
        CreativeMode::Standard
    } else {
        CreativeMode::Disabled
    };
    let quality_targets = quality_targets_for(deliverable_kind, creative_mode, game_like);
    let stretch_goals = stretch_goals_for(deliverable_kind, creative_mode, game_like);
    let (profile, profile_confidence, fallback_profile) = select_task_profile(
        requires_write.iter().any(|marker| lower.contains(marker)),
        deliverable_kind,
        game_like,
        &lower,
    );
    TaskContract {
        output_root: extract_path_like_tokens(task)
            .into_iter()
            .find(|path| path.components().count() > 1),
        requires_write: requires_write.iter().any(|marker| lower.contains(marker)),
        must_review,
        browser_runnable,
        deliverable_kind,
        creative_mode,
        quality_targets,
        stretch_goals,
        profile,
        profile_confidence,
        fallback_profile,
    }
}

fn has_write_evidence(turns: &[ModelTurn]) -> bool {
    turns.iter().any(|turn| {
        matches!(
            turn,
            ModelTurn::ToolResult { tool, .. } if tool == "write_file" || tool == "edit_file"
        )
    })
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ModelResponse {
    Final { content: String },
    ToolCalls { calls: Vec<RawToolCall> },
}

#[derive(Debug, Clone, Deserialize)]
struct RawToolCall {
    tool: String,
    #[serde(default)]
    args: Option<Value>,
    #[serde(flatten)]
    extra: Map<String, Value>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "tool", content = "args", rename_all = "snake_case")]
enum ToolCall {
    ReadFile(ReadFileArgs),
    WriteFile(WriteFileArgs),
    EditFile(EditFileArgs),
    Exec(ExecArgs),
    Diff(DiffArgs),
    Search(SearchArgs),
    Glob(GlobArgs),
    ListDir(ListDirArgs),
    StatPath(PathArgs),
    PathExists(PathArgs),
    Mkdir(PathArgs),
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct ReadFileArgs {
    path: PathBuf,
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct WriteFileArgs {
    path: PathBuf,
    content: String,
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct EditFileArgs {
    path: PathBuf,
    from: String,
    to: String,
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct ExecArgs {
    #[serde(default)]
    argv: Vec<String>,
    #[serde(default)]
    command: Option<String>,
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct DiffArgs {
    before: String,
    after: String,
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct SearchArgs {
    needle: String,
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct GlobArgs {
    pattern: String,
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct ListDirArgs {
    path: PathBuf,
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct PathArgs {
    path: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CreatePhase {
    Prepare,
    Write,
    Verify,
    Review,
    Finalize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StepPlan {
    phase: String,
    items: Vec<String>,
    workflow: Vec<String>,
    phase_index: usize,
    phase_total: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeliverableKind {
    HtmlApp,
    RustCode,
    GenericFile,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CreativeMode {
    Disabled,
    Standard,
    Enhanced,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TaskTypeProfile {
    HtmlApp,
    Game,
    CliTool,
    Refactor,
    GenericCreate,
    Inspect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProfileConfidence {
    High,
    Medium,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TaskContract {
    output_root: Option<PathBuf>,
    requires_write: bool,
    must_review: bool,
    browser_runnable: bool,
    deliverable_kind: DeliverableKind,
    creative_mode: CreativeMode,
    quality_targets: Vec<String>,
    stretch_goals: Vec<String>,
    profile: TaskTypeProfile,
    profile_confidence: ProfileConfidence,
    fallback_profile: Option<TaskTypeProfile>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum CreateRequirement {
    OutputRootExists,
    DeliverableWritten,
    EntryPointVerified,
    RequestedOutputVerified,
    RuntimeVerified,
    CoreLoopVerified,
    ReviewCompleted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProgressClass {
    None,
    Reinforcing,
    Advanced,
    Completed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum EvidenceResult {
    None,
    Reinforcing,
    Advanced(Vec<CreateRequirement>),
}

#[derive(Debug, Clone)]
struct RequirementState {
    contract: TaskContract,
    remaining: BTreeSet<CreateRequirement>,
    last_progress: ProgressClass,
    stall_count: usize,
    effective_budget: usize,
    hard_cap: usize,
    finalize_phase_budget: usize,
    finalize_started_step: Option<usize>,
    last_written_deliverable: Option<PathBuf>,
}

impl RequirementState {
    fn new(contract: &TaskContract, config: LoopConfig) -> Self {
        let mut remaining = BTreeSet::new();
        let effective_budget = if contract.requires_write {
            remaining.insert(CreateRequirement::OutputRootExists);
            remaining.insert(CreateRequirement::DeliverableWritten);
            remaining.insert(CreateRequirement::EntryPointVerified);
            remaining.insert(CreateRequirement::RequestedOutputVerified);
            if contract.browser_runnable {
                remaining.insert(CreateRequirement::RuntimeVerified);
            }
            if matches!(contract.deliverable_kind, DeliverableKind::HtmlApp)
                && contract.creative_mode == CreativeMode::Enhanced
            {
                remaining.insert(CreateRequirement::CoreLoopVerified);
            }
            if contract.must_review {
                remaining.insert(CreateRequirement::ReviewCompleted);
            }
            config.create_task_base_budget
        } else {
            config.inspect_task_base_budget
        };
        Self {
            contract: contract.clone(),
            remaining,
            last_progress: ProgressClass::None,
            stall_count: 0,
            effective_budget,
            hard_cap: config.max_steps,
            finalize_phase_budget: config.finalize_phase_budget,
            finalize_started_step: None,
            last_written_deliverable: None,
        }
    }

    fn note_phase(&mut self, step_number: usize, phase: CreatePhase) {
        if !self.contract.requires_write {
            self.finalize_started_step = None;
            return;
        }
        if phase == CreatePhase::Finalize {
            self.finalize_started_step
                .get_or_insert(step_number.saturating_sub(1));
        } else {
            self.finalize_started_step = None;
        }
    }

    fn remaining_labels(&self) -> Vec<String> {
        self.remaining
            .iter()
            .map(|requirement| requirement_label(*requirement).to_string())
            .collect()
    }

    fn last_progress_label(&self) -> &'static str {
        match self.last_progress {
            ProgressClass::None => "no_progress",
            ProgressClass::Reinforcing => "reinforcing",
            ProgressClass::Advanced => "advanced",
            ProgressClass::Completed => "completed",
        }
    }

    fn remaining_budget(&self, step_number: usize) -> usize {
        let global_remaining = self
            .effective_budget
            .saturating_sub(step_number.saturating_sub(1));
        let finalize_remaining = self
            .finalize_started_step
            .map(|start| {
                self.finalize_phase_budget
                    .saturating_sub(step_number.saturating_sub(start))
            })
            .unwrap_or(global_remaining);
        global_remaining.min(finalize_remaining)
    }

    fn should_stop(&self, step_index: usize) -> bool {
        step_index >= self.effective_budget
            || (self.contract.requires_write
                && self
                    .finalize_started_step
                    .is_some_and(|start| step_index >= start + self.finalize_phase_budget))
    }

    fn record_no_progress(&mut self) {
        self.last_progress = ProgressClass::None;
        self.stall_count += 1;
    }

    fn record_evidence(&mut self, task: &str, call: &ToolCall, result: &str) {
        if let ToolCall::WriteFile(args) = call
            && self
                .contract
                .output_root
                .as_deref()
                .is_some_and(|root| path_under_root(root, &args.path))
        {
            self.last_written_deliverable = Some(args.path.clone());
        }
        if let ToolCall::EditFile(args) = call
            && self
                .contract
                .output_root
                .as_deref()
                .is_some_and(|root| path_under_root(root, &args.path))
        {
            self.last_written_deliverable = Some(args.path.clone());
        }
        let evidence = evaluate_evidence(task, &self.contract, call, result, self.current_phase());
        match evidence {
            EvidenceResult::None => self.record_no_progress(),
            EvidenceResult::Reinforcing => {
                self.last_progress = if self.remaining.is_empty() {
                    ProgressClass::Completed
                } else {
                    ProgressClass::Reinforcing
                };
                self.stall_count = 0;
                self.effective_budget = (self.effective_budget + 1).min(self.hard_cap);
            }
            EvidenceResult::Advanced(requirements) => {
                for requirement in requirements {
                    self.remaining.remove(&requirement);
                }
                self.last_progress = if self.remaining.is_empty() {
                    ProgressClass::Completed
                } else {
                    ProgressClass::Advanced
                };
                self.stall_count = 0;
                self.effective_budget = (self.effective_budget + 2).min(self.hard_cap);
            }
        }
    }

    fn current_phase(&self) -> CreatePhase {
        if !self.contract.requires_write {
            return CreatePhase::Finalize;
        }
        if self
            .remaining
            .contains(&CreateRequirement::OutputRootExists)
        {
            return CreatePhase::Prepare;
        }
        if self
            .remaining
            .contains(&CreateRequirement::DeliverableWritten)
        {
            return CreatePhase::Write;
        }
        if self
            .remaining
            .contains(&CreateRequirement::EntryPointVerified)
            || self
                .remaining
                .contains(&CreateRequirement::RequestedOutputVerified)
            || self.remaining.contains(&CreateRequirement::RuntimeVerified)
            || self
                .remaining
                .contains(&CreateRequirement::CoreLoopVerified)
        {
            return CreatePhase::Verify;
        }
        if self.remaining.contains(&CreateRequirement::ReviewCompleted) {
            return CreatePhase::Review;
        }
        CreatePhase::Finalize
    }

    fn preferred_deliverable_path(&self) -> Option<&Path> {
        self.last_written_deliverable.as_deref()
    }
}

impl ToolCall {
    fn tool_name(&self) -> &'static str {
        match self {
            Self::ReadFile(_) => "read_file",
            Self::WriteFile(_) => "write_file",
            Self::EditFile(_) => "edit_file",
            Self::Exec(_) => "exec",
            Self::Diff(_) => "diff",
            Self::Search(_) => "search",
            Self::Glob(_) => "glob",
            Self::ListDir(_) => "list_dir",
            Self::StatPath(_) => "stat_path",
            Self::PathExists(_) => "path_exists",
            Self::Mkdir(_) => "mkdir",
        }
    }
}

fn requirement_label(requirement: CreateRequirement) -> &'static str {
    match requirement {
        CreateRequirement::OutputRootExists => "output_root_exists",
        CreateRequirement::DeliverableWritten => "deliverable_written",
        CreateRequirement::EntryPointVerified => "entry_point_verified",
        CreateRequirement::RequestedOutputVerified => "requested_output_verified",
        CreateRequirement::RuntimeVerified => "runtime_verified",
        CreateRequirement::CoreLoopVerified => "core_loop_verified",
        CreateRequirement::ReviewCompleted => "review_completed",
    }
}

fn creative_mode_label(mode: CreativeMode) -> &'static str {
    match mode {
        CreativeMode::Disabled => "disabled",
        CreativeMode::Standard => "standard",
        CreativeMode::Enhanced => "enhanced",
    }
}

fn profile_label(profile: TaskTypeProfile) -> &'static str {
    match profile {
        TaskTypeProfile::HtmlApp => "html_app",
        TaskTypeProfile::Game => "game",
        TaskTypeProfile::CliTool => "cli_tool",
        TaskTypeProfile::Refactor => "refactor",
        TaskTypeProfile::GenericCreate => "generic_create",
        TaskTypeProfile::Inspect => "inspect",
    }
}

fn profile_confidence_label(confidence: ProfileConfidence) -> &'static str {
    match confidence {
        ProfileConfidence::High => "high",
        ProfileConfidence::Medium => "medium",
    }
}

fn select_task_profile(
    requires_write: bool,
    deliverable_kind: DeliverableKind,
    game_like: bool,
    lower: &str,
) -> (TaskTypeProfile, ProfileConfidence, Option<TaskTypeProfile>) {
    if !requires_write {
        return (TaskTypeProfile::Inspect, ProfileConfidence::Medium, None);
    }
    if lower.contains("refactor") || lower.contains("リファクタ") {
        return (TaskTypeProfile::Refactor, ProfileConfidence::High, None);
    }
    if lower.contains("cli") || lower.contains("command line") || lower.contains("コマンド") {
        return (
            TaskTypeProfile::CliTool,
            ProfileConfidence::Medium,
            Some(TaskTypeProfile::GenericCreate),
        );
    }
    match deliverable_kind {
        DeliverableKind::HtmlApp if game_like => (
            TaskTypeProfile::Game,
            ProfileConfidence::High,
            Some(TaskTypeProfile::HtmlApp),
        ),
        DeliverableKind::HtmlApp => (TaskTypeProfile::HtmlApp, ProfileConfidence::High, None),
        _ => (
            TaskTypeProfile::GenericCreate,
            ProfileConfidence::Medium,
            None,
        ),
    }
}

fn execution_stance(
    contract: &TaskContract,
    phase: CreatePhase,
    last_progress: ProgressClass,
    remaining_budget: usize,
) -> &'static str {
    if phase == CreatePhase::Review {
        return "finding-first";
    }
    if !contract.requires_write {
        return "evidence-first";
    }
    if matches!(contract.profile, TaskTypeProfile::Refactor) {
        return "minimal-change-first";
    }
    if phase == CreatePhase::Write {
        return "deliverable-first";
    }
    if matches!(last_progress, ProgressClass::None) && remaining_budget <= 1 {
        return "evidence-first";
    }
    match contract.profile {
        TaskTypeProfile::Game | TaskTypeProfile::HtmlApp | TaskTypeProfile::GenericCreate => {
            "deliverable-first"
        }
        TaskTypeProfile::CliTool | TaskTypeProfile::Refactor => "minimal-change-first",
        TaskTypeProfile::Inspect => "evidence-first",
    }
}

fn quality_targets_for(
    deliverable_kind: DeliverableKind,
    creative_mode: CreativeMode,
    game_like: bool,
) -> Vec<String> {
    if creative_mode == CreativeMode::Disabled {
        return Vec::new();
    }
    match deliverable_kind {
        DeliverableKind::HtmlApp if game_like => vec![
            "browser-runnable single-file entry".to_string(),
            "playable core loop".to_string(),
            "clear HUD for score or status".to_string(),
            "obvious restart or replay path".to_string(),
            "basic visual polish beyond plain placeholders".to_string(),
        ],
        DeliverableKind::HtmlApp => vec![
            "browser-runnable entry".to_string(),
            "clear primary interaction path".to_string(),
            "visible user feedback or status".to_string(),
            "basic visual polish".to_string(),
        ],
        DeliverableKind::RustCode => vec![
            "buildable structure".to_string(),
            "clear module boundaries".to_string(),
            "sensible defaults and error handling".to_string(),
        ],
        DeliverableKind::GenericFile => vec![
            "self-contained output".to_string(),
            "clear user-facing structure".to_string(),
        ],
    }
}

fn stretch_goals_for(
    deliverable_kind: DeliverableKind,
    creative_mode: CreativeMode,
    game_like: bool,
) -> Vec<String> {
    if creative_mode != CreativeMode::Enhanced {
        return Vec::new();
    }
    match deliverable_kind {
        DeliverableKind::HtmlApp if game_like => vec![
            "start screen or attract mode".to_string(),
            "level progression or difficulty ramp".to_string(),
            "enemy fire or richer challenge loop".to_string(),
            "stronger retro presentation".to_string(),
        ],
        DeliverableKind::HtmlApp => vec![
            "more intentional layout polish".to_string(),
            "micro-feedback for key actions".to_string(),
        ],
        DeliverableKind::RustCode => vec!["one extra ergonomics improvement".to_string()],
        DeliverableKind::GenericFile => vec!["one small quality-of-life refinement".to_string()],
    }
}

fn evaluate_evidence(
    task: &str,
    contract: &TaskContract,
    call: &ToolCall,
    result: &str,
    phase: CreatePhase,
) -> EvidenceResult {
    let output_root = contract.output_root.as_deref();
    match call {
        ToolCall::Mkdir(args) => {
            if output_root.is_some_and(|root| path_matches_expected(root, &args.path)) {
                return EvidenceResult::Advanced(vec![CreateRequirement::OutputRootExists]);
            }
            EvidenceResult::Reinforcing
        }
        ToolCall::StatPath(args) => {
            if output_root.is_some_and(|root| path_matches_expected(root, &args.path))
                && result.contains("kind=directory")
            {
                return EvidenceResult::Advanced(vec![CreateRequirement::OutputRootExists]);
            }
            if result.contains("kind=file")
                && output_root.is_some_and(|root| path_under_root(root, &args.path))
                && matches_main_deliverable(contract, &args.path)
            {
                return EvidenceResult::Advanced(vec![
                    CreateRequirement::EntryPointVerified,
                    CreateRequirement::RequestedOutputVerified,
                ]);
            }
            EvidenceResult::Reinforcing
        }
        ToolCall::PathExists(args) => {
            if output_root.is_some_and(|root| path_matches_expected(root, &args.path))
                && result.trim() == "true"
            {
                return EvidenceResult::Advanced(vec![CreateRequirement::OutputRootExists]);
            }
            if result.trim() == "true"
                && output_root.is_some_and(|root| path_under_root(root, &args.path))
                && matches_main_deliverable(contract, &args.path)
            {
                return EvidenceResult::Advanced(vec![
                    CreateRequirement::EntryPointVerified,
                    CreateRequirement::RequestedOutputVerified,
                ]);
            }
            EvidenceResult::None
        }
        ToolCall::ListDir(args) => {
            if output_root.is_some_and(|root| path_matches_expected(root, &args.path)) {
                return EvidenceResult::Advanced(vec![CreateRequirement::OutputRootExists]);
            }
            EvidenceResult::None
        }
        ToolCall::WriteFile(_) | ToolCall::EditFile(_) => {
            let path = match call {
                ToolCall::WriteFile(args) => &args.path,
                ToolCall::EditFile(args) => &args.path,
                _ => unreachable!(),
            };
            if output_root.is_some_and(|root| path_under_root(root, path)) {
                return EvidenceResult::Advanced(vec![CreateRequirement::DeliverableWritten]);
            }
            EvidenceResult::Reinforcing
        }
        ToolCall::ReadFile(args) => {
            if output_root.is_some_and(|root| path_under_root(root, &args.path))
                && !result.trim().is_empty()
                && matches_main_deliverable(contract, &args.path)
            {
                if contract.must_review
                    && matches!(phase, CreatePhase::Review | CreatePhase::Finalize)
                {
                    return EvidenceResult::Advanced(vec![CreateRequirement::ReviewCompleted]);
                }
                let mut requirements = vec![CreateRequirement::EntryPointVerified];
                if verifies_requested_output(task, &args.path) {
                    requirements.push(CreateRequirement::RequestedOutputVerified);
                }
                if verifies_runtime(contract, result) {
                    requirements.push(CreateRequirement::RuntimeVerified);
                }
                if verifies_core_loop(contract, result) {
                    requirements.push(CreateRequirement::CoreLoopVerified);
                }
                return EvidenceResult::Advanced(requirements);
            }
            EvidenceResult::None
        }
        ToolCall::Diff(_) => {
            if contract.must_review && phase == CreatePhase::Review {
                EvidenceResult::Advanced(vec![CreateRequirement::ReviewCompleted])
            } else {
                EvidenceResult::Reinforcing
            }
        }
        ToolCall::Exec(args) => {
            if is_read_only_exec(&args.argv) {
                EvidenceResult::Reinforcing
            } else {
                EvidenceResult::None
            }
        }
        ToolCall::Glob(args) => {
            if let Some(root) = output_root
                && args.pattern.contains(&root.to_string_lossy().to_string())
            {
                return EvidenceResult::Reinforcing;
            }
            EvidenceResult::None
        }
        ToolCall::Search(_) => EvidenceResult::Reinforcing,
    }
}

fn path_under_root(root: &Path, actual: &Path) -> bool {
    path_matches_expected(root, actual)
}

fn matches_main_deliverable(contract: &TaskContract, path: &Path) -> bool {
    let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    match contract.deliverable_kind {
        DeliverableKind::HtmlApp => matches!(file_name, "index.html" | "space_invaders.html"),
        DeliverableKind::RustCode => file_name.ends_with(".rs"),
        DeliverableKind::GenericFile => true,
    }
}

fn verifies_requested_output(task: &str, path: &Path) -> bool {
    let Some(expected_root) = expected_output_root(task) else {
        return false;
    };
    path_matches_expected(&expected_root, path)
}

fn verifies_runtime(contract: &TaskContract, content: &str) -> bool {
    if !contract.browser_runnable {
        return false;
    }
    let lower = content.to_lowercase();
    lower.contains("<html") && (lower.contains("<script") || lower.contains("<canvas"))
}

fn verifies_core_loop(contract: &TaskContract, content: &str) -> bool {
    if !(matches!(contract.deliverable_kind, DeliverableKind::HtmlApp)
        && contract.creative_mode == CreativeMode::Enhanced)
    {
        return false;
    }
    let lower = content.to_lowercase();
    let markers = [
        "score",
        "player",
        "enemy",
        "invader",
        "requestanimationframe",
        "keydown",
    ];
    markers
        .iter()
        .filter(|marker| lower.contains(**marker))
        .count()
        >= 3
}

fn parse_model_response(raw: &str) -> Result<ModelResponse, LoopError> {
    serde_json::from_str(raw)
        .or_else(|_| {
            let start = raw.find('{').ok_or_else(|| anyhow!("missing json start"))?;
            let end = raw.rfind('}').ok_or_else(|| anyhow!("missing json end"))?;
            serde_json::from_str(&raw[start..=end]).map_err(anyhow::Error::from)
        })
        .map_err(|err| LoopError::InvalidToolCall(err.to_string()))
}

fn validate_tool_call(raw: &RawToolCall) -> Result<ToolCall, LoopError> {
    let args = match &raw.args {
        Some(args) => args.clone(),
        None if !raw.extra.is_empty() => Value::Object(raw.extra.clone()),
        None => {
            return Err(LoopError::InvalidToolCall(
                "missing field `args`".to_string(),
            ));
        }
    };
    match raw.tool.as_str() {
        "read_file" => serde_json::from_value(args)
            .map(ToolCall::ReadFile)
            .map_err(|err| LoopError::InvalidToolCall(err.to_string())),
        "write_file" => serde_json::from_value(args)
            .map(ToolCall::WriteFile)
            .map_err(|err| LoopError::InvalidToolCall(err.to_string())),
        "edit_file" => serde_json::from_value(args)
            .map(ToolCall::EditFile)
            .map_err(|err| LoopError::InvalidToolCall(err.to_string())),
        "exec" => {
            let parsed: ExecArgs = serde_json::from_value(args)
                .map_err(|err| LoopError::InvalidToolCall(err.to_string()))?;
            let normalized = normalize_exec_args(parsed)
                .map_err(|err| LoopError::InvalidToolCall(err.to_string()))?;
            Ok(ToolCall::Exec(normalized))
        }
        "diff" => serde_json::from_value(args)
            .map(ToolCall::Diff)
            .map_err(|err| LoopError::InvalidToolCall(err.to_string())),
        "search" => serde_json::from_value(args)
            .map(ToolCall::Search)
            .map_err(|err| LoopError::InvalidToolCall(err.to_string())),
        "glob" => serde_json::from_value(args)
            .map(ToolCall::Glob)
            .map_err(|err| LoopError::InvalidToolCall(err.to_string())),
        "list_dir" => serde_json::from_value(args)
            .map(ToolCall::ListDir)
            .map_err(|err| LoopError::InvalidToolCall(err.to_string())),
        "stat_path" => serde_json::from_value(args)
            .map(ToolCall::StatPath)
            .map_err(|err| LoopError::InvalidToolCall(err.to_string())),
        "path_exists" => serde_json::from_value(args)
            .map(ToolCall::PathExists)
            .map_err(|err| LoopError::InvalidToolCall(err.to_string())),
        "mkdir" => serde_json::from_value(args)
            .map(ToolCall::Mkdir)
            .map_err(|err| LoopError::InvalidToolCall(err.to_string())),
        other => Err(LoopError::InvalidToolCall(format!("unknown tool: {other}"))),
    }
}

fn execute_validated_tool_call(
    cwd: &Path,
    call: &ToolCall,
    max_chars: usize,
) -> anyhow::Result<String> {
    let output = match call {
        ToolCall::ReadFile(args) => read_file(&cwd.join(&args.path))?,
        ToolCall::WriteFile(args) => {
            write_file(&cwd.join(&args.path), &args.content)?;
            "ok".to_string()
        }
        ToolCall::EditFile(args) => {
            edit_file(&cwd.join(&args.path), &args.from, &args.to)?;
            "ok".to_string()
        }
        ToolCall::Exec(args) => {
            let out = exec_in_dir(cwd, &args.argv)?;
            format!(
                "status={}\nstdout:\n{}\nstderr:\n{}",
                out.status, out.stdout, out.stderr
            )
        }
        ToolCall::Diff(args) => unified_diff(&args.before, &args.after),
        ToolCall::Search(args) => search_in_files(cwd, &args.needle)?
            .into_iter()
            .map(|m| format!("{}:{}:{}", m.path.display(), m.line_number, m.line))
            .collect::<Vec<_>>()
            .join("\n"),
        ToolCall::Glob(args) => glob_paths(cwd, &args.pattern)?
            .into_iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join("\n"),
        ToolCall::ListDir(args) => list_dir(&cwd.join(&args.path))?
            .into_iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join("\n"),
        ToolCall::StatPath(args) => stat_path(&cwd.join(&args.path))?,
        ToolCall::PathExists(args) => path_exists(&cwd.join(&args.path)).to_string(),
        ToolCall::Mkdir(args) => {
            mkdir_p(&cwd.join(&args.path))?;
            "ok".to_string()
        }
    };
    Ok(truncate(&output, max_chars))
}

fn normalize_exec_args(args: ExecArgs) -> Result<ExecArgs, anyhow::Error> {
    if !args.argv.is_empty() {
        return Ok(ExecArgs {
            argv: args.argv,
            command: None,
        });
    }

    let Some(command) = args.command else {
        anyhow::bail!("exec requires argv or command");
    };
    if contains_shell_metacharacters(&command) {
        anyhow::bail!("shell-style command syntax is not allowed");
    }
    let argv = shlex::split(&command).ok_or_else(|| anyhow!("failed to parse command"))?;
    if argv.is_empty() {
        anyhow::bail!("command parsed to empty argv");
    }
    Ok(ExecArgs {
        argv,
        command: None,
    })
}

fn contains_shell_metacharacters(command: &str) -> bool {
    ["|", "&&", "||", ";", "$(", "`", ">", "<"]
        .iter()
        .any(|needle| command.contains(needle))
}

fn build_loop_prompt(
    task: &str,
    contract: &TaskContract,
    turns: &[ModelTurn],
    requirement_state: &RequirementState,
) -> String {
    let expected_root = contract.output_root.clone();
    let create_phase = requirement_state.current_phase();
    let mut prompt = String::from(
        "[BASE_POLICY]\n\
You are Anvil. Solve the task by selecting tools when needed.\n\
Return only JSON.\n\
For tools: {\"type\":\"tool_calls\",\"calls\":[{\"tool\":\"read_file\",\"args\":{...}}]}\n\
For final answer: {\"type\":\"final\",\"content\":\"...\"}\n\
Available tools: read_file, write_file, edit_file, exec, diff, search, glob, list_dir, stat_path, path_exists, mkdir.\n\
Use tools when context is missing. Do not ask the user for git details if you can inspect them.\n\
Use list_dir or stat_path for directories. Do not use read_file on directories.\n\
For create/output tasks, prefer mkdir then write_file. If glob/search returns empty, change strategy instead of repeating it.\n\
For inspect/explain tasks, once you have enough evidence, stop using tools and return final.\n\
For branch/repository questions, prefer focused git commands over broad filesystem scans.\n\
Do not repeat the same read-only call once its result already answers the question.\n\
Invalid or partial tool args are forbidden.\n\n",
    );
    prompt.push_str("[TASK_CONTRACT]\n");
    prompt.push_str(&format!(
        "requires_write={}\nmust_review={}\nbrowser_runnable={}\ndeliverable_kind={}\ncreative_mode={}\nprofile={}\nprofile_confidence={}\n",
        contract.requires_write,
        contract.must_review,
        contract.browser_runnable,
        deliverable_kind_label(contract.deliverable_kind),
        creative_mode_label(contract.creative_mode),
        profile_label(contract.profile),
        profile_confidence_label(contract.profile_confidence),
    ));
    if let Some(root) = &contract.output_root {
        prompt.push_str(&format!("output_root={}\n", root.display()));
    }
    if let Some(fallback_profile) = contract.fallback_profile {
        prompt.push_str(&format!(
            "fallback_profile={}\n",
            profile_label(fallback_profile)
        ));
    }
    prompt.push('\n');
    for turn in compact_turns(turns) {
        match turn {
            ModelTurn::ToolResult { tool, output } => {
                prompt.push_str(&format!(
                    "[WORKING_TRANSCRIPT]\nTOOL_RESULT {tool}\n{output}\n\n"
                ));
            }
            ModelTurn::ToolError {
                tool,
                error_kind,
                message,
                hint,
            } => {
                prompt.push_str(&format!(
                    "[WORKING_TRANSCRIPT]\nTOOL_ERROR {tool}\nkind={error_kind}\nmessage={message}\nhint={hint}\n\n"
                ));
            }
            ModelTurn::AssistantFinal(content) => {
                prompt.push_str(&format!(
                    "[WORKING_TRANSCRIPT]\nASSISTANT_FINAL\n{content}\n\n"
                ));
            }
            ModelTurn::ProtocolError {
                error_kind,
                message,
                hint,
            } => {
                prompt.push_str(&format!(
                    "[WORKING_TRANSCRIPT]\nMODEL_ERROR\nkind={error_kind}\nmessage={message}\nhint={hint}\n\n"
                ));
            }
        }
    }
    if let Some(expected_root) = &expected_root {
        prompt.push_str("EXPECTED_OUTPUT_ROOT\n");
        prompt.push_str(&expected_root.display().to_string());
        prompt.push_str("\n\n");
    }
    if contract.requires_write {
        prompt.push_str("CREATE_PHASE\n");
        prompt.push_str(match create_phase {
            CreatePhase::Prepare => "prepare",
            CreatePhase::Write => "write",
            CreatePhase::Verify => "verify",
            CreatePhase::Review => "review",
            CreatePhase::Finalize => "finalize",
        });
        prompt.push_str("\n\n");
        prompt.push_str("REMAINING_REQUIREMENTS\n");
        if requirement_state.remaining.is_empty() {
            prompt.push_str("- none\n");
        } else {
            for requirement in requirement_state.remaining_labels() {
                prompt.push_str("- ");
                prompt.push_str(&requirement);
                prompt.push('\n');
            }
        }
        prompt.push('\n');
        prompt.push_str("PROGRESS_STATE\n");
        prompt.push_str(&format!(
            "last_progress={}\nstall_count={}\ncurrent_budget={}\n\n",
            requirement_state.last_progress_label(),
            requirement_state.stall_count,
            requirement_state.effective_budget,
        ));
        prompt.push('\n');
    }
    if !contract.quality_targets.is_empty() {
        prompt.push_str("[QUALITY_TARGETS]\n");
        for target in &contract.quality_targets {
            prompt.push_str("- ");
            prompt.push_str(target);
            prompt.push('\n');
        }
        prompt.push('\n');
    }
    if !contract.stretch_goals.is_empty() {
        prompt.push_str("[STRETCH_GOALS]\n");
        for goal in &contract.stretch_goals {
            prompt.push_str("- ");
            prompt.push_str(goal);
            prompt.push('\n');
        }
        prompt.push('\n');
    }
    prompt.push_str("[USER_OBJECTIVE]\n");
    prompt.push_str(&step_objective(task));
    prompt.push_str("\n\n");
    let plan = step_plan(contract, requirement_state, task, turns);
    prompt.push_str("[PLAN]\n");
    for item in plan.items {
        prompt.push_str("- ");
        prompt.push_str(&item);
        prompt.push('\n');
    }
    prompt.push('\n');
    if let Some(hint) = completion_hint(contract, requirement_state, task, turns) {
        prompt.push_str("[NEXT_ACTION_HINT]\n");
        prompt.push_str(&hint);
        prompt.push_str("\n\n");
    }
    prompt.push_str("[TASK]\n");
    prompt.push_str(task);
    prompt
}

fn truncate(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    format!(
        "{} ... [truncated]",
        text.chars().take(max_chars).collect::<String>()
    )
}

fn tool_error_hint(tool: &str) -> String {
    match tool {
        "glob" => "glob requires args.pattern as a glob string".to_string(),
        "exec" => "exec requires args.argv as a string array, or args.command without shell syntax"
            .to_string(),
        "read_file" | "write_file" | "edit_file" | "list_dir" | "stat_path" | "path_exists"
        | "mkdir" => "path-based tools require args.path".to_string(),
        "search" => "search requires args.needle".to_string(),
        "diff" => "diff requires args.before and args.after".to_string(),
        _ => "review the tool schema and retry with valid arguments".to_string(),
    }
}

fn tool_execution_error_hint(task: &str, expected_root: Option<&Path>, call: &ToolCall) -> String {
    match call {
        ToolCall::ReadFile(args) => format!(
            "read_file requires a file path. If {} is a directory, use glob or exec ls instead",
            args.path.display()
        ),
        ToolCall::WriteFile(args) => format!(
            "write_file failed for {}. Verify the path and retry with valid file content",
            args.path.display()
        ),
        ToolCall::EditFile(args) => format!(
            "edit_file failed for {}. Read the file first and ensure the target text exists",
            args.path.display()
        ),
        ToolCall::Exec(args) => format!(
            "exec failed for `{}`. Review stderr and retry with a valid read-only or safe command",
            args.argv.join(" ")
        ),
        ToolCall::Glob(args) => format!(
            "glob failed for pattern {}. Retry with a valid glob pattern",
            args.pattern
        ),
        ToolCall::ListDir(args) => format!(
            "list_dir failed for {}. Retry with an existing directory path",
            args.path.display()
        ),
        ToolCall::StatPath(args) | ToolCall::PathExists(args) | ToolCall::Mkdir(args) => {
            if task_requires_write_action(task)
                && let Some(expected_root) = expected_root
                && path_matches_expected(expected_root, &args.path)
            {
                return format!(
                    "{} failed for {}. If the requested output root does not exist yet, use mkdir {} next.",
                    call.tool_name(),
                    args.path.display(),
                    expected_root.display()
                );
            }
            format!(
                "{} failed for {}. Verify the path and retry",
                call.tool_name(),
                args.path.display()
            )
        }
        ToolCall::Search(args) => format!(
            "search failed for needle {}. Retry after narrowing the query or checking paths",
            truncate(&args.needle, 80)
        ),
        ToolCall::Diff(_) => "diff failed. Retry with valid before/after content".to_string(),
    }
}

fn tool_call_summary(call: &ToolCall) -> String {
    match call {
        ToolCall::ReadFile(args) => format!("read {}", args.path.display()),
        ToolCall::WriteFile(args) => format!("write {}", args.path.display()),
        ToolCall::EditFile(args) => format!("edit {}", args.path.display()),
        ToolCall::Exec(args) => format!("exec {}", args.argv.join(" ")),
        ToolCall::Diff(_) => "diff provided content".to_string(),
        ToolCall::Search(args) => format!("search {}", truncate(&args.needle, 80)),
        ToolCall::Glob(args) => format!("glob {}", truncate(&args.pattern, 80)),
        ToolCall::ListDir(args) => format!("list_dir {}", args.path.display()),
        ToolCall::StatPath(args) => format!("stat_path {}", args.path.display()),
        ToolCall::PathExists(args) => format!("path_exists {}", args.path.display()),
        ToolCall::Mkdir(args) => format!("mkdir {}", args.path.display()),
    }
}

fn can_reuse_cached_result(call: &ToolCall) -> bool {
    match call {
        ToolCall::ReadFile(_)
        | ToolCall::Search(_)
        | ToolCall::Glob(_)
        | ToolCall::ListDir(_)
        | ToolCall::StatPath(_)
        | ToolCall::PathExists(_) => true,
        ToolCall::Exec(args) => is_read_only_exec(&args.argv),
        ToolCall::WriteFile(_) | ToolCall::EditFile(_) | ToolCall::Diff(_) | ToolCall::Mkdir(_) => {
            false
        }
    }
}

fn invalidates_read_only_cache(call: &ToolCall) -> bool {
    match call {
        ToolCall::WriteFile(_) | ToolCall::EditFile(_) | ToolCall::Mkdir(_) => true,
        ToolCall::Exec(args) => !is_read_only_exec(&args.argv),
        ToolCall::ReadFile(_)
        | ToolCall::Diff(_)
        | ToolCall::Search(_)
        | ToolCall::Glob(_)
        | ToolCall::ListDir(_)
        | ToolCall::StatPath(_)
        | ToolCall::PathExists(_) => false,
    }
}

fn is_read_only_exec(argv: &[String]) -> bool {
    matches!(
        argv.first().map(String::as_str),
        Some("ls" | "cat" | "pwd" | "git" | "find" | "rg")
    )
}

fn compact_turns(turns: &[ModelTurn]) -> Vec<ModelTurn> {
    if turns.len() <= 10 {
        return turns.to_vec();
    }
    let older = turns.len() - 8;
    let mut compacted = vec![ModelTurn::ProtocolError {
        error_kind: "compacted_history".to_string(),
        message: format!("{} earlier turns summarized", older),
        hint: "focus on the latest tool results and change strategy if prior attempts failed"
            .to_string(),
    }];
    compacted.extend_from_slice(&turns[older..]);
    compacted
}

fn completion_hint(
    contract: &TaskContract,
    requirement_state: &RequirementState,
    task: &str,
    turns: &[ModelTurn],
) -> Option<String> {
    let expected_root = contract.output_root.clone();
    if contract.requires_write {
        if let Some(unmet) = unmet_requirements(contract, requirement_state) {
            return Some(unmet);
        }
        match requirement_state.current_phase() {
            CreatePhase::Review => {
                return Some(
                    "Verification is complete. Read the generated file and perform the requested code review before returning final."
                        .to_string(),
                );
            }
            CreatePhase::Finalize => {
                return Some(
                    "The implementation work is complete. Return final with a concise summary of created or changed files and include the requested review findings if any."
                        .to_string(),
                );
            }
            CreatePhase::Verify => {
                return Some(
                    "A write/edit has already succeeded. Verify the generated output by reading the main deliverable file before returning final."
                        .to_string(),
                );
            }
            CreatePhase::Write => {
                return Some(
                    "Preparation is complete. Use write_file now to create the deliverable file at the requested output path instead of inspecting the directory again."
                        .to_string(),
                );
            }
            CreatePhase::Prepare => {
                if let Some(expected_root) = expected_root {
                    return Some(format!(
                        "The requested output root {} is not ready yet. Use mkdir on that exact path next, then use write_file. Do not inspect parent directories or repeat stat_path on the missing target.",
                        expected_root.display()
                    ));
                }
            }
        }
        return None;
    }

    if is_branch_inspection_task(task)
        && has_branch_evidence(turns)
        && has_commit_history_evidence(turns)
        && has_status_evidence(turns)
    {
        return Some(
            "You already have enough git evidence to answer the branch/repository question. Stop using tools and return final now."
                .to_string(),
        );
    }

    None
}

fn step_purpose(
    task: &str,
    turns: &[ModelTurn],
    phase: CreatePhase,
    requirement_state: &RequirementState,
) -> String {
    if requirement_state.contract.requires_write {
        return match phase {
            CreatePhase::Prepare => "prepare output path".to_string(),
            CreatePhase::Write => "write deliverable".to_string(),
            CreatePhase::Verify => "verify generated output".to_string(),
            CreatePhase::Review => "review generated output".to_string(),
            CreatePhase::Finalize => "finalize response".to_string(),
        };
    }

    if is_branch_inspection_task(task) {
        if has_branch_evidence(turns)
            && has_commit_history_evidence(turns)
            && has_status_evidence(turns)
        {
            return "summarize gathered git evidence".to_string();
        }
        return "inspect repository state".to_string();
    }

    "gather context".to_string()
}

fn deliverable_kind_label(kind: DeliverableKind) -> &'static str {
    match kind {
        DeliverableKind::HtmlApp => "html_app",
        DeliverableKind::RustCode => "rust_code",
        DeliverableKind::GenericFile => "generic_file",
    }
}

fn is_branch_inspection_task(task: &str) -> bool {
    let lower = task.to_lowercase();
    [
        "branch",
        "repository",
        "repo",
        "ブランチ",
        "リポジトリ",
        "差分",
        "変更",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

fn has_branch_evidence(turns: &[ModelTurn]) -> bool {
    turns.iter().any(|turn| match turn {
        ModelTurn::ToolResult { output, .. } => {
            output.contains("On branch ") || output.contains("remotes/origin/")
        }
        _ => false,
    })
}

fn has_commit_history_evidence(turns: &[ModelTurn]) -> bool {
    turns.iter().any(|turn| match turn {
        ModelTurn::ToolResult { output, .. } => output.lines().any(looks_like_commit_line),
        _ => false,
    })
}

fn has_status_evidence(turns: &[ModelTurn]) -> bool {
    turns.iter().any(|turn| match turn {
        ModelTurn::ToolResult { output, .. } => {
            output.contains("Changes not staged for commit")
                || output.contains("nothing to commit")
                || output.contains("Your branch is ahead of")
        }
        _ => false,
    })
}

fn task_requires_review(task: &str) -> bool {
    let lower = task.to_lowercase();
    ["review", "code review", "レビュー", "コードレビュー"]
        .iter()
        .any(|needle| lower.contains(needle))
}

fn step_objective(task: &str) -> String {
    truncate(task.trim(), 140)
}

fn step_instruction(
    _task: &str,
    contract: &TaskContract,
    phase: CreatePhase,
    requirement_state: &RequirementState,
) -> String {
    let output_root = contract
        .output_root
        .as_ref()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "(not specified)".to_string());
    let review_note = if contract.must_review {
        " Include a code review before finalizing."
    } else {
        ""
    };
    let quality_note = if contract.quality_targets.is_empty() {
        String::new()
    } else {
        format!(
            " Aim for {}.",
            contract
                .quality_targets
                .iter()
                .take(3)
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    let stretch_note = if contract.stretch_goals.is_empty() {
        String::new()
    } else {
        format!(
            " If budget allows, also try {}.",
            contract
                .stretch_goals
                .iter()
                .take(2)
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    let remaining = requirement_state.remaining_labels().join(", ");
    match phase {
        CreatePhase::Prepare => format!(
            "Prepare the requested output location at {output_root}. Do not write outside that root. After preparation, move to implementation. Remaining requirements: {remaining}.{review_note}{quality_note}"
        ),
        CreatePhase::Write => match contract.deliverable_kind {
            DeliverableKind::HtmlApp => format!(
                "Create the main browser-runnable HTML deliverable under {output_root}. Implement the requested game behavior from the task, keep it directly runnable in a browser, and prefer a complete playable result over placeholders. Remaining requirements: {remaining}.{review_note}{quality_note}{stretch_note}"
            ),
            DeliverableKind::RustCode => format!(
                "Write the requested Rust deliverable under {output_root}. Keep the code buildable and aligned with the task requirements. Remaining requirements: {remaining}.{review_note}{quality_note}{stretch_note}"
            ),
            DeliverableKind::GenericFile => format!(
                "Write the requested deliverable to {output_root}. Satisfy the task requirements and keep the output self-contained. Remaining requirements: {remaining}.{review_note}{quality_note}{stretch_note}"
            ),
        },
        CreatePhase::Verify => format!(
            "Verify the generated output under {output_root}. Read the main deliverable, confirm the key task requirements are present, and identify anything still missing. Remaining requirements: {remaining}.{review_note}{quality_note}"
        ),
        CreatePhase::Review => format!(
            "Perform the requested code review for the generated output under {output_root}. Note concrete findings, risks, obvious regressions, and missing polish before finalizing. Remaining requirements: {remaining}.{quality_note}"
        ),
        CreatePhase::Finalize => format!(
            "Prepare the final response for the work under {output_root}. Summarize created files, implementation status, and include review findings if requested. Only use more tools if they satisfy: {remaining}."
        ),
    }
}

fn step_plan(
    contract: &TaskContract,
    requirement_state: &RequirementState,
    task: &str,
    turns: &[ModelTurn],
) -> StepPlan {
    let workflow = build_workflow(contract);
    let phase = if contract.requires_write {
        workflow
            .get(phase_position(contract, requirement_state.current_phase()))
            .cloned()
            .unwrap_or_else(|| "finalize".to_string())
    } else if is_branch_inspection_task(task) {
        "inspect".to_string()
    } else {
        "gather".to_string()
    };
    let phase_index = if contract.requires_write {
        phase_position(contract, requirement_state.current_phase()) + 1
    } else {
        1
    };
    let phase_total = if contract.requires_write {
        workflow.len()
    } else {
        1
    };

    let mut items = Vec::new();
    if let Some(expected_root) = &contract.output_root {
        items.push(format!("output root: {}", expected_root.display()));
    }
    if contract.requires_write {
        items.push(format!(
            "deliverable: {}",
            deliverable_kind_label(contract.deliverable_kind)
        ));
        items.push(format!("profile: {}", profile_label(contract.profile)));
        items.push(format!(
            "profile confidence: {}",
            profile_confidence_label(contract.profile_confidence)
        ));
        if let Some(fallback_profile) = contract.fallback_profile {
            items.push(format!(
                "fallback profile: {}",
                profile_label(fallback_profile)
            ));
        }
        items.push(format!(
            "execution stance: {}",
            execution_stance(
                contract,
                requirement_state.current_phase(),
                requirement_state.last_progress,
                requirement_state.remaining_budget(phase_index)
            )
        ));
        if contract.creative_mode != CreativeMode::Disabled {
            items.push(format!(
                "creative mode: {}",
                creative_mode_label(contract.creative_mode)
            ));
        }
        if contract.browser_runnable {
            items.push("runtime: browser-runnable output required".to_string());
        }
        if contract.must_review {
            items.push("review requested: include code review before final".to_string());
        }
        if !contract.quality_targets.is_empty() {
            items.push(format!(
                "quality targets: {}",
                contract
                    .quality_targets
                    .iter()
                    .take(3)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if !contract.stretch_goals.is_empty() {
            items.push(format!(
                "stretch goals: {}",
                contract
                    .stretch_goals
                    .iter()
                    .take(2)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
    }
    if let Some(hint) = completion_hint(contract, requirement_state, task, turns) {
        items.push(format!("current guidance: {}", truncate(&hint, 160)));
    }
    StepPlan {
        phase,
        items,
        workflow,
        phase_index,
        phase_total,
    }
}

fn build_workflow(contract: &TaskContract) -> Vec<String> {
    if !contract.requires_write {
        return vec!["gather".to_string()];
    }
    let mut workflow = vec![
        "prepare".to_string(),
        "write".to_string(),
        "verify".to_string(),
    ];
    if contract.must_review {
        workflow.push("review".to_string());
    }
    workflow.push("finalize".to_string());
    workflow
}

fn phase_position(contract: &TaskContract, phase: CreatePhase) -> usize {
    match phase {
        CreatePhase::Prepare => 0,
        CreatePhase::Write => 1,
        CreatePhase::Verify => 2,
        CreatePhase::Review => {
            if contract.must_review {
                3
            } else {
                2
            }
        }
        CreatePhase::Finalize => {
            if contract.must_review {
                4
            } else {
                3
            }
        }
    }
}

fn unmet_requirements(
    contract: &TaskContract,
    requirement_state: &RequirementState,
) -> Option<String> {
    let first = requirement_state.remaining.iter().next().copied()?;
    Some(match first {
        CreateRequirement::OutputRootExists => {
            let root = contract
                .output_root
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "the requested output root".to_string());
            format!("The output root is not ready yet. Create or verify {root} before doing anything else.")
        }
        CreateRequirement::DeliverableWritten => {
            "A deliverable file is still missing. Use write_file or edit_file to create the requested output under the target root."
                .to_string()
        }
        CreateRequirement::EntryPointVerified => {
            requirement_state
                .preferred_deliverable_path()
                .map(|path| {
                    format!(
                        "The main deliverable has not been read back yet. Read {} before returning final.",
                        path.display()
                    )
                })
                .unwrap_or_else(|| {
                    "The main deliverable has not been read back yet. Read the entry file before returning final."
                        .to_string()
                })
        }
        CreateRequirement::RequestedOutputVerified => {
            requirement_state
                .preferred_deliverable_path()
                .map(|path| {
                    format!(
                        "The requested output path has not been verified yet. Confirm that {} is the generated file under the exact requested root.",
                        path.display()
                    )
                })
                .unwrap_or_else(|| {
                    "The requested output path has not been verified yet. Confirm the generated file lives under the exact requested root."
                        .to_string()
                })
        }
        CreateRequirement::RuntimeVerified => {
            requirement_state
                .preferred_deliverable_path()
                .map(|path| {
                    format!(
                        "Browser-runnable output was requested but runtime readiness is not yet verified. Read {} and confirm that it contains executable browser logic.",
                        path.display()
                    )
                })
                .unwrap_or_else(|| {
                    "Browser-runnable output was requested but runtime readiness is not yet verified. Confirm that the HTML entry contains executable browser logic."
                        .to_string()
                })
        }
        CreateRequirement::CoreLoopVerified => {
            requirement_state
                .preferred_deliverable_path()
                .map(|path| {
                    format!(
                        "The requested core behavior is not yet verified. Read {} and confirm the playable loop or key interaction exists.",
                        path.display()
                    )
                })
                .unwrap_or_else(|| {
                    "The requested core behavior is not yet verified. Read the generated file and confirm the playable loop or key interaction exists."
                        .to_string()
                })
        }
        CreateRequirement::ReviewCompleted => {
            requirement_state
                .preferred_deliverable_path()
                .map(|path| {
                    format!(
                        "A code review was requested and is still missing. Inspect {} and include review findings before final.",
                        path.display()
                    )
                })
                .unwrap_or_else(|| {
                    "A code review was requested and is still missing. Inspect the generated file and include review findings before final."
                        .to_string()
                })
        }
    })
}

fn expected_output_root(task: &str) -> Option<PathBuf> {
    extract_task_contract(task).output_root
}

fn extract_path_like_tokens(task: &str) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    let mut current = String::new();
    let mut capturing = false;

    for ch in task.chars() {
        let is_start = ch == '.' || ch == '/';
        let is_path_char = ch.is_ascii_alphanumeric() || matches!(ch, '.' | '/' | '_' | '-');
        if !capturing {
            if is_start {
                capturing = true;
                current.push(ch);
            }
            continue;
        }
        if is_path_char {
            current.push(ch);
            continue;
        }
        if current.len() > 1 {
            paths.push(PathBuf::from(trim_path_token(&current)));
        }
        current.clear();
        capturing = false;
    }

    if capturing && current.len() > 1 {
        paths.push(PathBuf::from(trim_path_token(&current)));
    }
    paths
}

fn trim_path_token(path: &str) -> &str {
    path.trim_end_matches(['.', ',', ')', ']', '"', '\''])
}

fn validate_expected_output_path(
    task: &str,
    expected_root: &Path,
    call: &ToolCall,
) -> Option<String> {
    if !task_requires_write_action(task) {
        return None;
    }
    let actual = tool_call_primary_path(call)?;
    if path_matches_expected(expected_root, actual) {
        return None;
    }
    Some(format!(
        "tool path {} does not match requested output path {}",
        actual.display(),
        expected_root.display()
    ))
}

fn duplicate_empty_result_turn(
    task: &str,
    expected_root: Option<&Path>,
    call: &ToolCall,
) -> ModelTurn {
    if task_requires_write_action(task)
        && let Some(expected_root) = expected_root
        && let Some(actual) = tool_call_primary_path(call)
    {
        if actual == expected_root {
            return ModelTurn::ToolError {
                tool: call.tool_name().to_string(),
                error_kind: "stalled_missing_output_root".to_string(),
                message:
                    "the requested output root is still missing; repeating the same probe will not help"
                        .to_string(),
                hint: format!(
                    "use mkdir on {} next, then write_file under that directory",
                    expected_root.display()
                ),
            };
        }
        if path_under_root(expected_root, actual) {
            return ModelTurn::ToolError {
                tool: call.tool_name().to_string(),
                error_kind: "stalled_missing_entry_point".to_string(),
                message: "the requested root exists, but this specific file probe is still empty"
                    .to_string(),
                hint: format!(
                    "read the actual written deliverable under {} or list the directory once to discover the correct file; do not keep probing the same missing file",
                    expected_root.display()
                ),
            };
        }
    }

    ModelTurn::ToolError {
        tool: call.tool_name().to_string(),
        error_kind: "duplicate_empty_result".to_string(),
        message: "previous identical read-only call returned no reusable result".to_string(),
        hint: "change strategy: use mkdir, list_dir, stat_path, or write_file instead of repeating the same empty lookup".to_string(),
    }
}

fn tool_call_primary_path(call: &ToolCall) -> Option<&Path> {
    match call {
        ToolCall::ReadFile(args) => Some(args.path.as_path()),
        ToolCall::WriteFile(args) => Some(args.path.as_path()),
        ToolCall::EditFile(args) => Some(args.path.as_path()),
        ToolCall::ListDir(args) => Some(args.path.as_path()),
        ToolCall::StatPath(args) => Some(args.path.as_path()),
        ToolCall::PathExists(args) => Some(args.path.as_path()),
        ToolCall::Mkdir(args) => Some(args.path.as_path()),
        ToolCall::Glob(_) | ToolCall::Exec(_) | ToolCall::Diff(_) | ToolCall::Search(_) => None,
    }
}

fn path_matches_expected(expected_root: &Path, actual: &Path) -> bool {
    let expected_text = expected_root.to_string_lossy();
    let actual_text = actual.to_string_lossy();
    actual_text == expected_text
        || actual_text.starts_with(&format!("{expected_text}/"))
        || actual_text.starts_with(&format!("{expected_text}."))
        || actual_text.ends_with(expected_text.as_ref())
        || actual_text.contains(&format!("/{expected_text}"))
}

fn looks_like_commit_line(line: &str) -> bool {
    let prefix = line.split_whitespace().next().unwrap_or_default();
    prefix.len() >= 7 && prefix.chars().take(7).all(|ch| ch.is_ascii_hexdigit())
}

fn should_block_pre_write_inspection(
    task: &str,
    create_phase: CreatePhase,
    call: &ToolCall,
) -> bool {
    if !task_requires_write_action(task) || create_phase != CreatePhase::Write {
        return false;
    }
    matches!(
        call,
        ToolCall::ListDir(_) | ToolCall::StatPath(_) | ToolCall::PathExists(_)
    )
}

fn tool_specs() -> Vec<NativeToolSpec> {
    vec![
        path_tool_spec("read_file", "Read a text file"),
        write_tool_spec(),
        edit_tool_spec(),
        NativeToolSpec {
            name: "exec",
            description: "Run a safe command using argv or a simple command string",
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "argv": {"type": "array", "items": {"type": "string"}},
                    "command": {"type": "string"}
                }
            }),
        },
        NativeToolSpec {
            name: "diff",
            description: "Produce a unified diff between two strings",
            input_schema: serde_json::json!({
                "type": "object",
                "required": ["before", "after"],
                "properties": {
                    "before": {"type": "string"},
                    "after": {"type": "string"}
                }
            }),
        },
        NativeToolSpec {
            name: "search",
            description: "Search files for a string",
            input_schema: serde_json::json!({
                "type": "object",
                "required": ["needle"],
                "properties": {"needle": {"type": "string"}}
            }),
        },
        NativeToolSpec {
            name: "glob",
            description: "Find paths matching a glob pattern",
            input_schema: serde_json::json!({
                "type": "object",
                "required": ["pattern"],
                "properties": {"pattern": {"type": "string"}}
            }),
        },
        path_tool_spec("list_dir", "List entries in a directory"),
        path_tool_spec("stat_path", "Describe a path"),
        path_tool_spec("path_exists", "Check whether a path exists"),
        path_tool_spec("mkdir", "Create a directory recursively"),
    ]
}

fn path_tool_spec(name: &'static str, description: &'static str) -> NativeToolSpec {
    NativeToolSpec {
        name,
        description,
        input_schema: serde_json::json!({
            "type": "object",
            "required": ["path"],
            "properties": {"path": {"type": "string"}}
        }),
    }
}

fn write_tool_spec() -> NativeToolSpec {
    NativeToolSpec {
        name: "write_file",
        description: "Write a text file, creating parent directories if needed",
        input_schema: serde_json::json!({
            "type": "object",
            "required": ["path", "content"],
            "properties": {
                "path": {"type": "string"},
                "content": {"type": "string"}
            }
        }),
    }
}

fn edit_tool_spec() -> NativeToolSpec {
    NativeToolSpec {
        name: "edit_file",
        description: "Replace text in an existing file",
        input_schema: serde_json::json!({
            "type": "object",
            "required": ["path", "from", "to"],
            "properties": {
                "path": {"type": "string"},
                "from": {"type": "string"},
                "to": {"type": "string"}
            }
        }),
    }
}

fn estimate_native_response_size(response: &NativeModelResponse) -> usize {
    match response {
        NativeModelResponse::Message(text) => text.len(),
        NativeModelResponse::ToolCalls(calls) => serde_json::to_string(calls)
            .map(|text| text.len())
            .unwrap_or(0),
    }
}

fn native_to_model_response(native: NativeModelResponse) -> Result<ModelResponse, LoopError> {
    match native {
        NativeModelResponse::Message(content) => Ok(ModelResponse::Final { content }),
        NativeModelResponse::ToolCalls(calls) => {
            if let Some(content) = extract_native_final(&calls)? {
                Ok(ModelResponse::Final { content })
            } else {
                Ok(ModelResponse::ToolCalls {
                    calls: calls.into_iter().map(raw_from_native_call).collect(),
                })
            }
        }
    }
}

fn raw_from_native_call(call: NativeToolCall) -> RawToolCall {
    RawToolCall {
        tool: call.name,
        args: Some(call.arguments),
        extra: Map::new(),
    }
}

fn extract_native_final(calls: &[NativeToolCall]) -> Result<Option<String>, LoopError> {
    if calls.len() != 1 || calls[0].name != "final" {
        return Ok(None);
    }

    let args = &calls[0].arguments;
    if let Some(content) = args.get("content").and_then(Value::as_str) {
        return Ok(Some(content.to_string()));
    }
    if let Some(message) = args.get("message").and_then(Value::as_str) {
        return Ok(Some(message.to_string()));
    }
    if let Some(text) = args.get("text").and_then(Value::as_str) {
        return Ok(Some(text.to_string()));
    }

    Err(LoopError::InvalidToolCall(
        "native final tool call requires content, message, or text".to_string(),
    ))
}
