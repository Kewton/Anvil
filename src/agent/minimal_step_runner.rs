use std::collections::BTreeSet;
use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::config::Config;
use crate::logging::log_llm_event;
use crate::provider_call::{self, ProviderCallScope};
use crate::session::store::{ConversationMessage, SessionSnapshot, SessionStore};
#[cfg(test)]
use crate::tools::registry::ToolSpec;

use super::minimal_loop::MinimalChatClient;
use super::minimal_repl::{run_turn_with_early_success_paths, run_turn_with_provider_scope};
use super::planner_llm::PlannerLlm;
use super::text_tokens;

mod intent;
mod plan_lint;
mod profile;
mod profiles;
mod repair;
mod verify;

use intent::{WorkIntent, detect_work_intent};
#[cfg(test)]
use plan_lint::lint_plan;
use plan_lint::{lint_plan_with_workspace, lint_ultra_plan};
#[cfg(test)]
use profile::{ProfileSnapshot, profile_snapshot, verify_profile_after_phase};
use profile::{
    build_profiled_phase_prompt, profile_generation_rules, profile_snapshot_for_ultra_plan,
    verify_profile_after_phase_with_requested_port,
};
#[cfg(test)]
use repair::REPAIR_REPLAN_PROMPT_MAX_CHARS;
use repair::{
    analyze_step_progress, build_repair_exhausted_report, build_repair_prompt,
    failed_step_stop_reason, repaired_step_stop_reason, verified_step_stop_reason,
    write_or_edit_paths_since,
};
#[cfg(test)]
use verify::VerificationReport;
use verify::{
    early_success_paths_for_step, missing_expected_paths, normalize_relative_path,
    validate_verify_command, verify_step,
};

const MAX_STEPS: usize = 12;
const MAX_GOAL_CHARS: usize = 4_000;
const MAX_STRING_CHARS: usize = 1_200;
const MAX_LIST_ITEMS: usize = 12;
const PLAN_GENERATION_ATTEMPTS: usize = 3;
const STEP_TURN_MAX_ITERATIONS: usize = 8;
const STEP_REPAIR_MAX_ITERATIONS: usize = 6;
const STEP_REPAIR_MAX_FILE_CHANGES: usize = 2;
const STEP_REPAIR_MAX_TURNS: usize = 4;
const MAX_ULTRA_PHASES: usize = 8;
const ULTRA_PLAN_GENERATION_ATTEMPTS: usize = 3;
const PLANNER_TIMEOUT_ATTEMPTS: usize = 2;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StepPlan {
    pub goal: String,
    pub steps: Vec<PlanStep>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanStep {
    pub id: String,
    #[serde(default)]
    pub kind: StepKind,
    pub instruction: String,
    #[serde(default)]
    pub expected_paths: Vec<String>,
    #[serde(default)]
    pub verify: Vec<String>,
    #[serde(default)]
    pub expected_result: VerifyExpectedResult,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StepKind {
    #[default]
    Work,
    Inspect,
    Create,
    Edit,
    Setup,
    Verify,
    Repair,
    Report,
}

impl StepKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Work => "work",
            Self::Inspect => "inspect",
            Self::Create => "create",
            Self::Edit => "edit",
            Self::Setup => "setup",
            Self::Verify => "verify",
            Self::Repair => "repair",
            Self::Report => "report",
        }
    }
}

impl fmt::Display for StepKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl FromStr for StepKind {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().replace('_', "-").as_str() {
            "work" | "task" => Ok(Self::Work),
            "inspect" | "investigate" | "read" => Ok(Self::Inspect),
            "create" | "scaffold" | "generate" => Ok(Self::Create),
            "edit" | "modify" | "update" => Ok(Self::Edit),
            "setup" | "install" | "prepare" => Ok(Self::Setup),
            "verify" | "validate" | "check" | "test" => Ok(Self::Verify),
            "repair" | "fix" => Ok(Self::Repair),
            "report" | "summarize" => Ok(Self::Report),
            other => Err(format!("unknown step kind: {other}")),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum VerifyExpectedResult {
    #[default]
    Pass,
    Fail,
}

impl VerifyExpectedResult {
    fn as_str(self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::Fail => "fail",
        }
    }
}

impl fmt::Display for VerifyExpectedResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl FromStr for VerifyExpectedResult {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().replace('_', "-").as_str() {
            "pass" | "passed" | "success" => Ok(Self::Pass),
            "fail" | "failed" | "failure" | "expected-failure" => Ok(Self::Fail),
            other => Err(format!("unknown expected_result: {other}")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UltraPlan {
    pub goal: String,
    #[serde(default)]
    pub profile: UltraProfile,
    #[serde(default)]
    pub style: UltraPlanStyle,
    #[serde(default)]
    pub intent: WorkIntent,
    pub phases: Vec<UltraPhase>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UltraPhase {
    pub id: String,
    pub prompt: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum UltraProfile {
    #[default]
    Generic,
    Nextjs,
    Python,
    Rust,
    Investigation,
    Docs,
    DataAnalysis,
    DataPipeline,
}

impl UltraProfile {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Generic => "generic",
            Self::Nextjs => "nextjs",
            Self::Python => "python",
            Self::Rust => "rust",
            Self::Investigation => "investigation",
            Self::Docs => "docs",
            Self::DataAnalysis => "data-analysis",
            Self::DataPipeline => "data-pipeline",
        }
    }

    pub fn inference_id(self) -> &'static str {
        match self {
            Self::Python => "python-cli",
            other => other.as_str(),
        }
    }
}

impl fmt::Display for UltraProfile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl FromStr for UltraProfile {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().replace('_', "-").as_str() {
            "generic" | "default" | "auto" => Ok(Self::Generic),
            "nextjs" | "next-js" | "next.js" => Ok(Self::Nextjs),
            "python" | "python-cli" | "py" => Ok(Self::Python),
            "rust" | "cargo" => Ok(Self::Rust),
            "investigation" | "triage" | "research" => Ok(Self::Investigation),
            "docs" | "documentation" => Ok(Self::Docs),
            "data-analysis" | "analysis" | "analytics" => Ok(Self::DataAnalysis),
            "data-pipeline" | "data" | "pipeline" | "etl" => Ok(Self::DataPipeline),
            other => Err(format!(
                "unknown ultra profile `{other}`; expected generic, nextjs, python, rust, investigation, docs, data-analysis, or data-pipeline"
            )),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileResolutionSource {
    Explicit,
    Goal,
    Workspace,
    Default,
}

impl ProfileResolutionSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Explicit => "explicit",
            Self::Goal => "goal",
            Self::Workspace => "workspace",
            Self::Default => "default",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProfileResolution {
    pub profile: UltraProfile,
    pub source: ProfileResolutionSource,
}

impl ProfileResolution {
    pub fn is_explicit(self) -> bool {
        self.source == ProfileResolutionSource::Explicit
    }

    pub fn is_inferred(self) -> bool {
        !self.is_explicit()
    }

    pub fn summary_line(self) -> Option<String> {
        self.is_inferred().then(|| {
            format!(
                "profile_inferred: {} (from: {})",
                self.profile.inference_id(),
                self.source.as_str()
            )
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssuranceLevel {
    Full,
    Reduced,
}

impl AssuranceLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Reduced => "reduced",
        }
    }

    pub fn status_label(self) -> &'static str {
        match self {
            Self::Full => "completed",
            Self::Reduced => "completed (reduced assurance)",
        }
    }

