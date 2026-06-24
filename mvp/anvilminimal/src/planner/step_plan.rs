use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StepPlan {
    pub goal: String,
    pub steps: Vec<PlanStep>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanStep {
    pub id: String,
    pub kind: String,
    pub instruction: String,
    pub expected_paths: Vec<String>,
    pub verify: Vec<String>,
}

impl StepPlan {
    pub fn single(goal: &str) -> Self {
        Self {
            goal: goal.to_string(),
            steps: vec![PlanStep {
                id: "step-1".to_string(),
                kind: "work".to_string(),
                instruction: goal.to_string(),
                expected_paths: Vec::new(),
                verify: Vec::new(),
            }],
        }
    }
}

pub fn render_step_plan(plan: &StepPlan) -> String {
    let mut out = format!("goal: {:?}\nsteps:\n", plan.goal);
    for step in &plan.steps {
        out.push_str(&format!("  - id: {:?}\n", step.id));
        out.push_str(&format!("    kind: {:?}\n", step.kind));
        out.push_str(&format!("    instruction: {:?}\n", step.instruction));
        out.push_str("    expected_paths:\n");
        for path in &step.expected_paths {
            out.push_str(&format!("      - {:?}\n", path));
        }
        out.push_str("    verify:\n");
        for command in &step.verify {
            out.push_str(&format!("      - {:?}\n", command));
        }
    }
    out
}

pub fn parse_step_plan(text: &str) -> anyhow::Result<StepPlan> {
    let text = extract_yaml(text);
    let mut goal = String::new();
    let mut steps = Vec::new();
    let mut current: Option<PlanStep> = None;
    let mut list_mode = "";
    for raw in text.lines() {
        let line = raw.trim_end();
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed == "steps:" {
            continue;
        }
        if let Some(value) = trimmed.strip_prefix("goal:") {
            goal = unquote(value.trim());
            continue;
        }
        if let Some(value) = trimmed.strip_prefix("- id:") {
            if let Some(step) = current.take() {
                steps.push(step);
            }
            current = Some(PlanStep {
                id: unquote(value.trim()),
                kind: "work".to_string(),
                instruction: String::new(),
                expected_paths: Vec::new(),
                verify: Vec::new(),
            });
            list_mode = "";
            continue;
        }
        let Some(step) = current.as_mut() else {
            continue;
        };
        if let Some(value) = trimmed.strip_prefix("kind:") {
            step.kind = unquote(value.trim());
        } else if let Some(value) = trimmed.strip_prefix("instruction:") {
            step.instruction = unquote(value.trim());
        } else if trimmed == "expected_paths:" {
            list_mode = "expected_paths";
        } else if trimmed == "verify:" {
            list_mode = "verify";
        } else if let Some(value) = trimmed.strip_prefix("- ") {
            match list_mode {
                "expected_paths" => step.expected_paths.push(unquote(value.trim())),
                "verify" => step.verify.push(unquote(value.trim())),
                _ => {}
            }
        }
    }
    if let Some(step) = current {
        steps.push(step);
    }
    if goal.is_empty() {
        anyhow::bail!("StepPlan missing goal");
    }
    if steps.is_empty() {
        anyhow::bail!("StepPlan has no steps");
    }
    Ok(StepPlan { goal, steps })
}

fn extract_yaml(text: &str) -> &str {
    if let Some(start) = text.find("```") {
        let after = &text[start + 3..];
        let after = after.strip_prefix("yaml").unwrap_or(after);
        if let Some(end) = after.find("```") {
            return &after[..end];
        }
    }
    text
}

fn unquote(value: &str) -> String {
    let value = value.trim();
    if value.len() >= 2 && value.starts_with('"') && value.ends_with('"') {
        serde_json::from_str(value).unwrap_or_else(|_| value.trim_matches('"').to_string())
    } else {
        value.trim_matches('"').trim_matches('\'').to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yaml_round_trip() {
        let mut plan = StepPlan::single("make thing");
        plan.steps[0]
            .expected_paths
            .push("package.json".to_string());
        let parsed = parse_step_plan(&render_step_plan(&plan)).unwrap();
        assert_eq!(parsed, plan);
    }
}
