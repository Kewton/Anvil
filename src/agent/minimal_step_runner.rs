use std::collections::BTreeSet;
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::str::FromStr;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::session::store::{ConversationMessage, SessionSnapshot, SessionStore};
use crate::tools::registry::ToolSpec;

use super::minimal_loop::MinimalChatClient;
use super::minimal_repl::run_turn;

const MAX_STEPS: usize = 12;
const MAX_STRING_CHARS: usize = 1_200;
const MAX_LIST_ITEMS: usize = 12;
const PLAN_GENERATION_ATTEMPTS: usize = 3;
const STEP_TURN_MAX_ITERATIONS: usize = 8;
const STEP_REPAIR_MAX_ITERATIONS: usize = 4;
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct VerificationReport {
    success: bool,
    failures: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StepProgressReport {
    missing_before: Vec<String>,
    missing_after: Vec<String>,
    write_or_edit_paths: Vec<String>,
    repeated_write_or_edit_paths: Vec<String>,
    no_expected_path_progress: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProfileSnapshot {
    lines: Vec<String>,
    protected_files: Vec<ProtectedFile>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProtectedFile {
    path: String,
    len: u64,
}

pub fn generate_step_plan<C: MinimalChatClient>(
    config: &Config,
    model: &str,
    client: &mut C,
    goal: &str,
) -> Result<PathBuf, String> {
    let mut messages = vec![
        ConversationMessage::system(plan_generation_system_prompt()),
        ConversationMessage::user(plan_generation_user_prompt(goal)),
    ];
    let mut last_error = String::new();
    for attempt in 0..PLAN_GENERATION_ATTEMPTS {
        let reply = client.chat(model, &messages, &[] as &[ToolSpec], false)?;
        if !reply.tool_calls.is_empty() {
            last_error = "plan generation must not emit tool calls".to_string();
        } else {
            match parse_plan_json(&reply.content).and_then(|plan| {
                validate_plan(&plan)?;
                lint_plan(&plan)?;
                Ok(plan)
            }) {
                Ok(plan) => return save_plan(&config.cwd, &plan),
                Err(err) => last_error = err,
            }
        }
        if attempt + 1 < PLAN_GENERATION_ATTEMPTS {
            messages.push(ConversationMessage::user(format!(
                "The previous plan was invalid: {last_error}\nReturn corrected JSON only. Step instructions must be natural-language tasks for Write/Edit, not shell commands. All expected_paths must be repository-relative paths with no leading slash and no '..'. The verify array is only for deterministic checks; never put npm install, create-next-app, dev servers, network commands, or setup commands in verify. If setup is needed, describe it in the instruction and use an allowed verify command such as cat <file>, node --check <file>, or npm run build only after the app entry point exists."
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
    lint_plan(&plan)?;
    let step_config = capped_config(config, STEP_TURN_MAX_ITERATIONS);
    let repair_config = capped_config(config, STEP_REPAIR_MAX_ITERATIONS);

    let mut completed = 0usize;
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
        let turn_result = run_turn(&step_config, model, client, session_store, session, &prompt);
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
            println!("step {}: ok", step.id);
            continue;
        }

        println!(
            "step {}: verification failed; running one repair turn",
            step.id
        );
        let progress = analyze_step_progress(session, turn_start, missing_before, &report);
        let repair_prompt =
            build_repair_prompt(&plan, step, &report, &progress, turn_error.as_deref());
        let repair_result = run_turn(
            &repair_config,
            model,
            client,
            session_store,
            session,
            &repair_prompt,
        );
        match repair_result {
            Ok(reply) => {
                if !reply.is_empty() {
                    println!("{reply}");
                }
            }
            Err(err) => {
                println!(
                    "step {}: repair turn stopped before completion: {err}",
                    step.id
                );
            }
        }
        report = verify_step(&config.cwd, step);

        if !report.success {
            return Err(format!(
                "step {} failed verification: {}",
                step.id,
                report.failures.join("; ")
            ));
        }
        completed += 1;
        println!("step {}: ok", step.id);
    }

    Ok(StepRunSummary {
        total: plan.steps.len(),
        completed,
    })
}

pub fn generate_and_run_step_plan<C: MinimalChatClient>(
    config: &Config,
    model: &str,
    client: &mut C,
    session_store: &SessionStore,
    session: &mut SessionSnapshot,
    goal: &str,
) -> Result<PlanRunSummary, String> {
    let plan_path = generate_step_plan(config, model, client, goal)?;
    let steps = run_plan(config, model, client, session_store, session, &plan_path)?;
    Ok(PlanRunSummary { plan_path, steps })
}

pub fn generate_ultra_plan<C: MinimalChatClient>(
    config: &Config,
    model: &str,
    client: &mut C,
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
        let reply = client.chat(model, &messages, &[] as &[ToolSpec], false)?;
        if !reply.tool_calls.is_empty() {
            last_error = "ultra plan generation must not emit tool calls".to_string();
        } else {
            match parse_ultra_plan_json(&reply.content).and_then(|plan| {
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

pub fn run_ultra_plan<C: MinimalChatClient>(
    config: &Config,
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

pub fn generate_and_run_ultra_plan<C: MinimalChatClient>(
    config: &Config,
    model: &str,
    client: &mut C,
    session_store: &SessionStore,
    session: &mut SessionSnapshot,
    goal: &str,
    profile: UltraProfile,
    style: UltraPlanStyle,
) -> Result<UltraPlanRunSummary, String> {
    let plan_path = generate_ultra_plan(config, model, client, goal, profile, style)?;
    let phases = run_ultra_plan(config, model, client, session_store, session, &plan_path)?;
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
- Put deterministic validation in expected_paths and verify.\n\
- expected_paths must be repository-relative paths with no leading slash and no '..'.\n\
- Use only safe local verify commands such as npm run build, npm test, cargo check, cargo test, python -m py_compile <file>, pytest, cat <file>, or node --check <file>.\n\
- The verify array is only for checks. Never include npm install, create-next-app, dev servers, package installation, or network/setup commands in verify.\n\
- Do not put npm run build on early scaffold/config steps. Use cat or node --check early, and place npm run build only after the app entry point exists.\n\
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

fn profile_generation_rules(profile: UltraProfile) -> &'static str {
    match profile {
        UltraProfile::Generic => "",
        UltraProfile::Nextjs => {
            "- Profile nextjs: preserve the existing Next.js structure when present. Keep package.json as a Next.js package, keep app/ or pages/ entrypoints, and end with a build verification phase.\n"
        }
        UltraProfile::Python => {
            "- Profile python: preserve the package/import layout, prefer pytest or python -m py_compile checks, and add tests for behavioral changes when practical.\n"
        }
        UltraProfile::Rust => {
            "- Profile rust: preserve Cargo.toml and crate entrypoints, prefer cargo check/cargo test checks, and keep changes scoped to the requested crate behavior.\n"
        }
        UltraProfile::Investigation => {
            "- Profile investigation: produce a concrete triage/report artifact, separate facts from hypotheses, and do not modify source code unless the user explicitly asks for fixes.\n"
        }
        UltraProfile::Docs => {
            "- Profile docs: produce or update documentation artifacts, avoid source-code changes unless explicitly requested, and include a final review phase for accuracy.\n"
        }
        UltraProfile::DataAnalysis => {
            "- Profile data-analysis: treat input data as read-only. First inspect local files, schema, headers, row counts, missingness, and samples using scripts or shell. Produce reusable analysis scripts under scripts/ when needed and a human-readable report under reports/ or docs/. Do not require network access. Do not put raw data into prompts except small samples or summaries.\n"
        }
        UltraProfile::DataPipeline => {
            "- Profile data-pipeline: treat raw input data as read-only. Create reusable extraction/cleaning/validation scripts under scripts/ and processed outputs under data/processed/. Include checks for row counts, schema, missing values, and reproducibility. Do not require network access unless the user explicitly asks and grants it.\n"
        }
    }
}

fn profile_snapshot(work_root: &Path, profile: UltraProfile) -> ProfileSnapshot {
    let mut lines = Vec::new();
    let mut protected_files = Vec::new();
    match profile {
        UltraProfile::DataAnalysis | UltraProfile::DataPipeline => {
            let data_files = discover_data_files(work_root);
            if data_files.is_empty() {
                lines.push("No local data files were detected yet.".to_string());
            } else {
                lines.push("Detected local data files:".to_string());
                for path in data_files.iter().take(12) {
                    let full_path = work_root.join(path);
                    let len = std::fs::metadata(&full_path).map(|m| m.len()).unwrap_or(0);
                    lines.push(format!("- {} ({} bytes)", path.display(), len));
                    if let Some(header) = data_file_header(&full_path) {
                        lines.push(format!("  header/sample: {header}"));
                    }
                    if is_protected_data_input(path) {
                        protected_files.push(ProtectedFile {
                            path: path.to_string_lossy().to_string(),
                            len,
                        });
                    }
                }
            }
            for dir in ["data/raw", "data/processed", "scripts", "reports", "docs"] {
                if work_root.join(dir).exists() {
                    lines.push(format!("Existing directory: {dir}"));
                }
            }
        }
        UltraProfile::Nextjs => {
            for path in [
                "package.json",
                "tsconfig.json",
                "app/page.tsx",
                "app/layout.tsx",
                "pages/index.tsx",
                "next.config.js",
            ] {
                if work_root.join(path).exists() {
                    lines.push(format!("Existing file: {path}"));
                }
            }
            if let Some(summary) = package_json_summary(&work_root.join("package.json")) {
                lines.push(summary);
            }
        }
        UltraProfile::Python => {
            for path in ["pyproject.toml", "requirements.txt", "src", "tests"] {
                if work_root.join(path).exists() {
                    lines.push(format!("Existing path: {path}"));
                }
            }
        }
        UltraProfile::Rust => {
            for path in ["Cargo.toml", "src/lib.rs", "src/main.rs", "tests"] {
                if work_root.join(path).exists() {
                    lines.push(format!("Existing path: {path}"));
                }
            }
        }
        UltraProfile::Investigation | UltraProfile::Docs | UltraProfile::Generic => {}
    }
    ProfileSnapshot {
        lines,
        protected_files,
    }
}

fn build_profiled_phase_prompt(
    ultra_plan: &UltraPlan,
    phase: &UltraPhase,
    snapshot: &ProfileSnapshot,
) -> String {
    let snapshot = if snapshot.lines.is_empty() {
        "- none detected".to_string()
    } else {
        snapshot.lines.join("\n")
    };
    format!(
        "Ultra goal:\n{goal}\n\nUltra profile: {profile}\nUltra style: {style}\n\nCurrent phase id: {id}\nCurrent phase goal:\n{prompt}\n\nExisting workspace snapshot:\n{snapshot}\n\nProfile contract:\n{contract}\n\nRun only this phase. Preserve the profile contract. Prefer Read/Bash inspection before changing existing project structure. Use Write/Edit for file changes. End with deterministic checks when practical.",
        goal = ultra_plan.goal,
        profile = ultra_plan.profile,
        style = ultra_plan.style,
        id = phase.id,
        prompt = phase.prompt,
        snapshot = snapshot,
        contract = profile_runtime_contract(ultra_plan.profile),
    )
}

fn profile_runtime_contract(profile: UltraProfile) -> &'static str {
    match profile {
        UltraProfile::Generic => "- Keep changes scoped to the current phase.",
        UltraProfile::Nextjs => {
            "- Preserve the workspace as a Next.js app when one exists.\n- Do not convert package.json to a standalone TypeScript/Node project.\n- Keep next/react/react-dom dependencies when already present.\n- Keep scripts.build as next build when already present.\n- If a 3011 port requirement exists, keep the dev script on port 3011.\n- Do not set tsconfig rootDir to ./src in a way that excludes app/."
        }
        UltraProfile::Python => {
            "- Preserve the existing Python package/import layout.\n- Prefer pytest and python -m py_compile for verification.\n- Do not rewrite project metadata unless this phase explicitly requires it."
        }
        UltraProfile::Rust => {
            "- Preserve Cargo.toml and crate entrypoints.\n- Prefer cargo check/cargo test for verification.\n- Keep public behavior scoped to the requested phase."
        }
        UltraProfile::Investigation => {
            "- Produce a concrete report artifact.\n- Separate observed facts, hypotheses, and proposed next steps.\n- Do not modify source code unless the phase explicitly asks for fixes."
        }
        UltraProfile::Docs => {
            "- Produce or update documentation artifacts.\n- Avoid source-code changes unless explicitly requested.\n- Keep claims grounded in files inspected during this phase."
        }
        UltraProfile::DataAnalysis => {
            "- Treat raw/input data files as read-only.\n- Do not paste full datasets into prompts; use schema, samples, counts, and summaries.\n- Put reusable analysis code under scripts/ when needed.\n- Put human-readable findings under reports/ or docs/.\n- Stay local-only unless the user explicitly requested network access."
        }
        UltraProfile::DataPipeline => {
            "- Treat raw/input data files as read-only.\n- Put reusable extraction/cleaning/validation scripts under scripts/.\n- Put processed outputs under data/processed/.\n- Include deterministic checks for row counts, schema, missing values, or reproducibility.\n- Stay local-only unless the user explicitly requested network access."
        }
    }
}

fn verify_profile_after_phase(
    work_root: &Path,
    profile: UltraProfile,
    before: &ProfileSnapshot,
) -> Result<(), String> {
    let mut failures = Vec::new();
    if matches!(
        profile,
        UltraProfile::DataAnalysis | UltraProfile::DataPipeline
    ) {
        for protected in &before.protected_files {
            let path = work_root.join(&protected.path);
            match std::fs::metadata(&path) {
                Ok(meta) if meta.len() == protected.len => {}
                Ok(meta) => failures.push(format!(
                    "protected input data changed: {} ({} -> {} bytes)",
                    protected.path,
                    protected.len,
                    meta.len()
                )),
                Err(_) => {
                    failures.push(format!("protected input data missing: {}", protected.path))
                }
            }
        }
    }
    if profile == UltraProfile::Nextjs {
        verify_nextjs_profile(work_root, &mut failures);
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("; "))
    }
}

fn verify_nextjs_profile(work_root: &Path, failures: &mut Vec<String>) {
    let package_path = work_root.join("package.json");
    let app_dir_exists = work_root.join("app").is_dir() || work_root.join("pages").is_dir();
    if package_path.exists() && app_dir_exists {
        let Ok(raw) = std::fs::read_to_string(&package_path) else {
            failures.push("package.json exists but could not be read".to_string());
            return;
        };
        if raw.contains("\"next\"") {
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&raw) {
                let build = json
                    .pointer("/scripts/build")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if !build.is_empty() && build != "next build" {
                    failures.push(format!(
                        "package.json build script is no longer `next build`: {build}"
                    ));
                }
                let dev = json
                    .pointer("/scripts/dev")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if dev.contains("3011") && !dev.contains("next dev") {
                    failures.push(format!(
                        "package.json dev script mentions 3011 but is not next dev: {dev}"
                    ));
                }
            }
        } else {
            failures.push("package.json no longer contains next dependency".to_string());
        }
    }
    let tsconfig_path = work_root.join("tsconfig.json");
    if tsconfig_path.exists()
        && app_dir_exists
        && let Ok(raw) = std::fs::read_to_string(tsconfig_path)
        && raw.contains("\"rootDir\"")
        && raw.contains("\"./src\"")
    {
        failures.push("tsconfig.json rootDir ./src excludes Next.js app/ files".to_string());
    }
}

fn discover_data_files(work_root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    discover_data_files_inner(work_root, work_root, 0, &mut out);
    out.sort();
    out
}

fn discover_data_files_inner(root: &Path, current: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if depth > 4 || out.len() >= 48 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(current) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') || name == "node_modules" || name == "target" {
            continue;
        }
        if path.is_dir() {
            discover_data_files_inner(root, &path, depth + 1, out);
        } else if is_data_file(&path)
            && let Ok(relative) = path.strip_prefix(root)
        {
            out.push(relative.to_path_buf());
        }
    }
}

fn is_data_file(path: &Path) -> bool {
    let Some(ext) = path.extension().and_then(|value| value.to_str()) else {
        return false;
    };
    matches!(
        ext.to_ascii_lowercase().as_str(),
        "csv" | "tsv" | "json" | "jsonl" | "ndjson" | "parquet" | "xlsx" | "xls" | "sqlite" | "db"
    )
}

fn is_protected_data_input(path: &Path) -> bool {
    let first = path
        .components()
        .next()
        .and_then(|component| match component {
            std::path::Component::Normal(value) => value.to_str(),
            _ => None,
        });
    if first == Some("data") {
        let second = path
            .components()
            .nth(1)
            .and_then(|component| match component {
                std::path::Component::Normal(value) => value.to_str(),
                _ => None,
            });
        return second != Some("processed");
    }
    is_data_file(path)
}

fn data_file_header(path: &Path) -> Option<String> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    if !matches!(ext.as_str(), "csv" | "tsv" | "json" | "jsonl" | "ndjson") {
        return None;
    }
    let raw = std::fs::read_to_string(path).ok()?;
    raw.lines()
        .next()
        .map(|line| line.chars().take(180).collect::<String>())
}

fn package_json_summary(path: &Path) -> Option<String> {
    let raw = std::fs::read_to_string(path).ok()?;
    let json = serde_json::from_str::<serde_json::Value>(&raw).ok()?;
    let build = json
        .pointer("/scripts/build")
        .and_then(|value| value.as_str())
        .unwrap_or("none");
    let dev = json
        .pointer("/scripts/dev")
        .and_then(|value| value.as_str())
        .unwrap_or("none");
    let deps = json
        .get("dependencies")
        .and_then(|value| value.as_object())
        .map(|map| map.keys().take(8).cloned().collect::<Vec<_>>().join(", "))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "none".to_string());
    Some(format!(
        "package.json scripts: build=`{build}`, dev=`{dev}`; dependencies: {deps}"
    ))
}

fn plan_generation_user_prompt(goal: &str) -> String {
    format!("Create a step plan for this task:\n{goal}")
}

fn build_step_prompt(plan: &StepPlan, step: &PlanStep) -> String {
    format!(
        "Overall goal:\n{goal}\n\nCurrent step id: {id}\nCurrent step instruction:\n{instruction}\n\nExpected paths after this step:\n{paths}\n\nVerification commands for this step:\n{verify}\n\nExpected verification result: {expected_result}\n\nWork only on this step. Use Write/Edit for file changes. Do not rely on network scaffolding unless this step explicitly says so. Create every expected path before revising an already-created file repeatedly. Before giving a final answer, make the expected paths exist and run the verification commands when possible. If expected_result is pass and verification fails, fix only this step's failure. If expected_result is fail, this is a TDD red step: the verification command should fail after the test is written.",
        goal = plan.goal,
        id = step.id,
        instruction = step.instruction,
        paths = bullet_list(&step.expected_paths),
        verify = bullet_list(&step.verify),
        expected_result = step.expected_result,
    )
}

fn build_repair_prompt(
    plan: &StepPlan,
    step: &PlanStep,
    report: &VerificationReport,
    progress: &StepProgressReport,
    turn_error: Option<&str>,
) -> String {
    let turn_error = turn_error
        .map(|err| format!("\nPrevious turn stop reason:\n- {err}\n"))
        .unwrap_or_default();
    let progress_note = if progress.no_expected_path_progress {
        format!(
            "\nStep progress warning:\n- Missing expected paths did not decrease.\n- Missing before: {}\n- Missing now: {}\n- Write/Edit paths observed: {}\n- Repeated Write/Edit paths: {}\n\nDo not keep rewriting only the observed existing files. Emit Write/Edit tool calls for the missing expected paths first.\n",
            inline_list(&progress.missing_before),
            inline_list(&progress.missing_after),
            inline_list(&progress.write_or_edit_paths),
            inline_list(&progress.repeated_write_or_edit_paths),
        )
    } else {
        String::new()
    };
    format!(
        "The previous step did not pass deterministic verification.{turn_error}\nOverall goal:\n{goal}\n\nCurrent step id: {id}\nCurrent step instruction:\n{instruction}\n\nExpected paths for this step:\n{paths}\n\nExpected verification result: {expected_result}\n\nVerification failures:\n{failures}\n{progress_note}\nRepair only this step. Create or edit the missing/incorrect files, then run the verification commands if possible. Do not move to later steps. If expected_result is fail, do not implement the production fix in this step; make the intended red test fail for the right reason.",
        goal = plan.goal,
        id = step.id,
        instruction = step.instruction,
        paths = bullet_list(&step.expected_paths),
        expected_result = step.expected_result,
        failures = bullet_list(&report.failures),
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
    validate_text("goal", &plan.goal)?;
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
    validate_text("goal", &plan.goal)?;
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

fn lint_plan(plan: &StepPlan) -> Result<(), String> {
    let mut errors = Vec::new();
    lint_instruction_specificity(plan, &mut errors);
    lint_nextjs_build_order(plan, &mut errors);
    if errors.is_empty() {
        Ok(())
    } else {
        Err(format!("plan lint failed: {}", errors.join("; ")))
    }
}

fn lint_ultra_plan(plan: &UltraPlan) -> Result<(), String> {
    let mut errors = Vec::new();
    if matches!(plan.style, UltraPlanStyle::Tdd) {
        let combined = plan
            .phases
            .iter()
            .map(|phase| phase.prompt.to_ascii_lowercase())
            .collect::<Vec<_>>()
            .join("\n");
        if !combined.contains("fail") && !combined.contains("red") && !combined.contains("失敗") {
            errors.push("TDD ultra plan must include a failing/red test phase".to_string());
        }
        if !combined.contains("test")
            && !combined.contains("cargo test")
            && !combined.contains("pytest")
            && !combined.contains("npm test")
            && !combined.contains("テスト")
        {
            errors.push("TDD ultra plan must mention tests".to_string());
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(format!("ultra plan lint failed: {}", errors.join("; ")))
    }
}

fn lint_instruction_specificity(plan: &StepPlan, errors: &mut Vec<String>) {
    for step in &plan.steps {
        if step.expected_paths.len() < 2 {
            continue;
        }
        let instruction = step.instruction.to_ascii_lowercase();
        let mentions_expected_path = step
            .expected_paths
            .iter()
            .any(|path| instruction_mentions_path(&instruction, path));
        if !mentions_expected_path {
            errors.push(format!(
                "step {} has multiple expected paths but the instruction does not name any concrete expected file",
                step.id
            ));
        }
    }
}

fn lint_nextjs_build_order(plan: &StepPlan, errors: &mut Vec<String>) {
    if !plan_looks_like_nextjs(plan) {
        return;
    }

    let all_expected = plan
        .steps
        .iter()
        .flat_map(|step| step.expected_paths.iter())
        .collect::<Vec<_>>();
    if !all_expected.iter().any(|path| is_nextjs_entry_path(path)) {
        errors.push(
            "Next.js plan must include app/page.tsx or pages/index.tsx in expected_paths"
                .to_string(),
        );
    }

    let mut package_seen = false;
    let mut entry_seen = false;
    for step in &plan.steps {
        package_seen |= step
            .expected_paths
            .iter()
            .any(|path| path == "package.json");
        entry_seen |= step
            .expected_paths
            .iter()
            .any(|path| is_nextjs_entry_path(path));
        if step.verify.iter().any(|command| command == "npm run build")
            && (!package_seen || !entry_seen)
        {
            errors.push(format!(
                "step {} runs npm run build before package.json and a Next.js entry path are present",
                step.id
            ));
        }
    }
}

fn plan_looks_like_nextjs(plan: &StepPlan) -> bool {
    let goal = plan.goal.to_ascii_lowercase();
    goal.contains("next.js")
        || goal.contains("nextjs")
        || plan.steps.iter().any(|step| {
            let instruction = step.instruction.to_ascii_lowercase();
            instruction.contains("next.js")
                || instruction.contains("nextjs")
                || step
                    .expected_paths
                    .iter()
                    .any(|path| path == "next.config.js" || is_nextjs_entry_path(path))
        })
}

fn is_nextjs_entry_path(path: &str) -> bool {
    matches!(
        path,
        "app/page.tsx" | "app/page.jsx" | "pages/index.tsx" | "pages/index.jsx"
    )
}

fn instruction_mentions_path(instruction_lower: &str, path: &str) -> bool {
    let path_lower = path.to_ascii_lowercase();
    if instruction_lower.contains(&path_lower) {
        return true;
    }
    let path = Path::new(path);
    if let Some(file_name) = path.file_name().and_then(|value| value.to_str()) {
        let file_name = file_name.to_ascii_lowercase();
        if instruction_lower.contains(&file_name) {
            return true;
        }
    }
    if let Some(stem) = path.file_stem().and_then(|value| value.to_str()) {
        let stem = stem.to_ascii_lowercase();
        if stem.len() >= 4 && instruction_lower.contains(&stem) {
            return true;
        }
    }
    false
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

fn validate_instruction(value: &str) -> Result<(), String> {
    validate_text("instruction", value)?;
    let trimmed = value.trim();
    let lower = trimmed.to_ascii_lowercase();
    let shell_starts = ["echo ", "cat ", "mkdir ", "touch ", "cd "];
    let shell_syntax = ["&&", "||", ";", "|", ">", "<", "`", "$("];
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

fn validate_verify_command(command: &str) -> Result<(), String> {
    if command.trim().is_empty() {
        return Err("verify command must not be empty".to_string());
    }
    if command.len() > 240 {
        return Err(format!("verify command is too long: {command}"));
    }
    let forbidden = ["&&", "||", ";", "|", ">", "<", "`", "$(", "\n", "\r"];
    if forbidden.iter().any(|needle| command.contains(needle)) {
        return Err(format!(
            "verify command contains shell control syntax: {command}"
        ));
    }
    let parts = command.split_whitespace().collect::<Vec<_>>();
    let allowed = match parts.as_slice() {
        ["npm", "run", "build"] | ["npm", "run", "test"] | ["npm", "test"] => true,
        ["cargo", "check"] | ["cargo", "test"] => true,
        ["cargo", "check", rest @ ..] | ["cargo", "test", rest @ ..] => rest
            .iter()
            .all(|part| safe_command_argument(part) && !part.eq_ignore_ascii_case("--release")),
        ["python", "-m", "py_compile", rest @ ..] | ["python3", "-m", "py_compile", rest @ ..] => {
            !rest.is_empty() && rest.iter().all(|part| safe_command_argument(part))
        }
        ["python", "-m", "pytest", rest @ ..] | ["python3", "-m", "pytest", rest @ ..] => {
            rest.iter().all(|part| safe_command_argument(part))
        }
        ["python", script, rest @ ..] | ["python3", script, rest @ ..] => {
            safe_python_script_path(script) && rest.iter().all(|part| safe_command_argument(part))
        }
        ["pytest", rest @ ..] => rest.iter().all(|part| safe_command_argument(part)),
        ["node", "--check", rest @ ..] => {
            !rest.is_empty() && rest.iter().all(|part| safe_command_argument(part))
        }
        ["cat", rest @ ..] => {
            !rest.is_empty() && rest.iter().all(|part| safe_command_argument(part))
        }
        _ => false,
    };
    if allowed {
        Ok(())
    } else {
        Err(format!(
            "verify command is not in the safe allowlist: {command}"
        ))
    }
}

fn safe_python_script_path(value: &str) -> bool {
    safe_command_argument(value)
        && value.ends_with(".py")
        && !value.starts_with('-')
        && !Path::new(value).is_absolute()
}

fn safe_command_argument(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '/' | '_' | '-' | '=' | ':'))
        && !value.contains("..")
}

fn verify_step(work_root: &Path, step: &PlanStep) -> VerificationReport {
    let mut failures = Vec::new();
    for path in missing_expected_paths(work_root, step) {
        failures.push(format!("missing expected path: {path}"));
    }
    for command in &step.verify {
        match (step.expected_result, run_verify_command(work_root, command)) {
            (VerifyExpectedResult::Pass, Ok(())) => {}
            (VerifyExpectedResult::Pass, Err(err)) => {
                failures.push(format!("verify failed `{command}`: {err}"));
            }
            (VerifyExpectedResult::Fail, Ok(())) => {
                failures.push(format!("verify unexpectedly passed `{command}`"));
            }
            (VerifyExpectedResult::Fail, Err(_)) => {}
        }
    }
    VerificationReport {
        success: failures.is_empty(),
        failures,
    }
}

fn missing_expected_paths(work_root: &Path, step: &PlanStep) -> Vec<String> {
    let mut missing = Vec::new();
    for path in &step.expected_paths {
        let Some(normalized) = normalize_relative_path(Path::new(path)) else {
            missing.push(path.clone());
            continue;
        };
        if !work_root.join(normalized).exists() {
            missing.push(path.clone());
        }
    }
    missing
}

fn analyze_step_progress(
    session: &SessionSnapshot,
    turn_start: usize,
    missing_before: Vec<String>,
    report: &VerificationReport,
) -> StepProgressReport {
    let missing_after = report_missing_paths(report);
    let write_or_edit_paths = write_or_edit_paths_since(session, turn_start);
    let repeated_write_or_edit_paths = repeated_items(&write_or_edit_paths);
    let no_expected_path_progress =
        !missing_after.is_empty() && missing_after.len() >= missing_before.len();
    StepProgressReport {
        missing_before,
        missing_after,
        write_or_edit_paths,
        repeated_write_or_edit_paths,
        no_expected_path_progress,
    }
}

fn report_missing_paths(report: &VerificationReport) -> Vec<String> {
    report
        .failures
        .iter()
        .filter_map(|failure| failure.strip_prefix("missing expected path: "))
        .map(str::to_string)
        .collect()
}

fn write_or_edit_paths_since(session: &SessionSnapshot, start: usize) -> Vec<String> {
    session
        .messages
        .iter()
        .skip(start)
        .filter(|message| message.role == "assistant")
        .flat_map(|message| message.tool_calls.iter())
        .filter(|call| call.name == "Write" || call.name == "Edit")
        .filter_map(|call| {
            call.arguments
                .get("path")
                .and_then(|value| value.as_str())
                .map(str::to_string)
        })
        .collect()
}

fn repeated_items(items: &[String]) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut repeated = BTreeSet::new();
    for item in items {
        if !seen.insert(item.clone()) {
            repeated.insert(item.clone());
        }
    }
    repeated.into_iter().collect()
}

fn run_verify_command(work_root: &Path, command: &str) -> Result<(), String> {
    validate_verify_command(command)?;
    let output = Command::new("sh")
        .arg("-lc")
        .arg(command)
        .current_dir(work_root)
        .output()
        .map_err(|err| format!("failed to run: {err}"))?;
    if output.status.success() {
        return Ok(());
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{stdout}{stderr}");
    Err(first_lines(&combined, 12))
}

fn first_lines(value: &str, max_lines: usize) -> String {
    let lines = value.lines().take(max_lines).collect::<Vec<_>>().join("\n");
    if lines.is_empty() {
        "command exited non-zero with no output".to_string()
    } else {
        lines
    }
}

fn normalize_relative_path(path: &Path) -> Option<PathBuf> {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::Normal(part) => normalized.push(part),
            std::path::Component::ParentDir => {
                if !normalized.pop() {
                    return None;
                }
            }
            std::path::Component::RootDir | std::path::Component::Prefix(_) => return None,
        }
    }
    Some(normalized)
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

        let path =
            generate_step_plan(&config(&temp), "qwen3:8b", &mut client, "Build app").unwrap();
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
            "qwen3:8b",
            &mut client,
            "Build app with TDD",
            UltraProfile::Generic,
            UltraPlanStyle::Tdd,
        )
        .unwrap();
        assert!(path.starts_with(temp.path().join(".anvil").join("plans")));
        let loaded = parse_ultra_plan_yaml(&std::fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(loaded.goal, "Build app");
        assert_eq!(loaded.profile, UltraProfile::Generic);
        assert_eq!(loaded.style, UltraPlanStyle::Tdd);
        assert_eq!(loaded.phases[0].id, "write-red-test");
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
        assert!(temp.path().join("report.md").is_file());
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

        let mut client = MockClient::default();
        client.push_reply(
            r#"{"goal":"Create a.txt","steps":[{"id":"create-a","instruction":"Create a.txt","expected_paths":["a.txt"],"verify":[]}]}"#,
            Vec::new(),
        );
        client.push_reply(
            "",
            vec![tool_call("Write", json!({"path":"a.txt","content":"a\n"}))],
        );
        client.push_reply("Created a.txt.", Vec::new());
        client.push_reply(
            r#"{"goal":"Create b.txt","steps":[{"id":"create-b","instruction":"Create b.txt","expected_paths":["b.txt"],"verify":[]}]}"#,
            Vec::new(),
        );
        client.push_reply(
            "",
            vec![tool_call("Write", json!({"path":"b.txt","content":"b\n"}))],
        );
        client.push_reply("Created b.txt.", Vec::new());

        let summary = run_ultra_plan(
            &config(&temp),
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
