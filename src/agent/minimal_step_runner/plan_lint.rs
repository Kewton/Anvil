use std::path::Path;

use super::{PlanStep, StepKind, StepPlan, UltraPlan, UltraPlanStyle};

#[cfg(test)]
pub(super) fn lint_plan(plan: &StepPlan) -> Result<(), String> {
    lint_plan_with_workspace(plan, None)
}

pub(super) fn lint_plan_with_workspace(
    plan: &StepPlan,
    work_root: Option<&Path>,
) -> Result<(), String> {
    let mut errors = Vec::new();
    lint_instruction_specificity(plan, work_root, &mut errors);
    lint_dependency_setup_before_verify(plan, work_root, &mut errors);
    lint_nextjs_build_order(plan, work_root, &mut errors);
    if errors.is_empty() {
        Ok(())
    } else {
        Err(format!("plan lint failed: {}", errors.join("; ")))
    }
}

pub(super) fn lint_ultra_plan(plan: &UltraPlan) -> Result<(), String> {
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

fn lint_instruction_specificity(
    plan: &StepPlan,
    work_root: Option<&Path>,
    errors: &mut Vec<String>,
) {
    let mut introduced_paths = Vec::new();
    for step in &plan.steps {
        if step.expected_paths.len() < 2 {
            introduced_paths.extend(step.expected_paths.iter().cloned());
            continue;
        }
        if is_verification_only_step(step)
            && expected_paths_are_known(step, &introduced_paths, work_root)
        {
            introduced_paths.extend(step.expected_paths.iter().cloned());
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
        introduced_paths.extend(step.expected_paths.iter().cloned());
    }
}

fn is_verification_only_step(step: &PlanStep) -> bool {
    if step.verify.is_empty() {
        return false;
    }
    if step.kind == StepKind::Verify {
        return true;
    }
    let instruction = step.instruction.to_ascii_lowercase();
    let id = step.id.replace('-', " ").to_ascii_lowercase();
    let combined = format!("{id}\n{instruction}");
    let verification_language = combined.contains("verify")
        || combined.contains("validate")
        || combined.contains("check")
        || combined.contains("test")
        || combined.contains("build")
        || combined.contains("compile");
    let change_language = combined.contains("create")
        || combined.contains("add ")
        || combined.contains("modify")
        || combined.contains("update")
        || combined.contains("edit")
        || combined.contains("write")
        || combined.contains("implement")
        || combined.contains("scaffold")
        || combined.contains("generate");
    verification_language && !change_language
}

fn lint_dependency_setup_before_verify(
    plan: &StepPlan,
    work_root: Option<&Path>,
    errors: &mut Vec<String>,
) {
    let mut dependency_setup_seen = work_root
        .map(|root| root.join("node_modules").is_dir())
        .unwrap_or(false);
    for step in &plan.steps {
        if step.kind == StepKind::Setup
            || instruction_has_dependency_setup_language(&step.instruction)
        {
            dependency_setup_seen = true;
        }
        if step.verify.iter().any(|command| {
            matches!(
                command.as_str(),
                "npm run build" | "npm test" | "npm run test"
            )
        }) && !dependency_setup_seen
        {
            errors.push(format!(
                "step {} runs npm verification before a dependency setup step or existing node_modules",
                step.id
            ));
        }
    }
}

fn instruction_has_dependency_setup_language(value: &str) -> bool {
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

fn expected_paths_are_known(
    step: &PlanStep,
    introduced_paths: &[String],
    work_root: Option<&Path>,
) -> bool {
    step.expected_paths.iter().all(|path| {
        introduced_paths.iter().any(|known| known == path)
            || work_root
                .map(|root| root.join(path).exists())
                .unwrap_or(false)
    })
}

fn lint_nextjs_build_order(plan: &StepPlan, work_root: Option<&Path>, errors: &mut Vec<String>) {
    if !plan_looks_like_nextjs(plan) {
        return;
    }

    let workspace_has_package = work_root
        .map(|root| root.join("package.json").is_file())
        .unwrap_or(false);
    let workspace_has_entry = work_root.map(workspace_has_nextjs_entry).unwrap_or(false);

    let mut package_seen = workspace_has_package;
    let mut entry_seen = workspace_has_entry;
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

fn workspace_has_nextjs_entry(work_root: &Path) -> bool {
    [
        "app/page.tsx",
        "app/page.jsx",
        "pages/index.tsx",
        "pages/index.jsx",
    ]
    .iter()
    .any(|path| work_root.join(path).is_file())
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
