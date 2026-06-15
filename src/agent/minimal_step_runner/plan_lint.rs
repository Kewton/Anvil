use std::path::Path;

use super::{PlanStep, StepPlan, UltraPlan, UltraPlanStyle};

#[cfg(test)]
pub(super) fn lint_plan(plan: &StepPlan) -> Result<(), String> {
    lint_plan_with_workspace(plan, None)
}

pub(super) fn lint_plan_with_workspace(
    plan: &StepPlan,
    work_root: Option<&Path>,
) -> Result<(), String> {
    let mut errors = Vec::new();
    lint_instruction_specificity(plan, &mut errors);
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

fn lint_instruction_specificity(plan: &StepPlan, errors: &mut Vec<String>) {
    for step in &plan.steps {
        if step.expected_paths.len() < 2 {
            continue;
        }
        if is_verification_only_step(step) {
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

fn is_verification_only_step(step: &PlanStep) -> bool {
    if step.verify.is_empty() {
        return false;
    }
    let instruction = step.instruction.to_ascii_lowercase();
    let verification_language = instruction.contains("verify")
        || instruction.contains("validate")
        || instruction.contains("check")
        || instruction.contains("run the build")
        || instruction.contains("run build");
    let change_language = instruction.contains("create")
        || instruction.contains("add ")
        || instruction.contains("modify")
        || instruction.contains("update")
        || instruction.contains("edit")
        || instruction.contains("write")
        || instruction.contains("implement");
    verification_language && !change_language
}

fn lint_nextjs_build_order(plan: &StepPlan, work_root: Option<&Path>, errors: &mut Vec<String>) {
    if !plan_looks_like_nextjs(plan) {
        return;
    }

    let all_expected = plan
        .steps
        .iter()
        .flat_map(|step| step.expected_paths.iter())
        .collect::<Vec<_>>();
    let workspace_has_package = work_root
        .map(|root| root.join("package.json").is_file())
        .unwrap_or(false);
    let workspace_has_entry = work_root.map(workspace_has_nextjs_entry).unwrap_or(false);

    if !workspace_has_entry && !all_expected.iter().any(|path| is_nextjs_entry_path(path)) {
        errors.push(
            "Next.js plan must include app/page.tsx or pages/index.tsx in expected_paths"
                .to_string(),
        );
    }

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
