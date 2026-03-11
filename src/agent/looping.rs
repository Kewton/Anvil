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
    pub max_tool_output_chars: usize,
    pub max_schema_retries: usize,
    pub max_cached_reuses_per_call: usize,
}

impl Default for LoopConfig {
    fn default() -> Self {
        Self {
            max_steps: 12,
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
        let mut seen = BTreeSet::new();
        let mut cached_results = BTreeMap::<String, (String, usize)>::new();
        let mut protocol_retry_state: Option<(String, usize)> = None;
        let mut tool_retry_state: Option<(String, usize)> = None;
        let mut tool_exec_retry_state: Option<(String, usize)> = None;
        let task_requires_write = task_requires_write_action(task);
        let expected_output_root = expected_output_root(task);

        for step in 0..self.config.max_steps {
            let step_number = step + 1;
            let step_started_at = Instant::now();
            observer(LoopEvent::StepStarted { step: step_number });
            let prompt = build_loop_prompt(task, &turns);
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
                        let create_phase = create_phase_for_task(task, &turns);
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
                        if !seen.insert(normalized.clone()) {
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
                                if *reuse_count > self.config.max_cached_reuses_per_call {
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
    let lower = task.to_lowercase();
    let markers = [
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
    markers.iter().any(|marker| lower.contains(marker))
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
    Finalize,
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

fn build_loop_prompt(task: &str, turns: &[ModelTurn]) -> String {
    let expected_root = expected_output_root(task);
    let create_phase = create_phase_for_task(task, turns);
    let mut prompt = String::from(
        "You are Anvil. Solve the task by selecting tools when needed.\n\
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
    for turn in compact_turns(turns) {
        match turn {
            ModelTurn::ToolResult { tool, output } => {
                prompt.push_str(&format!("TOOL_RESULT {tool}\n{output}\n\n"));
            }
            ModelTurn::ToolError {
                tool,
                error_kind,
                message,
                hint,
            } => {
                prompt.push_str(&format!(
                    "TOOL_ERROR {tool}\nkind={error_kind}\nmessage={message}\nhint={hint}\n\n"
                ));
            }
            ModelTurn::AssistantFinal(content) => {
                prompt.push_str(&format!("ASSISTANT_FINAL\n{content}\n\n"));
            }
            ModelTurn::ProtocolError {
                error_kind,
                message,
                hint,
            } => {
                prompt.push_str(&format!(
                    "MODEL_ERROR\nkind={error_kind}\nmessage={message}\nhint={hint}\n\n"
                ));
            }
        }
    }
    if let Some(expected_root) = &expected_root {
        prompt.push_str("EXPECTED_OUTPUT_ROOT\n");
        prompt.push_str(&expected_root.display().to_string());
        prompt.push_str("\n\n");
    }
    if task_requires_write_action(task) {
        prompt.push_str("CREATE_PHASE\n");
        prompt.push_str(match create_phase {
            CreatePhase::Prepare => "prepare",
            CreatePhase::Write => "write",
            CreatePhase::Verify => "verify",
            CreatePhase::Finalize => "finalize",
        });
        prompt.push_str("\n\n");
    }
    if let Some(hint) = completion_hint(task, turns) {
        prompt.push_str("COMPLETION_HINT\n");
        prompt.push_str(&hint);
        prompt.push_str("\n\n");
    }
    prompt.push_str("TASK\n");
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

fn completion_hint(task: &str, turns: &[ModelTurn]) -> Option<String> {
    let expected_root = expected_output_root(task);
    if task_requires_write_action(task) {
        match create_phase_for_task(task, turns) {
            CreatePhase::Verify | CreatePhase::Finalize => {
                return Some(
                    "A write/edit has already succeeded. Prefer returning final with a concise summary of created or changed files unless another tool is strictly necessary."
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

fn has_mkdir_evidence(turns: &[ModelTurn]) -> bool {
    turns.iter().any(|turn| match turn {
        ModelTurn::ToolResult { tool, output } => tool == "mkdir" && output.trim() == "ok",
        _ => false,
    })
}

fn has_empty_directory_evidence(turns: &[ModelTurn]) -> bool {
    turns.iter().any(|turn| match turn {
        ModelTurn::ToolResult { tool, output } if tool == "list_dir" => output.trim().is_empty(),
        ModelTurn::ToolResult { tool, output } if tool == "stat_path" => {
            output.contains("kind=directory")
        }
        _ => false,
    })
}

fn create_phase_for_task(task: &str, turns: &[ModelTurn]) -> CreatePhase {
    if !task_requires_write_action(task) {
        return CreatePhase::Finalize;
    }
    if has_write_evidence(turns) {
        if has_verification_evidence(turns) {
            return CreatePhase::Finalize;
        }
        return CreatePhase::Verify;
    }
    if has_mkdir_evidence(turns) || has_empty_directory_evidence(turns) {
        return CreatePhase::Write;
    }
    CreatePhase::Prepare
}

fn has_verification_evidence(turns: &[ModelTurn]) -> bool {
    turns.iter().rev().take(4).any(|turn| match turn {
        ModelTurn::ToolResult { tool, output } if tool == "read_file" => !output.trim().is_empty(),
        ModelTurn::ToolResult { tool, output } if tool == "stat_path" => {
            output.contains("kind=file")
        }
        ModelTurn::ToolResult { tool, output } if tool == "path_exists" => {
            output.contains("exists=true")
        }
        _ => false,
    })
}

fn expected_output_root(task: &str) -> Option<PathBuf> {
    if !task_requires_write_action(task) {
        return None;
    }
    extract_path_like_tokens(task)
        .into_iter()
        .find(|path| path.components().count() > 1)
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
        && path_matches_expected(expected_root, actual)
    {
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
