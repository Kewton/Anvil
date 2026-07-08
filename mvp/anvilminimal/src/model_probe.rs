use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Context;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::config::{Config, Provider};
use crate::minimal_loop::loop_run::{
    ActionNoToolPolicy, CompletionContractPathMerge, CompletionContractVerification,
    ContractEnforcement, PromptArtifactExtraction, RunSessionOptions, RunSessionScope,
    RunSessionStepKind, run_session_with_outcome_with_options,
};
use crate::provider_call::{ProviderCallScope, ProviderChatRequest, chat_with_cancel};
use crate::providers::{AssistantReply, ChatClient};
use crate::state::{ConversationMessage, SessionSnapshot, ToolCall};
use crate::tui::NOOP_UI;

pub const MODEL_PROBE_VERSION: &str = "model-probe-v1";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelProbeOutput {
    pub profile_path: PathBuf,
    pub card_path: PathBuf,
    pub card: String,
    pub scratch_path: PathBuf,
    pub scratch_cleaned: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelProbeReport {
    pub version: String,
    pub generated_at: String,
    pub scope: String,
    pub executor: ProbeRole,
    pub planner: ProbeRole,
    pub tasks: Vec<ModelProbeTaskEvidence>,
    pub metrics: ModelProbeMetrics,
    pub no_network_guarantee: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProbeRole {
    pub role: String,
    pub provider: Provider,
    pub model: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ModelProbeMetrics {
    pub task_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelProbeTaskEvidence {
    pub id: String,
    pub role: String,
    pub session_mode: String,
    pub ok: bool,
    pub error: String,
    pub final_text: String,
    pub changed_paths: Vec<String>,
    pub iterations: usize,
    pub tool_call_count: usize,
    pub raw_tool_calls: Vec<RawToolCallEvidence>,
    pub raw_commands: Vec<String>,
    pub provider_turns: Vec<Value>,
    pub notable_events: Vec<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawToolCallEvidence {
    pub name: String,
    pub arguments: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProbeRoleKind {
    Executor,
    Planner,
}

impl ProbeRoleKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Executor => "executor",
            Self::Planner => "planner",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProbeSessionMode {
    Fresh,
    Appended,
}

impl ProbeSessionMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::Fresh => "fresh",
            Self::Appended => "appended",
        }
    }
}

#[derive(Debug, Clone)]
struct ProbeTask {
    id: &'static str,
    role: ProbeRoleKind,
    session_mode: ProbeSessionMode,
    kind: ProbeTaskKind,
    required_paths: Vec<String>,
    step_kind: RunSessionStepKind,
    prompt: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProbeTaskKind {
    Session,
    JsonSchema,
}

struct ScratchWorkspace {
    path: PathBuf,
    cleaned: bool,
}

impl ScratchWorkspace {
    fn create() -> anyhow::Result<Self> {
        let path = std::env::temp_dir().join(format!("anvil-model-probe-{}", Uuid::now_v7()));
        fs::create_dir_all(&path)
            .with_context(|| format!("failed to create scratch workspace {}", path.display()))?;
        Ok(Self {
            path,
            cleaned: false,
        })
    }

    fn cleanup(&mut self) -> bool {
        if self.cleaned {
            return true;
        }
        self.cleaned = match fs::remove_dir_all(&self.path) {
            Ok(()) => true,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => true,
            Err(_) => false,
        };
        self.cleaned
    }
}

impl Drop for ScratchWorkspace {
    fn drop(&mut self) {
        let _ = self.cleanup();
    }
}

pub fn run_configured(
    config: &Config,
    planner: &mut dyn ChatClient,
    execution: &mut dyn ChatClient,
) -> anyhow::Result<ModelProbeOutput> {
    run_with_output_dir(
        config,
        planner,
        execution,
        default_model_profiles_dir(config)?,
    )
}

pub fn run_with_output_dir(
    config: &Config,
    planner: &mut dyn ChatClient,
    execution: &mut dyn ChatClient,
    output_dir: PathBuf,
) -> anyhow::Result<ModelProbeOutput> {
    let mut scratch = ScratchWorkspace::create()?;
    seed_scratch_workspace(&scratch.path)?;
    fs::create_dir_all(&output_dir)
        .with_context(|| format!("failed to create {}", output_dir.display()))?;
    let events_path = scratch.path.join(".anvil/model-probe/events.jsonl");
    if let Some(parent) = events_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut probe_config = config.clone();
    probe_config.workspace_root = scratch.path.clone();
    probe_config.eval_events_path = Some(events_path.clone());
    probe_config.completion_contract_path = None;
    probe_config.yes = true;
    probe_config.offline = true;
    probe_config.max_iterations = 1;
    probe_config.no_footer = true;

    let mut tasks = Vec::new();
    let mut write_simple_session: Option<SessionSnapshot> = None;
    for task in battery(&scratch.path) {
        let events_before = read_event_values(&events_path).len();
        let evidence = match task.kind {
            ProbeTaskKind::Session => {
                let mut session = match task.id {
                    "edit_own" => write_simple_session
                        .clone()
                        .unwrap_or_else(SessionSnapshot::new),
                    "repair_appended" => appended_repair_session(),
                    _ => SessionSnapshot::new(),
                };
                let result = run_probe_session(
                    execution,
                    &mut session,
                    &probe_config,
                    &task,
                    &events_path,
                    events_before,
                );
                if task.id == "write_simple" {
                    write_simple_session = Some(session);
                }
                result?
            }
            ProbeTaskKind::JsonSchema => {
                run_json_schema_probe(planner, &probe_config, &task, &events_path, events_before)?
            }
        };
        tasks.push(evidence);
    }

    let report = ModelProbeReport {
        version: MODEL_PROBE_VERSION.to_string(),
        generated_at: timestamp_label(),
        scope: "N=10 micro-tasks; dialect indicators, not a capability benchmark".to_string(),
        executor: ProbeRole {
            role: "executor".to_string(),
            provider: config.provider,
            model: config.model.clone(),
        },
        planner: ProbeRole {
            role: "planner".to_string(),
            provider: config.planner_provider,
            model: config.planner_model.clone(),
        },
        metrics: ModelProbeMetrics {
            task_count: tasks.len(),
        },
        no_network_guarantee: no_network_commands(&tasks),
        tasks,
    };
    let card = render_card(&report);
    let basename = format!(
        "{}-{}.json",
        sanitize_filename(&config.model),
        report.generated_at
    );
    let profile_path = output_dir.join(basename);
    let card_path = profile_path.with_extension("md");
    fs::write(&profile_path, serde_json::to_string_pretty(&report)?)?;
    fs::write(&card_path, &card)?;
    let scratch_path = scratch.path.clone();
    let scratch_cleaned = scratch.cleanup();
    Ok(ModelProbeOutput {
        profile_path,
        card_path,
        card,
        scratch_path,
        scratch_cleaned,
    })
}

fn run_probe_session(
    client: &mut dyn ChatClient,
    session: &mut SessionSnapshot,
    config: &Config,
    task: &ProbeTask,
    events_path: &Path,
    events_before: usize,
) -> anyhow::Result<ModelProbeTaskEvidence> {
    let mut task_config = config.clone();
    task_config.model = config.model.clone();
    let options = probe_run_options(task.step_kind);
    let result = run_session_with_outcome_with_options(
        client,
        session,
        &task.prompt,
        &task.required_paths,
        &task_config,
        &NOOP_UI,
        options,
    );
    let raw_tool_calls = raw_tool_calls_from_session(session);
    let raw_commands = raw_commands_from_calls(&raw_tool_calls);
    let events = read_event_values(events_path);
    let new_events = events.into_iter().skip(events_before).collect::<Vec<_>>();
    let (ok, error, final_text, changed_paths, iterations, tool_call_count) = match result {
        Ok(outcome) => (
            true,
            String::new(),
            outcome.final_text,
            outcome.changed_paths,
            outcome.iterations,
            outcome.tool_calls,
        ),
        Err(err) => (
            false,
            err.to_string(),
            String::new(),
            Vec::new(),
            0,
            raw_tool_calls.len(),
        ),
    };
    Ok(ModelProbeTaskEvidence {
        id: task.id.to_string(),
        role: task.role.as_str().to_string(),
        session_mode: task.session_mode.as_str().to_string(),
        ok,
        error,
        final_text,
        changed_paths,
        iterations,
        tool_call_count,
        raw_tool_calls,
        raw_commands,
        provider_turns: events_named(&new_events, "provider_turn_duration"),
        notable_events: notable_events(new_events),
    })
}

fn run_json_schema_probe(
    client: &mut dyn ChatClient,
    config: &Config,
    task: &ProbeTask,
    events_path: &Path,
    events_before: usize,
) -> anyhow::Result<ModelProbeTaskEvidence> {
    let mut task_config = config.clone();
    task_config.model = config.planner_model.clone();
    let outcome = chat_with_cancel(
        client,
        &task_config,
        ProviderChatRequest {
            scope: ProviderCallScope::PlannerStep,
            model: &task_config.model,
            messages: &[ConversationMessage::user(task.prompt.clone())],
            tools: &[],
            native_tools_enabled: false,
        },
        || false,
    );
    let events = read_event_values(events_path);
    let new_events = events.into_iter().skip(events_before).collect::<Vec<_>>();
    let (ok, error, final_text, raw_tool_calls) = match outcome.result {
        Ok(reply) => {
            let raw_tool_calls = raw_tool_calls_from_reply(&reply);
            (true, String::new(), reply.content, raw_tool_calls)
        }
        Err(err) => (false, err.to_string(), String::new(), Vec::new()),
    };
    let raw_commands = raw_commands_from_calls(&raw_tool_calls);
    Ok(ModelProbeTaskEvidence {
        id: task.id.to_string(),
        role: task.role.as_str().to_string(),
        session_mode: task.session_mode.as_str().to_string(),
        ok,
        error,
        final_text,
        changed_paths: Vec::new(),
        iterations: 1,
        tool_call_count: raw_tool_calls.len(),
        raw_tool_calls,
        raw_commands,
        provider_turns: events_named(&new_events, "provider_turn_duration"),
        notable_events: notable_events(new_events),
    })
}

fn probe_run_options(step_kind: RunSessionStepKind) -> RunSessionOptions {
    RunSessionOptions {
        prompt_artifact_extraction: PromptArtifactExtraction::Disabled,
        completion_contract_path_merge: CompletionContractPathMerge::Disabled,
        completion_contract_verification: CompletionContractVerification::DisabledDuringStep,
        contract_enforcement: ContractEnforcement::Observe,
        phase_scope: Some("model-probe".to_string()),
        action_no_tool_policy: ActionNoToolPolicy::RequireToolOnlyIfNoToolSeen,
        scope: RunSessionScope::PlanRunStep,
        step_kind: Some(step_kind),
        dependency_setup_authority:
            crate::minimal_loop::dependency_setup::NodeDependencySetupAuthority::None,
    }
}

fn seed_scratch_workspace(root: &Path) -> anyhow::Result<()> {
    fs::create_dir_all(root.join("src/util"))?;
    fs::create_dir_all(root.join("src/repair"))?;
    fs::create_dir_all(root.join("src/provided"))?;
    fs::write(
        root.join("package.json"),
        "{\n  \"scripts\": {\n    \"build\": \"node -e \\\"console.log('build')\\\"\"\n  }\n}\n",
    )?;
    fs::write(
        root.join("src/provided/edit-target.txt"),
        "alpha beta\ngamma delta\n",
    )?;
    fs::write(
        root.join("src/repair/appended.ts"),
        "export function value() {\n  return ;\n}\n",
    )?;
    fs::write(
        root.join("src/repair/compact.ts"),
        "export function value() {\n  return ;\n}\n",
    )?;
    fs::write(
        root.join("src/repair/regenerate.ts"),
        "export function value() {\n  return ;\n}\n",
    )?;
    Ok(())
}

fn battery(root: &Path) -> Vec<ProbeTask> {
    let deep_path =
        "src/a/b/c/d/e/model-probe-long-file-name-with-many-segments-and-dashes.ts".to_string();
    vec![
        ProbeTask {
            id: "write_simple",
            role: ProbeRoleKind::Executor,
            session_mode: ProbeSessionMode::Fresh,
            kind: ProbeTaskKind::Session,
            required_paths: vec!["src/util/math.ts".to_string()],
            step_kind: RunSessionStepKind::Implement,
            prompt: "MODEL PROBE task write_simple: create src/util/math.ts using the Write tool. The file must contain a 5-line TypeScript function named add(a: number, b: number) that returns a + b. Do not install packages.".to_string(),
        },
        ProbeTask {
            id: "write_deep",
            role: ProbeRoleKind::Executor,
            session_mode: ProbeSessionMode::Fresh,
            kind: ProbeTaskKind::Session,
            required_paths: vec![deep_path.clone()],
            step_kind: RunSessionStepKind::Implement,
            prompt: format!("MODEL PROBE task write_deep: create {deep_path} using the Write tool with a tiny exported function. Do not install packages."),
        },
        ProbeTask {
            id: "edit_provided",
            role: ProbeRoleKind::Executor,
            session_mode: ProbeSessionMode::Fresh,
            kind: ProbeTaskKind::Session,
            required_paths: vec!["src/provided/edit-target.txt".to_string()],
            step_kind: RunSessionStepKind::Implement,
            prompt: "MODEL PROBE task edit_provided: the file src/provided/edit-target.txt currently contains exactly:\nalpha beta\ngamma delta\nUse the Edit tool once to change gamma delta to gamma epsilon. Do not rewrite the full file.".to_string(),
        },
        ProbeTask {
            id: "edit_own",
            role: ProbeRoleKind::Executor,
            session_mode: ProbeSessionMode::Appended,
            kind: ProbeTaskKind::Session,
            required_paths: vec!["src/util/math.ts".to_string()],
            step_kind: RunSessionStepKind::Implement,
            prompt: "MODEL PROBE task edit_own: edit the src/util/math.ts file you wrote earlier in this same session. Use the Edit tool to rename add to sum. Do not install packages.".to_string(),
        },
        ProbeTask {
            id: "verify_exist",
            role: ProbeRoleKind::Executor,
            session_mode: ProbeSessionMode::Fresh,
            kind: ProbeTaskKind::Session,
            required_paths: Vec::new(),
            step_kind: RunSessionStepKind::Verify,
            prompt: "MODEL PROBE task verify_exist: verify src/util/math.ts exists. Use one Bash command only. Do not install packages.".to_string(),
        },
        ProbeTask {
            id: "verify_json",
            role: ProbeRoleKind::Executor,
            session_mode: ProbeSessionMode::Fresh,
            kind: ProbeTaskKind::Session,
            required_paths: Vec::new(),
            step_kind: RunSessionStepKind::Verify,
            prompt: "MODEL PROBE task verify_json: verify package.json declares a build script. Use one local Bash command only. Do not run npm, pip, or install anything.".to_string(),
        },
        ProbeTask {
            id: "repair_appended",
            role: ProbeRoleKind::Executor,
            session_mode: ProbeSessionMode::Appended,
            kind: ProbeTaskKind::Session,
            required_paths: vec!["src/repair/appended.ts".to_string()],
            step_kind: RunSessionStepKind::Implement,
            prompt: "MODEL PROBE task repair_appended: repair this one-line TypeScript compile error using Edit or Write. Frame:\n./src/repair/appended.ts:2:10\nError: Expression expected\n  1 | export function value() {\n> 2 |   return ;\n    |          ^\n  3 | }\nChange it to return 1. Do not install packages.".to_string(),
        },
        ProbeTask {
            id: "repair_compact",
            role: ProbeRoleKind::Executor,
            session_mode: ProbeSessionMode::Fresh,
            kind: ProbeTaskKind::Session,
            required_paths: vec!["src/repair/compact.ts".to_string()],
            step_kind: RunSessionStepKind::Implement,
            prompt: "MODEL PROBE task repair_compact: repair this one-line TypeScript compile error using Edit or Write. Frame:\n./src/repair/compact.ts:2:10\nError: Expression expected\n  1 | export function value() {\n> 2 |   return ;\n    |          ^\n  3 | }\nChange it to return 1. Do not install packages.".to_string(),
        },
        ProbeTask {
            id: "regenerate",
            role: ProbeRoleKind::Executor,
            session_mode: ProbeSessionMode::Fresh,
            kind: ProbeTaskKind::Session,
            required_paths: vec!["src/repair/regenerate.ts".to_string()],
            step_kind: RunSessionStepKind::Implement,
            prompt: "MODEL PROBE task regenerate: rewrite the full corrected file src/repair/regenerate.ts via the Write tool. It must export function value() { return 1; }. Do not install packages.".to_string(),
        },
        ProbeTask {
            id: "json_schema",
            role: ProbeRoleKind::Planner,
            session_mode: ProbeSessionMode::Fresh,
            kind: ProbeTaskKind::JsonSchema,
            required_paths: Vec::new(),
            step_kind: RunSessionStepKind::Report,
            prompt: format!(
                "MODEL PROBE task json_schema for workspace {}: respond ONLY with JSON matching this schema: {{\"steps\":[{{\"instruction\":\"string\",\"kind\":\"implement|verify|report\",\"expected_paths\":[\"string\"],\"expected_result\":\"string\"}}]}}. Use one step that writes src/util/math.ts.",
                root.display()
            ),
        },
    ]
}

fn appended_repair_session() -> SessionSnapshot {
    let mut session = SessionSnapshot::new();
    session.messages.push(ConversationMessage::user(format!(
        "Prior context for model-probe context sensitivity. {}\nRemember the current task is unrelated unless the next user asks for it.",
        "The previous implementation discussed route state, score counters, restart affordances, and TypeScript helpers. ".repeat(25)
    )));
    session
        .messages
        .push(ConversationMessage::assistant("Acknowledged.", Vec::new()));
    session
}

fn raw_tool_calls_from_session(session: &SessionSnapshot) -> Vec<RawToolCallEvidence> {
    session
        .messages
        .iter()
        .filter(|message| message.role == "assistant")
        .flat_map(|message| message.tool_calls.iter())
        .map(raw_tool_call_from_call)
        .collect()
}

fn raw_tool_calls_from_reply(reply: &AssistantReply) -> Vec<RawToolCallEvidence> {
    reply
        .tool_calls
        .iter()
        .map(raw_tool_call_from_call)
        .collect()
}

fn raw_tool_call_from_call(call: &ToolCall) -> RawToolCallEvidence {
    RawToolCallEvidence {
        name: call.name.clone(),
        arguments: call.arguments.clone(),
    }
}

fn raw_commands_from_calls(calls: &[RawToolCallEvidence]) -> Vec<String> {
    calls
        .iter()
        .filter(|call| call.name == "Bash")
        .filter_map(|call| {
            call.arguments
                .get("command")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)
        })
        .collect()
}

fn no_network_commands(tasks: &[ModelProbeTaskEvidence]) -> bool {
    tasks
        .iter()
        .flat_map(|task| task.raw_commands.iter())
        .all(|cmd| {
            let lower = cmd.to_ascii_lowercase();
            !lower.contains("npm install")
                && !lower.contains("pnpm install")
                && !lower.contains("yarn add")
                && !lower.contains("pip install")
        })
}

fn read_event_values(path: &Path) -> Vec<Value> {
    let Ok(text) = fs::read_to_string(path) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .collect()
}

fn events_named(events: &[Value], name: &str) -> Vec<Value> {
    events
        .iter()
        .filter(|event| event.get("event").and_then(Value::as_str) == Some(name))
        .cloned()
        .collect()
}

fn notable_events(events: Vec<Value>) -> Vec<Value> {
    events
        .into_iter()
        .filter(|event| {
            matches!(
                event.get("event").and_then(Value::as_str),
                Some(
                    "tool_call_raw"
                        | "tool_execute"
                        | "tool_validation_error"
                        | "tool_args_path_normalized"
                        | "tool_args_path_salvaged"
                        | "runtime_bash_policy"
                        | "verify_command_normalized_at_runtime"
                        | "edit_anchor_salvaged"
                        | "context_truncation_suspected"
                )
            )
        })
        .collect()
}

fn render_card(report: &ModelProbeReport) -> String {
    format!(
        "# Model Probe Card\n\n- Version: {}\n- Scope: {}\n- Executor: {:?} `{}`\n- Planner: {:?} `{}`\n- Tasks: {}\n- Profile JSON records raw tool calls and commands verbatim.\n- No-network guarantee: {}\n\nThis is a dialect indicator battery, not a capability benchmark.\n",
        report.version,
        report.scope,
        report.executor.provider,
        report.executor.model,
        report.planner.provider,
        report.planner.model,
        report.metrics.task_count,
        if report.no_network_guarantee {
            "passed"
        } else {
            "failed"
        }
    )
}

fn default_model_profiles_dir(config: &Config) -> anyhow::Result<PathBuf> {
    if let Some(home) = std::env::var_os("HOME") {
        return Ok(PathBuf::from(home).join(".anvil/model-profiles"));
    }
    Ok(config.state_dir.join("model-profiles"))
}

fn sanitize_filename(value: &str) -> String {
    let mut out = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
                ch
            } else {
                '-'
            }
        })
        .collect::<String>();
    while out.contains("--") {
        out = out.replace("--", "-");
    }
    out.trim_matches('-').to_string()
}

fn timestamp_label() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default();
    let days = (secs / 86_400) as i64;
    let seconds_of_day = secs % 86_400;
    let (year, month, day) = civil_from_days(days);
    let hour = seconds_of_day / 3_600;
    let minute = (seconds_of_day % 3_600) / 60;
    let second = seconds_of_day % 60;
    format!("{year:04}{month:02}{day:02}-{hour:02}{minute:02}{second:02}")
}

fn civil_from_days(days_since_epoch: i64) -> (i64, u64, u64) {
    let z = days_since_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = mp + if mp < 10 { 3 } else { -9 };
    let year = y + if m <= 2 { 1 } else { 0 };
    (year, m as u64, d as u64)
}
