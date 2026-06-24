use crate::planner::step_plan::StepPlan;
use crate::planner::ultra_plan::UltraPlan;
use crate::tools::path_guard::validate_workspace_relative;

pub fn lint_step_plan(plan: &StepPlan) -> anyhow::Result<()> {
    if plan.steps.len() > 12 {
        anyhow::bail!("StepPlan has too many steps");
    }
    for step in &plan.steps {
        if step.id.trim().is_empty() {
            anyhow::bail!("step id is empty");
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
    for phase in &plan.phases {
        if phase.id.trim().is_empty() || phase.prompt.trim().is_empty() {
            anyhow::bail!("ultra phase must have id and prompt");
        }
    }
    Ok(())
}
