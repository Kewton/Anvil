use std::collections::BTreeMap;
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

pub const MODEL_PROBE_VERSION: &str = "model-probe-v2";

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
    pub path_argument_count: usize,
    pub absolute_path_count: usize,
    pub absolute_path_rate: f64,
    pub corrupted_path_count: usize,
    pub shell_command_count: usize,
    pub shell_control_count: usize,
    pub shell_control_rate: f64,
    pub shell_control_breakdown: ShellControlBreakdown,
    pub edit_anchor: EditAnchorMetrics,
    pub repair_follow_through: RepairFollowThroughMetrics,
    pub regeneration_follow_through: String,
    pub json_response_count: usize,
    pub json_valid_count: usize,
    pub json_valid_rate: f64,
    pub missing_field_kinds: BTreeMap<String, usize>,
    pub empty_response_count: usize,
    pub empty_response_rate: f64,
    pub malformed_tool_call_count: usize,
    pub malformed_tool_call_rate: f64,
    pub latency_ms: LatencyStats,
    pub first_turn_latency_ms: LatencyStats,
    pub later_turn_latency_ms: LatencyStats,
    pub token_telemetry: TokenTelemetryMetrics,
    pub context_truncation_suspected_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ShellControlBreakdown {
    pub and_and: usize,
    pub semicolon: usize,
    pub pipe: usize,
    pub redirect: usize,
    pub cd: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct EditAnchorMetrics {
    pub exact: usize,
    pub salvageable: usize,
    pub miss: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RepairFollowThroughMetrics {
    pub appended: String,
    pub compact: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LatencyStats {
    pub count: usize,
    pub min_ms: Option<u64>,
    pub p50_ms: Option<u64>,
    pub max_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TokenTelemetryMetrics {
    pub estimated_prompt_tokens_sent_total: u64,
    pub prompt_eval_count_total: u64,
    pub eval_count_total: u64,
    pub missing_prompt_eval_count: usize,
    pub finish_reasons: BTreeMap<String, usize>,
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
                    "edit_own" => write_simple_session.clone().unwrap_or_default(),
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
        scope: format!(
            "N={} micro-tasks; dialect indicators, not a capability benchmark",
            tasks.len()
        ),
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
            ..compute_metrics(&tasks)
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
    let message_count_before = session.messages.len();
    let result = run_session_with_outcome_with_options(
        client,
        session,
        &task.prompt,
        &task.required_paths,
        &task_config,
        &NOOP_UI,
        options,
    );
    let raw_tool_calls = raw_tool_calls_from_session_since(session, message_count_before);
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
        step_wall_clock_cap: None,
        path_fallback_candidates: Vec::new(),
        require_mutation_before_contract_short_circuit: false,
        escalation_carryover: None,
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
            id: "csv_fixture_verify",
            role: ProbeRoleKind::Executor,
            session_mode: ProbeSessionMode::Fresh,
            kind: ProbeTaskKind::Session,
            required_paths: vec!["fixtures/model-probe.csv".to_string()],
            step_kind: RunSessionStepKind::Implement,
            prompt: "MODEL PROBE task csv_fixture_verify: create a small CSV fixture at fixtures/model-probe.csv with the Write tool, then verify a local program can process it with one Bash command. Do not use redirects, heredocs, npm, pip, or installs.".to_string(),
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

fn raw_tool_calls_from_session_since(
    session: &SessionSnapshot,
    message_count_before: usize,
) -> Vec<RawToolCallEvidence> {
    session
        .messages
        .iter()
        .skip(message_count_before)
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

fn compute_metrics(tasks: &[ModelProbeTaskEvidence]) -> ModelProbeMetrics {
    let mut metrics = ModelProbeMetrics {
        task_count: tasks.len(),
        ..ModelProbeMetrics::default()
    };
    let mut latencies = Vec::new();
    let mut first_turn_latencies = Vec::new();
    let mut later_turn_latencies = Vec::new();
    for task in tasks {
        if task.final_text.trim().is_empty() && task.raw_tool_calls.is_empty() {
            metrics.empty_response_count += 1;
        }
        let task_edit_calls = task
            .raw_tool_calls
            .iter()
            .filter(|call| call.name == "Edit")
            .count();
        for call in &task.raw_tool_calls {
            if malformed_tool_call(call) {
                metrics.malformed_tool_call_count += 1;
            }
            if let Some(path) = call.arguments.get("path").and_then(Value::as_str) {
                metrics.path_argument_count += 1;
                if path_is_absolute(path) {
                    metrics.absolute_path_count += 1;
                }
                if path_looks_corrupted(path) {
                    metrics.corrupted_path_count += 1;
                }
            }
        }
        for command in &task.raw_commands {
            metrics.shell_command_count += 1;
            let mut controlled = false;
            if command.contains("&&") {
                metrics.shell_control_breakdown.and_and += 1;
                controlled = true;
            }
            if command_contains_semicolon(command) {
                metrics.shell_control_breakdown.semicolon += 1;
                controlled = true;
            }
            if command.contains('|') {
                metrics.shell_control_breakdown.pipe += 1;
                controlled = true;
            }
            if command.contains('>') || command.contains('<') {
                metrics.shell_control_breakdown.redirect += 1;
                controlled = true;
            }
            if command_uses_cd(command) {
                metrics.shell_control_breakdown.cd += 1;
                controlled = true;
            }
            if controlled {
                metrics.shell_control_count += 1;
            }
        }
        let mut task_salvageable = 0usize;
        let mut task_miss = 0usize;
        for event in &task.notable_events {
            match event.get("event").and_then(Value::as_str) {
                Some("edit_anchor_salvaged") => task_salvageable += 1,
                Some("tool_validation_error")
                    if event.get("name").and_then(Value::as_str) == Some("Edit")
                        && event.get("error_kind").and_then(Value::as_str)
                            == Some("edit_anchor_not_found") =>
                {
                    task_miss += 1;
                }
                Some("context_truncation_suspected") => {
                    metrics.context_truncation_suspected_count += 1;
                }
                _ => {}
            }
        }
        let task_salvageable = task_salvageable.min(task_edit_calls);
        let remaining_edit_calls = task_edit_calls.saturating_sub(task_salvageable);
        let task_miss = task_miss.min(remaining_edit_calls);
        metrics.edit_anchor.salvageable += task_salvageable;
        metrics.edit_anchor.miss += task_miss;
        metrics.edit_anchor.exact += remaining_edit_calls.saturating_sub(task_miss);
        for (index, event) in task.provider_turns.iter().enumerate() {
            if let Some(value) = event.get("duration_ms").and_then(Value::as_u64) {
                latencies.push(value);
                if index == 0 {
                    first_turn_latencies.push(value);
                } else {
                    later_turn_latencies.push(value);
                }
            }
            if let Some(value) = event
                .get("estimated_prompt_tokens_sent")
                .and_then(Value::as_u64)
            {
                metrics.token_telemetry.estimated_prompt_tokens_sent_total += value;
            }
            if let Some(value) = event.get("prompt_eval_count").and_then(Value::as_u64) {
                metrics.token_telemetry.prompt_eval_count_total += value;
            } else {
                metrics.token_telemetry.missing_prompt_eval_count += 1;
            }
            if let Some(value) = event.get("eval_count").and_then(Value::as_u64) {
                metrics.token_telemetry.eval_count_total += value;
            }
            if let Some(reason) = event.get("finish_reason").and_then(Value::as_str) {
                *metrics
                    .token_telemetry
                    .finish_reasons
                    .entry(reason.to_string())
                    .or_default() += 1;
            }
        }
        if task.id == "repair_appended" {
            metrics.repair_follow_through.appended = follow_through(task);
        } else if task.id == "repair_compact" {
            metrics.repair_follow_through.compact = follow_through(task);
        } else if task.id == "regenerate" {
            metrics.regeneration_follow_through = follow_through(task);
        } else if task.id == "json_schema" {
            metrics.json_response_count += 1;
            match analyze_json_schema_response(&task.final_text) {
                JsonSchemaAnalysis::Valid { missing_fields } => {
                    metrics.json_valid_count += 1;
                    for field in missing_fields {
                        *metrics.missing_field_kinds.entry(field).or_default() += 1;
                    }
                }
                JsonSchemaAnalysis::Invalid => {}
            }
        }
    }
    metrics.absolute_path_rate = ratio(metrics.absolute_path_count, metrics.path_argument_count);
    metrics.shell_control_rate = ratio(metrics.shell_control_count, metrics.shell_command_count);
    metrics.empty_response_rate = ratio(metrics.empty_response_count, tasks.len());
    let tool_call_count = tasks
        .iter()
        .map(|task| task.raw_tool_calls.len())
        .sum::<usize>();
    metrics.malformed_tool_call_rate = ratio(metrics.malformed_tool_call_count, tool_call_count);
    metrics.json_valid_rate = ratio(metrics.json_valid_count, metrics.json_response_count);
    metrics.latency_ms = latency_stats(latencies);
    metrics.first_turn_latency_ms = latency_stats(first_turn_latencies);
    metrics.later_turn_latency_ms = latency_stats(later_turn_latencies);
    metrics
}

enum JsonSchemaAnalysis {
    Valid { missing_fields: Vec<String> },
    Invalid,
}

fn analyze_json_schema_response(text: &str) -> JsonSchemaAnalysis {
    let Ok(value) = serde_json::from_str::<Value>(text.trim()) else {
        return JsonSchemaAnalysis::Invalid;
    };
    let Some(steps) = value.get("steps").and_then(Value::as_array) else {
        return JsonSchemaAnalysis::Invalid;
    };
    let mut missing = Vec::new();
    for step in steps {
        if !step.get("instruction").is_some_and(Value::is_string) {
            missing.push("semantic:instruction".to_string());
        }
        if !step.get("kind").is_some_and(Value::is_string) {
            missing.push("semantic:kind".to_string());
        }
        if !step.get("expected_paths").is_some_and(Value::is_array) {
            missing.push("semantic:expected_paths".to_string());
        }
        if !step.get("expected_result").is_some_and(Value::is_string) {
            missing.push("descriptive:expected_result".to_string());
        }
    }
    JsonSchemaAnalysis::Valid {
        missing_fields: missing,
    }
}

fn follow_through(task: &ModelProbeTaskEvidence) -> String {
    if !task.changed_paths.is_empty() {
        "edited".to_string()
    } else if task.final_text.trim().is_empty() && task.raw_tool_calls.is_empty() {
        "empty".to_string()
    } else if task.raw_tool_calls.is_empty() {
        "prose".to_string()
    } else {
        "tool_error".to_string()
    }
}

fn malformed_tool_call(call: &RawToolCallEvidence) -> bool {
    match call.name.as_str() {
        "Bash" => !call.arguments.get("command").is_some_and(Value::is_string),
        "Read" | "Write" => {
            !call.arguments.get("path").is_some_and(Value::is_string)
                || (call.name == "Write"
                    && !call.arguments.get("content").is_some_and(Value::is_string))
        }
        "Edit" => {
            !call.arguments.get("path").is_some_and(Value::is_string)
                || !call
                    .arguments
                    .get("old_string")
                    .is_some_and(Value::is_string)
                || !call
                    .arguments
                    .get("new_string")
                    .is_some_and(Value::is_string)
        }
        "Glob" => !call.arguments.get("pattern").is_some_and(Value::is_string),
        "Grep" => !call.arguments.get("pattern").is_some_and(Value::is_string),
        _ => true,
    }
}

fn path_is_absolute(path: &str) -> bool {
    path.starts_with('/') || path.get(1..3).is_some_and(|rest| rest == ":\\")
}

fn path_looks_corrupted(path: &str) -> bool {
    path.starts_with("workdir/")
        || path.contains("/workdir/")
        || path.contains("commandagent_mvp")
        || path.contains("//")
}

fn command_contains_semicolon(command: &str) -> bool {
    command.contains(';')
}

fn command_uses_cd(command: &str) -> bool {
    let trimmed = command.trim_start();
    trimmed.starts_with("cd ")
        || trimmed.starts_with("cd\t")
        || command.contains("&& cd ")
        || command.contains("; cd ")
}

fn latency_stats(mut values: Vec<u64>) -> LatencyStats {
    if values.is_empty() {
        return LatencyStats::default();
    }
    values.sort_unstable();
    LatencyStats {
        count: values.len(),
        min_ms: values.first().copied(),
        p50_ms: values.get(values.len() / 2).copied(),
        max_ms: values.last().copied(),
    }
}

fn ratio(numerator: usize, denominator: usize) -> f64 {
    if denominator == 0 {
        0.0
    } else {
        numerator as f64 / denominator as f64
    }
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
    let metrics = &report.metrics;
    let mut lines = vec![
        "# Model Probe Card".to_string(),
        String::new(),
        format!("- Version: {}", report.version),
        format!("- Scope: {}", report.scope),
        format!(
            "- Executor: {:?} `{}`",
            report.executor.provider, report.executor.model
        ),
        format!(
            "- Planner: {:?} `{}`",
            report.planner.provider, report.planner.model
        ),
        format!("- Tasks: {}", metrics.task_count),
        format!(
            "- No-network guarantee: {}",
            if report.no_network_guarantee {
                "passed"
            } else {
                "failed"
            }
        ),
        String::new(),
        "## Dialect Metrics".to_string(),
        String::new(),
        format!(
            "- absolute_path_rate: {} ({}/{})",
            percent(metrics.absolute_path_rate),
            metrics.absolute_path_count,
            metrics.path_argument_count
        ),
        format!("- corrupted_path_count: {}", metrics.corrupted_path_count),
        format!(
            "- shell_control_rate: {} ({}/{})",
            percent(metrics.shell_control_rate),
            metrics.shell_control_count,
            metrics.shell_command_count
        ),
        format!(
            "- shell_control_breakdown: &&={} ;={} pipe={} redirect={} cd={}",
            metrics.shell_control_breakdown.and_and,
            metrics.shell_control_breakdown.semicolon,
            metrics.shell_control_breakdown.pipe,
            metrics.shell_control_breakdown.redirect,
            metrics.shell_control_breakdown.cd
        ),
        format!(
            "- edit_anchor: exact={} salvageable={} miss={}",
            metrics.edit_anchor.exact, metrics.edit_anchor.salvageable, metrics.edit_anchor.miss
        ),
        format!(
            "- repair_follow_through: appended={} compact={}",
            empty_if_missing(&metrics.repair_follow_through.appended),
            empty_if_missing(&metrics.repair_follow_through.compact)
        ),
        format!(
            "- regeneration_follow_through: {}",
            empty_if_missing(&metrics.regeneration_follow_through)
        ),
        format!(
            "- json_valid_rate: {} ({}/{})",
            percent(metrics.json_valid_rate),
            metrics.json_valid_count,
            metrics.json_response_count
        ),
        format!(
            "- missing_field_kinds: {}",
            render_map(&metrics.missing_field_kinds)
        ),
        format!(
            "- empty_response_rate: {} ({}/{})",
            percent(metrics.empty_response_rate),
            metrics.empty_response_count,
            metrics.task_count
        ),
        format!(
            "- malformed_tool_call_rate: {} ({})",
            percent(metrics.malformed_tool_call_rate),
            metrics.malformed_tool_call_count
        ),
        format!(
            "- latency_ms: count={} min={} p50={} max={}",
            metrics.latency_ms.count,
            option_u64(metrics.latency_ms.min_ms),
            option_u64(metrics.latency_ms.p50_ms),
            option_u64(metrics.latency_ms.max_ms)
        ),
        format!(
            "- latency_cache_note: first_turn_p50={} later_turn_p50={} (cache-effect visibility; compare only within the same provider/model run)",
            option_u64(metrics.first_turn_latency_ms.p50_ms),
            option_u64(metrics.later_turn_latency_ms.p50_ms)
        ),
        format!(
            "- token_telemetry: estimated_prompt_total={} prompt_eval_total={} eval_total={} missing_prompt_eval_count={} finish_reasons={}",
            metrics.token_telemetry.estimated_prompt_tokens_sent_total,
            metrics.token_telemetry.prompt_eval_count_total,
            metrics.token_telemetry.eval_count_total,
            metrics.token_telemetry.missing_prompt_eval_count,
            render_map(&metrics.token_telemetry.finish_reasons)
        ),
        format!(
            "- context_truncation_suspected_count: {}",
            metrics.context_truncation_suspected_count
        ),
        String::new(),
        "## Absorption Map".to_string(),
        String::new(),
    ];
    let absorption = absorption_lines(metrics);
    if absorption.is_empty() {
        lines.push("- No elevated dialect indicator in this battery.".to_string());
    } else {
        lines.extend(absorption);
    }
    lines.extend([
        String::new(),
        "This card is for human review and tier-table evidence only. Probe results never auto-configure runtime behavior.".to_string(),
        "Profile JSON records raw tool calls and commands verbatim.".to_string(),
        "This is a dialect indicator battery, not a capability benchmark.".to_string(),
    ]);
    lines.join("\n")
}

fn absorption_lines(metrics: &ModelProbeMetrics) -> Vec<String> {
    let mut lines = Vec::new();
    if metrics.absolute_path_rate > 0.0 {
        lines.push(
            "- Elevated absolute path use => tool path normalization will be hot.".to_string(),
        );
    }
    if metrics.corrupted_path_count > 0 {
        lines.push("- Corrupted/root-anchored paths observed => path salvage and confinement feedback will be hot.".to_string());
    }
    if metrics.shell_control_count > 0 {
        lines.push(
            "- Shell-control commands observed => bash/verify normalization will be hot."
                .to_string(),
        );
    }
    if metrics.edit_anchor.salvageable > 0 || metrics.edit_anchor.miss > 0 {
        lines.push("- Edit anchor drift observed => edit-anchor salvage and full-file Write escalation are relevant.".to_string());
    }
    if metrics.repair_follow_through.appended != "edited"
        && metrics.repair_follow_through.compact == "edited"
    {
        lines.push(
            "- Compact-only repair follow-through => expect the 85 compact-session rung to matter."
                .to_string(),
        );
    }
    if metrics.regeneration_follow_through != "edited"
        && !metrics.regeneration_follow_through.is_empty()
    {
        lines.push("- Regeneration did not follow through => expect the 96 full-file regeneration rung to be risky.".to_string());
    }
    if metrics.json_valid_rate < 1.0 || !metrics.missing_field_kinds.is_empty() {
        lines.push("- JSON/schema drift observed => planner schema repair and descriptive-field defaulting will be hot.".to_string());
    }
    if metrics.empty_response_count > 0 {
        lines.push("- Empty responses observed => empty-response ladder and fresh-session retry will be hot.".to_string());
    }
    if metrics.malformed_tool_call_count > 0 {
        lines.push("- Malformed tool calls observed => argument recovery and recoverable tool feedback will be hot.".to_string());
    }
    if metrics.context_truncation_suspected_count > 0 {
        lines.push(
            "- Context truncation warning fired => review 98C token telemetry before UAT."
                .to_string(),
        );
    }
    lines
}

fn percent(value: f64) -> String {
    format!("{:.0}%", value * 100.0)
}

fn empty_if_missing(value: &str) -> &str {
    if value.is_empty() {
        "not_observed"
    } else {
        value
    }
}

fn option_u64(value: Option<u64>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "n/a".to_string())
}

fn render_map<T: std::fmt::Display>(map: &BTreeMap<String, T>) -> String {
    if map.is_empty() {
        return "none".to_string();
    }
    map.iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join(", ")
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

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use clap::Parser;
    use serde_json::json;

    use super::*;
    use crate::cli::Cli;
    use crate::tools::registry::ToolSpec;
    use std::sync::{Arc, Mutex};

    #[test]
    fn scripted_probe_run_computes_metrics_card_and_cleans_scratch() {
        let cwd = tempfile::tempdir().unwrap();
        let output = tempfile::tempdir().unwrap();
        let cwd_arg = cwd.path().to_string_lossy().into_owned();
        let mut config = Config::from_cli(Cli::parse_from([
            "anvilminimal",
            "--cwd",
            &cwd_arg,
            "--model",
            "fixture-executor",
            "--planner-model",
            "fixture-planner",
            "--model-probe",
        ]))
        .unwrap();
        config.chat_timeout_secs = 5;
        config.state_dir = output.path().join("state");

        let mut planner = ScriptedProbeClient::new("planner");
        let mut executor = ScriptedProbeClient::new("executor");
        let result = run_with_output_dir(
            &config,
            &mut planner,
            &mut executor,
            output.path().join("profiles"),
        )
        .unwrap();

        assert!(result.scratch_cleaned);
        assert!(!result.scratch_path.exists());
        assert!(result.profile_path.exists());
        assert!(result.card_path.exists());

        let report: ModelProbeReport =
            serde_json::from_str(&fs::read_to_string(&result.profile_path).unwrap()).unwrap();
        assert_eq!(report.version, MODEL_PROBE_VERSION);
        assert_eq!(report.metrics.task_count, 11);
        assert!(report.no_network_guarantee);
        assert_eq!(report.metrics.absolute_path_count, 1);
        assert!(report.metrics.absolute_path_rate > 0.0);
        assert_eq!(report.metrics.corrupted_path_count, 1);
        assert_eq!(report.metrics.shell_command_count, 3);
        assert_eq!(report.metrics.shell_control_count, 2);
        assert_eq!(report.metrics.shell_control_breakdown.and_and, 2);
        assert_eq!(report.metrics.shell_control_breakdown.pipe, 1);
        assert_eq!(report.metrics.shell_control_breakdown.redirect, 1);
        assert_eq!(report.metrics.shell_control_breakdown.cd, 1);
        assert_eq!(report.metrics.edit_anchor.salvageable, 1);
        assert_eq!(report.metrics.edit_anchor.miss, 2);
        assert_eq!(report.metrics.edit_anchor.exact, 1);
        assert_eq!(report.metrics.repair_follow_through.appended, "tool_error");
        assert_eq!(report.metrics.repair_follow_through.compact, "edited");
        assert_eq!(report.metrics.regeneration_follow_through, "edited");
        assert_eq!(report.metrics.json_response_count, 1);
        assert_eq!(report.metrics.json_valid_count, 1);
        assert_eq!(
            report
                .metrics
                .missing_field_kinds
                .get("descriptive:expected_result"),
            Some(&1)
        );
        assert_eq!(report.metrics.malformed_tool_call_count, 1);
        assert!(report.metrics.latency_ms.count >= 10);
        assert!(report.metrics.first_turn_latency_ms.count >= 10);
        assert!(report.metrics.token_telemetry.prompt_eval_count_total > 0);
        assert!(report.metrics.token_telemetry.eval_count_total > 0);
        assert_eq!(
            report.metrics.token_telemetry.finish_reasons.get("stop"),
            Some(&report.metrics.latency_ms.count)
        );

        let write_deep = report
            .tasks
            .iter()
            .find(|task| task.id == "write_deep")
            .unwrap();
        assert!(write_deep.raw_tool_calls.iter().any(|call| {
            call.arguments
                .get("path")
                .and_then(Value::as_str)
                .is_some_and(|path| path.starts_with('/'))
        }));
        assert!(write_deep.raw_tool_calls.iter().any(|call| {
            call.arguments
                .get("path")
                .and_then(Value::as_str)
                .is_some_and(|path| path.contains("workdir"))
        }));

        let edit_own = report
            .tasks
            .iter()
            .find(|task| task.id == "edit_own")
            .unwrap();
        assert_eq!(edit_own.raw_tool_calls.len(), 1);
        assert_eq!(edit_own.raw_tool_calls[0].name, "Edit");
        let csv_task = report
            .tasks
            .iter()
            .find(|task| task.id == "csv_fixture_verify")
            .unwrap();
        assert!(
            csv_task
                .raw_tool_calls
                .iter()
                .any(|call| call.name == "Write")
        );
        assert!(
            csv_task
                .raw_commands
                .iter()
                .any(|command| command.contains("__import__('csv').DictReader")),
            "{csv_task:?}"
        );

        let card = fs::read_to_string(&result.card_path).unwrap();
        for expected in [
            "# Model Probe Card",
            "- Scope: N=11 micro-tasks; dialect indicators, not a capability benchmark",
            "- No-network guarantee: passed",
            "- shell_control_breakdown: &&=2 ;=0 pipe=1 redirect=1 cd=1",
            "- latency_cache_note: first_turn_p50=",
            "- Compact-only repair follow-through => expect the 85 compact-session rung to matter.",
            "- JSON/schema drift observed => planner schema repair and descriptive-field defaulting will be hot.",
            "Probe results never auto-configure runtime behavior.",
        ] {
            assert!(card.contains(expected), "card missing {expected}\n{card}");
        }
    }

    #[derive(Clone)]
    struct ScriptedProbeClient {
        label: &'static str,
        calls: Arc<Mutex<Vec<String>>>,
    }

    impl ScriptedProbeClient {
        fn new(label: &'static str) -> Self {
            Self {
                label,
                calls: Arc::new(Mutex::new(Vec::new())),
            }
        }
    }

    impl ChatClient for ScriptedProbeClient {
        fn label(&self) -> &str {
            self.label
        }

        fn boxed_clone(&self) -> Box<dyn ChatClient> {
            Box::new(self.clone())
        }

        fn supports_native_tools(&self, _model: &str) -> bool {
            true
        }

        fn chat(
            &mut self,
            _model: &str,
            messages: &[ConversationMessage],
            _tools: &[ToolSpec],
            _native_tools_enabled: bool,
        ) -> anyhow::Result<AssistantReply> {
            let joined = messages
                .iter()
                .map(|message| message.content.as_str())
                .collect::<Vec<_>>()
                .join("\n");
            let task = task_id(&joined).unwrap_or("unknown");
            let mut calls = self.calls.lock().unwrap();
            let prior_same_task = calls
                .iter()
                .filter(|call| task_id(call).is_some_and(|prior| prior == task))
                .count();
            calls.push(joined.clone());
            let root = workspace_root_from_messages(messages);
            if prior_same_task > 0 && task != "verify_json" {
                return Ok(reply_text("no further tool action after probe feedback"));
            }
            Ok(match task {
                "write_simple" => reply_with_tools(vec![ToolCall::new(
                    "Write",
                    json!({
                        "path": "src/util/math.ts",
                        "content": "export function add(a: number, b: number) {\n  const total = a + b;\n  return total;\n}\n",
                    }),
                )]),
                "write_deep" => {
                    let deep =
                        "src/a/b/c/d/e/model-probe-long-file-name-with-many-segments-and-dashes.ts";
                    reply_with_tools(vec![
                        ToolCall::new(
                            "Write",
                            json!({
                                "path": root.join(deep).to_string_lossy(),
                                "content": "export const deepProbe = true;\n",
                            }),
                        ),
                        ToolCall::new("Write", json!({"path": "workdir/corrupted.ts"})),
                    ])
                }
                "edit_provided" => reply_with_tools(vec![ToolCall::new(
                    "Edit",
                    json!({
                        "path": "src/provided/edit-target.txt",
                        "old_string": "gamma   delta",
                        "new_string": "gamma epsilon",
                    }),
                )]),
                "edit_own" => reply_with_tools(vec![ToolCall::new(
                    "Edit",
                    json!({
                        "path": "src/util/math.ts",
                        "old_string": "export function add",
                        "new_string": "export function sum",
                    }),
                )]),
                "verify_exist" => reply_with_tools(vec![ToolCall::new(
                    "Bash",
                    json!({"command": "test -f src/util/math.ts && echo pass"}),
                )]),
                "verify_json" if prior_same_task == 0 => reply_with_tools(vec![ToolCall::new(
                    "Bash",
                    json!({"command": "cd . && node -p \"require('./package.json').scripts.build\" | cat > probe.txt"}),
                )]),
                "verify_json" => reply_text("verification attempt recorded after policy feedback"),
                "repair_appended" => reply_with_tools(vec![ToolCall::new(
                    "Edit",
                    json!({
                        "path": "src/repair/appended.ts",
                        "old_string": "return 0;",
                        "new_string": "return 1;",
                    }),
                )]),
                "repair_compact" => reply_with_tools(vec![
                    ToolCall::new(
                        "Edit",
                        json!({
                            "path": "src/repair/compact.ts",
                            "old_string": "return 2;",
                            "new_string": "return 1;",
                        }),
                    ),
                    ToolCall::new(
                        "Write",
                        json!({
                            "path": "src/repair/compact.ts",
                            "content": "export function value() {\n  return 1;\n}\n",
                        }),
                    ),
                ]),
                "regenerate" => reply_with_tools(vec![ToolCall::new(
                    "Write",
                    json!({
                        "path": "src/repair/regenerate.ts",
                        "content": "export function value() {\n  return 1;\n}\n",
                    }),
                )]),
                "csv_fixture_verify" => reply_with_tools(vec![
                    ToolCall::new(
                        "Write",
                        json!({
                            "path": "fixtures/model-probe.csv",
                            "content": "name,score\nalpha,1\nbeta,2\n",
                        }),
                    ),
                    ToolCall::new(
                        "Bash",
                        json!({"command": "python3 -c \"print(len(list(__import__('csv').DictReader(open('fixtures/model-probe.csv')))))\""}),
                    ),
                ]),
                "json_schema" => reply_text(
                    r#"{"steps":[{"instruction":"write math helper","kind":"implement","expected_paths":["src/util/math.ts"]}]}"#,
                ),
                other => anyhow::bail!("unhandled model-probe task {other}: {joined}"),
            })
        }
    }

    fn reply_with_tools(tool_calls: Vec<ToolCall>) -> AssistantReply {
        AssistantReply {
            content: String::new(),
            tool_calls,
            prompt_tokens: Some(120),
            completion_tokens: Some(20),
        }
    }

    fn reply_text(content: impl Into<String>) -> AssistantReply {
        AssistantReply {
            content: content.into(),
            tool_calls: Vec::new(),
            prompt_tokens: Some(90),
            completion_tokens: Some(12),
        }
    }

    fn task_id(text: &str) -> Option<&'static str> {
        let marker = "MODEL PROBE task ";
        let (_, after) = text.rsplit_once(marker)?;
        [
            "write_simple",
            "write_deep",
            "edit_provided",
            "edit_own",
            "verify_exist",
            "verify_json",
            "repair_appended",
            "repair_compact",
            "regenerate",
            "csv_fixture_verify",
            "json_schema",
        ]
        .into_iter()
        .find(|task| after.starts_with(task))
    }

    fn workspace_root_from_messages(messages: &[ConversationMessage]) -> PathBuf {
        messages
            .iter()
            .find_map(|message| {
                let (_, after) = message.content.split_once("Work only inside workspace `")?;
                let (path, _) = after.split_once('`')?;
                Some(PathBuf::from(path))
            })
            .unwrap_or_else(std::env::temp_dir)
    }
}
