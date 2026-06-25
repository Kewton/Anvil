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

pub fn lint_step_plan(plan: &StepPlan) -> anyhow::Result<()> {
    let report = lint_step_plan_report(plan);
    if report.is_pass() {
        return Ok(());
    }
    anyhow::bail!("{}", report.primary_message())
}

pub fn lint_step_plan_report(plan: &StepPlan) -> PlanLintReport {
    let mut report = PlanLintReport::pass();
    if plan.steps.len() > 12 {
        report.push("contract", "StepPlan has too many steps");
    }
    let mut ids = std::collections::BTreeSet::new();
    let mut path_owners = std::collections::BTreeMap::new();
    let mut seen_paths = std::collections::BTreeSet::new();
    let mut setup_seen = false;
    for step in &plan.steps {
        if step.id.trim().is_empty() {
            report.push("contract", "step id is empty");
        }
        if !ids.insert(step.id.as_str()) {
            report.push("contract", format!("duplicate step id: {}", step.id));
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
            if is_build_verify(command) && !setup_seen && !step_creates_dependency_manifest(step) {
                report.push(
                    "dependency_order",
                    "verify command requires dependency setup or package manifest first",
                );
            }
            if is_nextjs_build(command) && !has_nextjs_entrypoint(&seen_paths, step) {
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

fn validate_step_kind_contract(step: &crate::planner::step_plan::PlanStep) -> anyhow::Result<()> {
    match step.step_kind() {
        StepKind::Inspect => {
            if !step.expected_paths.is_empty() || !step.verify.is_empty() {
                anyhow::bail!("inspect step may not declare expected paths or verify commands");
            }
        }
        StepKind::Setup => {
            if step.verify.iter().any(|command| is_build_verify(command)) {
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

fn is_build_verify(command: &str) -> bool {
    let lower = command.to_ascii_lowercase();
    lower.contains("test") || lower.contains("build")
}

fn is_nextjs_build(command: &str) -> bool {
    let lower = command.to_ascii_lowercase();
    lower == "npm run build" || lower == "pnpm build" || lower == "yarn build"
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
    let entrypoints = [
        "src/app/page.tsx",
        "src/app/page.jsx",
        "app/page.tsx",
        "app/page.jsx",
        "pages/index.tsx",
        "pages/index.jsx",
        "src/pages/index.tsx",
        "src/pages/index.jsx",
    ];
    entrypoints.iter().any(|path| {
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
}