    pub fn summary_line(self) -> Option<&'static str> {
        match self {
            Self::Full => None,
            Self::Reduced => Some(
                "Assurance: reduced (generic profile — no capability contract, no behavioral verification)",
            ),
        }
    }
}

pub fn assurance_for_profile(profile: UltraProfile) -> AssuranceLevel {
    match profile {
        UltraProfile::Generic => AssuranceLevel::Reduced,
        _ => AssuranceLevel::Full,
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum UltraPlanStyle {
    #[default]
    Default,
    Tdd,
    TestHardening,
}

impl UltraPlanStyle {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Tdd => "tdd",
            Self::TestHardening => "test-hardening",
        }
    }
}

impl fmt::Display for UltraPlanStyle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl FromStr for UltraPlanStyle {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().replace('_', "-").as_str() {
            "default" | "auto" => Ok(Self::Default),
            "tdd" => Ok(Self::Tdd),
            "test-hardening" | "test" | "tests" | "hardening" => Ok(Self::TestHardening),
            other => Err(format!(
                "unknown ultra style `{other}`; expected default, tdd, or test-hardening"
            )),
        }
    }
}

impl UltraPlan {
    fn effective_intent(&self) -> WorkIntent {
        if self.intent == WorkIntent::Unknown {
            detect_work_intent(&self.goal)
        } else {
            self.intent
        }
    }
}

pub fn resolve_ultra_profile(
    work_root: &Path,
    goal: &str,
    explicit_profile: Option<&str>,
) -> Result<ProfileResolution, String> {
    if let Some(value) = explicit_profile {
        return Ok(ProfileResolution {
            profile: value.parse()?,
            source: ProfileResolutionSource::Explicit,
        });
    }
    if text_tokens::contains_nextjs_profile_token(goal) {
        return Ok(ProfileResolution {
            profile: UltraProfile::Nextjs,
            source: ProfileResolutionSource::Goal,
        });
    }
    if text_tokens::contains_python_cli_profile_token(goal) {
        return Ok(ProfileResolution {
            profile: UltraProfile::Python,
            source: ProfileResolutionSource::Goal,
        });
    }
    if workspace_has_next_dependency(work_root) {
        return Ok(ProfileResolution {
            profile: UltraProfile::Nextjs,
            source: ProfileResolutionSource::Workspace,
        });
    }
    if work_root.join("pyproject.toml").is_file() {
        return Ok(ProfileResolution {
            profile: UltraProfile::Python,
            source: ProfileResolutionSource::Workspace,
        });
    }
    Ok(ProfileResolution {
        profile: UltraProfile::Generic,
        source: ProfileResolutionSource::Default,
    })
}

pub fn log_profile_resolution(resolution: ProfileResolution, requested_port: Option<u16>) {
    if !resolution.is_inferred() {
        return;
    }
    log_llm_event(
        "profile_inferred",
        json!({
            "profile": resolution.profile.inference_id(),
            "from": resolution.source.as_str(),
            "requested_port": requested_port,
        }),
    );
}

fn workspace_has_next_dependency(work_root: &Path) -> bool {
    let Ok(raw) = std::fs::read_to_string(work_root.join("package.json")) else {
        return false;
    };
    package_json_has_dependency(&raw, "next")
}

fn package_json_has_dependency(raw: &str, name: &str) -> bool {
    let Ok(package) = serde_json::from_str::<serde_json::Value>(raw) else {
        return false;
    };
    ["dependencies", "devDependencies", "peerDependencies"]
        .into_iter()
        .any(|section| {
            package
                .get(section)
                .and_then(|value| value.get(name))
                .is_some()
        })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepRunSummary {
    pub total: usize,
    pub completed: usize,
    pub outcomes: Vec<StepOutcome>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepOutcome {
    pub id: String,
    pub stop_reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanRunSummary {
    pub plan_path: PathBuf,
    pub steps: StepRunSummary,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UltraRunSummary {
    pub total: usize,
    pub completed: usize,
    pub assurance_level: AssuranceLevel,
}

impl UltraRunSummary {
    pub fn status_label(&self) -> &'static str {
        self.assurance_level.status_label()
    }

    pub fn assurance_summary_line(&self) -> Option<&'static str> {
        self.assurance_level.summary_line()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UltraPlanRunSummary {
    pub plan_path: PathBuf,
    pub phases: UltraRunSummary,
}

pub fn generate_step_plan<P: PlannerLlm + ?Sized>(
    config: &Config,
    planner: &mut P,
    goal: &str,
) -> Result<PathBuf, String> {
    let mut messages = vec![
        ConversationMessage::system(plan_generation_system_prompt()),
        ConversationMessage::user(plan_generation_user_prompt(goal)),
    ];
    let mut last_error = String::new();
    for attempt in 0..PLAN_GENERATION_ATTEMPTS {
        let reply =
            chat_plan_with_timeout_retry(planner, ProviderCallScope::PlannerStep, &messages)?;
        if !reply.tool_calls.is_empty() {
            last_error = "plan generation must not emit tool calls".to_string();
        } else {
            match parse_plan_json(&reply.content).and_then(|mut plan| {
                plan.goal = goal.to_string();
                validate_plan(&plan)?;
                lint_plan_with_workspace(&plan, Some(&config.cwd))?;
                Ok(plan)
            }) {
                Ok(plan) => return save_plan(&config.cwd, &plan),
                Err(err) => last_error = err,
            }
        }
        if attempt + 1 < PLAN_GENERATION_ATTEMPTS {
            messages.push(ConversationMessage::user(format!(
                "The previous plan was invalid: {last_error}\nReturn corrected JSON only. Use kind to keep responsibilities separate: inspect reads, create/edit writes artifacts, setup prepares dependencies or local environment, verify runs deterministic checks, repair fixes verifier failures, report records an unfixable blocker. Step instructions must be natural-language tasks, not shell commands. expected_paths are artifacts that should exist after the step, not files to inspect. Inspection-only steps should usually have empty expected_paths. If a creation/edit step has multiple expected_paths, name concrete files or component names in the instruction. A validation-only step may list multiple expected_paths only when those paths were created by earlier steps in this plan or already exist in the workspace. All expected_paths must be repository-relative paths with no leading slash and no '..'. The verify array is only for deterministic checks; never put npm install, create-next-app, dev servers, network commands, or setup commands in verify. If dependency setup is required before verification, create a separate kind:\"setup\" step before the kind:\"verify\" step. If setup is not allowed or cannot run, use kind:\"report\" and do not claim success. Do not use node --check for .ts or .tsx files."
            )));
        }
    }
    Err(format!("invalid generated step plan: {last_error}"))
}

pub fn run_plan<C: MinimalChatClient>(
    config: &Config,
    model: &str,
    client: &mut C,
    session_store: &SessionStore,
    session: &mut SessionSnapshot,
    plan_path: &Path,
) -> Result<StepRunSummary, String> {
    let plan = load_plan(config, plan_path)?;
    validate_plan(&plan)?;
    lint_plan_with_workspace(&plan, Some(&config.cwd))?;
    let step_config = capped_config(config, STEP_TURN_MAX_ITERATIONS);
    let repair_config = capped_config(config, STEP_REPAIR_MAX_ITERATIONS);

    let mut completed = 0usize;
    let mut outcomes = Vec::new();
    for (index, step) in plan.steps.iter().enumerate() {
        println!(
            "step {}/{} {}: running",
            index + 1,
            plan.steps.len(),
            step.id
        );
        let missing_before = missing_expected_paths(&config.cwd, step);
        let turn_start = session.messages.len();
        let prompt = build_step_prompt(&plan, step);
        let early_success_paths = early_success_paths_for_step(&config.cwd, step);
        let turn_result = run_turn_with_early_success_paths(
            &step_config,
            model,
            client,
            session_store,
            session,
            &prompt,
            early_success_paths,
        );
        let turn_error = match turn_result {
            Ok(reply) => {
                if !reply.is_empty() {
                    println!("{reply}");
                }
                None
            }
            Err(err) => {
                println!("step {}: turn stopped before completion: {err}", step.id);
                Some(err)
            }
        };

        let mut report = verify_step(&config.cwd, step);
        if report.success {
            completed += 1;
            let stop_reason = verified_step_stop_reason(turn_error.as_deref());
            println!("step {}: stop_reason={stop_reason}", step.id);
            outcomes.push(StepOutcome {
                id: step.id.clone(),
                stop_reason,
            });
            println!("step {}: ok", step.id);
            continue;
        }

        println!("step {}: verification failed; running repair", step.id);
        let mut repair_error = None;
        let mut file_change_repairs = 0usize;
        for repair_turn in 1..=STEP_REPAIR_MAX_TURNS {
            if file_change_repairs >= STEP_REPAIR_MAX_FILE_CHANGES {
                break;
            }
            let progress =
                analyze_step_progress(session, turn_start, missing_before.clone(), &report);
            let repair_cycle = file_change_repairs + 1;
            let repair_prompt = build_repair_prompt(
                &plan,
                step,
                &report,
                &progress,
                turn_error.as_deref(),
                repair_cycle,
                STEP_REPAIR_MAX_FILE_CHANGES,
            );
            let repair_start = session.messages.len();
            let early_success_paths = early_success_paths_for_step(&config.cwd, step);
            let repair_result = run_turn_with_provider_scope(
                &repair_config,
                model,
                client,
                session_store,
                session,
                &repair_prompt,
                early_success_paths,
                ProviderCallScope::Repair,
            );
            repair_error = match repair_result {
                Ok(reply) => {
                    if !reply.is_empty() {
                        println!("{reply}");
                    }
                    None
                }
                Err(err) => {
                    println!(
                        "step {}: repair cycle {repair_cycle} stopped before completion: {err}",
                        step.id
                    );
                    Some(err)
                }
            };
            let changed_files = !write_or_edit_paths_since(session, repair_start).is_empty();
            if changed_files {
                file_change_repairs += 1;
            }
            report = verify_step(&config.cwd, step);
            if report.success {
                break;
            }
            if repair_turn < STEP_REPAIR_MAX_TURNS
                && file_change_repairs < STEP_REPAIR_MAX_FILE_CHANGES
            {
                println!(
                    "step {}: verification still failing after repair turn {repair_turn}",
                    step.id
                );
            }
        }

        if !report.success {
            let stop_reason =
                failed_step_stop_reason(turn_error.as_deref(), repair_error.as_deref());
            println!("step {}: stop_reason={stop_reason}", step.id);
            let progress =
                analyze_step_progress(session, turn_start, missing_before.clone(), &report);
            let exhausted_report = build_repair_exhausted_report(
                &config.cwd,
                &plan,
                step,
                &report,
                &progress,
                turn_error.as_deref(),
                repair_error.as_deref(),
                file_change_repairs,
                STEP_REPAIR_MAX_FILE_CHANGES,
            );
            println!("{exhausted_report}");
            return Err(format!(
                "step {} failed verification: {}\n\n{}",
                step.id,
                report.failures.join("; "),
                exhausted_report
            ));
        }
        completed += 1;
        let stop_reason = repaired_step_stop_reason(turn_error.as_deref(), repair_error.as_deref());
        println!("step {}: stop_reason={stop_reason}", step.id);
        outcomes.push(StepOutcome {
            id: step.id.clone(),
            stop_reason,
        });
        println!("step {}: ok", step.id);
    }

    Ok(StepRunSummary {
        total: plan.steps.len(),
        completed,
        outcomes,
    })
}

pub fn generate_and_run_step_plan<P: PlannerLlm + ?Sized, C: MinimalChatClient>(
    config: &Config,
    planner: &mut P,
    model: &str,
    client: &mut C,
    session_store: &SessionStore,
    session: &mut SessionSnapshot,
    goal: &str,
) -> Result<PlanRunSummary, String> {
    let plan_path = generate_step_plan(config, planner, goal)?;
    let steps = run_plan(config, model, client, session_store, session, &plan_path)?;
    Ok(PlanRunSummary { plan_path, steps })
}

pub fn generate_ultra_plan<P: PlannerLlm + ?Sized>(
    config: &Config,
    planner: &mut P,
    goal: &str,
    profile: UltraProfile,
    style: UltraPlanStyle,
) -> Result<PathBuf, String> {
    let intent = detect_work_intent(goal);
    let mut messages = vec![
        ConversationMessage::system(ultra_plan_generation_system_prompt(profile, style, intent)),
        ConversationMessage::user(ultra_plan_generation_user_prompt(
            goal, profile, style, intent,
        )),
    ];
    let mut last_error = String::new();
    for attempt in 0..ULTRA_PLAN_GENERATION_ATTEMPTS {
        let reply =
            chat_plan_with_timeout_retry(planner, ProviderCallScope::PlannerUltra, &messages)?;
        if !reply.tool_calls.is_empty() {
            last_error = "ultra plan generation must not emit tool calls".to_string();
        } else {
            match parse_ultra_plan_json(&reply.content).and_then(|mut plan| {
                plan.goal = goal.to_string();
                plan.profile = profile;
                plan.style = style;
                plan.intent = detect_work_intent(goal);
                validate_ultra_plan(&plan)?;
                lint_ultra_plan(&plan)?;
                Ok(plan)
            }) {
                Ok(plan) => {
                    let requested_port = requested_port_for_ultra_plan(&plan);
                    log_llm_event(
                        "agent.minimal.ultra_plan.generated",
                        json!({
                            "profile": profile.inference_id(),
                            "style": style.as_str(),
                            "requested_port": requested_port,
                        }),
                    );
                    return save_ultra_plan(&config.cwd, &plan);
                }
                Err(err) => last_error = err,
            }
        }
        if attempt + 1 < ULTRA_PLAN_GENERATION_ATTEMPTS {
            messages.push(ConversationMessage::user(format!(
                "The previous ultra plan was invalid: {last_error}\nReturn corrected JSON only. Each phase prompt must be a focused natural-language task for /plan-run, not a shell command. Keep 2 to 8 phases. If the goal asks for TDD, include a red-test phase whose prompt explicitly asks to confirm a failing test before implementation."
            )));
        }
    }
    Err(format!("invalid generated ultra plan: {last_error}"))
}

pub fn run_ultra_plan<P: PlannerLlm + ?Sized, C: MinimalChatClient>(
    config: &Config,
    planner: &mut P,
    model: &str,
    client: &mut C,
    session_store: &SessionStore,
    session: &mut SessionSnapshot,
    ultra_plan_path: &Path,
) -> Result<UltraRunSummary, String> {
    let ultra_plan = load_ultra_plan(config, ultra_plan_path)?;
    validate_ultra_plan(&ultra_plan)?;
    lint_ultra_plan(&ultra_plan)?;
    let intent = ultra_plan.effective_intent();
    let requested_port = requested_port_for_ultra_plan(&ultra_plan);
    let assurance_level = assurance_for_profile(ultra_plan.profile);

    let mut completed = 0usize;
    for (index, phase) in ultra_plan.phases.iter().enumerate() {
        println!(
            "phase {}/{} {}: planning and running",
            index + 1,
            ultra_plan.phases.len(),
            phase.id
        );
        let snapshot = profile_snapshot_for_ultra_plan(&config.cwd, &ultra_plan);
        log_llm_event(
            "agent.minimal.ultra_phase.start",
            json!({
                "phase_id": phase.id,
                "profile": ultra_plan.profile.inference_id(),
                "requested_port": requested_port,
                "probe_port": snapshot.probe_port,
            }),
        );
        let phase_prompt = build_profiled_phase_prompt(&ultra_plan, phase, &snapshot, intent);
        let summary = generate_and_run_step_plan(
            config,
            planner,
            model,
            client,
            session_store,
            session,
            &phase_prompt,
        )
        .map_err(|err| format!("phase {} failed: {err}", phase.id))?;
        verify_profile_after_phase_with_requested_port(
            &config.cwd,
            ultra_plan.profile,
            intent,
            &snapshot,
            requested_port,
        )
        .map_err(|err| format!("phase {} failed profile verification: {err}", phase.id))?;
        completed += 1;
        println!(
            "phase {}: ok (step plan: {}, completed {}/{})",
            phase.id,
            summary.plan_path.display(),
            summary.steps.completed,
            summary.steps.total
        );
    }

    if assurance_level == AssuranceLevel::Reduced {
        log_llm_event(
            "tui_command_stop",
            json!({
                "status": assurance_level.status_label(),
                "assurance_level": assurance_level.as_str(),
                "profile": ultra_plan.profile.inference_id(),
                "completed": completed,
                "total": ultra_plan.phases.len(),
            }),
        );
    }

    Ok(UltraRunSummary {
        total: ultra_plan.phases.len(),
        completed,
        assurance_level,
    })
}

#[allow(clippy::too_many_arguments)]
pub fn generate_and_run_ultra_plan<P: PlannerLlm + ?Sized, C: MinimalChatClient>(
    config: &Config,
    planner: &mut P,
    model: &str,
    client: &mut C,
    session_store: &SessionStore,
    session: &mut SessionSnapshot,
    goal: &str,
    profile: UltraProfile,
    style: UltraPlanStyle,
) -> Result<UltraPlanRunSummary, String> {
    let plan_path = generate_ultra_plan(config, planner, goal, profile, style)?;
    let phases = run_ultra_plan(
        config,
        planner,
        model,
        client,
        session_store,
        session,
        &plan_path,
    )?;
    Ok(UltraPlanRunSummary { plan_path, phases })
}

fn chat_plan_with_timeout_retry<P: PlannerLlm + ?Sized>(
    planner: &mut P,
    scope: ProviderCallScope,
    messages: &[ConversationMessage],
) -> Result<crate::ollama::client::AssistantReply, String> {
    let mut last_timeout: Option<String> = None;
    for attempt in 1..=PLANNER_TIMEOUT_ATTEMPTS {
        match planner.chat_plan(scope, messages) {
            Ok(reply) => return Ok(reply),
            Err(err) if provider_call::is_scoped_planner_timeout(scope, &err) => {
                log_llm_event(
                    "agent.minimal.planner_timeout",
                    json!({
                        "caller_scope": scope.as_str(),
                        "attempt": attempt,
                        "max_attempts": PLANNER_TIMEOUT_ATTEMPTS,
                        "timeout_kind": scope.planner_timeout_kind(),
                        "retrying": attempt < PLANNER_TIMEOUT_ATTEMPTS,
                        "error": err,
                    }),
                );
                if attempt == PLANNER_TIMEOUT_ATTEMPTS {
                    return Err(err);
                }
                last_timeout = Some(err);
            }
            Err(err) => return Err(err),
        }
    }
    Err(last_timeout.unwrap_or_else(|| "planner failed".to_string()))
}

fn plan_generation_system_prompt() -> String {
    "You are Anvil in Plan mode. You do not execute tools. Produce a step plan for a local coding agent.\n\
Output JSON only, with this exact shape:\n\
{\"goal\":\"...\",\"steps\":[{\"id\":\"kebab-id\",\"kind\":\"create\",\"instruction\":\"...\",\"expected_paths\":[\"relative/path\"],\"verify\":[\"npm run build\"],\"expected_result\":\"pass\"}]}\n\
Rules:\n\
- Split large tasks into small sequential steps.\n\
- Each step must be executable by a minimal local coding agent in one turn.\n\
- Use kind to separate responsibility: inspect, create, edit, setup, verify, repair, or report.\n\
- setup steps prepare dependencies or local environment. Put setup commands in the setup instruction, never in verify.\n\
- verify steps only run deterministic checks. They should not create, edit, install, scaffold, or repair.\n\
- report steps are for explicit blockers such as dependency_missing when setup is not allowed or cannot run; report is not success.\n\
- Step instruction must be natural language, not a shell command, and must name the concrete files it will create or edit.\n\
- expected_paths are artifacts that should exist after the step. They are not a list of files to inspect.\n\
- Inspection-only steps should usually have empty expected_paths unless they verify files that already exist.\n\
- If a step has multiple expected_paths, mention those files or component names in the instruction.\n\
- Validation-only steps may list multiple expected_paths without naming each one only when those paths were created by earlier steps or already exist in the workspace.\n\
- If the goal contains a Required final artifacts list, keep those exact repository-relative paths and include relevant paths in expected_paths. Do not rename or relocate them.\n\
- Put deterministic validation in expected_paths and verify.\n\
- expected_paths must be repository-relative paths with no leading slash and no '..'.\n\
- Use only safe local verify commands such as npm run build, npm test, cargo check, cargo test, python -m py_compile <file>, pytest, cat <file>, or node --check <js-file>.\n\
- The verify array is only for checks. Never include npm install, create-next-app, dev servers, package installation, or network/setup commands in verify.\n\
- If a later verify step needs dependencies installed, add a prior setup step that explicitly installs or prepares them.\n\
- Do not put npm run build on early scaffold/config steps. Use cat early, use node --check only for .js/.mjs/.cjs files, and place npm run build only after the app entry point exists.\n\
- For Next.js apps, include app/page.tsx or pages/index.tsx before the first npm run build. Config-only steps should create package.json, next.config.js, tailwind.config.js, postcss.config.js, and tsconfig.json without build verification.\n\
- Do not include long-running dev servers in verify.\n\
- Avoid network scaffolding unless the user explicitly requires it.\n\
- expected_result defaults to pass. Use expected_result:\"fail\" only for an explicit TDD red step where the verification command should fail before implementation.\n\
- Return 2 to 8 steps for most tasks, never more than 12."
        .to_string()
}

fn ultra_plan_generation_system_prompt(
    profile: UltraProfile,
    style: UltraPlanStyle,
    intent: WorkIntent,
) -> String {
    let style_rules = match style {
        UltraPlanStyle::Default => {
            "- Use ordinary phased delivery unless the user explicitly asks for TDD or test hardening.\n"
        }
        UltraPlanStyle::Tdd => {
            "- Use TDD phases: inspect existing structure, add a focused failing test, implement the minimal fix, run focused tests, then run broader tests/refactor.\n- The failing-test phase prompt must explicitly ask the step planner to use expected_result:\"fail\" for the red verification step.\n"
        }
        UltraPlanStyle::TestHardening => {
            "- Use test-hardening phases: inspect existing tests, identify uncovered behavior, add focused tests, make only necessary fixes, then run broader tests.\n"
        }
    };
    let profile_rules = profile_generation_rules(profile, intent);
    format!(
        "You are Anvil's ultra planner. You do not execute tools. Produce a top-level phase plan whose phases will each be executed by /plan-run.\n\
Output JSON only, with this exact shape:\n\
{{\"goal\":\"...\",\"profile\":\"{}\",\"style\":\"{}\",\"intent\":\"{}\",\"phases\":[{{\"id\":\"kebab-id\",\"prompt\":\"focused /plan-run goal\"}}]}}\n\
Rules:\n\
- Return 2 to 6 phases for most tasks, never more than 8.\n\
- Each phase prompt must be a focused natural-language task that can be handled by one /plan-run.\n\
- Phase prompts should name the concrete outcome and the verification expectation when practical.\n\
- If the user goal contains a Required final artifacts list, preserve those exact repository-relative paths across phases. Do not rename or relocate them.\n\
- Do not make a phase prompt a shell command.\n\
- Do not include long-running dev servers, network setup, or package installation as a phase unless the user explicitly requires it.\n\
- Stop at a clean final verification/cleanup phase.\n\
{profile_rules}\
{style_rules}",
        profile.as_str(),
        style.as_str(),
        intent.as_str()
    )
}

fn ultra_plan_generation_user_prompt(
    goal: &str,
    profile: UltraProfile,
    style: UltraPlanStyle,
    intent: WorkIntent,
) -> String {
    format!(
        "Create an ultra phase plan for this task using profile `{}`, style `{}`, and work intent `{}`:\n{goal}",
        profile.as_str(),
        style.as_str(),
        intent.as_str()
    )
}

pub fn requested_port_for_goal(goal: &str) -> Option<u16> {
    text_tokens::requested_port(goal)
}

fn requested_port_for_ultra_plan(plan: &UltraPlan) -> Option<u16> {
    text_tokens::requested_port_from_texts(
        std::iter::once(plan.goal.as_str())
            .chain(plan.phases.iter().map(|phase| phase.prompt.as_str())),
    )
}

fn plan_generation_user_prompt(goal: &str) -> String {
    format!("Create a step plan for this task:\n{goal}")
}

fn build_step_prompt(plan: &StepPlan, step: &PlanStep) -> String {
    let required_artifacts = required_artifact_contract_prompt(&plan.goal);
    format!(
        "Overall goal:\n{goal}\n\n{required_artifacts}Current step id: {id}\nCurrent step instruction:\n{instruction}\n\nExpected paths after this step:\n{paths}\n\nVerification commands for this step:\n{verify}\n\nExpected verification result: {expected_result}\n\nWork only on this step. Use Write/Edit for file changes. Do not rely on network scaffolding unless this step explicitly says so. Create every expected path before revising an already-created file repeatedly. Before giving a final answer, make the expected paths exist and run the verification commands when possible. If expected_result is pass and verification fails, fix only this step's failure. If expected_result is fail, this is a TDD red step: the verification command should fail after the test is written.",
        goal = plan.goal,
        required_artifacts = required_artifacts,
        id = step.id,
        instruction = step.instruction,
        paths = bullet_list(&step.expected_paths),
        verify = bullet_list(&step.verify),
        expected_result = step.expected_result,
    )
}

fn bullet_list(items: &[String]) -> String {
    if items.is_empty() {
        return "- none".to_string();
    }
    items
        .iter()
        .map(|item| format!("- {item}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn inline_list(items: &[String]) -> String {
    if items.is_empty() {
        "none".to_string()
    } else {
        items.join(", ")
    }
}

fn required_artifact_contract_prompt(goal: &str) -> String {
    let artifacts = extract_required_artifacts(goal);
    if artifacts.is_empty() {
        return String::new();
    }
    format!(
        "Required final artifacts from the overall goal:\n{}\nDo not rename or relocate these repository-relative paths. Carry relevant paths into phase step plans and verify they exist before final completion.\n\n",
        bullet_list(&artifacts)
    )
}

fn extract_required_artifacts(text: &str) -> Vec<String> {
    let mut artifacts = Vec::new();
    let mut in_block = false;
    for line in text.lines() {
        let trimmed = line.trim();
        let lower = trimmed.to_ascii_lowercase();
        if lower.starts_with("required final artifacts")
            || lower.starts_with("required artifact paths")
        {
            in_block = true;
            continue;
        }
        if !in_block {
            continue;
        }
        if trimmed.is_empty() {
            if artifacts.is_empty() {
                continue;
            }
            break;
        }
        let Some(path) = trimmed.strip_prefix("- ") else {
            if artifacts.is_empty() {
                continue;
            }
            break;
        };
        if is_valid_required_artifact_path(path) {
            artifacts.push(path.to_string());
        }
    }
    artifacts
}

fn is_valid_required_artifact_path(path: &str) -> bool {
    let path = path.trim();
    !path.is_empty()
        && !path.starts_with('/')
        && !path.contains("..")
        && normalize_relative_path(Path::new(path)).is_some()
}

fn save_plan(work_root: &Path, plan: &StepPlan) -> Result<PathBuf, String> {
    let dir = work_root.join(".anvil").join("plans");
    std::fs::create_dir_all(&dir)
        .map_err(|err| format!("failed to create {}: {err}", dir.display()))?;
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|err| format!("system time before unix epoch: {err}"))?
        .as_secs();
    let file_name = format!("plan-{ts}-{}.yaml", slug(&plan.goal));
    let path = dir.join(file_name);
    std::fs::write(&path, render_plan_yaml(plan))
        .map_err(|err| format!("failed to write {}: {err}", path.display()))?;
    Ok(path)
}

fn save_ultra_plan(work_root: &Path, plan: &UltraPlan) -> Result<PathBuf, String> {
    let dir = work_root.join(".anvil").join("plans");
    std::fs::create_dir_all(&dir)
        .map_err(|err| format!("failed to create {}: {err}", dir.display()))?;
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|err| format!("system time before unix epoch: {err}"))?
        .as_secs();
    let file_name = format!("ultra-plan-{ts}-{}.yaml", slug(&plan.goal));
    let path = dir.join(file_name);
    std::fs::write(&path, render_ultra_plan_yaml(plan))
        .map_err(|err| format!("failed to write {}: {err}", path.display()))?;
    Ok(path)
}

fn load_plan(config: &Config, path: &Path) -> Result<StepPlan, String> {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        config.cwd.join(path)
    };
    let content = std::fs::read_to_string(&path)
        .map_err(|err| format!("failed to read plan {}: {err}", path.display()))?;
    parse_plan_yaml(&content).map_err(|err| format!("{}: {err}", path.display()))
}

fn load_ultra_plan(config: &Config, path: &Path) -> Result<UltraPlan, String> {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        config.cwd.join(path)
    };
    let content = std::fs::read_to_string(&path)
        .map_err(|err| format!("failed to read ultra plan {}: {err}", path.display()))?;
    parse_ultra_plan_yaml(&content).map_err(|err| format!("{}: {err}", path.display()))
}

fn parse_plan_json(raw: &str) -> Result<StepPlan, String> {
    let json =
        extract_json_object(raw).ok_or_else(|| "plan response did not contain JSON".to_string())?;
    serde_json::from_str::<StepPlan>(&json).map_err(|err| format!("invalid plan JSON: {err}"))
}

fn parse_ultra_plan_json(raw: &str) -> Result<UltraPlan, String> {
    let json = extract_json_object(raw)
        .ok_or_else(|| "ultra plan response did not contain JSON".to_string())?;
    serde_json::from_str::<UltraPlan>(&json)
        .map_err(|err| format!("invalid ultra plan JSON: {err}"))
}

fn extract_json_object(raw: &str) -> Option<String> {
    let start = raw.find('{')?;
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escaped = false;
    for (offset, ch) in raw[start..].char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        match ch {
            '"' => in_string = true,
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(raw[start..start + offset + ch.len_utf8()].to_string());
                }
            }
            _ => {}
        }
    }
    None
}

fn render_plan_yaml(plan: &StepPlan) -> String {
    let mut out = String::new();
    out.push_str("# Generated by anvil minimal step planner. Edit before /run-plan if needed.\n");
    out.push_str("goal: ");
    out.push_str(&quote_yaml(&plan.goal));
    out.push('\n');
    out.push_str("steps:\n");
    for step in &plan.steps {
        out.push_str("  - id: ");
        out.push_str(&quote_yaml(&step.id));
        out.push('\n');
        out.push_str("    kind: ");
        out.push_str(&quote_yaml(step.kind.as_str()));
        out.push('\n');
        out.push_str("    instruction: ");
        out.push_str(&quote_yaml(&step.instruction));
        out.push('\n');
        if step.expected_result != VerifyExpectedResult::Pass {
            out.push_str("    expected_result: ");
            out.push_str(&quote_yaml(step.expected_result.as_str()));
            out.push('\n');
        }
        out.push_str("    expected_paths:\n");
        for path in &step.expected_paths {
            out.push_str("      - ");
            out.push_str(&quote_yaml(path));
            out.push('\n');
        }
        out.push_str("    verify:\n");
        for command in &step.verify {
            out.push_str("      - ");
            out.push_str(&quote_yaml(command));
            out.push('\n');
        }
    }
    out
}

fn render_ultra_plan_yaml(plan: &UltraPlan) -> String {
    let mut out = String::new();
    out.push_str("# Generated by anvil ultra planner. Edit before /run-ultra-plan if needed.\n");
    out.push_str("goal: ");
    out.push_str(&quote_yaml(&plan.goal));
    out.push('\n');
    out.push_str("profile: ");
    out.push_str(&quote_yaml(plan.profile.as_str()));
    out.push('\n');
    out.push_str("style: ");
    out.push_str(&quote_yaml(plan.style.as_str()));
    out.push('\n');
    out.push_str("intent: ");
    out.push_str(&quote_yaml(plan.effective_intent().as_str()));
    out.push('\n');
    out.push_str("phases:\n");
    for phase in &plan.phases {
        out.push_str("  - id: ");
        out.push_str(&quote_yaml(&phase.id));
        out.push('\n');
        out.push_str("    prompt: ");
        out.push_str(&quote_yaml(&phase.prompt));
        out.push('\n');
    }
    out
}

fn parse_plan_yaml(raw: &str) -> Result<StepPlan, String> {
    let mut goal: Option<String> = None;
    let mut steps: Vec<PlanStep> = Vec::new();
    let mut current: Option<PlanStep> = None;
    enum ListKind {
        None,
        ExpectedPaths,
        Verify,
    }
    let mut list_kind = ListKind::None;

    for raw_line in raw.lines() {
        let line = raw_line.trim_end();
        if line.trim().is_empty() || line.trim_start().starts_with('#') || line == "steps:" {
            continue;
        }
        if let Some(rest) = line.strip_prefix("goal: ") {
            goal = Some(parse_quoted(rest)?);
            continue;
        }
        if let Some(rest) = line.strip_prefix("  - id: ") {
            if let Some(step) = current.take() {
                steps.push(step);
            }
            current = Some(PlanStep {
                id: parse_quoted(rest)?,
                kind: StepKind::Work,
                instruction: String::new(),
                expected_paths: Vec::new(),
                verify: Vec::new(),
                expected_result: VerifyExpectedResult::Pass,
            });
            list_kind = ListKind::None;
            continue;
        }
        if let Some(rest) = line.strip_prefix("    kind: ") {
            let step = current
                .as_mut()
                .ok_or_else(|| "kind before step id".to_string())?;
            step.kind = parse_quoted(rest)?.parse()?;
            list_kind = ListKind::None;
            continue;
        }
        if let Some(rest) = line.strip_prefix("    instruction: ") {
            let step = current
                .as_mut()
                .ok_or_else(|| "instruction before step id".to_string())?;
            step.instruction = parse_quoted(rest)?;
            list_kind = ListKind::None;
            continue;
        }
        if let Some(rest) = line.strip_prefix("    expected_result: ") {
            let step = current
                .as_mut()
                .ok_or_else(|| "expected_result before step id".to_string())?;
            step.expected_result = parse_quoted(rest)?.parse()?;
            list_kind = ListKind::None;
            continue;
        }
        if line == "    expected_paths:" {
            list_kind = ListKind::ExpectedPaths;
            continue;
        }
        if line == "    verify:" {
            list_kind = ListKind::Verify;
            continue;
        }
        if let Some(rest) = line.strip_prefix("      - ") {
            let step = current
                .as_mut()
                .ok_or_else(|| "list item before step id".to_string())?;
            match list_kind {
                ListKind::ExpectedPaths => step.expected_paths.push(parse_quoted(rest)?),
                ListKind::Verify => step.verify.push(parse_quoted(rest)?),
                ListKind::None => return Err("list item outside expected_paths/verify".into()),
            }
            continue;
        }
        return Err(format!("unsupported plan line: {line}"));
    }
    if let Some(step) = current.take() {
        steps.push(step);
    }
    let plan = StepPlan {
        goal: goal.ok_or_else(|| "missing goal".to_string())?,
        steps,
    };
    validate_plan(&plan)?;
    Ok(plan)
}

fn parse_ultra_plan_yaml(raw: &str) -> Result<UltraPlan, String> {
    let mut goal: Option<String> = None;
    let mut profile = UltraProfile::Generic;
    let mut style = UltraPlanStyle::Default;
    let mut intent = WorkIntent::Unknown;
    let mut phases: Vec<UltraPhase> = Vec::new();
    let mut current: Option<UltraPhase> = None;

    for raw_line in raw.lines() {
        let line = raw_line.trim_end();
        if line.trim().is_empty() || line.trim_start().starts_with('#') || line == "phases:" {
            continue;
        }
        if let Some(rest) = line.strip_prefix("goal: ") {
            goal = Some(parse_quoted(rest)?);
            continue;
        }
        if let Some(rest) = line.strip_prefix("profile: ") {
            profile = parse_quoted(rest)?.parse()?;
            continue;
        }
        if let Some(rest) = line.strip_prefix("style: ") {
            style = parse_quoted(rest)?.parse()?;
            continue;
        }
        if let Some(rest) = line.strip_prefix("intent: ") {
            intent = parse_quoted(rest)?.parse()?;
            continue;
        }
        if let Some(rest) = line.strip_prefix("  - id: ") {
            if let Some(phase) = current.take() {
                phases.push(phase);
            }
            current = Some(UltraPhase {
                id: parse_quoted(rest)?,
                prompt: String::new(),
            });
            continue;
        }
        if let Some(rest) = line.strip_prefix("    prompt: ") {
            let phase = current
                .as_mut()
                .ok_or_else(|| "prompt before phase id".to_string())?;
            phase.prompt = parse_quoted(rest)?;
            continue;
        }
        return Err(format!("unsupported ultra plan line: {line}"));
    }
    if let Some(phase) = current.take() {
        phases.push(phase);
    }
    let plan = UltraPlan {
        goal: goal.ok_or_else(|| "missing goal".to_string())?,
        profile,
        style,
        intent,
        phases,
    };
    validate_ultra_plan(&plan)?;
    Ok(plan)
}

fn quote_yaml(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "\"\"".to_string())
}

fn parse_quoted(value: &str) -> Result<String, String> {
    serde_json::from_str::<String>(value)
        .map_err(|err| format!("invalid quoted value {value:?}: {err}"))
}

fn validate_plan(plan: &StepPlan) -> Result<(), String> {
    validate_goal(&plan.goal)?;
    if plan.steps.is_empty() {
        return Err("plan must contain at least one step".to_string());
    }
    if plan.steps.len() > MAX_STEPS {
        return Err(format!("plan has too many steps: {}", plan.steps.len()));
    }
    let mut ids = BTreeSet::new();
    for step in &plan.steps {
        validate_step_id(&step.id)?;
        if !ids.insert(step.id.clone()) {
            return Err(format!("duplicate step id: {}", step.id));
        }
        validate_instruction(&step.instruction)?;
        if step.expected_paths.len() > MAX_LIST_ITEMS {
            return Err(format!("step {} has too many expected paths", step.id));
        }
        if step.verify.len() > MAX_LIST_ITEMS {
            return Err(format!("step {} has too many verify commands", step.id));
        }
        for path in &step.expected_paths {
            validate_relative_path(path)?;
        }
        for command in &step.verify {
            validate_verify_command(command)?;
        }
        validate_step_kind_contract(step)?;
        if step.expected_result == VerifyExpectedResult::Fail && step.verify.is_empty() {
            return Err(format!(
                "step {} expected_result fail requires at least one verify command",
                step.id
            ));
        }
    }
    Ok(())
}

fn validate_step_kind_contract(step: &PlanStep) -> Result<(), String> {
    match step.kind {
        StepKind::Inspect => {
            if !step.expected_paths.is_empty() && step.verify.is_empty() {
                return Err(format!(
                    "inspect step {} must not use expected_paths as files to inspect",
                    step.id
                ));
            }
        }
        StepKind::Setup => {
            if step
                .verify
                .iter()
                .any(|command| is_build_or_test_verify(command))
            {
                return Err(format!(
                    "setup step {} must not run build/test verification; add a later verify step",
                    step.id
                ));
            }
        }
        StepKind::Verify => {
            if step.verify.is_empty() {
                return Err(format!("verify step {} requires verify commands", step.id));
            }
            if instruction_has_setup_language(&step.instruction) {
                return Err(format!(
                    "verify step {} must not install or prepare dependencies; add a prior setup step",
                    step.id
                ));
            }
        }
        StepKind::Report => {
            if !step.verify.is_empty() {
                return Err(format!(
                    "report step {} must not run verification commands",
                    step.id
                ));
            }
        }
        StepKind::Work | StepKind::Create | StepKind::Edit | StepKind::Repair => {}
    }
    Ok(())
}

fn is_build_or_test_verify(command: &str) -> bool {
    matches!(
        command.split_whitespace().collect::<Vec<_>>().as_slice(),
        ["npm", "run", "build"]
            | ["npm", "run", "test"]
            | ["npm", "test"]
            | ["cargo", "check", ..]
            | ["cargo", "test", ..]
            | ["pytest", ..]
            | ["python", "-m", "pytest", ..]
            | ["python3", "-m", "pytest", ..]
    )
}

fn instruction_has_setup_language(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    lower.contains("npm install")
        || lower.contains("npm ci")
        || lower.contains("pnpm install")
        || lower.contains("yarn install")
        || lower.contains("pip install")
        || lower.contains("poetry install")
        || lower.contains("uv sync")
        || lower.contains("bundle install")
        || lower.contains("composer install")
}

fn validate_ultra_plan(plan: &UltraPlan) -> Result<(), String> {
    validate_goal(&plan.goal)?;
    if plan.phases.len() < 2 {
        return Err("ultra plan must contain at least two phases".to_string());
    }
    if plan.phases.len() > MAX_ULTRA_PHASES {
        return Err(format!(
            "ultra plan has too many phases: {}",
            plan.phases.len()
        ));
    }
    let mut ids = BTreeSet::new();
    for phase in &plan.phases {
        validate_step_id(&phase.id)?;
        if !ids.insert(phase.id.clone()) {
            return Err(format!("duplicate ultra phase id: {}", phase.id));
        }
        validate_instruction(&phase.prompt)?;
        if phase.prompt.trim_start().starts_with('/') {
            return Err(format!(
                "ultra phase prompt must be a plain goal, not a REPL command: {}",
                phase.prompt
            ));
        }
    }
    Ok(())
}

fn validate_text(label: &str, value: &str) -> Result<(), String> {
    if value.trim().is_empty() {
        return Err(format!("{label} must not be empty"));
    }
    if value.chars().count() > MAX_STRING_CHARS {
        return Err(format!("{label} is too long"));
    }
    Ok(())
}

fn validate_goal(value: &str) -> Result<(), String> {
    if value.trim().is_empty() {
        return Err("goal must not be empty".to_string());
    }
    if value.chars().count() > MAX_GOAL_CHARS {
        return Err("goal is too long".to_string());
    }
    Ok(())
}

fn validate_instruction(value: &str) -> Result<(), String> {
    validate_text("instruction", value)?;
    let trimmed = value.trim();
    let lower = trimmed.to_ascii_lowercase();
    let shell_starts = ["echo ", "cat ", "mkdir ", "touch ", "cd "];
    let shell_syntax = ["&&", "||", "|", "`", "$("];
    if shell_starts.iter().any(|prefix| lower.starts_with(prefix))
        || shell_syntax.iter().any(|needle| trimmed.contains(needle))
    {
        return Err(format!(
            "step instruction must be natural language, not a shell command: {value}"
        ));
    }
    Ok(())
}

fn validate_step_id(id: &str) -> Result<(), String> {
    if id.is_empty()
        || id.len() > 64
        || !id
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
    {
        return Err(format!("invalid step id: {id}"));
    }
    Ok(())
}

fn validate_relative_path(path: &str) -> Result<(), String> {
    let path = Path::new(path);
    if path.is_absolute() {
        return Err(format!("expected relative path, got {}", path.display()));
    }
    normalize_relative_path(path)
        .ok_or_else(|| format!("path escapes workspace: {}", path.display()))?;
    Ok(())
}

fn capped_config(config: &Config, max_iterations: usize) -> Config {
    let mut capped = config.clone();
    let requested = if capped.max_iterations == 0 {
        max_iterations
    } else {
        capped.max_iterations.min(max_iterations)
    };
    capped.max_iterations = requested.max(1);
    capped
}

fn slug(value: &str) -> String {
    let slug = value
        .chars()
        .filter_map(|ch| {
            if ch.is_ascii_alphanumeric() {
                Some(ch.to_ascii_lowercase())
            } else if ch.is_whitespace() || ch == '-' || ch == '_' {
                Some('-')
            } else {
                None
            }
        })
        .collect::<String>();
    let mut collapsed = String::new();
    let mut last_dash = false;
    for ch in slug.chars() {
        if ch == '-' {
            if !last_dash {
                collapsed.push(ch);
            }
            last_dash = true;
        } else {
            collapsed.push(ch);
            last_dash = false;
        }
    }
    let trimmed = collapsed.trim_matches('-');
    if trimmed.is_empty() {
        "task".to_string()
    } else {
        trimmed.chars().take(48).collect()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use serde_json::json;
    use tempfile::TempDir;

    use crate::config::Config;
    use crate::modes::plan_act::ExecutionMode;
    use crate::ollama::client::AssistantReply;
    use crate::ollama::xml_fallback::ToolCall;

    use super::*;

    #[derive(Default)]
    struct MockClient {
        replies: VecDeque<AssistantReply>,
    }

    impl MockClient {
        fn push_reply(&mut self, content: &str, tool_calls: Vec<ToolCall>) {
            self.replies.push_back(AssistantReply {
                content: content.to_string(),
                tool_calls,
                prompt_tokens: None,
                completion_tokens: None,
            });
        }
    }

    impl MinimalChatClient for MockClient {
        fn chat(
            &mut self,
            _model: &str,
            _messages: &[ConversationMessage],
            _tools: &[ToolSpec],
            _native_tools_enabled: bool,
        ) -> Result<AssistantReply, String> {
            self.replies
                .pop_front()
                .ok_or_else(|| "no reply".to_string())
        }
    }

    impl PlannerLlm for MockClient {
        fn chat_plan(
            &mut self,
            _scope: ProviderCallScope,
            _messages: &[ConversationMessage],
        ) -> Result<AssistantReply, String> {
            self.replies
                .pop_front()
                .ok_or_else(|| "no planner reply".to_string())
        }

        fn label(&self) -> String {
            "mock-planner".to_string()
        }
    }

    fn config(root: &TempDir) -> Config {
        Config {
            cwd: root.path().to_path_buf(),
            context_budget: 24_000,
            max_iterations: 4,
            yes_mode: true,
            ..Default::default()
        }
    }

    fn session_snapshot(mode: ExecutionMode) -> SessionSnapshot {
        SessionSnapshot {
            id: "session-1".to_string(),
            workspace_key: "workspace-1".to_string(),
            mode_state: crate::modes::plan_act::ModeState {
                mode,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    fn tool_call(name: &str, arguments: serde_json::Value) -> ToolCall {
        ToolCall {
            id: format!("call_{name}"),
            name: name.to_string(),
            arguments,
        }
    }

    struct TimeoutPlanner {
        attempts: usize,
        error: &'static str,
    }

    impl PlannerLlm for TimeoutPlanner {
        fn chat_plan(
            &mut self,
            _scope: ProviderCallScope,
            _messages: &[ConversationMessage],
        ) -> Result<AssistantReply, String> {
            self.attempts += 1;
            Err(self.error.to_string())
        }

        fn label(&self) -> String {
            "timeout-planner".to_string()
        }
    }

    #[test]
    fn plan_json_is_saved_as_valid_yaml() {
        let temp = tempfile::tempdir().unwrap();
        let mut client = MockClient::default();
        client.push_reply(
            r#"{"goal":"Build app","steps":[{"id":"scaffold","instruction":"Create files","expected_paths":["package.json"],"verify":[]}]}"#,
            Vec::new(),
        );

        let path = generate_step_plan(&config(&temp), &mut client, "Build app").unwrap();
        assert!(path.starts_with(temp.path().join(".anvil").join("plans")));
        let loaded = parse_plan_yaml(&std::fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(loaded.goal, "Build app");
        assert_eq!(loaded.steps[0].id, "scaffold");
    }

    #[test]
    fn step_planner_timeout_uses_two_attempts_then_fails_with_kind() {
        let mut planner = TimeoutPlanner {
            attempts: 0,
            error: "phase_step_planner_timeout: provider call timed out after 1s",
        };

        let err = chat_plan_with_timeout_retry(
            &mut planner,
            ProviderCallScope::PlannerStep,
            &[ConversationMessage::user("plan".to_string())],
        )
        .unwrap_err();

        assert_eq!(planner.attempts, PLANNER_TIMEOUT_ATTEMPTS);
        assert!(err.starts_with("phase_step_planner_timeout:"), "{err}");
    }

    #[test]
    fn ultra_planner_timeout_uses_ultra_kind() {
        let mut planner = TimeoutPlanner {
            attempts: 0,
            error: "planner_ultra_timeout: provider call timed out after 1s",
        };

        let err = chat_plan_with_timeout_retry(
            &mut planner,
            ProviderCallScope::PlannerUltra,
            &[ConversationMessage::user("plan".to_string())],
        )
        .unwrap_err();

        assert_eq!(planner.attempts, PLANNER_TIMEOUT_ATTEMPTS);
        assert!(err.starts_with("planner_ultra_timeout:"), "{err}");
    }

    #[test]
    fn ultra_plan_json_is_saved_as_valid_yaml() {
        let temp = tempfile::tempdir().unwrap();
        let mut client = MockClient::default();
        client.push_reply(
            r#"{"goal":"Build app","style":"tdd","phases":[{"id":"write-red-test","prompt":"Add a focused failing test and confirm it fails."},{"id":"make-test-pass","prompt":"Implement the minimal fix and run the focused test."}]}"#,
            Vec::new(),
        );

        let path = generate_ultra_plan(
            &config(&temp),
            &mut client,
            "Build app with TDD",
            UltraProfile::Generic,
            UltraPlanStyle::Tdd,
        )
        .unwrap();
        assert!(path.starts_with(temp.path().join(".anvil").join("plans")));
        let loaded = parse_ultra_plan_yaml(&std::fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(loaded.goal, "Build app with TDD");
        assert_eq!(loaded.profile, UltraProfile::Generic);
        assert_eq!(loaded.style, UltraPlanStyle::Tdd);
        assert_eq!(loaded.intent, WorkIntent::Create);
        assert_eq!(loaded.phases[0].id, "write-red-test");
    }

    #[test]
    fn generated_plans_preserve_original_goal_contract() {
        let temp = tempfile::tempdir().unwrap();
        let goal = "Build app\n\nRequired final artifacts:\n- components/SpaceOpsGame.tsx\n";

        let mut step_client = MockClient::default();
        step_client.push_reply(
            r#"{"goal":"Summarized goal","steps":[{"id":"create-game","instruction":"Create components/SpaceOpsGame.tsx","expected_paths":["components/SpaceOpsGame.tsx"],"verify":[]}]}"#,
            Vec::new(),
        );
        let step_path = generate_step_plan(&config(&temp), &mut step_client, goal).unwrap();
        let step_plan = parse_plan_yaml(&std::fs::read_to_string(step_path).unwrap()).unwrap();
        assert_eq!(step_plan.goal, goal);

        let mut ultra_client = MockClient::default();
        ultra_client.push_reply(
            r#"{"goal":"Summarized ultra goal","profile":"generic","style":"default","phases":[{"id":"create-game","prompt":"Create the game component."},{"id":"verify-game","prompt":"Verify the required game component exists."}]}"#,
            Vec::new(),
        );
        let ultra_path = generate_ultra_plan(
            &config(&temp),
            &mut ultra_client,
            goal,
            UltraProfile::Nextjs,
            UltraPlanStyle::Default,
        )
        .unwrap();
        let ultra_plan =
            parse_ultra_plan_yaml(&std::fs::read_to_string(ultra_path).unwrap()).unwrap();
        assert_eq!(ultra_plan.goal, goal);
        assert_eq!(ultra_plan.profile, UltraProfile::Nextjs);
        assert_eq!(ultra_plan.intent, WorkIntent::Create);
    }

    #[test]
    fn invalid_verify_command_is_rejected() {
        let plan = StepPlan {
            goal: "x".into(),
            steps: vec![PlanStep {
                id: "bad".into(),
                kind: StepKind::Verify,
                instruction: "x".into(),
                expected_paths: vec![],
                verify: vec!["npm install left-pad".into()],
                expected_result: VerifyExpectedResult::Pass,
            }],
        };
        assert!(validate_plan(&plan).is_err());
    }

    #[test]
    fn runtime_verify_splits_safe_and_commands() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("a.txt"), "a").unwrap();
        std::fs::write(temp.path().join("b.txt"), "b").unwrap();
        let step = PlanStep {
            id: "verify".into(),
            kind: StepKind::Verify,
            instruction: "verify".into(),
            expected_paths: vec![],
            verify: vec!["cat a.txt && cat b.txt".into()],
            expected_result: VerifyExpectedResult::Pass,
        };

        let report = verify_step(temp.path(), &step);

        assert!(report.success, "{:?}", report.failures);
    }

    #[test]
    fn runtime_verify_normalizes_cd_and_python_smoke_shape() {
        let temp = tempfile::tempdir().unwrap();
        let cli_dir = temp.path().join("src/csv_stats_cli");
        std::fs::create_dir_all(&cli_dir).unwrap();
        std::fs::write(cli_dir.join("main.py"), "print('ok')\n").unwrap();
        std::fs::write(
            cli_dir.join("smoke-check.py"),
            "from pathlib import Path\nassert Path('main.py').is_file()\n",
        )
        .unwrap();
        let step = PlanStep {
            id: "verify-cli".into(),
            kind: StepKind::Verify,
            instruction: "verify cli".into(),
            expected_paths: vec!["src/csv_stats_cli/main.py".into()],
            verify: vec!["cd src/csv_stats_cli && python smoke-check.py".into()],
            expected_result: VerifyExpectedResult::Pass,
        };

        let report = verify_step(temp.path(), &step);

        assert!(report.success, "{:?}", report.failures);
    }

    #[test]
    fn runtime_verify_still_rejects_pipes() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("a.txt"), "a").unwrap();
        let step = PlanStep {
            id: "verify".into(),
            kind: StepKind::Verify,
            instruction: "verify".into(),
            expected_paths: vec![],
            verify: vec!["cat a.txt | cat".into()],
            expected_result: VerifyExpectedResult::Pass,
        };

        let report = verify_step(temp.path(), &step);

        assert!(!report.success);
        assert!(
            report
                .failures
                .iter()
                .any(|failure| failure.contains("shell control syntax")),
            "{:?}",
            report.failures
        );
    }

    #[test]
    fn shell_like_instruction_is_rejected() {
        let plan = StepPlan {
            goal: "x".into(),
            steps: vec![PlanStep {
                id: "bad".into(),
                kind: StepKind::Create,
                instruction: "echo 'hello' > hello.txt".into(),
                expected_paths: vec!["hello.txt".into()],
                verify: vec![],
                expected_result: VerifyExpectedResult::Pass,
            }],
        };
        assert!(validate_plan(&plan).is_err());
    }

    #[test]
    fn natural_language_instruction_may_mention_build_command() {
        assert!(validate_instruction("npm run buildを実行し、結果を確認する").is_ok());
    }

    #[test]
    fn natural_language_instruction_may_use_semicolon() {
        assert!(
            validate_instruction(
                "Inspect the current workspace; verify whether package.json already exists."
            )
            .is_ok()
        );
    }

    #[test]
    fn natural_language_instruction_may_mention_html_tags() {
        assert!(
            validate_instruction(
                "Implement AnalyticsPanel.tsx using semantic HTML like <dl> and <div>."
            )
            .is_ok()
        );
    }

    #[test]
    fn work_intent_is_detected_from_goal_language() {
        assert_eq!(
            detect_work_intent("Next.js アプリを新規開発してください"),
            WorkIntent::Create
        );
        assert_eq!(
            detect_work_intent("原因調査してレポートにまとめる"),
            WorkIntent::Investigate
        );
        assert_eq!(
            detect_work_intent("既存のバグを修正してください"),
            WorkIntent::Fix
        );
        assert_eq!(
            detect_work_intent("README ドキュメントを更新する"),
            WorkIntent::Document
        );
    }

    #[test]
    fn profile_inference_prefers_goal_tokens_then_workspace_then_default() {
        let temp = tempfile::tempdir().unwrap();
        let res = resolve_ultra_profile(temp.path(), "Web アプリを作成してください", None).unwrap();
        assert_eq!(res.profile, UltraProfile::Nextjs);
        assert_eq!(res.source, ProfileResolutionSource::Goal);
        assert_eq!(
            res.summary_line().as_deref(),
            Some("profile_inferred: nextjs (from: goal)")
        );

        let res = resolve_ultra_profile(temp.path(), "コマンドラインツールを作成", None).unwrap();
        assert_eq!(res.profile, UltraProfile::Python);
        assert_eq!(res.source, ProfileResolutionSource::Goal);
        assert_eq!(
            res.summary_line().as_deref(),
            Some("profile_inferred: python-cli (from: goal)")
        );

        std::fs::write(
            temp.path().join("package.json"),
            r#"{"dependencies":{"next":"14.0.0"}}"#,
        )
        .unwrap();
        let res = resolve_ultra_profile(temp.path(), "小さな画面を作成", None).unwrap();
        assert_eq!(res.profile, UltraProfile::Nextjs);
        assert_eq!(res.source, ProfileResolutionSource::Workspace);
        assert_eq!(
            res.summary_line().as_deref(),
            Some("profile_inferred: nextjs (from: workspace)")
        );

        let py = tempfile::tempdir().unwrap();
        std::fs::write(py.path().join("pyproject.toml"), "[project]\nname='x'\n").unwrap();
        let res = resolve_ultra_profile(py.path(), "小さなツールを作成", None).unwrap();
        assert_eq!(res.profile, UltraProfile::Python);
        assert_eq!(res.source, ProfileResolutionSource::Workspace);

        let generic = tempfile::tempdir().unwrap();
        let res = resolve_ultra_profile(generic.path(), "調査メモをまとめる", None).unwrap();
        assert_eq!(res.profile, UltraProfile::Generic);
        assert_eq!(res.source, ProfileResolutionSource::Default);
        assert_eq!(
            res.summary_line().as_deref(),
            Some("profile_inferred: generic (from: default)")
        );
    }

    #[test]
    fn explicit_profile_wins_over_goal_and_workspace_inference() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(
            temp.path().join("package.json"),
            r#"{"dependencies":{"next":"14.0.0"}}"#,
        )
        .unwrap();

        let res = resolve_ultra_profile(
            temp.path(),
            "React Web アプリを作成してください",
            Some("generic"),
        )
        .unwrap();

        assert_eq!(res.profile, UltraProfile::Generic);
        assert_eq!(res.source, ProfileResolutionSource::Explicit);
        assert_eq!(res.summary_line(), None);
    }

    #[test]
    fn requested_port_for_ultra_plan_uses_first_goal_or_phase_match() {
        let ultra = UltraPlan {
            goal: "4000番ポートでNext.jsアプリを作成".into(),
            profile: UltraProfile::Nextjs,
            style: UltraPlanStyle::Default,
            intent: WorkIntent::Create,
            phases: vec![
                UltraPhase {
                    id: "build".into(),
                    prompt: "ポート5000に変更せず実装する".into(),
                },
                UltraPhase {
                    id: "verify".into(),
                    prompt: "Verify".into(),
                },
            ],
        };
        assert_eq!(requested_port_for_ultra_plan(&ultra), Some(4000));

        let ultra = UltraPlan {
            goal: "Next.jsアプリを作成".into(),
            profile: UltraProfile::Nextjs,
            style: UltraPlanStyle::Default,
            intent: WorkIntent::Create,
            phases: vec![
                UltraPhase {
                    id: "build".into(),
                    prompt: "dev script should use port 4100".into(),
                },
                UltraPhase {
                    id: "verify".into(),
                    prompt: "Verify".into(),
                },
            ],
        };
        assert_eq!(requested_port_for_ultra_plan(&ultra), Some(4100));
    }

    #[test]
    fn read_only_cat_verify_is_allowed() {
        assert!(validate_verify_command("cat hello.txt").is_ok());
        assert!(validate_verify_command("cat ../secret.txt").is_err());
    }

    #[test]
    fn python_script_verify_is_allowed_for_local_scripts() {
        assert!(validate_verify_command("python scripts/check_data.py --check").is_ok());
        assert!(validate_verify_command("python3 scripts/check_data.py --check").is_ok());
        assert!(validate_verify_command("python /tmp/check_data.py").is_err());
        assert!(validate_verify_command("python scripts/check_data.sh").is_err());
    }

    #[test]
    fn node_check_verify_is_limited_to_javascript_files() {
        assert!(validate_verify_command("node --check scripts/check.js").is_ok());
        assert!(validate_verify_command("node --check scripts/check.mjs").is_ok());
        assert!(validate_verify_command("node --check components/Panel.tsx").is_err());
        assert!(validate_verify_command("node --check src/index.ts").is_err());
    }

    #[test]
    fn data_profile_snapshot_includes_headers_and_protects_raw_inputs() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join("data/raw")).unwrap();
        std::fs::write(
            temp.path().join("data/raw/sales.csv"),
            "date,total\n2026-01-01,42\n",
        )
        .unwrap();

        let snapshot = profile_snapshot(temp.path(), UltraProfile::DataAnalysis);

        assert!(
            snapshot
                .lines
                .iter()
                .any(|line| line.contains("data/raw/sales.csv"))
        );
        assert!(
            snapshot
                .lines
                .iter()
                .any(|line| line.contains("date,total"))
        );
        assert_eq!(snapshot.protected_files[0].path, "data/raw/sales.csv");
    }

