use std::collections::BTreeSet;
use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::session::store::{ConversationMessage, SessionSnapshot, SessionStore};
#[cfg(test)]
use crate::tools::registry::ToolSpec;

use super::minimal_loop::MinimalChatClient;
use super::minimal_repl::run_turn_with_early_success_paths;
use super::planner_llm::PlannerLlm;

mod plan_lint;
mod profile;
mod repair;
mod verify;

#[cfg(test)]
use plan_lint::lint_plan;
use plan_lint::{lint_plan_with_workspace, lint_ultra_plan};
#[cfg(test)]
use profile::ProfileSnapshot;
use profile::{
    build_profiled_phase_prompt, profile_generation_rules, profile_snapshot,
    verify_profile_after_phase,
};
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StepPlan {
    pub goal: String,
    pub steps: Vec<PlanStep>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanStep {
    pub id: String,
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
            "python" | "py" => Ok(Self::Python),
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
        let reply = planner.chat_plan(&messages)?;
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
                "The previous plan was invalid: {last_error}\nReturn corrected JSON only. Step instructions must be natural-language tasks for Write/Edit, not shell commands. All expected_paths must be repository-relative paths with no leading slash and no '..'. The verify array is only for deterministic checks; never put npm install, create-next-app, dev servers, network commands, or setup commands in verify. If setup is needed, describe it in the instruction and use an allowed verify command such as cat <file>, node --check <js-file>, or npm run build only after the app entry point exists. Do not use node --check for .ts or .tsx files."
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
            let repair_result = run_turn_with_early_success_paths(
                &repair_config,
                model,
                client,
                session_store,
                session,
                &repair_prompt,
                early_success_paths,
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
    let mut messages = vec![
        ConversationMessage::system(ultra_plan_generation_system_prompt(profile, style)),
        ConversationMessage::user(ultra_plan_generation_user_prompt(goal, profile, style)),
    ];
    let mut last_error = String::new();
    for attempt in 0..ULTRA_PLAN_GENERATION_ATTEMPTS {
        let reply = planner.chat_plan(&messages)?;
        if !reply.tool_calls.is_empty() {
            last_error = "ultra plan generation must not emit tool calls".to_string();
        } else {
            match parse_ultra_plan_json(&reply.content).and_then(|mut plan| {
                plan.goal = goal.to_string();
                plan.profile = profile;
                plan.style = style;
                validate_ultra_plan(&plan)?;
                lint_ultra_plan(&plan)?;
                Ok(plan)
            }) {
                Ok(plan) => return save_ultra_plan(&config.cwd, &plan),
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

    let mut completed = 0usize;
    for (index, phase) in ultra_plan.phases.iter().enumerate() {
        println!(
            "phase {}/{} {}: planning and running",
            index + 1,
            ultra_plan.phases.len(),
            phase.id
        );
        let snapshot = profile_snapshot(&config.cwd, ultra_plan.profile);
        let phase_prompt = build_profiled_phase_prompt(&ultra_plan, phase, &snapshot);
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
        verify_profile_after_phase(&config.cwd, ultra_plan.profile, &snapshot)
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

    Ok(UltraRunSummary {
        total: ultra_plan.phases.len(),
        completed,
    })
}

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

fn plan_generation_system_prompt() -> String {
    "You are Anvil in Plan mode. You do not execute tools. Produce a step plan for a local coding agent.\n\
Output JSON only, with this exact shape:\n\
{\"goal\":\"...\",\"steps\":[{\"id\":\"kebab-id\",\"instruction\":\"...\",\"expected_paths\":[\"relative/path\"],\"verify\":[\"npm run build\"],\"expected_result\":\"pass\"}]}\n\
Rules:\n\
- Split large tasks into small sequential steps.\n\
- Each step must be executable by a minimal local coding agent in one turn.\n\
- Step instruction must be natural language, not a shell command, and must name the concrete files it will create or edit.\n\
- If a step has multiple expected_paths, mention those files or component names in the instruction.\n\
- If the goal contains a Required final artifacts list, keep those exact repository-relative paths and include relevant paths in expected_paths. Do not rename or relocate them.\n\
- Put deterministic validation in expected_paths and verify.\n\
- expected_paths must be repository-relative paths with no leading slash and no '..'.\n\
- Use only safe local verify commands such as npm run build, npm test, cargo check, cargo test, python -m py_compile <file>, pytest, cat <file>, or node --check <js-file>.\n\
- The verify array is only for checks. Never include npm install, create-next-app, dev servers, package installation, or network/setup commands in verify.\n\
- Do not put npm run build on early scaffold/config steps. Use cat early, use node --check only for .js/.mjs/.cjs files, and place npm run build only after the app entry point exists.\n\
- For Next.js apps, include app/page.tsx or pages/index.tsx before the first npm run build. Config-only steps should create package.json, next.config.js, tailwind.config.js, postcss.config.js, and tsconfig.json without build verification.\n\
- Do not include long-running dev servers in verify.\n\
- Avoid network scaffolding unless the user explicitly requires it.\n\
- expected_result defaults to pass. Use expected_result:\"fail\" only for an explicit TDD red step where the verification command should fail before implementation.\n\
- Return 2 to 8 steps for most tasks, never more than 12."
        .to_string()
}

fn ultra_plan_generation_system_prompt(profile: UltraProfile, style: UltraPlanStyle) -> String {
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
    let profile_rules = profile_generation_rules(profile);
    format!(
        "You are Anvil's ultra planner. You do not execute tools. Produce a top-level phase plan whose phases will each be executed by /plan-run.\n\
Output JSON only, with this exact shape:\n\
{{\"goal\":\"...\",\"profile\":\"{}\",\"style\":\"{}\",\"phases\":[{{\"id\":\"kebab-id\",\"prompt\":\"focused /plan-run goal\"}}]}}\n\
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
        style.as_str()
    )
}

fn ultra_plan_generation_user_prompt(
    goal: &str,
    profile: UltraProfile,
    style: UltraPlanStyle,
) -> String {
    format!(
        "Create an ultra phase plan for this task using profile `{}` and style `{}`:\n{goal}",
        profile.as_str(),
        style.as_str()
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
                instruction: String::new(),
                expected_paths: Vec::new(),
                verify: Vec::new(),
                expected_result: VerifyExpectedResult::Pass,
            });
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
        if step.expected_result == VerifyExpectedResult::Fail && step.verify.is_empty() {
            return Err(format!(
                "step {} expected_result fail requires at least one verify command",
                step.id
            ));
        }
    }
    Ok(())
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
    let shell_syntax = ["&&", "||", ";", "|", "`", "$("];
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
        let mut config = Config::default();
        config.cwd = root.path().to_path_buf();
        config.context_budget = 24_000;
        config.max_iterations = 4;
        config.yes_mode = true;
        config
    }

    fn tool_call(name: &str, arguments: serde_json::Value) -> ToolCall {
        ToolCall {
            id: format!("call_{name}"),
            name: name.to_string(),
            arguments,
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
    }

    #[test]
    fn invalid_verify_command_is_rejected() {
        let plan = StepPlan {
            goal: "x".into(),
            steps: vec![PlanStep {
                id: "bad".into(),
                instruction: "x".into(),
                expected_paths: vec![],
                verify: vec!["npm install left-pad".into()],
                expected_result: VerifyExpectedResult::Pass,
            }],
        };
        assert!(validate_plan(&plan).is_err());
    }

    #[test]
    fn shell_like_instruction_is_rejected() {
        let plan = StepPlan {
            goal: "x".into(),
            steps: vec![PlanStep {
                id: "bad".into(),
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
    fn natural_language_instruction_may_mention_html_tags() {
        assert!(
            validate_instruction(
                "Implement AnalyticsPanel.tsx using semantic HTML like <dl> and <div>."
            )
            .is_ok()
        );
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
    fn data_profile_verifier_rejects_raw_input_changes() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join("data/raw")).unwrap();
        let raw = temp.path().join("data/raw/sales.csv");
        std::fs::write(&raw, "date,total\n2026-01-01,42\n").unwrap();
        let snapshot = profile_snapshot(temp.path(), UltraProfile::DataPipeline);

        std::fs::write(&raw, "date,total\n2026-01-01,99\nextra,row\n").unwrap();

        let err = verify_profile_after_phase(temp.path(), UltraProfile::DataPipeline, &snapshot)
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

        let err =
            verify_profile_after_phase(temp.path(), UltraProfile::Nextjs, &snapshot).unwrap_err();
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

        verify_profile_after_phase(temp.path(), UltraProfile::Nextjs, &snapshot).unwrap();
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

        let err =
            verify_profile_after_phase(temp.path(), UltraProfile::Nextjs, &snapshot).unwrap_err();

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

        verify_profile_after_phase(temp.path(), UltraProfile::Nextjs, &snapshot).unwrap();
    }

    #[test]
    fn expected_failure_verify_treats_nonzero_as_success() {
        let temp = tempfile::tempdir().unwrap();
        let step = PlanStep {
            id: "red-test".into(),
            instruction: "Add a failing test".into(),
            expected_paths: vec![],
            verify: vec!["cat missing.txt".into()],
            expected_result: VerifyExpectedResult::Fail,
        };
        assert!(verify_step(temp.path(), &step).success);

        std::fs::write(temp.path().join("present.txt"), "ok\n").unwrap();
        let step = PlanStep {
            id: "red-test".into(),
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
    fn nextjs_build_before_entry_path_is_rejected_by_lint() {
        let plan = StepPlan {
            goal: "Create a Next.js app".into(),
            steps: vec![
                PlanStep {
                    id: "init-next-project".into(),
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
        let plan = StepPlan {
            goal: "Modify an existing Next.js app".into(),
            steps: vec![PlanStep {
                id: "integrate-panel".into(),
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
            phases: vec![UltraPhase {
                id: "game".into(),
                prompt: "Implement the game screen.".into(),
            }],
        };
        let prompt = build_profiled_phase_prompt(
            &ultra,
            &ultra.phases[0],
            &ProfileSnapshot {
                lines: Vec::new(),
                protected_files: Vec::new(),
            },
        );

        assert!(prompt.contains("Required final artifacts from the overall goal"));
        assert!(prompt.contains("components/SpaceOpsGame.tsx"));
    }

    #[test]
    fn multi_path_step_instruction_must_name_concrete_files() {
        let plan = StepPlan {
            goal: "Create config".into(),
            steps: vec![PlanStep {
                id: "setup-config".into(),
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
                    instruction: "Create package.json and app/page.tsx.".into(),
                    expected_paths: vec!["package.json".into(), "app/page.tsx".into()],
                    verify: vec!["cat app/page.tsx".into()],
                    expected_result: VerifyExpectedResult::Pass,
                },
                PlanStep {
                    id: "validate-build".into(),
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
        let mut session = SessionSnapshot::default();
        session.id = "session-1".to_string();
        session.workspace_key = "workspace-1".to_string();
        session.mode_state.mode = ExecutionMode::Act;
        let plan = StepPlan {
            goal: "Create report".into(),
            steps: vec![PlanStep {
                id: "report".into(),
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
        let mut session = SessionSnapshot::default();
        session.id = "session-1".to_string();
        session.workspace_key = "workspace-1".to_string();
        session.mode_state.mode = ExecutionMode::Act;
        let plan = StepPlan {
            goal: "Create report".into(),
            steps: vec![PlanStep {
                id: "report".into(),
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
        let mut session = SessionSnapshot::default();
        session.id = "session-1".to_string();
        session.workspace_key = "workspace-1".to_string();
        session.mode_state.mode = ExecutionMode::Act;
        let plan = StepPlan {
            goal: "Fix report".into(),
            steps: vec![PlanStep {
                id: "report".into(),
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
        let mut session = SessionSnapshot::default();
        session.id = "session-1".to_string();
        session.workspace_key = "workspace-1".to_string();
        session.mode_state.mode = ExecutionMode::Act;
        let plan = StepPlan {
            goal: "Fix report.md so check.py passes".into(),
            steps: vec![PlanStep {
                id: "fix-report".into(),
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
        assert!(repair_prompt.contains("Repair failed step fix-report."));
        assert!(repair_prompt.contains("Original goal:"));
        assert!(repair_prompt.contains("Verification commands:"));
        assert!(repair_prompt.contains("- python3 check.py"));
        assert!(repair_prompt.chars().count() <= MAX_GOAL_CHARS);
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
            repair_prompt.chars().count() <= MAX_GOAL_CHARS,
            "repair prompt was {} chars",
            repair_prompt.chars().count()
        );
        assert!(repair_prompt.contains("[truncated]"));
    }

    #[test]
    fn run_ultra_plan_runs_each_phase_with_step_plans() {
        let temp = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let store = SessionStore::new(state.path(), "session-1", "workspace-1");
        let mut session = SessionSnapshot::default();
        session.id = "session-1".to_string();
        session.workspace_key = "workspace-1".to_string();
        session.mode_state.mode = ExecutionMode::Act;
        let ultra = UltraPlan {
            goal: "Create two files".into(),
            profile: UltraProfile::Generic,
            style: UltraPlanStyle::Default,
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
        assert!(temp.path().join("a.txt").is_file());
        assert!(temp.path().join("b.txt").is_file());
    }
}
