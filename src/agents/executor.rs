use crate::agents::planning::PlanStep;
use crate::agents::pm::AgentRole;
use crate::agents::AgentResult;

#[derive(Debug, Default, Clone, Copy)]
pub struct StepExecutor;

#[derive(Debug, Clone)]
pub struct ExecutionTrace {
    pub delegated_roles: Vec<AgentRole>,
    pub results: Vec<AgentResult>,
}

impl StepExecutor {
    pub fn execute<F>(&self, steps: &[PlanStep], base_context: &str, mut run_step: F) -> ExecutionTrace
    where
        F: FnMut(&PlanStep, &str) -> AgentResult,
    {
        let mut delegated_roles = Vec::new();
        let mut results = Vec::new();
        let mut step_context = base_context.to_string();

        for step in steps {
            let result = run_step(step, &step_context);
            delegated_roles.push(step.role);
            step_context.push_str("\n[source=subagent]\n");
            step_context.push_str(&result.role);
            step_context.push_str(": ");
            step_context.push_str(&result.summary);
            let should_stop = result.is_blocked() || result.needs_confirmation();
            results.push(result);
            if should_stop {
                break;
            }
        }

        ExecutionTrace {
            delegated_roles,
            results,
        }
    }
}