    #[test]
    fn step_kind_is_saved_and_loaded_in_plan_yaml() {
        let plan = StepPlan {
            goal: "Create app".into(),
            steps: vec![PlanStep {
                id: "setup-dependencies".into(),
                kind: StepKind::Setup,
                instruction: "Install or prepare dependencies for package.json.".into(),
                expected_paths: vec![],
                verify: vec![],
                expected_result: VerifyExpectedResult::Pass,
            }],
        };

        let rendered = render_plan_yaml(&plan);
        assert!(rendered.contains(r#"kind: "setup""#), "{rendered}");
        let parsed = parse_plan_yaml(&rendered).unwrap();
        assert_eq!(parsed.steps[0].kind, StepKind::Setup);
    }

    #[test]
    fn legacy_plan_yaml_without_kind_defaults_to_work() {
        let raw = r#"
goal: "Create report"
steps:
  - id: "create-report"
    instruction: "Create report.md"
    expected_paths:
      - "report.md"
    verify:
"#;

        let plan = parse_plan_yaml(raw).unwrap();
        assert_eq!(plan.steps[0].kind, StepKind::Work);
    }

    #[test]
    fn setup_step_must_not_run_build_verification() {
        let plan = StepPlan {
            goal: "Prepare project".into(),
            steps: vec![PlanStep {
                id: "setup-dependencies".into(),
                kind: StepKind::Setup,
                instruction: "Install dependencies for package.json.".into(),
                expected_paths: vec![],
                verify: vec!["npm run build".into()],
                expected_result: VerifyExpectedResult::Pass,
            }],
        };

        let err = validate_plan(&plan).unwrap_err();
        assert!(err.contains("setup step setup-dependencies must not run build/test"));
    }

    #[test]
    fn verify_step_must_not_prepare_dependencies() {
        let plan = StepPlan {
            goal: "Verify project".into(),
            steps: vec![PlanStep {
                id: "verify-build".into(),
                kind: StepKind::Verify,
                instruction: "Run npm install and then verify the build.".into(),
                expected_paths: vec![],
                verify: vec!["npm run build".into()],
                expected_result: VerifyExpectedResult::Pass,
            }],
        };

        let err = validate_plan(&plan).unwrap_err();
        assert!(err.contains("verify step verify-build must not install"));
    }

    #[test]
    fn npm_verify_before_dependency_setup_is_rejected_by_lint() {
        let plan = StepPlan {
            goal: "Create app".into(),
            steps: vec![
                PlanStep {
                    id: "create-package".into(),
                    kind: StepKind::Create,
                    instruction: "Create package.json and app/page.tsx.".into(),
                    expected_paths: vec!["package.json".into(), "app/page.tsx".into()],
                    verify: vec![],
                    expected_result: VerifyExpectedResult::Pass,
                },
                PlanStep {
                    id: "verify-build".into(),
                    kind: StepKind::Verify,
                    instruction: "Verify the build.".into(),
                    expected_paths: vec!["package.json".into(), "app/page.tsx".into()],
                    verify: vec!["npm run build".into()],
                    expected_result: VerifyExpectedResult::Pass,
                },
            ],
        };

        let err = lint_plan(&plan).unwrap_err();
        assert!(
            err.contains("runs npm verification before a dependency setup step"),
            "{err}"
        );
    }

    #[test]
    fn npm_verify_after_dependency_setup_is_allowed_by_lint() {
        let plan = StepPlan {
            goal: "Create app".into(),
            steps: vec![
                PlanStep {
                    id: "create-package".into(),
                    kind: StepKind::Create,
                    instruction: "Create package.json and app/page.tsx.".into(),
                    expected_paths: vec!["package.json".into(), "app/page.tsx".into()],
                    verify: vec![],
                    expected_result: VerifyExpectedResult::Pass,
                },
                PlanStep {
                    id: "setup-dependencies".into(),
                    kind: StepKind::Setup,
                    instruction: "Install or prepare dependencies for package.json.".into(),
                    expected_paths: vec![],
                    verify: vec![],
                    expected_result: VerifyExpectedResult::Pass,
                },
                PlanStep {
                    id: "verify-build".into(),
                    kind: StepKind::Verify,
                    instruction: "Verify the build.".into(),
                    expected_paths: vec!["package.json".into(), "app/page.tsx".into()],
                    verify: vec!["npm run build".into()],
                    expected_result: VerifyExpectedResult::Pass,
                },
            ],
        };

        assert!(lint_plan(&plan).is_ok());
    }

    #[test]
    fn data_profile_verifier_rejects_raw_input_changes() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join("data/raw")).unwrap();
        let raw = temp.path().join("data/raw/sales.csv");
        std::fs::write(&raw, "date,total\n2026-01-01,42\n").unwrap();
        let snapshot = profile_snapshot(temp.path(), UltraProfile::DataPipeline);

        std::fs::write(&raw, "date,total\n2026-01-01,99\nextra,row\n").unwrap();

        let err = verify_profile_after_phase(
            temp.path(),
            UltraProfile::DataPipeline,
            WorkIntent::Create,
            &snapshot,
        )
        .unwrap_err();
        assert!(err.contains("protected input data changed"));
    }

