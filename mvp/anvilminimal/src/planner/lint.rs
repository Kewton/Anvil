use crate::planner::step_plan::StepPlan;
use crate::planner::ultra_plan::UltraPlan;
use crate::tools::path_guard::validate_workspace_relative;

pub fn lint_step_plan(plan: &StepPlan) -> anyhow::Result<()> {
    if plan.steps.len() > 12 {
        anyhow::bail!("StepPlan has too many steps");
    }
    let mut ids = std::collections::BTreeSet::new();
    for step in &plan.steps {
        if step.id.trim().is_empty() {
            anyhow::bail!("step id is empty");
        }
        if !ids.insert(step.id.as_str()) {
            anyhow::bail!("duplicate step id: {}", step.id);
        }
        if looks_like_shell_command(&step.instruction) {
            anyhow::bail!("step instruction must be natural language, not a shell command");
        }
        for path in &step.expected_paths {
            validate_workspace_relative(path)?;
        }
        for command in &step.verify {
            crate::planner::verify::validate_verify_command(command)?;
        }
    }
    Ok(())
}

pub fn lint_ultra_plan(plan: &UltraPlan) -> anyhow::Result<()> {
    if !(2..=8).contains(&plan.phases.len()) {
        anyhow::bail!("UltraPlan must have 2-8 phases");
    }
    let mut ids = std::collections::BTreeSet::new();
    for phase in &plan.phases {
        if phase.id.trim().is_empty() || phase.prompt.trim().is_empty() {
            anyhow::bail!("ultra phase must have id and prompt");
        }
        if !ids.insert(phase.id.as_str()) {
            anyhow::bail!("duplicate ultra phase id: {}", phase.id);
        }
        if phase.prompt.trim_start().starts_with('/') {
            anyhow::bail!("ultra phase prompt must not be a REPL command");
        }
    }
    Ok(())
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
            kind: "work".to_string(),
            instruction: instruction.to_string(),
            expected_paths: Vec::new(),
            verify: Vec::new(),
        }
    }
}
