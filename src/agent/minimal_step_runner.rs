use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;
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

fn plan_generation_system_prompt() -> String {
    "You are Anvil in Plan mode. You do not execute tools. Produce a step plan for a local coding agent.\n\
Output JSON only, with this exact shape:\n\
{\"goal\":\"...\",\"steps\":[{\"id\":\"kebab-id\",\"instruction\":\"...\",\"expected_paths\":[\"relative/path\"],\"verify\":[\"npm run build\"]}]}\n\
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
- Return 2 to 8 steps for most tasks, never more than 12."
        .to_string()
}

fn plan_generation_user_prompt(goal: &str) -> String {
    format!("Create a step plan for this task:\n{goal}")
}

fn build_step_prompt(plan: &StepPlan, step: &PlanStep) -> String {
    format!(
        "Overall goal:\n{goal}\n\nCurrent step id: {id}\nCurrent step instruction:\n{instruction}\n\nExpected paths after this step:\n{paths}\n\nVerification commands for this step:\n{verify}\n\nWork only on this step. Use Write/Edit for file changes. Do not rely on network scaffolding unless this step explicitly says so. Create every expected path before revising an already-created file repeatedly. Before giving a final answer, make the expected paths exist and run the verification commands when possible. If verification fails, fix only this step's failure.",
        goal = plan.goal,
        id = step.id,
        instruction = step.instruction,
        paths = bullet_list(&step.expected_paths),
        verify = bullet_list(&step.verify),
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
        "The previous step did not pass deterministic verification.{turn_error}\nOverall goal:\n{goal}\n\nCurrent step id: {id}\nCurrent step instruction:\n{instruction}\n\nExpected paths for this step:\n{paths}\n\nVerification failures:\n{failures}\n{progress_note}\nRepair only this step. Create or edit the missing/incorrect files, then run the verification commands if possible. Do not move to later steps.",
        goal = plan.goal,
        id = step.id,
        instruction = step.instruction,
        paths = bullet_list(&step.expected_paths),
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

fn parse_plan_json(raw: &str) -> Result<StepPlan, String> {
    let json =
        extract_json_object(raw).ok_or_else(|| "plan response did not contain JSON".to_string())?;
    serde_json::from_str::<StepPlan>(&json).map_err(|err| format!("invalid plan JSON: {err}"))
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
        match run_verify_command(work_root, command) {
            Ok(()) => {}
            Err(err) => failures.push(format!("verify failed `{command}`: {err}")),
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
    fn invalid_verify_command_is_rejected() {
        let plan = StepPlan {
            goal: "x".into(),
            steps: vec![PlanStep {
                id: "bad".into(),
                instruction: "x".into(),
                expected_paths: vec![],
                verify: vec!["npm install left-pad".into()],
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
                },
                PlanStep {
                    id: "create-app-entry".into(),
                    instruction: "Create app/page.tsx".into(),
                    expected_paths: vec!["app/page.tsx".into()],
                    verify: vec![],
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
}
