use std::path::Path;

use crate::planner::step_plan::{ExpectedResult, StepKind, StepPlan};
use crate::planner::ultra_plan::UltraPlan;
use crate::tools::path_guard::validate_workspace_relative;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanLintReport {
    pub errors: Vec<PlanLintError>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanLintError {
    pub category: String,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanQualitySeverity {
    Fatal,
    RetryableQuality,
    Advisory,
}

impl PlanQualitySeverity {
    pub fn as_str(self) -> &'static str {
        match self {
            PlanQualitySeverity::Fatal => "fatal",
            PlanQualitySeverity::RetryableQuality => "retryable_quality",
            PlanQualitySeverity::Advisory => "advisory",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanQualityIssue {
    pub category: String,
    pub message: String,
    pub severity: PlanQualitySeverity,
    pub step_id: Option<String>,
    pub evidence: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanQualityReport {
    pub issues: Vec<PlanQualityIssue>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlanQualityContext {
    pub profile: String,
    pub required_artifacts: Vec<String>,
    pub preferred_verify: Vec<String>,
    pub dependency_order_hint: Option<String>,
    pub task_intent: String,
    pub workspace_context_known: bool,
    pub workspace_snapshot_class: String,
    pub has_user_seed_files: bool,
    pub has_only_agent_metadata: bool,
}

impl PlanLintReport {
    pub fn pass() -> Self {
        Self { errors: Vec::new() }
    }

    pub fn is_pass(&self) -> bool {
        self.errors.is_empty()
    }

    pub fn push(&mut self, category: impl Into<String>, message: impl Into<String>) {
        self.errors.push(PlanLintError {
            category: category.into(),
            message: message.into(),
        });
    }

    pub fn primary_message(&self) -> String {
        self.errors
            .first()
            .map(|err| err.message.clone())
            .unwrap_or_else(|| "pass".to_string())
    }

    pub fn primary_category(&self) -> String {
        self.errors
            .first()
            .map(|err| err.category.clone())
            .unwrap_or_else(|| "pass".to_string())
    }

    pub fn has_category(&self, category: &str) -> bool {
        self.errors.iter().any(|err| err.category == category)
    }
}

impl PlanQualityReport {
    pub fn pass() -> Self {
        Self { issues: Vec::new() }
    }

    pub fn is_pass(&self) -> bool {
        self.issues.is_empty()
    }

    pub fn has_retryable_quality(&self) -> bool {
        self.issues
            .iter()
            .any(|issue| issue.severity == PlanQualitySeverity::RetryableQuality)
    }

    pub fn has_fatal(&self) -> bool {
        self.issues
            .iter()
            .any(|issue| issue.severity == PlanQualitySeverity::Fatal)
    }

    pub fn primary_message(&self) -> String {
        self.issues
            .first()
            .map(|issue| issue.message.clone())
            .unwrap_or_else(|| "pass".to_string())
    }

    pub fn push(
        &mut self,
        severity: PlanQualitySeverity,
        category: impl Into<String>,
        message: impl Into<String>,
        step_id: Option<String>,
        evidence: Option<String>,
    ) {
        self.issues.push(PlanQualityIssue {
            severity,
            category: category.into(),
            message: message.into(),
            step_id,
            evidence,
        });
    }
}

pub fn lint_step_plan(plan: &StepPlan) -> anyhow::Result<()> {
    let report = lint_step_plan_report(plan);
    if report.is_pass() {
        return Ok(());
    }
    anyhow::bail!("{}", report.primary_message())
}

pub fn lint_step_plan_report(plan: &StepPlan) -> PlanLintReport {
    lint_step_plan_report_with_workspace(plan, None)
}

pub fn lint_step_plan_report_with_workspace(
    plan: &StepPlan,
    work_root: Option<&Path>,
) -> PlanLintReport {
    let mut report = PlanLintReport::pass();
    if plan.goal.chars().count() > 4000 {
        report.push("contract", "StepPlan goal is too long");
    }
    if plan.steps.len() > 12 {
        report.push("contract", "StepPlan has too many steps");
    }
    let mut ids = std::collections::BTreeSet::new();
    let mut path_owners = std::collections::BTreeMap::new();
    let mut seen_paths = std::collections::BTreeSet::new();
    let mut setup_seen = work_root.is_some_and(workspace_has_node_dependency_context);
    let workspace_has_nextjs_entrypoint = work_root.is_some_and(workspace_has_nextjs_entrypoint);
    for step in &plan.steps {
        if step.id.trim().is_empty() {
            report.push("contract", "step id is empty");
        }
        if let Err(err) = validate_step_id(&step.id) {
            report.push("contract", err.to_string());
        }
        if !ids.insert(step.id.as_str()) {
            report.push("contract", format!("duplicate step id: {}", step.id));
        }
        if step.instruction.chars().count() > 2500 {
            report.push(
                "contract",
                format!("step {} instruction is too long", step.id),
            );
        }
        if is_placeholder_instruction(&step.instruction) {
            report.push(
                "contract",
                format!("step {} instruction is placeholder text", step.id),
            );
        }
        if looks_like_shell_command(&step.instruction) {
            report.push(
                "contract",
                "step instruction must be natural language, not a shell command",
            );
        }
        if let Err(err) = validate_step_kind_contract(step) {
            report.push("contract", err.to_string());
        }
        for path in &step.expected_paths {
            if let Err(err) = validate_workspace_relative(path) {
                report.push("contract", err.to_string());
                continue;
            }
            if let Some(owner) = path_owners.insert(path.as_str(), step.id.as_str()) {
                report.push(
                    "path_ownership",
                    format!(
                        "duplicate expected path ownership: {path} in {owner} and {}",
                        step.id
                    ),
                );
            }
        }
        for command in &step.verify {
            if let Err(err) = crate::planner::verify::validate_verify_command(command) {
                report.push("verify_policy", err.to_string());
                continue;
            }
            if requires_dependency_setup_before_verify(command)
                && !setup_seen
                && !step_creates_dependency_manifest(step)
            {
                report.push(
                    "dependency_order",
                    "verify command requires dependency setup or package manifest first",
                );
            }
            if is_nextjs_build(command)
                && !workspace_has_nextjs_entrypoint
                && !has_nextjs_entrypoint(&seen_paths, step)
            {
                report.push(
                    "dependency_order",
                    "Next.js build verify requires an entrypoint expected path first",
                );
            }
        }
        if step.step_kind() == StepKind::Setup || step_creates_dependency_manifest(step) {
            setup_seen = true;
        }
        for path in &step.expected_paths {
            seen_paths.insert(path.as_str());
        }
    }
    report
}

pub fn step_plan_quality_warnings(plan: &StepPlan) -> Vec<String> {
    let mut warnings = Vec::new();
    let expected_path_count: usize = plan
        .steps
        .iter()
        .map(|step| step.expected_paths.len())
        .sum();
    let has_verify = plan.steps.iter().any(|step| !step.verify.is_empty());
    let has_setup = plan
        .steps
        .iter()
        .any(|step| step.step_kind() == StepKind::Setup || step_creates_dependency_manifest(step));
    let lower_goal = plan.goal.to_ascii_lowercase();
    let looks_medium_or_large = expected_path_count > 1
        || lower_goal.contains("app")
        || lower_goal.contains("next.js")
        || lower_goal.contains("nextjs")
        || lower_goal.contains("game")
        || lower_goal.contains("project")
        || lower_goal.contains("テスト")
        || lower_goal.contains("アプリ");
    if plan.steps.len() == 1 && looks_medium_or_large {
        warnings.push("single-step plan for medium/large task".to_string());
    }
    if expected_path_count > 1 {
        let owners = plan
            .steps
            .iter()
            .filter(|step| !step.expected_paths.is_empty())
            .count();
        if owners <= 1 {
            warnings.push("multiple expected paths owned by one step".to_string());
        }
    }
    if !has_verify
        && (lower_goal.contains("test")
            || lower_goal.contains("build")
            || lower_goal.contains("verify")
            || lower_goal.contains("検証"))
    {
        warnings.push(
            "task likely needs deterministic verify but plan has no verify command".to_string(),
        );
    }
    if has_verify && !has_setup && lower_goal.contains("next") {
        warnings.push("verify appears without setup for framework task".to_string());
    }
    warnings
}

pub fn step_plan_quality_report(
    plan: &StepPlan,
    context: &PlanQualityContext,
) -> PlanQualityReport {
    let mut report = PlanQualityReport::pass();
    let all_paths: Vec<&str> = plan
        .steps
        .iter()
        .flat_map(|step| step.expected_paths.iter().map(String::as_str))
        .collect();
    let verify_commands: Vec<&str> = plan
        .steps
        .iter()
        .flat_map(|step| step.verify.iter().map(String::as_str))
        .collect();
    let lower_goal = plan.goal.to_ascii_lowercase();
    let looks_next_profile = matches!(context.profile.as_str(), "nextjs" | "next-js" | "next.js");
    let has_strong_verify = verify_commands
        .iter()
        .any(|command| is_strong_verify_command(command));
    let has_later_or_any_strong_verify = has_strong_verify;

    if let Some(last) = plan.steps.last()
        && last.step_kind() == StepKind::Report
        && !goal_allows_report_blocker(&lower_goal)
        && (!all_paths.is_empty() || !context.required_artifacts.is_empty())
    {
        report.push(
            PlanQualitySeverity::RetryableQuality,
            "terminal_report_step",
            "normal implementation plans should not end with a final summary report step",
            Some(last.id.clone()),
            Some(last.instruction.clone()),
        );
    }

    if is_fresh_create_workspace(context)
        && let Some(first) = plan.steps.first()
        && matches!(first.step_kind(), StepKind::Inspect | StepKind::Report)
        && first.expected_paths.is_empty()
        && first.verify.is_empty()
    {
        report.push(
            PlanQualitySeverity::RetryableQuality,
            "fresh_workspace_read_before_write",
            "fresh create/scaffold workspace should start by owning an artifact instead of an empty wrapper step",
            Some(first.id.clone()),
            Some(context.workspace_snapshot_class.clone()),
        );
    }

    if looks_next_profile
        && !context.preferred_verify.is_empty()
        && !has_preferred_verify(&verify_commands, &context.preferred_verify)
        && has_nextjs_artifact_intent(&all_paths, context)
    {
        report.push(
            PlanQualitySeverity::RetryableQuality,
            "profile_verify_missing",
            "profile expects deterministic build/test verification but plan has no preferred verify command",
            None,
            Some(context.preferred_verify.join(", ")),
        );
    }

    if looks_code_task(&lower_goal, &all_paths) && !has_strong_verify {
        let severity = if !verify_commands.is_empty()
            || (looks_next_profile
                && (lower_goal.contains("test")
                    || lower_goal.contains("build")
                    || lower_goal.contains("verify")
                    || lower_goal.contains("検証")))
        {
            PlanQualitySeverity::RetryableQuality
        } else {
            PlanQualitySeverity::Advisory
        };
        report.push(
            severity,
            "weak_code_verify",
            "code task lacks test, build, smoke, or compile verification",
            None,
            Some(
                verify_commands
                    .iter()
                    .copied()
                    .collect::<Vec<_>>()
                    .join(", "),
            ),
        );
    }

    if looks_docs_task(&lower_goal, &all_paths) && !has_content_assertion(&verify_commands) {
        report.push(
            PlanQualitySeverity::Advisory,
            "weak_docs_verify",
            "docs task has no content assertion for requested text or headings",
            None,
            Some(
                verify_commands
                    .iter()
                    .copied()
                    .collect::<Vec<_>>()
                    .join(", "),
            ),
        );
    }

    if !verify_commands.is_empty()
        && !all_paths.is_empty()
        && !has_strong_verify
        && verify_commands
            .iter()
            .all(|command| is_weak_verify_command(command))
    {
        report.push(
            PlanQualitySeverity::Advisory,
            "weak_verify_only",
            "verify commands only check existence or print files",
            None,
            Some(verify_commands.join(", ")),
        );
    }

    for step in &plan.steps {
        if step.expected_paths.is_empty() {
            continue;
        }
        if step.verify.is_empty()
            && !has_later_or_any_strong_verify
            && looks_code_task(&lower_goal, &all_paths)
        {
            report.push(
                PlanQualitySeverity::Advisory,
                "artifact_owner_without_local_verify",
                "artifact-owning step has no local verify and the plan has no strong project verification",
                Some(step.id.clone()),
                Some(step.expected_paths.join(", ")),
            );
        }
        if instruction_mentions_expected_path_or_content(&step.instruction, &step.expected_paths) {
            continue;
        }
        report.push(
            PlanQualitySeverity::Advisory,
            "instruction_path_alignment",
            "implement/setup instruction does not mention expected paths or concrete artifact content",
            Some(step.id.clone()),
            Some(step.expected_paths.join(", ")),
        );
    }

    let mut prior_paths: Vec<&str> = Vec::new();
    for step in &plan.steps {
        if step.step_kind() == StepKind::Verify
            && !all_paths.is_empty()
            && prior_paths.is_empty()
            && !step.verify.is_empty()
        {
            report.push(
                PlanQualitySeverity::RetryableQuality,
                "detached_verify_without_prior_artifact_context",
                "verify step appears before any artifact-owning step",
                Some(step.id.clone()),
                Some(step.verify.join(", ")),
            );
        }
        for command in &step.verify {
            if is_strong_verify_command(command) || is_content_assertion_command(command) {
                continue;
            }
            if command_mentions_any_expected_path(command, &all_paths) {
                continue;
            }
            report.push(
                PlanQualitySeverity::RetryableQuality,
                "verify_artifact_coupling",
                "verify command does not appear to validate any expected artifact",
                Some(step.id.clone()),
                Some(command.clone()),
            );
        }
        for path in &step.expected_paths {
            prior_paths.push(path);
        }
    }

    report
}

fn validate_step_id(id: &str) -> anyhow::Result<()> {
    let trimmed = id.trim();
    if trimmed.is_empty() {
        anyhow::bail!("step id is empty");
    }
    if trimmed.chars().count() > 64 {
        anyhow::bail!("step id is too long: {trimmed}");
    }
    if trimmed.starts_with('-') || trimmed.ends_with('-') {
        anyhow::bail!("step id must not start or end with '-': {trimmed}");
    }
    if !trimmed
        .chars()
        .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
    {
        anyhow::bail!("step id must use lowercase kebab-case: {trimmed}");
    }
    Ok(())
}

fn is_placeholder_instruction(instruction: &str) -> bool {
    matches!(
        instruction.trim().to_ascii_lowercase().as_str(),
        "" | "todo" | "tbd" | "implement" | "fix" | "do it"
    )
}

fn goal_allows_report_blocker(lower_goal: &str) -> bool {
    lower_goal.contains("blocker")
        || lower_goal.contains("dependency_missing")
        || lower_goal.contains("dependency missing")
        || lower_goal.contains("unavailable")
        || lower_goal.contains("user input")
        || lower_goal.contains("cannot")
        || lower_goal.contains("can't")
        || lower_goal.contains("blocked")
}

fn is_fresh_create_workspace(context: &PlanQualityContext) -> bool {
    context.workspace_context_known
        && context.has_only_agent_metadata
        && !context.has_user_seed_files
        && !context.required_artifacts.is_empty()
        && matches!(
            context.task_intent.as_str(),
            "create" | "scaffold" | "new" | "docs"
        )
}

fn validate_step_kind_contract(step: &crate::planner::step_plan::PlanStep) -> anyhow::Result<()> {
    match step.step_kind() {
        StepKind::Inspect => {
            if !step.expected_paths.is_empty() || !step.verify.is_empty() {
                anyhow::bail!("inspect step may not declare expected paths or verify commands");
            }
        }
        StepKind::Setup => {
            if step
                .verify
                .iter()
                .any(|command| is_verify_like_command(command))
            {
                anyhow::bail!("setup step may not run build/test verification");
            }
        }
        StepKind::Implement => {
            if step.expected_paths.is_empty() {
                anyhow::bail!("implement step must declare concrete expected paths");
            }
        }
        StepKind::Verify => {
            if step.verify.is_empty() {
                anyhow::bail!("verify step requires at least one verify command");
            }
            if looks_like_file_change_instruction(&step.instruction) {
                anyhow::bail!("verify step instruction must not request file changes");
            }
        }
        StepKind::Report => {
            if !step.expected_paths.is_empty() || !step.verify.is_empty() {
                anyhow::bail!("report step may not declare expected paths or verify commands");
            }
        }
        StepKind::Unknown(kind) => anyhow::bail!("unknown step kind: {kind}"),
    }
    if matches!(step.expected_result_kind(), ExpectedResult::Unknown(_)) {
        anyhow::bail!("unknown expected_result: {}", step.expected_result);
    }
    if step.expected_result_kind() == ExpectedResult::Fail && step.verify.is_empty() {
        anyhow::bail!("expected_result fail requires a verify command");
    }
    Ok(())
}

fn is_verify_like_command(command: &str) -> bool {
    let lower = command.trim().to_ascii_lowercase();
    lower == "cargo test"
        || lower.starts_with("cargo test ")
        || lower == "npm test"
        || lower == "npm run test"
        || lower == "npm run build"
        || lower == "pnpm test"
        || lower == "pnpm build"
        || lower == "yarn test"
        || lower == "yarn build"
        || lower.starts_with("python -m unittest")
        || lower.starts_with("python3 -m unittest")
        || lower == "pytest"
        || lower.starts_with("pytest ")
        || lower.contains(" build")
}

fn requires_dependency_setup_before_verify(command: &str) -> bool {
    let lower = command.trim().to_ascii_lowercase();
    lower == "npm test"
        || lower == "npm run test"
        || lower == "npm run build"
        || lower == "pnpm test"
        || lower == "pnpm build"
        || lower == "yarn test"
        || lower == "yarn build"
        || lower.starts_with("npm run build ")
        || lower.starts_with("npm run test ")
        || lower.starts_with("npm test ")
        || lower.starts_with("pnpm build ")
        || lower.starts_with("pnpm test ")
        || lower.starts_with("yarn build ")
        || lower.starts_with("yarn test ")
}

fn is_nextjs_build(command: &str) -> bool {
    let lower = command.to_ascii_lowercase();
    lower == "npm run build" || lower == "pnpm build" || lower == "yarn build"
}

fn workspace_has_node_dependency_context(root: &Path) -> bool {
    root.join("node_modules").is_dir() || root.join("package.json").is_file()
}

fn workspace_has_nextjs_entrypoint(root: &Path) -> bool {
    nextjs_entrypoints()
        .iter()
        .any(|path| root.join(path).is_file())
}

fn step_creates_dependency_manifest(step: &crate::planner::step_plan::PlanStep) -> bool {
    step.expected_paths.iter().any(|path| {
        matches!(
            path.as_str(),
            "package.json" | "Cargo.toml" | "pyproject.toml"
        )
    })
}

fn has_nextjs_entrypoint(
    seen_paths: &std::collections::BTreeSet<&str>,
    step: &crate::planner::step_plan::PlanStep,
) -> bool {
    nextjs_entrypoints().iter().any(|path| {
        seen_paths.contains(path)
            || step
                .expected_paths
                .iter()
                .any(|expected| expected.as_str() == *path)
    })
}

fn looks_like_file_change_instruction(instruction: &str) -> bool {
    let lower = instruction.to_ascii_lowercase();
    lower.contains("write")
        || lower.contains("edit")
        || lower.contains("create")
        || lower.contains("modify")
        || lower.contains("fix")
        || lower.contains("実装")
        || lower.contains("作成")
        || lower.contains("修正")
}

pub fn lint_ultra_plan(plan: &UltraPlan) -> anyhow::Result<()> {
    let report = lint_ultra_plan_report(plan);
    if report.is_pass() {
        return Ok(());
    }
    anyhow::bail!("{}", report.primary_message())
}

pub fn lint_ultra_plan_report(plan: &UltraPlan) -> PlanLintReport {
    let mut report = PlanLintReport::pass();
    if !(2..=8).contains(&plan.phases.len()) {
        report.push("scaffold", "UltraPlan must have 2-8 phases");
    }
    let mut ids = std::collections::BTreeSet::new();
    for phase in &plan.phases {
        if phase.id.trim().is_empty() || phase.prompt.trim().is_empty() {
            report.push("scaffold", "ultra phase must have id and prompt");
        }
        if !ids.insert(phase.id.as_str()) {
            report.push(
                "scaffold",
                format!("duplicate ultra phase id: {}", phase.id),
            );
        }
        if phase.prompt.trim_start().starts_with('/') {
            report.push("scaffold", "ultra phase prompt must not be a REPL command");
        }
        if looks_like_shell_command(&phase.prompt) {
            report.push(
                "scaffold",
                "ultra phase prompt must be a plain natural-language goal, not a shell command",
            );
        }
    }
    report
}

fn looks_like_shell_command(value: &str) -> bool {
    let trimmed = value.trim_start();
    [
        "npm ", "pnpm ", "yarn ", "cargo ", "python ", "python3 ", "sh ", "bash ",
    ]
    .iter()
    .any(|prefix| trimmed.starts_with(prefix))
}

fn has_preferred_verify(commands: &[&str], preferred: &[String]) -> bool {
    commands.iter().any(|command| {
        let lower = command.trim().to_ascii_lowercase();
        preferred.iter().any(|expected| {
            let expected = expected.trim().to_ascii_lowercase();
            lower == expected || lower.starts_with(&(expected + " "))
        })
    })
}

fn nextjs_entrypoints() -> &'static [&'static str] {
    &[
        "src/app/page.tsx",
        "src/app/page.jsx",
        "app/page.tsx",
        "app/page.jsx",
        "pages/index.tsx",
        "pages/index.jsx",
        "src/pages/index.tsx",
        "src/pages/index.jsx",
    ]
}

fn has_nextjs_artifact_intent(paths: &[&str], context: &PlanQualityContext) -> bool {
    let expected = paths
        .iter()
        .copied()
        .chain(context.required_artifacts.iter().map(String::as_str))
        .collect::<Vec<_>>();
    expected.iter().any(|path| path.ends_with("package.json"))
        && expected.iter().any(|path| {
            matches!(
                *path,
                "src/app/page.tsx"
                    | "src/app/page.jsx"
                    | "app/page.tsx"
                    | "app/page.jsx"
                    | "pages/index.tsx"
                    | "pages/index.jsx"
                    | "src/pages/index.tsx"
                    | "src/pages/index.jsx"
            )
        })
}

fn looks_code_task(lower_goal: &str, paths: &[&str]) -> bool {
    if lower_goal.contains("test")
        || lower_goal.contains("build")
        || lower_goal.contains("app")
        || lower_goal.contains("game")
        || lower_goal.contains("script")
        || lower_goal.contains("cli")
        || lower_goal.contains("code")
        || lower_goal.contains("unit")
    {
        return true;
    }
    paths.iter().any(|path| {
        path.ends_with(".py")
            || path.ends_with(".rs")
            || path.ends_with(".js")
            || path.ends_with(".jsx")
            || path.ends_with(".ts")
            || path.ends_with(".tsx")
            || path.ends_with("Cargo.toml")
            || path.ends_with("package.json")
    })
}

fn looks_docs_task(lower_goal: &str, paths: &[&str]) -> bool {
    if lower_goal.contains("doc")
        || lower_goal.contains("readme")
        || lower_goal.contains("heading")
        || lower_goal.contains("markdown")
        || lower_goal.contains("section")
        || lower_goal.contains("見出し")
        || lower_goal.contains("ドキュメント")
    {
        return true;
    }
    paths.iter().any(|path| {
        path.ends_with(".md")
            || path.ends_with(".mdx")
            || path.ends_with(".txt")
            || path.ends_with("README")
            || path.ends_with("README.md")
    })
}

fn is_strong_verify_command(command: &str) -> bool {
    let lower = command.trim().to_ascii_lowercase();
    lower == "cargo test"
        || lower.starts_with("cargo test ")
        || lower == "cargo build"
        || lower.starts_with("cargo build ")
        || lower == "npm test"
        || lower == "npm run test"
        || lower.starts_with("npm run test ")
        || lower == "npm run build"
        || lower.starts_with("npm run build ")
        || lower == "pnpm test"
        || lower.starts_with("pnpm test ")
        || lower == "pnpm build"
        || lower.starts_with("pnpm build ")
        || lower == "yarn test"
        || lower.starts_with("yarn test ")
        || lower == "yarn build"
        || lower.starts_with("yarn build ")
        || lower.starts_with("python -m unittest")
        || lower.starts_with("python3 -m unittest")
        || lower == "pytest"
        || lower.starts_with("pytest ")
        || lower.starts_with("node ")
        || lower.starts_with("python -m py_compile")
        || lower.starts_with("python3 -m py_compile")
        || lower.contains(" tsc")
        || lower == "tsc"
}

fn is_content_assertion_command(command: &str) -> bool {
    let lower = command.trim().to_ascii_lowercase();
    (lower.starts_with("grep ") || lower.starts_with("grep -q ") || lower.contains(" grep "))
        && (command.contains('"') || command.contains('\''))
}

fn has_content_assertion(commands: &[&str]) -> bool {
    commands
        .iter()
        .any(|command| is_content_assertion_command(command))
}

fn is_weak_verify_command(command: &str) -> bool {
    let lower = command.trim().to_ascii_lowercase();
    lower.starts_with("test -f ")
        || lower.starts_with("test -s ")
        || lower.starts_with("cat ")
        || lower.starts_with("ls ")
        || lower.starts_with("grep ")
        || lower.starts_with("grep -q ")
        || lower.starts_with("python -m py_compile")
        || lower.starts_with("python3 -m py_compile")
}

fn instruction_mentions_expected_path_or_content(instruction: &str, paths: &[String]) -> bool {
    let lower = instruction.to_ascii_lowercase();
    if lower.contains("package")
        || lower.contains("layout")
        || lower.contains("page")
        || lower.contains("readme")
        || lower.contains("test")
        || lower.contains("build")
        || lower.contains("content")
        || lower.contains("heading")
        || lower.contains("見出し")
    {
        return true;
    }
    paths.iter().any(|path| {
        let path_lower = path.to_ascii_lowercase();
        lower.contains(&path_lower)
            || path_lower
                .rsplit('/')
                .next()
                .is_some_and(|name| lower.contains(name))
    })
}

fn command_mentions_any_expected_path(command: &str, paths: &[&str]) -> bool {
    let lower = command.to_ascii_lowercase();
    paths
        .iter()
        .any(|path| lower.contains(&path.to_ascii_lowercase()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planner::step_plan::{PlanStep, StepPlan};
    use crate::planner::ultra_plan::{UltraPhase, UltraPlan};

    #[test]
    fn duplicate_step_ids_are_rejected() {
        let plan = StepPlan {
            goal: "goal".to_string(),
            steps: vec![
                step("same", "Create the file"),
                step("same", "Verify the file"),
            ],
        };
        assert!(lint_step_plan(&plan).is_err());
    }

    #[test]
    fn step_plan_rejects_duplicate_step_ids() {
        duplicate_step_ids_are_rejected();
    }

    #[test]
    fn step_kind_source_aliases() {
        for kind in ["work", "create", "edit", "repair"] {
            let mut plan = StepPlan {
                goal: "goal".to_string(),
                steps: vec![step("s1", "Create the file")],
            };
            plan.steps[0].kind = kind.to_string();
            assert!(lint_step_plan(&plan).is_ok(), "{kind}");
        }
    }

    #[test]
    fn shell_command_instruction_is_rejected() {
        let plan = StepPlan {
            goal: "goal".to_string(),
            steps: vec![step("s1", "npm run build")],
        };
        assert!(lint_step_plan(&plan).is_err());
    }

    #[test]
    fn ultra_phase_repl_command_is_rejected() {
        let plan = UltraPlan {
            goal: "goal".to_string(),
            profile: "generic".to_string(),
            style: "default".to_string(),
            intent: "create".to_string(),
            phases: vec![
                UltraPhase {
                    id: "a".to_string(),
                    prompt: "Plan the work".to_string(),
                },
                UltraPhase {
                    id: "b".to_string(),
                    prompt: "/plan-run do it".to_string(),
                },
            ],
        };
        assert!(lint_ultra_plan(&plan).is_err());
    }

    fn step(id: &str, instruction: &str) -> PlanStep {
        PlanStep {
            id: id.to_string(),
            kind: "implement".to_string(),
            expected_result: "pass".to_string(),
            instruction: instruction.to_string(),
            expected_paths: vec!["out.txt".to_string()],
            verify: Vec::new(),
        }
    }

    #[test]
    fn step_kind_contract_rejects_setup_with_build_verify() {
        let mut plan = StepPlan {
            goal: "goal".to_string(),
            steps: vec![step("s1", "Install dependencies")],
        };
        plan.steps[0].kind = "setup".to_string();
        plan.steps[0].expected_paths.clear();
        plan.steps[0].verify = vec!["npm run build".to_string()];
        assert!(lint_step_plan(&plan).is_err());
    }

    #[test]
    fn shell_test_file_check_does_not_require_dependency_setup() {
        let mut plan = StepPlan {
            goal: "Create README".to_string(),
            steps: vec![step("s1", "Create README.md")],
        };
        plan.steps[0].expected_paths = vec!["README.md".to_string()];
        plan.steps[0].verify = vec!["test -f README.md".to_string()];
        assert!(lint_step_plan(&plan).is_ok());
    }

    #[test]
    fn inspect_step_rejects_expected_paths() {
        let mut plan = StepPlan {
            goal: "goal".to_string(),
            steps: vec![step("s1", "Inspect files")],
        };
        plan.steps[0].kind = "inspect".to_string();
        assert!(lint_step_plan(&plan).is_err());
    }

    #[test]
    fn verify_step_requires_verify_command() {
        let plan = StepPlan {
            goal: "goal".to_string(),
            steps: vec![PlanStep {
                id: "s1".to_string(),
                kind: "verify".to_string(),
                expected_result: "pass".to_string(),
                instruction: "Check result".to_string(),
                expected_paths: Vec::new(),
                verify: Vec::new(),
            }],
        };
        assert!(lint_step_plan(&plan).is_err());
    }

    #[test]
    fn expected_result_fail_requires_verify() {
        let plan = StepPlan {
            goal: "goal".to_string(),
            steps: vec![PlanStep {
                id: "s1".to_string(),
                kind: "implement".to_string(),
                expected_result: "fail".to_string(),
                instruction: "Create result".to_string(),
                expected_paths: vec!["out.txt".to_string()],
                verify: Vec::new(),
            }],
        };
        let err = lint_step_plan(&plan).unwrap_err().to_string();
        assert!(err.contains("expected_result fail requires a verify command"));
    }

    #[test]
    fn implement_step_requires_concrete_expected_paths() {
        let plan = StepPlan {
            goal: "goal".to_string(),
            steps: vec![PlanStep {
                id: "s1".to_string(),
                kind: "implement".to_string(),
                expected_result: "pass".to_string(),
                instruction: "Create result".to_string(),
                expected_paths: Vec::new(),
                verify: Vec::new(),
            }],
        };
        assert!(lint_step_plan(&plan).is_err());
    }

    #[test]
    fn semantic_lint_rejects_next_build_before_entrypoint() {
        let plan = StepPlan {
            goal: "goal".to_string(),
            steps: vec![PlanStep {
                id: "s1".to_string(),
                kind: "implement".to_string(),
                expected_result: "pass".to_string(),
                instruction: "Create package only".to_string(),
                expected_paths: vec!["package.json".to_string()],
                verify: vec!["npm run build".to_string()],
            }],
        };
        assert!(lint_step_plan(&plan).is_err());
    }

    #[test]
    fn workspace_manifest_and_entrypoint_allow_final_nextjs_verify() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src/app")).unwrap();
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"scripts":{"build":"next build"}}"#,
        )
        .unwrap();
        std::fs::write(
            dir.path().join("src/app/page.tsx"),
            "export default function Page() { return null; }\n",
        )
        .unwrap();
        let plan = StepPlan {
            goal: "Verify the existing Next.js app".to_string(),
            steps: vec![PlanStep {
                id: "final-verify".to_string(),
                kind: "verify".to_string(),
                expected_result: "pass".to_string(),
                instruction: "Run deterministic Next.js build verification".to_string(),
                expected_paths: Vec::new(),
                verify: vec!["npm run build".to_string()],
            }],
        };

        let report = lint_step_plan_report_with_workspace(&plan, Some(dir.path()));

        assert!(report.is_pass(), "{report:?}");
    }

    #[test]
    fn workspace_manifest_without_entrypoint_still_rejects_nextjs_build() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"scripts":{"build":"next build"}}"#,
        )
        .unwrap();
        let plan = StepPlan {
            goal: "Verify the existing Next.js app".to_string(),
            steps: vec![PlanStep {
                id: "final-verify".to_string(),
                kind: "verify".to_string(),
                expected_result: "pass".to_string(),
                instruction: "Run deterministic Next.js build verification".to_string(),
                expected_paths: Vec::new(),
                verify: vec!["npm run build".to_string()],
            }],
        };

        let report = lint_step_plan_report_with_workspace(&plan, Some(dir.path()));

        assert!(report.has_category("dependency_order"), "{report:?}");
        assert!(
            report
                .errors
                .iter()
                .any(|err| err.message.contains("entrypoint")),
            "{report:?}"
        );
    }

    #[test]
    fn semantic_lint_rejects_dependency_verify_without_setup() {
        let plan = StepPlan {
            goal: "goal".to_string(),
            steps: vec![PlanStep {
                id: "s1".to_string(),
                kind: "verify".to_string(),
                expected_result: "pass".to_string(),
                instruction: "Verify project".to_string(),
                expected_paths: Vec::new(),
                verify: vec!["npm run build".to_string()],
            }],
        };
        assert!(lint_step_plan(&plan).is_err());
    }

    #[test]
    fn dependency_verify_without_setup_has_specific_category() {
        let plan = StepPlan {
            goal: "goal".to_string(),
            steps: vec![PlanStep {
                id: "s1".to_string(),
                kind: "verify".to_string(),
                expected_result: "pass".to_string(),
                instruction: "Verify project".to_string(),
                expected_paths: Vec::new(),
                verify: vec!["npm run build".to_string()],
            }],
        };
        let report = lint_step_plan_report(&plan);
        assert!(report.has_category("dependency_order"), "{report:?}");
        assert!(
            report
                .errors
                .iter()
                .any(|err| err.message.contains("requires dependency setup")),
            "{report:?}"
        );
    }

    #[test]
    fn planner_lint_python_unittest_without_setup_passes() {
        let plan = StepPlan {
            goal: "Create Python linter".to_string(),
            steps: vec![
                PlanStep {
                    id: "s1".to_string(),
                    kind: "implement".to_string(),
                    expected_result: "pass".to_string(),
                    instruction: "Implement the linter".to_string(),
                    expected_paths: vec!["markdown_lint.py".to_string()],
                    verify: Vec::new(),
                },
                PlanStep {
                    id: "s2".to_string(),
                    kind: "verify".to_string(),
                    expected_result: "pass".to_string(),
                    instruction: "Run deterministic unit tests".to_string(),
                    expected_paths: Vec::new(),
                    verify: vec!["python3 -m unittest test_markdown_lint.py".to_string()],
                },
            ],
        };
        assert!(lint_step_plan(&plan).is_ok());
    }

    #[test]
    fn planner_lint_python_unittest_alias_without_setup_passes() {
        let plan = StepPlan {
            goal: "Create Python module".to_string(),
            steps: vec![
                PlanStep {
                    id: "s1".to_string(),
                    kind: "implement".to_string(),
                    expected_result: "pass".to_string(),
                    instruction: "Implement the module".to_string(),
                    expected_paths: vec!["app.py".to_string()],
                    verify: Vec::new(),
                },
                PlanStep {
                    id: "s2".to_string(),
                    kind: "verify".to_string(),
                    expected_result: "pass".to_string(),
                    instruction: "Run stdlib unittest".to_string(),
                    expected_paths: Vec::new(),
                    verify: vec!["python -m unittest test_app.py".to_string()],
                },
            ],
        };
        assert!(lint_step_plan(&plan).is_ok());
    }

    #[test]
    fn planner_lint_pytest_and_cargo_test_are_not_dependency_order_failures() {
        for command in ["pytest", "pytest tests", "cargo test"] {
            let plan = StepPlan {
                goal: "Run verifier".to_string(),
                steps: vec![
                    PlanStep {
                        id: "s1".to_string(),
                        kind: "implement".to_string(),
                        expected_result: "pass".to_string(),
                        instruction: "Create source".to_string(),
                        expected_paths: vec!["src/lib.rs".to_string()],
                        verify: Vec::new(),
                    },
                    PlanStep {
                        id: "s2".to_string(),
                        kind: "verify".to_string(),
                        expected_result: "pass".to_string(),
                        instruction: "Run verifier".to_string(),
                        expected_paths: Vec::new(),
                        verify: vec![command.to_string()],
                    },
                ],
            };
            let report = lint_step_plan_report(&plan);
            assert!(
                !report.has_category("dependency_order"),
                "{command}: {report:?}"
            );
        }
    }

    #[test]
    fn plan_lint_report_aggregates_multiple_errors() {
        let plan = StepPlan {
            goal: "goal".to_string(),
            steps: vec![
                PlanStep {
                    id: "s1".to_string(),
                    kind: "implement".to_string(),
                    expected_result: "pass".to_string(),
                    instruction: "Create first".to_string(),
                    expected_paths: Vec::new(),
                    verify: vec!["npm test && npm run build".to_string()],
                },
                PlanStep {
                    id: "s2".to_string(),
                    kind: "implement".to_string(),
                    expected_result: "pass".to_string(),
                    instruction: "Create second".to_string(),
                    expected_paths: vec!["out.txt".to_string()],
                    verify: Vec::new(),
                },
                PlanStep {
                    id: "s3".to_string(),
                    kind: "implement".to_string(),
                    expected_result: "pass".to_string(),
                    instruction: "Create duplicate".to_string(),
                    expected_paths: vec!["out.txt".to_string()],
                    verify: Vec::new(),
                },
            ],
        };
        let report = lint_step_plan_report(&plan);
        assert!(report.has_category("contract"));
        assert!(report.has_category("verify_policy"));
        assert!(report.has_category("path_ownership"));
        assert!(report.errors.len() >= 3);
    }

    #[test]
    fn lint_step_plan_wrapper_preserves_existing_first_error() {
        let plan = StepPlan {
            goal: "goal".to_string(),
            steps: vec![PlanStep {
                id: "s1".to_string(),
                kind: "implement".to_string(),
                expected_result: "pass".to_string(),
                instruction: "Create result".to_string(),
                expected_paths: Vec::new(),
                verify: Vec::new(),
            }],
        };
        let err = lint_step_plan(&plan).unwrap_err().to_string();
        assert!(err.contains("implement step must declare concrete expected paths"));
    }

    #[test]
    fn duplicate_expected_path_ownership_is_rejected() {
        let plan = StepPlan {
            goal: "goal".to_string(),
            steps: vec![step("s1", "Create file"), step("s2", "Update file")],
        };
        assert!(lint_step_plan(&plan).is_err());
    }

    #[test]
    fn step_plan_rejects_duplicate_expected_path_ownership() {
        duplicate_expected_path_ownership_is_rejected();
    }

    #[test]
    fn step_plan_quality_diagnostic() {
        let plan = StepPlan {
            goal: "Build a Next.js game app".to_string(),
            steps: vec![PlanStep {
                id: "s1".to_string(),
                kind: "implement".to_string(),
                expected_result: "pass".to_string(),
                instruction: "Create app".to_string(),
                expected_paths: vec!["package.json".to_string(), "src/app/page.tsx".to_string()],
                verify: Vec::new(),
            }],
        };
        let warnings = step_plan_quality_warnings(&plan);
        assert!(!warnings.is_empty());
    }

    #[test]
    fn step_plan_quality_diagnostic_does_not_reject_small_task() {
        let plan = StepPlan {
            goal: "Update README heading".to_string(),
            steps: vec![PlanStep {
                id: "s1".to_string(),
                kind: "implement".to_string(),
                expected_result: "pass".to_string(),
                instruction: "Update README".to_string(),
                expected_paths: vec!["README.md".to_string()],
                verify: Vec::new(),
            }],
        };
        assert!(lint_step_plan(&plan).is_ok());
        assert!(step_plan_quality_warnings(&plan).is_empty());
    }

    #[test]
    fn quality_report_marks_nextjs_missing_build_retryable() {
        let plan = StepPlan {
            goal: "Build a Next.js game app".to_string(),
            steps: vec![PlanStep {
                id: "s1".to_string(),
                kind: "implement".to_string(),
                expected_result: "pass".to_string(),
                instruction: "Create package.json and src/app/page.tsx for the app".to_string(),
                expected_paths: vec!["package.json".to_string(), "src/app/page.tsx".to_string()],
                verify: Vec::new(),
            }],
        };
        let report = step_plan_quality_report(&plan, &nextjs_quality_context());
        assert!(report.has_retryable_quality(), "{report:?}");
        assert!(
            report
                .issues
                .iter()
                .any(|issue| issue.category == "profile_verify_missing")
        );
    }

    #[test]
    fn quality_report_accepts_nextjs_build_verify() {
        let plan = StepPlan {
            goal: "Build a Next.js game app".to_string(),
            steps: vec![
                PlanStep {
                    id: "setup".to_string(),
                    kind: "setup".to_string(),
                    expected_result: "pass".to_string(),
                    instruction: "Create package.json with next dependencies".to_string(),
                    expected_paths: vec!["package.json".to_string()],
                    verify: Vec::new(),
                },
                PlanStep {
                    id: "page".to_string(),
                    kind: "implement".to_string(),
                    expected_result: "pass".to_string(),
                    instruction: "Create src/app/page.tsx game page".to_string(),
                    expected_paths: vec!["src/app/page.tsx".to_string()],
                    verify: Vec::new(),
                },
                PlanStep {
                    id: "verify".to_string(),
                    kind: "verify".to_string(),
                    expected_result: "pass".to_string(),
                    instruction: "Run deterministic build".to_string(),
                    expected_paths: Vec::new(),
                    verify: vec!["npm run build".to_string()],
                },
            ],
        };
        assert!(lint_step_plan(&plan).is_ok());
        let report = step_plan_quality_report(&plan, &nextjs_quality_context());
        assert!(!report.has_retryable_quality(), "{report:?}");
    }

    #[test]
    fn quality_report_does_not_retry_docs_content_assertion() {
        let plan = StepPlan {
            goal: "Update README Usage heading".to_string(),
            steps: vec![PlanStep {
                id: "docs".to_string(),
                kind: "implement".to_string(),
                expected_result: "pass".to_string(),
                instruction: "Update README.md with a Usage heading".to_string(),
                expected_paths: vec!["README.md".to_string()],
                verify: vec!["grep -q \"Usage\" README.md".to_string()],
            }],
        };
        let report = step_plan_quality_report(&plan, &PlanQualityContext::default());
        assert!(!report.has_retryable_quality(), "{report:?}");
        assert!(
            !report
                .issues
                .iter()
                .any(|issue| issue.category == "weak_docs_verify"),
            "{report:?}"
        );
    }

    #[test]
    fn quality_report_marks_unrelated_verify_retryable() {
        let plan = StepPlan {
            goal: "Create Python script and verify it".to_string(),
            steps: vec![PlanStep {
                id: "code".to_string(),
                kind: "implement".to_string(),
                expected_result: "pass".to_string(),
                instruction: "Create app.py script".to_string(),
                expected_paths: vec!["app.py".to_string()],
                verify: vec!["test -f README.md".to_string()],
            }],
        };
        let report = step_plan_quality_report(&plan, &PlanQualityContext::default());
        assert!(report.has_retryable_quality(), "{report:?}");
        assert!(
            report
                .issues
                .iter()
                .any(|issue| issue.category == "verify_artifact_coupling")
        );
    }

    #[test]
    fn terminal_report_step_is_retryable_quality_for_implementation_task() {
        let plan = StepPlan {
            goal: "Create a Python script".to_string(),
            steps: vec![
                PlanStep {
                    id: "code".to_string(),
                    kind: "implement".to_string(),
                    expected_result: "pass".to_string(),
                    instruction: "Create app.py".to_string(),
                    expected_paths: vec!["app.py".to_string()],
                    verify: vec!["python3 -m py_compile app.py".to_string()],
                },
                PlanStep {
                    id: "report-completion".to_string(),
                    kind: "report".to_string(),
                    expected_result: "pass".to_string(),
                    instruction: "Summarize completion".to_string(),
                    expected_paths: Vec::new(),
                    verify: Vec::new(),
                },
            ],
        };
        let report = step_plan_quality_report(&plan, &PlanQualityContext::default());
        assert!(report.has_retryable_quality(), "{report:?}");
        assert!(
            report
                .issues
                .iter()
                .any(|issue| issue.category == "terminal_report_step")
        );
    }

    #[test]
    fn blocker_report_step_is_allowed() {
        let plan = StepPlan {
            goal: "Report dependency missing blocker".to_string(),
            steps: vec![PlanStep {
                id: "report-blocker".to_string(),
                kind: "report".to_string(),
                expected_result: "pass".to_string(),
                instruction: "Report dependency_missing blocker".to_string(),
                expected_paths: Vec::new(),
                verify: Vec::new(),
            }],
        };
        let report = step_plan_quality_report(&plan, &PlanQualityContext::default());
        assert!(
            !report
                .issues
                .iter()
                .any(|issue| issue.category == "terminal_report_step"),
            "{report:?}"
        );
    }

    #[test]
    fn fresh_workspace_inspect_is_retryable_quality_issue() {
        let plan = StepPlan {
            goal: "Create a small app".to_string(),
            steps: vec![
                PlanStep {
                    id: "inspect-workspace".to_string(),
                    kind: "inspect".to_string(),
                    expected_result: "pass".to_string(),
                    instruction: "Inspect the workspace".to_string(),
                    expected_paths: Vec::new(),
                    verify: Vec::new(),
                },
                PlanStep {
                    id: "code".to_string(),
                    kind: "implement".to_string(),
                    expected_result: "pass".to_string(),
                    instruction: "Create app.py".to_string(),
                    expected_paths: vec!["app.py".to_string()],
                    verify: vec!["python3 -m py_compile app.py".to_string()],
                },
            ],
        };
        let context = PlanQualityContext {
            task_intent: "create".to_string(),
            workspace_context_known: true,
            workspace_snapshot_class: "metadata_only".to_string(),
            has_user_seed_files: false,
            has_only_agent_metadata: true,
            required_artifacts: vec!["app.py".to_string()],
            ..PlanQualityContext::default()
        };
        let report = step_plan_quality_report(&plan, &context);
        assert!(
            report
                .issues
                .iter()
                .any(|issue| issue.category == "fresh_workspace_read_before_write"),
            "{report:?}"
        );
    }

    #[test]
    fn fresh_workspace_unknown_context_does_not_penalize_inspect() {
        let plan = StepPlan {
            goal: "Create a small app".to_string(),
            steps: vec![PlanStep {
                id: "inspect-workspace".to_string(),
                kind: "inspect".to_string(),
                expected_result: "pass".to_string(),
                instruction: "Inspect the workspace".to_string(),
                expected_paths: Vec::new(),
                verify: Vec::new(),
            }],
        };
        let report = step_plan_quality_report(&plan, &PlanQualityContext::default());
        assert!(
            !report
                .issues
                .iter()
                .any(|issue| issue.category == "fresh_workspace_read_before_write"),
            "{report:?}"
        );
    }

    #[test]
    fn invalid_step_id_is_rejected() {
        let mut plan = StepPlan {
            goal: "goal".to_string(),
            steps: vec![step("Bad_ID", "Create file")],
        };
        plan.steps[0].id = "Bad_ID".to_string();
        let report = lint_step_plan_report(&plan);
        assert!(report.has_category("contract"), "{report:?}");
        assert!(
            report.primary_message().contains("kebab-case"),
            "{report:?}"
        );
    }

    #[test]
    fn valid_step_id_examples_are_accepted() {
        for id in ["setup", "create-file-1", "verify-build"] {
            let plan = StepPlan {
                goal: "goal".to_string(),
                steps: vec![step(id, "Create file")],
            };
            let report = lint_step_plan_report(&plan);
            assert!(
                !report.primary_message().contains("step id"),
                "{id}: {report:?}"
            );
        }
    }

    #[test]
    fn ultra_plan_lint_report_uses_same_category_vocabulary() {
        let plan = UltraPlan {
            goal: "goal".to_string(),
            profile: "generic".to_string(),
            style: "default".to_string(),
            intent: "create".to_string(),
            phases: vec![
                UltraPhase {
                    id: "".to_string(),
                    prompt: "".to_string(),
                },
                UltraPhase {
                    id: "x".to_string(),
                    prompt: "/plan-run do it".to_string(),
                },
            ],
        };
        let report = lint_ultra_plan_report(&plan);
        assert!(report.has_category("scaffold"));
        assert!(report.errors.len() >= 2);
    }

    #[test]
    fn ultra_plan_lint_rejects_shell_command_phase_prompt() {
        let plan = UltraPlan {
            goal: "goal".to_string(),
            profile: "generic".to_string(),
            style: "default".to_string(),
            intent: "create".to_string(),
            phases: vec![
                UltraPhase {
                    id: "setup".to_string(),
                    prompt: "npm run build".to_string(),
                },
                UltraPhase {
                    id: "verify".to_string(),
                    prompt: "Verify the generated app with deterministic checks.".to_string(),
                },
            ],
        };
        let report = lint_ultra_plan_report(&plan);
        assert!(report.has_category("scaffold"));
        assert!(report.primary_message().contains("natural-language"));
    }

    fn nextjs_quality_context() -> PlanQualityContext {
        PlanQualityContext {
            profile: "nextjs".to_string(),
            required_artifacts: vec![
                "package.json".to_string(),
                "src/app/page.tsx".to_string(),
                "src/app/layout.tsx".to_string(),
                "src/app/global.d.ts".to_string(),
            ],
            preferred_verify: vec!["npm run build".to_string()],
            dependency_order_hint: Some(
                "Create package.json and an app entrypoint before npm run build".to_string(),
            ),
            task_intent: "create".to_string(),
            workspace_context_known: false,
            workspace_snapshot_class: "unknown".to_string(),
            has_user_seed_files: false,
            has_only_agent_metadata: false,
        }
    }
}
