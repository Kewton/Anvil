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
    pub expected_result: String,
    pub instruction: String,
    pub expected_paths: Vec<String>,
    pub verify: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StepKind {
    Inspect,
    Setup,
    Implement,
    Verify,
    Report,
    Unknown(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExpectedResult {
    Pass,
    Fail,
    Unknown(String),
}

impl PlanStep {
    pub fn step_kind(&self) -> StepKind {
        match self.kind.trim().to_ascii_lowercase().as_str() {
            "inspect" => StepKind::Inspect,
            "setup" => StepKind::Setup,
            "implement" | "work" => StepKind::Implement,
            "verify" => StepKind::Verify,
            "report" => StepKind::Report,
            other => StepKind::Unknown(other.to_string()),
        }
    }

    pub fn expected_result_kind(&self) -> ExpectedResult {
        match self.expected_result.trim().to_ascii_lowercase().as_str() {
            "" | "pass" => ExpectedResult::Pass,
            "fail" => ExpectedResult::Fail,
            other => ExpectedResult::Unknown(other.to_string()),
        }
    }
}

impl StepPlan {
    pub fn single(goal: &str) -> Self {
        Self {
            goal: goal.to_string(),
            steps: vec![PlanStep {
                id: "step-1".to_string(),
                kind: "report".to_string(),
                expected_result: "pass".to_string(),
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
        out.push_str(&format!(
            "    expected_result: {:?}\n",
            step.expected_result
        ));
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
                steps.push(normalize_legacy_step(step));
            }
            current = Some(PlanStep {
                id: unquote(value.trim()),
                kind: "work".to_string(),
                expected_result: "pass".to_string(),
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
        } else if let Some(value) = trimmed.strip_prefix("expected_result:") {
            step.expected_result = unquote(value.trim());
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
        steps.push(normalize_legacy_step(step));
    }
    if goal.is_empty() {
        anyhow::bail!("StepPlan missing goal");
    }
    if steps.is_empty() {
        anyhow::bail!("StepPlan has no steps");
    }
    Ok(StepPlan { goal, steps })
}

pub fn parse_step_plan_with_default_goal(
    text: &str,
    fallback_goal: &str,
) -> anyhow::Result<(StepPlan, bool)> {
    match parse_step_plan(text) {
        Ok(plan) => Ok((plan, false)),
        Err(err) if err.to_string().contains("StepPlan missing goal") => {
            let repaired = format!("goal: {:?}\n{}", fallback_goal, extract_yaml(text));
            parse_step_plan(&repaired)
                .map(|plan| (plan, true))
                .map_err(|_| err)
        }
        Err(err) => Err(err),
    }
}

fn normalize_legacy_step(mut step: PlanStep) -> PlanStep {
    if step.kind == "report" && (!step.expected_paths.is_empty() || !step.verify.is_empty()) {
        step.kind = "implement".to_string();
    }
    step
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
        plan.steps[0].kind = "implement".to_string();
        plan.steps[0]
            .expected_paths
            .push("package.json".to_string());
        let parsed = parse_step_plan(&render_step_plan(&plan)).unwrap();
        assert_eq!(parsed, plan);
    }

    #[test]
    fn legacy_step_plan_defaults_kind_and_expected_result() {
        let parsed = parse_step_plan(
            r#"goal: "goal"
steps:
  - id: "s1"
    instruction: "write code"
"#,
        )
        .unwrap();
        assert_eq!(parsed.steps[0].kind, "work");
        assert_eq!(parsed.steps[0].step_kind(), StepKind::Implement);
        assert_eq!(parsed.steps[0].expected_result_kind(), ExpectedResult::Pass);
    }

    #[test]
    fn step_plan_parses_typed_kind_and_expected_result() {
        let parsed = parse_step_plan(
            r#"goal: "goal"
steps:
  - id: "s1"
    kind: "verify"
    expected_result: "fail"
    instruction: "run failing test"
    verify:
      - "cargo test"
"#,
        )
        .unwrap();
        assert_eq!(parsed.steps[0].step_kind(), StepKind::Verify);
        assert_eq!(parsed.steps[0].expected_result_kind(), ExpectedResult::Fail);
        let rendered = render_step_plan(&parsed);
        assert!(rendered.contains("expected_result"));
    }

    #[test]
    fn missing_goal_can_be_repaired_from_user_goal_when_steps_exist() {
        let (parsed, repaired) = parse_step_plan_with_default_goal(
            r#"steps:
  - id: "s1"
    instruction: "write code"
"#,
            "fallback goal",
        )
        .unwrap();
        assert!(repaired);
        assert_eq!(parsed.goal, "fallback goal");
        assert_eq!(parsed.steps.len(), 1);
    }

    #[test]
    fn missing_steps_is_not_auto_repaired() {
        let err = parse_step_plan_with_default_goal("goal: x", "fallback")
            .unwrap_err()
            .to_string();
        assert!(err.contains("StepPlan has no steps"));
    }
}
