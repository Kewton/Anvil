use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::anyhow;
use serde::Deserialize;
use serde_json::{Map, Value};

use crate::tools::{
    edit_file, exec_in_dir, glob_paths, read_file, search_in_files, unified_diff, write_file,
};

#[async_trait::async_trait]
pub trait ModelExchange: Send + Sync {
    async fn complete(&self, prompt: &str) -> anyhow::Result<String>;
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

        for step in 0..self.config.max_steps {
            let step_number = step + 1;
            let step_started_at = Instant::now();
            observer(LoopEvent::StepStarted { step: step_number });
            let prompt = build_loop_prompt(task, &turns);
            let model_started_at = Instant::now();
            let raw = model.complete(&prompt).await?;
            observer(LoopEvent::ModelResponseReceived {
                bytes: raw.len(),
                elapsed_ms: model_started_at.elapsed().as_millis(),
            });
            observer(LoopEvent::ModelResponsePreview {
                preview: truncate(&raw.replace('\n', "\\n"), 240),
            });
            let response = match parse_model_response(&raw) {
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
                    turns.push(ModelTurn::ProtocolError {
                        error_kind: "invalid_json".to_string(),
                        message,
                        hint: "return one complete JSON object only; do not emit partial or truncated JSON".to_string(),
                    });
                    observer(LoopEvent::StepFinished {
                        step: step_number,
                        elapsed_ms: step_started_at.elapsed().as_millis(),
                    });
                    continue;
                }
                Err(other) => return Err(other),
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
                        turns.push(ModelTurn::ProtocolError {
                            error_kind: "final_without_action".to_string(),
                            message: reason,
                            hint: "if the task requires creating or updating files, use write_file or edit_file before returning final".to_string(),
                        });
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
                                turns.push(ModelTurn::ToolError {
                                    tool: call.tool.clone(),
                                    error_kind: "schema_validation".to_string(),
                                    message: message.clone(),
                                    hint: tool_error_hint(&call.tool),
                                });
                                continue;
                            }
                            Err(other) => return Err(other),
                        };
                        let normalized = serde_json::to_string(&validated)
                            .map_err(|err| LoopError::InvalidToolCall(err.to_string()))?;
                        observer(LoopEvent::ToolCallValidated {
                            tool: validated.tool_name().to_string(),
                            normalized: truncate(&normalized, 240),
                        });
                        if !seen.insert(normalized.clone()) {
                            if can_reuse_cached_result(&validated) {
                                let Some((cached_output, reuse_count)) =
                                    cached_results.get_mut(&normalized)
                                else {
                                    return Err(LoopError::DuplicateToolCall(normalized));
                                };
                                *reuse_count += 1;
                                if *reuse_count > self.config.max_cached_reuses_per_call {
                                    return Err(LoopError::DuplicateToolCall(normalized));
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
                                turns.push(ModelTurn::ToolError {
                                    tool: validated.tool_name().to_string(),
                                    error_kind: "execution_error".to_string(),
                                    message,
                                    hint: tool_execution_error_hint(&validated),
                                });
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
                        if can_reuse_cached_result(&validated) {
                            cached_results.insert(normalized, (result.clone(), 0));
                        }
                        turns.push(ModelTurn::ToolResult {
                            tool: validated.tool_name().to_string(),
                            output: result,
                        });
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
    let mut prompt = String::from(
        "You are Anvil. Solve the task by selecting tools when needed.\n\
Return only JSON.\n\
For tools: {\"type\":\"tool_calls\",\"calls\":[{\"tool\":\"read_file\",\"args\":{...}}]}\n\
For final answer: {\"type\":\"final\",\"content\":\"...\"}\n\
Available tools: read_file, write_file, edit_file, exec, diff, search, glob.\n\
Use tools when context is missing. Do not ask the user for git details if you can inspect them.\n\
Invalid or partial tool args are forbidden.\n\n",
    );
    for turn in turns {
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
        "read_file" | "write_file" | "edit_file" => "file tools require args.path".to_string(),
        "search" => "search requires args.needle".to_string(),
        "diff" => "diff requires args.before and args.after".to_string(),
        _ => "review the tool schema and retry with valid arguments".to_string(),
    }
}

fn tool_execution_error_hint(call: &ToolCall) -> String {
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
    }
}

fn can_reuse_cached_result(call: &ToolCall) -> bool {
    match call {
        ToolCall::ReadFile(_) | ToolCall::Search(_) | ToolCall::Glob(_) => true,
        ToolCall::Exec(args) => is_read_only_exec(&args.argv),
        ToolCall::WriteFile(_) | ToolCall::EditFile(_) | ToolCall::Diff(_) => false,
    }
}

fn is_read_only_exec(argv: &[String]) -> bool {
    matches!(
        argv.first().map(String::as_str),
        Some("ls" | "cat" | "pwd" | "git" | "find" | "rg")
    )
}