    #[test]
    fn nextjs_profile_verifier_rejects_standalone_ts_conversion() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join("app")).unwrap();
        std::fs::write(
            temp.path().join("app/page.tsx"),
            "export default function Page(){}\n",
        )
        .unwrap();
        std::fs::write(
            temp.path().join("package.json"),
            r#"{"dependencies":{"next":"14.0.0","react":"18.0.0"},"scripts":{"build":"tsc","dev":"node dist/game.js"}}"#,
        )
        .unwrap();
        std::fs::write(
            temp.path().join("tsconfig.json"),
            r#"{"compilerOptions":{"rootDir":"./src"}}"#,
        )
        .unwrap();
        let snapshot = profile_snapshot(temp.path(), UltraProfile::Nextjs);

        let err = verify_profile_after_phase(
            temp.path(),
            UltraProfile::Nextjs,
            WorkIntent::Create,
            &snapshot,
        )
        .unwrap_err();
        assert!(err.contains("build script"));
        assert!(err.contains("rootDir"));
    }

    #[test]
    fn nextjs_profile_verifier_accepts_at_alias_paths_without_base_url() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join("app")).unwrap();
        std::fs::create_dir_all(temp.path().join("components")).unwrap();
        std::fs::write(
            temp.path().join("package.json"),
            r#"{"dependencies":{"next":"14.0.0","react":"18.0.0","react-dom":"18.0.0"},"scripts":{"build":"next build","dev":"next dev -p 3011"}}"#,
        )
        .unwrap();
        std::fs::write(
            temp.path().join("app/page.tsx"),
            "import SpaceOpsGame from '@/components/SpaceOpsGame';\nexport default function Page(){ return <SpaceOpsGame/>; }\n",
        )
        .unwrap();
        std::fs::write(
            temp.path().join("components/SpaceOpsGame.tsx"),
            "export default function SpaceOpsGame(){ return null; }\n",
        )
        .unwrap();
        std::fs::write(
            temp.path().join("tsconfig.json"),
            r#"{"compilerOptions":{"paths":{"@/*":["./*"]}}}"#,
        )
        .unwrap();
        let snapshot = profile_snapshot(temp.path(), UltraProfile::Nextjs);

        verify_profile_after_phase(
            temp.path(),
            UltraProfile::Nextjs,
            WorkIntent::Create,
            &snapshot,
        )
        .unwrap();
    }

    #[test]
    fn nextjs_requested_port_invariant_enforces_dynamic_port() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join("app")).unwrap();
        std::fs::write(
            temp.path().join("app/page.tsx"),
            "export default function Page(){ return null; }\n",
        )
        .unwrap();
        std::fs::write(
            temp.path().join("package.json"),
            r#"{"dependencies":{"next":"14.0.0","react":"18.0.0","react-dom":"18.0.0"},"scripts":{"build":"next build","dev":"next dev -p 3011","start":"next start -p 3011"}}"#,
        )
        .unwrap();
        let snapshot = profile_snapshot(temp.path(), UltraProfile::Nextjs);

        let err = verify_profile_after_phase_with_requested_port(
            temp.path(),
            UltraProfile::Nextjs,
            WorkIntent::Create,
            &snapshot,
            Some(4000),
        )
        .unwrap_err();

        assert!(err.contains("requested port 4000"), "{err}");
        assert!(err.contains("dev script"), "{err}");
        assert!(err.contains("start script"), "{err}");
    }

    #[test]
    fn nextjs_requested_port_invariant_accepts_matching_port_and_skips_absent_request() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join("app")).unwrap();
        std::fs::write(
            temp.path().join("app/page.tsx"),
            "export default function Page(){ return null; }\n",
        )
        .unwrap();
        std::fs::write(
            temp.path().join("package.json"),
            r#"{"dependencies":{"next":"14.0.0","react":"18.0.0","react-dom":"18.0.0"},"scripts":{"build":"next build","dev":"next dev -p 4000","start":"next start -p 4000"}}"#,
        )
        .unwrap();
        let snapshot = profile_snapshot(temp.path(), UltraProfile::Nextjs);

        verify_profile_after_phase_with_requested_port(
            temp.path(),
            UltraProfile::Nextjs,
            WorkIntent::Create,
            &snapshot,
            Some(4000),
        )
        .unwrap();

        std::fs::write(
            temp.path().join("package.json"),
            r#"{"dependencies":{"next":"14.0.0","react":"18.0.0","react-dom":"18.0.0"},"scripts":{"build":"next build","dev":"next dev -p 3011","start":"next start"}}"#,
        )
        .unwrap();
        verify_profile_after_phase(
            temp.path(),
            UltraProfile::Nextjs,
            WorkIntent::Create,
            &snapshot,
        )
        .unwrap();
    }

    #[test]
    fn nextjs_profile_snapshot_selects_requested_script_and_default_probe_ports() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(
            temp.path().join("package.json"),
            r#"{"dependencies":{"next":"14.0.0"},"scripts":{"dev":"next dev -p 4100"}}"#,
        )
        .unwrap();

        let requested = UltraPlan {
            goal: "4000番ポートでNext.jsアプリを作成".into(),
            profile: UltraProfile::Nextjs,
            style: UltraPlanStyle::Default,
            intent: WorkIntent::Create,
            phases: vec![
                UltraPhase {
                    id: "build".into(),
                    prompt: "Create the app".into(),
                },
                UltraPhase {
                    id: "verify".into(),
                    prompt: "Verify the app".into(),
                },
            ],
        };
        let snapshot = profile_snapshot_for_ultra_plan(temp.path(), &requested);
        assert_eq!(snapshot.requested_port, Some(4000));
        assert_eq!(snapshot.probe_port, Some(4000));

        let fallback = UltraPlan {
            goal: "Next.jsアプリを作成".into(),
            profile: UltraProfile::Nextjs,
            style: UltraPlanStyle::Default,
            intent: WorkIntent::Create,
            phases: requested.phases.clone(),
        };
        let snapshot = profile_snapshot_for_ultra_plan(temp.path(), &fallback);
        assert_eq!(snapshot.requested_port, None);
        assert_eq!(snapshot.probe_port, Some(4100));

        std::fs::remove_file(temp.path().join("package.json")).unwrap();
        let snapshot = profile_snapshot_for_ultra_plan(temp.path(), &fallback);
        assert_eq!(snapshot.probe_port, Some(3000));
    }

    #[test]
    fn profiled_phase_prompt_includes_port_and_canvas_phase_lines() {
        let temp = tempfile::tempdir().unwrap();
        let ultra = UltraPlan {
            goal: "4000番ポートでNext.jsアプリを作成".into(),
            profile: UltraProfile::Nextjs,
            style: UltraPlanStyle::Default,
            intent: WorkIntent::Create,
            phases: vec![
                UltraPhase {
                    id: "build".into(),
                    prompt: "HTML5 Canvasで操作できる画面を実装する".into(),
                },
                UltraPhase {
                    id: "verify".into(),
                    prompt: "Verify the app".into(),
                },
            ],
        };
        let snapshot = profile_snapshot_for_ultra_plan(temp.path(), &ultra);
        let prompt =
            build_profiled_phase_prompt(&ultra, &ultra.phases[0], &snapshot, WorkIntent::Create);

        assert!(prompt.contains("Requested port invariant"));
        assert!(prompt.contains("4000"));
        assert!(prompt.contains("Readiness/interaction probe target port: 4000"));
        assert!(prompt.contains("Canvas surface requirement detected"));
    }

    #[test]
    fn nextjs_profile_verifier_rejects_incomplete_tailwind_contract() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join("app")).unwrap();
        std::fs::write(
            temp.path().join("package.json"),
            r#"{"dependencies":{"next":"14.0.0","react":"18.0.0","react-dom":"18.0.0"},"scripts":{"build":"next build"}}"#,
        )
        .unwrap();
        std::fs::write(temp.path().join("app/page.tsx"), "export default function Page() { return <main className=\"min-h-screen text-white\" />; }\n").unwrap();
        std::fs::write(
            temp.path().join("app/globals.css"),
            "@tailwind base;\n@tailwind components;\n@tailwind utilities;\n",
        )
        .unwrap();
        let snapshot = profile_snapshot(temp.path(), UltraProfile::Nextjs);

        let err = verify_profile_after_phase(
            temp.path(),
            UltraProfile::Nextjs,
            WorkIntent::Create,
            &snapshot,
        )
        .unwrap_err();

        assert!(err.contains("tailwindcss dependency"), "got: {err}");
        assert!(err.contains("postcss dependency"), "got: {err}");
        assert!(err.contains("autoprefixer dependency"), "got: {err}");
        assert!(err.contains("tailwind.config"), "got: {err}");
        assert!(err.contains("postcss.config"), "got: {err}");
    }

    #[test]
    fn nextjs_profile_verifier_accepts_complete_tailwind_contract() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join("app")).unwrap();
        std::fs::write(
            temp.path().join("package.json"),
            r#"{"dependencies":{"next":"14.0.0","react":"18.0.0","react-dom":"18.0.0"},"devDependencies":{"tailwindcss":"3.4.0","postcss":"8.4.0","autoprefixer":"10.4.0"},"scripts":{"build":"next build"}}"#,
        )
        .unwrap();
        std::fs::write(
            temp.path().join("app/page.tsx"),
            "export default function Page() { return <main className=\"min-h-screen text-white\" />; }\n",
        )
        .unwrap();
        std::fs::write(
            temp.path().join("app/globals.css"),
            "@tailwind base;\n@tailwind components;\n@tailwind utilities;\n",
        )
        .unwrap();
        std::fs::write(
            temp.path().join("tailwind.config.js"),
            "module.exports = { content: ['./app/**/*.{ts,tsx}'] };\n",
        )
        .unwrap();
        std::fs::write(
            temp.path().join("postcss.config.js"),
            "module.exports = { plugins: { tailwindcss: {}, autoprefixer: {} } };\n",
        )
        .unwrap();
        let snapshot = profile_snapshot(temp.path(), UltraProfile::Nextjs);

        verify_profile_after_phase(
            temp.path(),
            UltraProfile::Nextjs,
            WorkIntent::Create,
            &snapshot,
        )
        .unwrap();
    }

    #[test]
    fn nextjs_profile_verifier_rejects_at_alias_without_paths_mapping() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join("app")).unwrap();
        std::fs::create_dir_all(temp.path().join("components")).unwrap();
        std::fs::write(
            temp.path().join("package.json"),
            r#"{"dependencies":{"next":"14.0.0","react":"18.0.0","react-dom":"18.0.0"},"scripts":{"build":"next build","dev":"next dev -p 3011"}}"#,
        )
        .unwrap();
        std::fs::write(
            temp.path().join("app/page.tsx"),
            "import SpaceOpsGame from '@/components/SpaceOpsGame';\nexport default function Page(){ return <SpaceOpsGame/>; }\n",
        )
        .unwrap();
        std::fs::write(
            temp.path().join("components/SpaceOpsGame.tsx"),
            "export default function SpaceOpsGame(){ return null; }\n",
        )
        .unwrap();
        std::fs::write(
            temp.path().join("tsconfig.json"),
            r#"{"compilerOptions":{"jsx":"preserve"}}"#,
        )
        .unwrap();
        let snapshot = profile_snapshot(temp.path(), UltraProfile::Nextjs);

        let err = verify_profile_after_phase(
            temp.path(),
            UltraProfile::Nextjs,
            WorkIntent::Create,
            &snapshot,
        )
        .unwrap_err();

        assert!(err.contains("paths mapping"), "got: {err}");
    }

    #[test]
    fn nextjs_profile_verifier_accepts_at_alias_with_base_url() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join("app")).unwrap();
        std::fs::create_dir_all(temp.path().join("components")).unwrap();
        std::fs::write(
            temp.path().join("package.json"),
            r#"{"dependencies":{"next":"14.0.0","react":"18.0.0","react-dom":"18.0.0"},"scripts":{"build":"next build","dev":"next dev -p 3011"}}"#,
        )
        .unwrap();
        std::fs::write(
            temp.path().join("app/page.tsx"),
            "import SpaceOpsGame from '@/components/SpaceOpsGame';\nexport default function Page(){ return <SpaceOpsGame/>; }\n",
        )
        .unwrap();
        std::fs::write(
            temp.path().join("components/SpaceOpsGame.tsx"),
            "export default function SpaceOpsGame(){ return null; }\n",
        )
        .unwrap();
        std::fs::write(
            temp.path().join("tsconfig.json"),
            r#"{"compilerOptions":{"baseUrl":".","paths":{"@/*":["./*"]}}}"#,
        )
        .unwrap();
        let snapshot = profile_snapshot(temp.path(), UltraProfile::Nextjs);

        verify_profile_after_phase(
            temp.path(),
            UltraProfile::Nextjs,
            WorkIntent::Create,
            &snapshot,
        )
        .unwrap();
    }

    #[test]
    fn expected_failure_verify_treats_nonzero_as_success() {
        let temp = tempfile::tempdir().unwrap();
        let step = PlanStep {
            id: "red-test".into(),
            kind: StepKind::Verify,
            instruction: "Add a failing test".into(),
            expected_paths: vec![],
            verify: vec!["cat missing.txt".into()],
            expected_result: VerifyExpectedResult::Fail,
        };
        assert!(verify_step(temp.path(), &step).success);

        std::fs::write(temp.path().join("present.txt"), "ok\n").unwrap();
        let step = PlanStep {
            id: "red-test".into(),
            kind: StepKind::Verify,
            instruction: "Add a failing test".into(),
            expected_paths: vec![],
            verify: vec!["cat present.txt".into()],
            expected_result: VerifyExpectedResult::Fail,
        };
        let report = verify_step(temp.path(), &step);
        assert!(!report.success);
        assert!(report.failures[0].contains("unexpectedly passed"));
    }

    #[test]
    fn nextjs_build_verify_reports_dependency_missing_before_shell_error() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(
            temp.path().join("package.json"),
            r#"{"dependencies":{"next":"14.0.0","react":"18.0.0","react-dom":"18.0.0"},"scripts":{"build":"next build"}}"#,
        )
        .unwrap();
        let step = PlanStep {
            id: "verify-build".into(),
            kind: StepKind::Verify,
            instruction: "Verify the Next.js app build.".into(),
            expected_paths: vec!["package.json".into()],
            verify: vec!["npm run build".into()],
            expected_result: VerifyExpectedResult::Pass,
        };

        let report = verify_step(temp.path(), &step);

        assert!(!report.success);
        assert!(
            report.failures[0].contains("dependency_missing"),
            "{:?}",
            report.failures
        );
        assert!(
            report.failures[0].contains("node_modules/.bin/next"),
            "{:?}",
            report.failures
        );
        assert!(
            report.failures[0].contains("do not change scripts.build"),
            "{:?}",
            report.failures
        );
    }

    #[test]
    fn verifier_failure_excerpt_preserves_diagnostic_and_source_context() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join("app")).unwrap();
        std::fs::create_dir_all(temp.path().join("scripts")).unwrap();
        let mut page_lines = (1..=60)
            .map(|index| format!("// filler line {index}"))
            .collect::<Vec<_>>();
        page_lines[42] =
            "  const { score, lives, level, gameOver } = game.renderState;".to_string();
        std::fs::write(temp.path().join("app/page.tsx"), page_lines.join("\n")).unwrap();
        std::fs::write(
            temp.path().join("scripts/fail.py"),
            r#"for i in range(14):
    print(f"build prelude {i}")
print("Failed to compile.")
print("./app/page.tsx:43:18")
print("Type error: Property 'lives' does not exist on type 'GameInternalState'.")
print("")
print("  41 |   }, [game]);")
print("  42 |")
print("> 43 |   const { score, lives, level, gameOver } = game.renderState;")
print("     |                  ^")
raise SystemExit(1)
"#,
        )
        .unwrap();
        let step = PlanStep {
            id: "verify-build".into(),
            kind: StepKind::Verify,
            instruction: "Verify build output.".into(),
            expected_paths: vec!["app/page.tsx".into()],
            verify: vec!["python3 scripts/fail.py".into()],
            expected_result: VerifyExpectedResult::Pass,
        };

        let report = verify_step(temp.path(), &step);

        assert!(!report.success);
        let failure = &report.failures[0];
        assert!(
            failure.contains("Type error: Property 'lives' does not exist"),
            "{failure}"
        );
        assert!(failure.contains("related source excerpt"), "{failure}");
        assert!(failure.contains("app/page.tsx:40-46"), "{failure}");
        assert!(
            failure
                .contains(">   43 |   const { score, lives, level, gameOver } = game.renderState;"),
            "{failure}"
        );
    }

    #[test]
    fn nextjs_build_before_entry_path_is_rejected_by_lint() {
        let plan = StepPlan {
            goal: "Create a Next.js app".into(),
            steps: vec![
                PlanStep {
                    id: "init-next-project".into(),
                    kind: StepKind::Create,
                    instruction: "Create package.json, next.config.js, and tailwind.config.js"
                        .into(),
                    expected_paths: vec![
                        "package.json".into(),
                        "next.config.js".into(),
                        "tailwind.config.js".into(),
                    ],
                    verify: vec!["npm run build".into()],
                    expected_result: VerifyExpectedResult::Pass,
                },
                PlanStep {
                    id: "create-app-entry".into(),
                    kind: StepKind::Create,
                    instruction: "Create app/page.tsx".into(),
                    expected_paths: vec!["app/page.tsx".into()],
                    verify: vec![],
                    expected_result: VerifyExpectedResult::Pass,
                },
            ],
        };

        let err = lint_plan(&plan).unwrap_err();
        assert!(err.contains("before package.json and a Next.js entry path"));
    }

    #[test]
    fn nextjs_build_lint_accepts_existing_workspace_entry() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join("app")).unwrap();
        std::fs::write(
            temp.path().join("package.json"),
            r#"{"scripts":{"build":"next build"}}"#,
        )
        .unwrap();
        std::fs::write(
            temp.path().join("app/page.tsx"),
            "export default function Page() { return null; }\n",
        )
        .unwrap();
        std::fs::create_dir_all(temp.path().join("node_modules")).unwrap();
        let plan = StepPlan {
            goal: "Modify an existing Next.js app".into(),
            steps: vec![PlanStep {
                id: "integrate-panel".into(),
                kind: StepKind::Edit,
                instruction: "Create components/AnalyticsPanel.tsx and integrate it.".into(),
                expected_paths: vec!["components/AnalyticsPanel.tsx".into()],
                verify: vec!["npm run build".into()],
                expected_result: VerifyExpectedResult::Pass,
            }],
        };

        assert!(lint_plan_with_workspace(&plan, Some(temp.path())).is_ok());
    }

    #[test]
    fn nextjs_non_build_phase_can_omit_entry_path() {
        let plan = StepPlan {
            goal: "Create a Next.js app architecture plan".into(),
            steps: vec![PlanStep {
                id: "scope-and-architecture".into(),
                kind: StepKind::Create,
                instruction: "Create docs/architecture.md describing the Next.js app structure."
                    .into(),
                expected_paths: vec!["docs/architecture.md".into()],
                verify: vec!["cat docs/architecture.md".into()],
                expected_result: VerifyExpectedResult::Pass,
            }],
        };

        assert!(lint_plan(&plan).is_ok());
    }

    #[test]
    fn required_artifact_contract_is_preserved_in_phase_prompt() {
        let ultra = UltraPlan {
            goal: "Build app\n\nRequired final artifacts:\n- components/SpaceOpsGame.tsx\n- app/page.tsx\n".into(),
            profile: UltraProfile::Nextjs,
            style: UltraPlanStyle::Default,
            intent: WorkIntent::Create,
            phases: vec![UltraPhase {
                id: "game".into(),
                prompt: "Implement the game screen.".into(),
            }],
        };
        let prompt = build_profiled_phase_prompt(
            &ultra,
            &ultra.phases[0],
            &ProfileSnapshot::new(Vec::new(), Vec::new()),
            WorkIntent::Create,
        );

        assert!(prompt.contains("Required final artifacts from the overall goal"));
        assert!(prompt.contains("components/SpaceOpsGame.tsx"));
        assert!(prompt.contains("Detected intent: create"));
    }

    #[test]
    fn multi_path_step_instruction_must_name_concrete_files() {
        let plan = StepPlan {
            goal: "Create config".into(),
            steps: vec![PlanStep {
                id: "setup-config".into(),
                kind: StepKind::Create,
                instruction: "Initialize the project configuration".into(),
                expected_paths: vec!["package.json".into(), "tsconfig.json".into()],
                verify: vec![],
                expected_result: VerifyExpectedResult::Pass,
            }],
        };

        let err = lint_plan(&plan).unwrap_err();
        assert!(err.contains("does not name any concrete expected file"));
    }

    #[test]
    fn verification_step_can_reference_multiple_expected_paths_without_naming_each_one() {
        let plan = StepPlan {
            goal: "Create Next.js app".into(),
            steps: vec![
                PlanStep {
                    id: "create-app".into(),
                    kind: StepKind::Create,
                    instruction: "Create package.json and app/page.tsx.".into(),
                    expected_paths: vec!["package.json".into(), "app/page.tsx".into()],
                    verify: vec!["cat app/page.tsx".into()],
                    expected_result: VerifyExpectedResult::Pass,
                },
                PlanStep {
                    id: "create-game-component".into(),
                    kind: StepKind::Create,
                    instruction: "Create components/SpaceOpsGame.tsx.".into(),
                    expected_paths: vec!["components/SpaceOpsGame.tsx".into()],
                    verify: vec!["cat components/SpaceOpsGame.tsx".into()],
                    expected_result: VerifyExpectedResult::Pass,
                },
                PlanStep {
                    id: "setup-dependencies".into(),
                    kind: StepKind::Setup,
                    instruction:
                        "Install or prepare project dependencies for the existing package.json."
                            .into(),
                    expected_paths: vec![],
                    verify: vec![],
                    expected_result: VerifyExpectedResult::Pass,
                },
                PlanStep {
                    id: "validate-build".into(),
                    kind: StepKind::Verify,
                    instruction: "Run the build script to verify the app compiles.".into(),
                    expected_paths: vec![
                        "package.json".into(),
                        "app/page.tsx".into(),
                        "components/SpaceOpsGame.tsx".into(),
                    ],
                    verify: vec!["npm run build".into()],
                    expected_result: VerifyExpectedResult::Pass,
                },
            ],
        };

        assert!(lint_plan(&plan).is_ok());
    }

    #[test]
    fn final_build_check_id_allows_multiple_known_paths_without_instruction_names() {
        let plan = StepPlan {
            goal: "Create Next.js app".into(),
            steps: vec![
                PlanStep {
                    id: "scaffold-app".into(),
                    kind: StepKind::Create,
                    instruction: "Create package.json and app/page.tsx.".into(),
                    expected_paths: vec!["package.json".into(), "app/page.tsx".into()],
                    verify: vec![],
                    expected_result: VerifyExpectedResult::Pass,
                },
                PlanStep {
                    id: "setup-dependencies".into(),
                    kind: StepKind::Setup,
                    instruction: "Install or prepare project dependencies for package.json.".into(),
                    expected_paths: vec![],
                    verify: vec![],
                    expected_result: VerifyExpectedResult::Pass,
                },
                PlanStep {
                    id: "final-build-check".into(),
                    kind: StepKind::Verify,
                    instruction: "Confirm production readiness.".into(),
                    expected_paths: vec!["package.json".into(), "app/page.tsx".into()],
                    verify: vec!["npm run build".into()],
                    expected_result: VerifyExpectedResult::Pass,
                },
            ],
        };

        assert!(lint_plan(&plan).is_ok());
    }

    #[test]
    fn verification_step_still_rejects_unknown_multiple_paths_without_names() {
        let plan = StepPlan {
            goal: "Create reports".into(),
            steps: vec![PlanStep {
                id: "final-check".into(),
                kind: StepKind::Verify,
                instruction: "Run final validation.".into(),
                expected_paths: vec!["report.md".into(), "summary.md".into()],
                verify: vec!["cat report.md".into()],
                expected_result: VerifyExpectedResult::Pass,
            }],
        };

        let err = lint_plan(&plan).unwrap_err();
        assert!(err.contains("does not name any concrete expected file"));
    }

    #[test]
    fn progress_report_detects_repeated_write_without_expected_path_progress() {
        let mut session = SessionSnapshot::default();
        session.messages.push(ConversationMessage::assistant(
            String::new(),
            vec![
                tool_call("Write", json!({"path":"package.json","content":"{}"})),
                tool_call("Write", json!({"path":"package.json","content":"{}"})),
            ],
        ));
        let report = VerificationReport {
            success: false,
            failures: vec![
                "missing expected path: next.config.js".into(),
                "missing expected path: tailwind.config.js".into(),
            ],
        };

        let progress = analyze_step_progress(
            &session,
            0,
            vec!["next.config.js".into(), "tailwind.config.js".into()],
            &report,
        );

        assert!(progress.no_expected_path_progress);
        assert_eq!(progress.repeated_write_or_edit_paths, vec!["package.json"]);
    }

    #[test]
    fn run_plan_repairs_missing_expected_path_once() {
        let temp = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let store = SessionStore::new(state.path(), "session-1", "workspace-1");
        let mut session = session_snapshot(ExecutionMode::Act);
        let plan = StepPlan {
            goal: "Create report".into(),
            steps: vec![PlanStep {
                id: "report".into(),
                kind: StepKind::Create,
                instruction: "Create report.md".into(),
                expected_paths: vec!["report.md".into()],
                verify: vec![],
                expected_result: VerifyExpectedResult::Pass,
            }],
        };
        let plan_path = temp.path().join("plan.yaml");
        std::fs::write(&plan_path, render_plan_yaml(&plan)).unwrap();

        let mut client = MockClient::default();
        client.push_reply("I will create it next.", Vec::new());
        client.push_reply(
            "",
            vec![tool_call(
                "Write",
                json!({"path":"report.md","content":"done\n"}),
            )],
        );
        client.push_reply("Created report.md.", Vec::new());

        let summary = run_plan(
            &config(&temp),
            "qwen3:8b",
            &mut client,
            &store,
            &mut session,
            &plan_path,
        )
        .unwrap();

        assert_eq!(summary.completed, 1);
        assert_eq!(summary.outcomes[0].stop_reason, "completed");
        assert!(temp.path().join("report.md").is_file());
    }

    #[test]
    fn run_plan_allows_second_repair_cycle_for_verifier_failures() {
        let temp = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let store = SessionStore::new(state.path(), "session-1", "workspace-1");
        let mut session = session_snapshot(ExecutionMode::Act);
        let plan = StepPlan {
            goal: "Create report".into(),
            steps: vec![PlanStep {
                id: "report".into(),
                kind: StepKind::Create,
                instruction: "Create report.md".into(),
                expected_paths: vec!["report.md".into()],
                verify: vec!["cat report.md".into()],
                expected_result: VerifyExpectedResult::Pass,
            }],
        };
        let plan_path = temp.path().join("plan.yaml");
        std::fs::write(&plan_path, render_plan_yaml(&plan)).unwrap();

        let mut client = MockClient::default();
        client.push_reply("Done.", Vec::new());
        client.push_reply(
            "",
            vec![tool_call(
                "Write",
                json!({"path":"notes.md","content":"wrong file\n"}),
            )],
        );
        client.push_reply("Created notes.md.", Vec::new());
        client.push_reply(
            "",
            vec![tool_call(
                "Write",
                json!({"path":"report.md","content":"done\n"}),
            )],
        );
        client.push_reply("Created report.md.", Vec::new());

        let summary = run_plan(
            &config(&temp),
            "qwen3:8b",
            &mut client,
            &store,
            &mut session,
            &plan_path,
        )
        .unwrap();

        assert_eq!(summary.completed, 1);
        assert!(temp.path().join("notes.md").is_file());
        assert!(temp.path().join("report.md").is_file());
    }

    #[test]
    fn run_plan_does_not_early_stop_repair_when_expected_path_already_exists() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("report.md"), "bad\n").unwrap();
        std::fs::write(
            temp.path().join("check.py"),
            "from pathlib import Path\nimport sys\nsys.exit(0 if 'good' in Path('report.md').read_text() else 1)\n",
        )
        .unwrap();
        let state = tempfile::tempdir().unwrap();
        let store = SessionStore::new(state.path(), "session-1", "workspace-1");
        let mut session = session_snapshot(ExecutionMode::Act);
        let plan = StepPlan {
            goal: "Fix report".into(),
            steps: vec![PlanStep {
                id: "report".into(),
                kind: StepKind::Edit,
                instruction: "Fix report.md so check.py passes".into(),
                expected_paths: vec!["report.md".into()],
                verify: vec!["python3 check.py".into()],
                expected_result: VerifyExpectedResult::Pass,
            }],
        };
        let plan_path = temp.path().join("plan.yaml");
        std::fs::write(&plan_path, render_plan_yaml(&plan)).unwrap();

        let mut client = MockClient::default();
        client.push_reply("No change needed.", Vec::new());
        client.push_reply("", vec![tool_call("Read", json!({"path":"report.md"}))]);
        client.push_reply(
            "",
            vec![tool_call(
                "Edit",
                json!({"path":"report.md","old_string":"bad","new_string":"good"}),
            )],
        );
        client.push_reply("Fixed report.md.", Vec::new());

        let summary = run_plan(
            &config(&temp),
            "qwen3:8b",
            &mut client,
            &store,
            &mut session,
            &plan_path,
        )
        .unwrap();

        assert_eq!(summary.completed, 1);
        assert_eq!(
            std::fs::read_to_string(temp.path().join("report.md")).unwrap(),
            "good\n"
        );
        assert!(
            client.replies.is_empty(),
            "repair should not stop immediately after Read when expected path already exists"
        );
    }

    #[test]
    fn repair_exhaustion_reports_ultra_plan_run_and_ultra_plan_can_fix() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("report.md"), "bad\n").unwrap();
        std::fs::write(
            temp.path().join("check.py"),
            "from pathlib import Path\nimport sys\nsys.exit(0 if Path('report.md').read_text() == 'good\\n' else 1)\n",
        )
        .unwrap();
        let state = tempfile::tempdir().unwrap();
        let store = SessionStore::new(state.path(), "session-1", "workspace-1");
        let mut session = session_snapshot(ExecutionMode::Act);
        let plan = StepPlan {
            goal: "Fix report.md so check.py passes".into(),
            steps: vec![PlanStep {
                id: "fix-report".into(),
                kind: StepKind::Edit,
                instruction: "Edit report.md so check.py passes".into(),
                expected_paths: vec!["report.md".into()],
                verify: vec!["python3 check.py".into()],
                expected_result: VerifyExpectedResult::Pass,
            }],
        };
        let plan_path = temp.path().join("plan.yaml");
        std::fs::write(&plan_path, render_plan_yaml(&plan)).unwrap();

        let mut failing_client = MockClient::default();
        failing_client.push_reply(
            "",
            vec![tool_call(
                "Edit",
                json!({"path":"report.md","old_string":"bad","new_string":"bad0"}),
            )],
        );
        failing_client.push_reply("Updated report.md.", Vec::new());
        failing_client.push_reply(
            "",
            vec![tool_call(
                "Edit",
                json!({"path":"report.md","old_string":"bad0","new_string":"bad1"}),
            )],
        );
        failing_client.push_reply("Updated report.md again.", Vec::new());
        failing_client.push_reply(
            "",
            vec![tool_call(
                "Edit",
                json!({"path":"report.md","old_string":"bad1","new_string":"still bad again"}),
            )],
        );
        failing_client.push_reply("Updated report.md a third time.", Vec::new());

        let err = run_plan(
            &config(&temp),
            "qwen3:8b",
            &mut failing_client,
            &store,
            &mut session,
            &plan_path,
        )
        .unwrap_err();

        assert!(err.contains("repair attempts exhausted"), "{err}");
        assert!(
            err.contains("repair prompt saved: .anvil/repairs/repair-fix-report-"),
            "{err}"
        );
        assert!(
            err.contains(
                r#"suggested command: /ultra-plan-run --profile python "$(cat .anvil/repairs/repair-fix-report-"#
            ),
            "{err}"
        );
        assert!(err.contains("verify failed `python3 check.py`"), "{err}");
        let repair_files = std::fs::read_dir(temp.path().join(".anvil").join("repairs"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect::<Vec<_>>();
        assert_eq!(repair_files.len(), 1);
        let repair_prompt = std::fs::read_to_string(&repair_files[0]).unwrap();
        assert!(repair_prompt.contains("Repair failed step: fix-report"));
        assert!(repair_prompt.contains("Original goal excerpt:"));
        assert!(repair_prompt.contains("Verification commands:"));
        assert!(repair_prompt.contains("- python3 check.py"));
        assert!(repair_prompt.chars().count() <= REPAIR_REPLAN_PROMPT_MAX_CHARS);
        assert_eq!(
            std::fs::read_to_string(temp.path().join("report.md")).unwrap(),
            "still bad again\n"
        );

        let mut recovery_planner = MockClient::default();
        recovery_planner.push_reply(
            r#"{"goal":"Fix report.md so check.py passes","profile":"generic","style":"default","phases":[{"id":"repair-report","prompt":"Repair report.md so python3 check.py passes."},{"id":"verify-report","prompt":"Keep report.md valid and verify python3 check.py still passes."}]}"#,
            Vec::new(),
        );
        recovery_planner.push_reply(
            r#"{"goal":"Repair report.md so python3 check.py passes.","steps":[{"id":"fix-report","instruction":"Edit report.md so python3 check.py passes.","expected_paths":["report.md"],"verify":["python3 check.py"]}]}"#,
            Vec::new(),
        );
        recovery_planner.push_reply(
            r#"{"goal":"Keep report.md valid and verify python3 check.py still passes.","steps":[{"id":"verify-report","instruction":"Rewrite report.md with the verified good content and run python3 check.py.","expected_paths":["report.md"],"verify":["python3 check.py"]}]}"#,
            Vec::new(),
        );

        let mut recovery_client = MockClient::default();
        recovery_client.push_reply(
            "",
            vec![tool_call(
                "Edit",
                json!({"path":"report.md","old_string":"still bad again","new_string":"good"}),
            )],
        );
        recovery_client.push_reply("Fixed report.md and check.py passes.", Vec::new());
        recovery_client.push_reply(
            "",
            vec![tool_call(
                "Write",
                json!({"path":"report.md","content":"good\n"}),
            )],
        );
        recovery_client.push_reply("Verified report.md still passes.", Vec::new());

        let summary = generate_and_run_ultra_plan(
            &config(&temp),
            &mut recovery_planner,
            "qwen3:8b",
            &mut recovery_client,
            &store,
            &mut session,
            "Repair report.md so python3 check.py passes.",
            UltraProfile::Generic,
            UltraPlanStyle::Default,
        )
        .unwrap();

        assert_eq!(summary.phases.completed, 2);
        assert_eq!(
            std::fs::read_to_string(temp.path().join("report.md")).unwrap(),
            "good\n"
        );
    }

    #[test]
    fn repair_prompt_saved_for_suggested_ultra_plan_is_bounded() {
        let temp = tempfile::tempdir().unwrap();
        let plan = StepPlan {
            goal: "Ultra goal: ".to_string() + &"large context ".repeat(800),
            steps: vec![PlanStep {
                id: "verify-build".into(),
                kind: StepKind::Verify,
                instruction: "Run npm run build and repair the failed Next.js project.".repeat(80),
                expected_paths: vec!["package.json".into(), "app/page.tsx".into()],
                verify: vec!["npm run build".into()],
                expected_result: VerifyExpectedResult::Pass,
            }],
        };
        let report = VerificationReport {
            success: false,
            failures: (0..20)
                .map(|idx| {
                    format!(
                        "verify failed `npm run build` error {idx}: {}",
                        "TypeScript compiler output ".repeat(80)
                    )
                })
                .collect(),
        };
        let progress = super::repair::StepProgressReport {
            missing_before: Vec::new(),
            missing_after: Vec::new(),
            write_or_edit_paths: Vec::new(),
            repeated_write_or_edit_paths: Vec::new(),
            no_expected_path_progress: false,
        };

        let report_text = build_repair_exhausted_report(
            temp.path(),
            &plan,
            &plan.steps[0],
            &report,
            &progress,
            Some("minimal loop reached max_iterations (8)"),
            Some("minimal loop reached max_iterations (6)"),
            2,
            2,
        );

        assert!(
            report_text.contains(
                r#"suggested command: /ultra-plan-run --profile nextjs "$(cat .anvil/repairs/repair-verify-build-"#
            ),
            "{report_text}"
        );
        let repair_files = std::fs::read_dir(temp.path().join(".anvil").join("repairs"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect::<Vec<_>>();
        assert_eq!(repair_files.len(), 1);
        let repair_prompt = std::fs::read_to_string(&repair_files[0]).unwrap();
        assert!(
            repair_prompt.chars().count() <= REPAIR_REPLAN_PROMPT_MAX_CHARS,
            "repair prompt was {} chars",
            repair_prompt.chars().count()
        );
        assert!(repair_prompt.contains("[truncated]"));

        let ultra = UltraPlan {
            goal: repair_prompt.clone(),
            profile: UltraProfile::Nextjs,
            style: UltraPlanStyle::Default,
            intent: WorkIntent::Fix,
            phases: vec![UltraPhase {
                id: "repair-build".into(),
                prompt: "Repair only the current build failure, then run npm run build.".into(),
            }],
        };
        let phase_prompt = build_profiled_phase_prompt(
            &ultra,
            &ultra.phases[0],
            &ProfileSnapshot::new(
                vec![
                    "- package.json exists".into(),
                    "- app/page.tsx exists".into(),
                    "- components/SpaceInvaders.tsx exists".into(),
                ],
                Vec::new(),
            ),
            WorkIntent::Fix,
        );
        assert!(
            phase_prompt.chars().count() <= MAX_GOAL_CHARS,
            "expanded phase prompt was {} chars",
            phase_prompt.chars().count()
        );
    }

    #[test]
    fn saved_repair_prompt_preserves_verifier_diagnostic_excerpt() {
        let temp = tempfile::tempdir().unwrap();
        let plan = StepPlan {
            goal: "Fix the Next.js page".into(),
            steps: vec![PlanStep {
                id: "verify-build".into(),
                kind: StepKind::Verify,
                instruction: "Run npm run build and fix TypeScript errors.".into(),
                expected_paths: vec!["app/page.tsx".into()],
                verify: vec!["npm run build".into()],
                expected_result: VerifyExpectedResult::Pass,
            }],
        };
        let report = VerificationReport {
            success: false,
            failures: vec![format!(
                "verify failed `npm run build`: diagnostic excerpt:\n{}\n./app/page.tsx:43:18\nType error: Property 'lives' does not exist on type 'GameInternalState'.\n\nrelated source excerpt:\napp/page.tsx:40-46\n>   43 |   const {{ score, lives }} = game.renderState;",
                "build prelude\n".repeat(30)
            )],
        };
        let progress = super::repair::StepProgressReport {
            missing_before: Vec::new(),
            missing_after: Vec::new(),
            write_or_edit_paths: Vec::new(),
            repeated_write_or_edit_paths: Vec::new(),
            no_expected_path_progress: false,
        };

        let report_text = build_repair_exhausted_report(
            temp.path(),
            &plan,
            &plan.steps[0],
            &report,
            &progress,
            None,
            None,
            2,
            2,
        );

        assert!(
            report_text.contains("repair prompt saved: .anvil/repairs/repair-verify-build-"),
            "{report_text}"
        );
        let repair_files = std::fs::read_dir(temp.path().join(".anvil").join("repairs"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect::<Vec<_>>();
        assert_eq!(repair_files.len(), 1);
        let repair_prompt = std::fs::read_to_string(&repair_files[0]).unwrap();
        assert!(
            repair_prompt.contains("Type error: Property 'lives' does not exist"),
            "{repair_prompt}"
        );
        assert!(
            repair_prompt.contains("related source excerpt"),
            "{repair_prompt}"
        );
    }

    #[test]
    fn assurance_labels_are_reduced_only_for_generic_profile() {
        assert_eq!(
            assurance_for_profile(UltraProfile::Generic),
            AssuranceLevel::Reduced
        );
        assert_eq!(
            assurance_for_profile(UltraProfile::Nextjs),
            AssuranceLevel::Full
        );
        let reduced = UltraRunSummary {
            total: 2,
            completed: 2,
            assurance_level: AssuranceLevel::Reduced,
        };
        assert_eq!(reduced.status_label(), "completed (reduced assurance)");
        assert!(reduced.assurance_summary_line().is_some());

        let full = UltraRunSummary {
            total: 2,
            completed: 2,
            assurance_level: AssuranceLevel::Full,
        };
        assert_eq!(full.status_label(), "completed");
        assert_eq!(full.assurance_summary_line(), None);
    }

    #[test]
    fn run_ultra_plan_runs_each_phase_with_step_plans() {
        let temp = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let store = SessionStore::new(state.path(), "session-1", "workspace-1");
        let mut session = session_snapshot(ExecutionMode::Act);
        let ultra = UltraPlan {
            goal: "Create two files".into(),
            profile: UltraProfile::Generic,
            style: UltraPlanStyle::Default,
            intent: WorkIntent::Create,
            phases: vec![
                UltraPhase {
                    id: "create-a".into(),
                    prompt: "Create a.txt".into(),
                },
                UltraPhase {
                    id: "create-b".into(),
                    prompt: "Create b.txt".into(),
                },
            ],
        };
        let ultra_path = temp.path().join("ultra.yaml");
        std::fs::write(&ultra_path, render_ultra_plan_yaml(&ultra)).unwrap();

        let mut planner = MockClient::default();
        planner.push_reply(
            r#"{"goal":"Create a.txt","steps":[{"id":"create-a","instruction":"Create a.txt","expected_paths":["a.txt"],"verify":[]}]}"#,
            Vec::new(),
        );
        planner.push_reply(
            r#"{"goal":"Create b.txt","steps":[{"id":"create-b","instruction":"Create b.txt","expected_paths":["b.txt"],"verify":[]}]}"#,
            Vec::new(),
        );

        let mut client = MockClient::default();
        client.push_reply(
            "",
            vec![tool_call("Write", json!({"path":"a.txt","content":"a\n"}))],
        );
        client.push_reply("Created a.txt.", Vec::new());
        client.push_reply(
            "",
            vec![tool_call("Write", json!({"path":"b.txt","content":"b\n"}))],
        );
        client.push_reply("Created b.txt.", Vec::new());

        let summary = run_ultra_plan(
            &config(&temp),
            &mut planner,
            "qwen3:8b",
            &mut client,
            &store,
            &mut session,
            &ultra_path,
        )
        .unwrap();

        assert_eq!(summary.completed, 2);
        assert_eq!(summary.assurance_level, AssuranceLevel::Reduced);
        assert_eq!(summary.status_label(), "completed (reduced assurance)");
        assert!(temp.path().join("a.txt").is_file());
        assert!(temp.path().join("b.txt").is_file());
    }
}
