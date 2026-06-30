use serde::{Deserialize, Serialize};
use serde_json::Value;

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

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct GeneratedStepPlan {
    goal: String,
    steps: Vec<GeneratedPlanStep>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct GeneratedPlanStep {
    id: Value,
    #[serde(default = "default_step_kind")]
    kind: String,
    #[serde(default)]
    expected_result: Option<String>,
    instruction: String,
    #[serde(default)]
    expected_paths: Value,
    #[serde(default)]
    verify: Value,
}

impl PlanStep {
    pub fn step_kind(&self) -> StepKind {
        match self.kind.trim().to_ascii_lowercase().as_str() {
            "inspect" => StepKind::Inspect,
            "setup" => StepKind::Setup,
            "implement" | "work" | "create" | "edit" | "repair" => StepKind::Implement,
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

fn default_step_kind() -> String {
    "implement".to_string()
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

pub fn parse_generated_step_plan_json(raw: &str, original_goal: &str) -> anyhow::Result<StepPlan> {
    let json_text = extract_json_object(raw)?;
    let value: Value = serde_json::from_str(json_text)
        .map_err(|err| anyhow::anyhow!("StepPlan invalid JSON: {}", err))?;
    let goal = value
        .get("goal")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    if goal.is_empty() {
        anyhow::bail!("StepPlan missing goal");
    }
    let steps = value.get("steps").and_then(Value::as_array);
    if steps.is_none_or(|steps| steps.is_empty()) {
        anyhow::bail!("StepPlan has no steps");
    }
    let generated: GeneratedStepPlan = serde_json::from_value(value)
        .map_err(|err| anyhow::anyhow!("StepPlan invalid JSON: {}", err))?;
    normalize_generated_step_plan(generated, original_goal)
}

pub fn repair_generated_step_plan_contract(plan: &mut StepPlan) {
    normalize_verify_commands(plan);
    normalize_verify_steps(plan);
    normalize_duplicate_expected_path_ownership(plan);
}

pub fn extract_json_object(raw: &str) -> anyhow::Result<&str> {
    let bytes = raw.as_bytes();
    let mut start = None;
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (index, byte) in bytes.iter().enumerate() {
        if start.is_none() {
            if *byte == b'{' {
                start = Some(index);
                depth = 1;
            }
            continue;
        }
        if in_string {
            if escaped {
                escaped = false;
            } else if *byte == b'\\' {
                escaped = true;
            } else if *byte == b'"' {
                in_string = false;
            }
            continue;
        }
        match *byte {
            b'"' => in_string = true,
            b'{' => depth += 1,
            b'}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    let begin = start.unwrap_or(0);
                    return Ok(&raw[begin..=index]);
                }
            }
            _ => {}
        }
    }
    anyhow::bail!("StepPlan invalid JSON: no JSON object found")
}

fn normalize_generated_step_plan(
    generated: GeneratedStepPlan,
    original_goal: &str,
) -> anyhow::Result<StepPlan> {
    let _provider_goal = generated.goal;
    let mut steps = Vec::new();
    for (index, step) in generated.steps.into_iter().enumerate() {
        let mut expected_paths = Vec::new();
        for path in normalize_string_list(&step.expected_paths, "expected_paths")? {
            crate::tools::path_guard::validate_workspace_relative(&path)
                .map_err(|err| anyhow::anyhow!("StepPlan unsafe expected path: {err}"))?;
            expected_paths.push(path);
        }
        let verify = normalize_string_list(&step.verify, "verify")?;
        let normalized = PlanStep {
            id: normalize_step_id(&step.id, index),
            kind: normalize_step_kind(&step.kind),
            expected_result: normalize_generated_expected_result(step.expected_result, index)?,
            instruction: step.instruction,
            expected_paths,
            verify,
        };
        steps.push(normalized);
    }
    if steps.is_empty() {
        anyhow::bail!("StepPlan has no steps");
    }
    Ok(StepPlan {
        goal: original_goal.to_string(),
        steps,
    })
}

fn normalize_string_list(value: &Value, field: &str) -> anyhow::Result<Vec<String>> {
    match value {
        Value::Null => Ok(Vec::new()),
        Value::String(text) => {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                Ok(Vec::new())
            } else {
                Ok(vec![trimmed.to_string()])
            }
        }
        Value::Array(items) => {
            let mut out = Vec::new();
            for item in items {
                let Some(text) = item.as_str() else {
                    anyhow::bail!("StepPlan invalid JSON: {field} must contain only strings");
                };
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    out.push(trimmed.to_string());
                }
            }
            Ok(out)
        }
        _ => anyhow::bail!("StepPlan invalid JSON: {field} must be a string or string array"),
    }
}

fn normalize_step_id(value: &Value, index: usize) -> String {
    match value {
        Value::String(text) if !text.trim().is_empty() => text.trim().to_string(),
        Value::Number(number) => format!("step-{number}"),
        _ => format!("step-{}", index + 1),
    }
}

fn normalize_step_kind(kind: &str) -> String {
    match kind.trim().to_ascii_lowercase().as_str() {
        "work" | "create" | "edit" | "repair" => "implement".to_string(),
        "inspect" | "setup" | "implement" | "verify" | "report" => kind.trim().to_ascii_lowercase(),
        other => other.to_string(),
    }
}

fn normalize_generated_expected_result(
    value: Option<String>,
    index: usize,
) -> anyhow::Result<String> {
    let Some(value) = value else {
        anyhow::bail!("StepPlan missing expected_result in step {}", index + 1);
    };
    if value.trim().is_empty() {
        anyhow::bail!("StepPlan missing expected_result in step {}", index + 1);
    }
    Ok(normalize_expected_result(&value))
}

fn normalize_expected_result(value: &str) -> String {
    match value.trim().to_ascii_lowercase().as_str() {
        "" | "pass" | "passed" | "success" | "succeed" | "succeeds" => "pass".to_string(),
        "fail" | "failed" | "failure" | "expected-failure" => "fail".to_string(),
        other if other.contains("fail") || other.contains("red") => "fail".to_string(),
        _ => "pass".to_string(),
    }
}

fn normalize_verify_steps(plan: &mut StepPlan) {
    for step in &mut plan.steps {
        if step.step_kind() == StepKind::Verify {
            step.expected_paths.clear();
            if !step.verify.is_empty() && looks_like_file_change_instruction(&step.instruction) {
                step.instruction =
                    "Run the listed deterministic verification commands without changing files."
                        .to_string();
            }
        }
    }
}

fn normalize_verify_commands(plan: &mut StepPlan) {
    for step in &mut plan.steps {
        let mut normalized = Vec::new();
        let mut seen = std::collections::BTreeSet::new();
        for command in &step.verify {
            for item in normalize_verify_command(command) {
                if seen.insert(item.clone()) {
                    normalized.push(item);
                }
            }
        }
        step.verify = normalized;
    }
}

fn normalize_verify_command(command: &str) -> Vec<String> {
    let trimmed = command.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    if let Some(path) = node_exists_sync_path(trimmed) {
        return vec![format!("test -f {path}")];
    }
    vec![trimmed.to_string()]
}

fn node_exists_sync_path(command: &str) -> Option<String> {
    let lower = command.to_ascii_lowercase();
    if !lower.starts_with("node -e") || !lower.contains("existssync") {
        return None;
    }
    let marker = "existsSync(";
    let start = command.find(marker)? + marker.len();
    let rest = command.get(start..)?.trim_start();
    let quote = rest.chars().next()?;
    if quote != '\'' && quote != '"' {
        return None;
    }
    let after_quote = rest.get(quote.len_utf8()..)?;
    let end = after_quote.find(quote)?;
    let path = after_quote.get(..end)?.trim().trim_start_matches("./");
    if path.is_empty() {
        return None;
    }
    crate::tools::path_guard::validate_workspace_relative(path).ok()?;
    Some(path.to_string())
}

fn normalize_duplicate_expected_path_ownership(plan: &mut StepPlan) {
    use std::collections::BTreeMap;

    let mut owners: BTreeMap<String, usize> = BTreeMap::new();
    for (index, step) in plan.steps.iter().enumerate() {
        for path in &step.expected_paths {
            let replace = owners.get(path).is_none_or(|current| {
                ownership_priority(step, path) > ownership_priority(&plan.steps[*current], path)
            });
            if replace {
                owners.insert(path.clone(), index);
            }
        }
    }
    for (index, step) in plan.steps.iter_mut().enumerate() {
        let mut seen_in_step = std::collections::BTreeSet::new();
        step.expected_paths.retain(|path| {
            seen_in_step.insert(path.clone())
                && owners.get(path).is_some_and(|owner| *owner == index)
        });
    }
}

fn ownership_priority(step: &PlanStep, path: &str) -> u8 {
    let manifest_or_config = matches!(
        path,
        "package.json" | "Cargo.toml" | "pyproject.toml" | "tsconfig.json" | "next.config.js"
    );
    match step.step_kind() {
        StepKind::Setup if manifest_or_config => 5,
        StepKind::Implement => 4,
        StepKind::Setup => 3,
        StepKind::Verify => 2,
        StepKind::Inspect | StepKind::Report | StepKind::Unknown(_) => 1,
    }
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
    fn generated_plan_repair_normalizes_verify_instruction_without_paths() {
        let mut plan = StepPlan {
            goal: "goal".to_string(),
            steps: vec![PlanStep {
                id: "verify".to_string(),
                kind: "verify".to_string(),
                expected_result: "pass".to_string(),
                instruction: "Fix the implementation and run tests".to_string(),
                expected_paths: vec!["src/lib.rs".to_string()],
                verify: vec!["cargo test".to_string()],
            }],
        };
        repair_generated_step_plan_contract(&mut plan);
        assert!(plan.steps[0].expected_paths.is_empty());
        assert_eq!(
            plan.steps[0].instruction,
            "Run the listed deterministic verification commands without changing files."
        );
    }

    #[test]
    fn generated_plan_repair_sanitizes_common_verify_policy_issues() {
        let mut plan = StepPlan {
            goal: "goal".to_string(),
            steps: vec![PlanStep {
                id: "verify".to_string(),
                kind: "verify".to_string(),
                expected_result: "pass".to_string(),
                instruction: "Run checks".to_string(),
                expected_paths: Vec::new(),
                verify: vec![
                    "npm test && npm run build".to_string(),
                    "npm install".to_string(),
                    "node -e \"const fs=require('fs'); if (!fs.existsSync('src/app/page.tsx')) process.exit(1)\"".to_string(),
                ],
            }],
        };
        repair_generated_step_plan_contract(&mut plan);
        assert_eq!(
            plan.steps[0].verify,
            vec![
                "npm test && npm run build",
                "npm install",
                "test -f src/app/page.tsx"
            ]
        );
    }

    #[test]
    fn generated_plan_repair_prefers_implement_owner_for_source_paths() {
        let mut plan = StepPlan {
            goal: "goal".to_string(),
            steps: vec![
                PlanStep {
                    id: "setup".to_string(),
                    kind: "setup".to_string(),
                    expected_result: "pass".to_string(),
                    instruction: "Create scaffolding".to_string(),
                    expected_paths: vec![
                        "markdown_lint.py".to_string(),
                        "package.json".to_string(),
                    ],
                    verify: Vec::new(),
                },
                PlanStep {
                    id: "implement".to_string(),
                    kind: "implement".to_string(),
                    expected_result: "pass".to_string(),
                    instruction: "Implement linter".to_string(),
                    expected_paths: vec!["markdown_lint.py".to_string()],
                    verify: Vec::new(),
                },
            ],
        };
        repair_generated_step_plan_contract(&mut plan);
        assert_eq!(plan.steps[0].expected_paths, vec!["package.json"]);
        assert_eq!(plan.steps[1].expected_paths, vec!["markdown_lint.py"]);
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

    #[test]
    fn parse_step_plan_json() {
        let plan = parse_generated_step_plan_json(
            r#"{"goal":"short","steps":[{"id":"create-file","kind":"create","instruction":"Create the file","expected_paths":["src/lib.rs"],"verify":["cargo test"],"expected_result":"pass"}]}"#,
            "original goal",
        )
        .unwrap();
        assert_eq!(plan.goal, "original goal");
        assert_eq!(plan.steps[0].kind, "implement");
        assert_eq!(plan.steps[0].expected_paths, vec!["src/lib.rs"]);
    }

    #[test]
    fn extract_step_plan_json_from_fenced_or_prefixed_text() {
        let raw = "Here is the plan:\n```json\n{\"goal\":\"g\",\"steps\":[{\"id\":\"s1\",\"expected_result\":\"pass\",\"instruction\":\"do it\"}]}\n```";
        let extracted = extract_json_object(raw).unwrap();
        assert!(extracted.starts_with('{'));
        let plan = parse_generated_step_plan_json(raw, "goal").unwrap();
        assert_eq!(plan.steps[0].kind, "implement");
    }

    #[test]
    fn generated_step_plan_preserves_original_goal() {
        let plan = parse_generated_step_plan_json(
            r#"{"goal":"short","steps":[{"id":"s1","expected_result":"pass","instruction":"do it"}]}"#,
            "long original goal",
        )
        .unwrap();
        assert_eq!(plan.goal, "long original goal");
    }

    #[test]
    fn generated_step_plan_rejects_missing_goal_or_steps() {
        let missing_goal =
            parse_generated_step_plan_json(r#"{"steps":[{"id":"s1","expected_result":"pass","instruction":"do it"}]}"#, "g")
                .unwrap_err()
                .to_string();
        assert!(missing_goal.contains("StepPlan missing goal"));
        let empty_steps = parse_generated_step_plan_json(r#"{"goal":"g","steps":[]}"#, "g")
            .unwrap_err()
            .to_string();
        assert!(empty_steps.contains("StepPlan has no steps"));
    }

    #[test]
    fn generated_step_plan_rejects_missing_expected_result() {
        let err = parse_generated_step_plan_json(
            r#"{"goal":"g","steps":[{"id":"s1","instruction":"do it"}]}"#,
            "g",
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("StepPlan missing expected_result"));
    }

    #[test]
    fn generated_step_plan_rejects_unsafe_expected_paths() {
        let err = parse_generated_step_plan_json(
            r#"{"goal":"g","steps":[{"id":"s1","expected_result":"pass","instruction":"do it","expected_paths":["../secret"]}]}"#,
            "g",
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("StepPlan unsafe expected path"));
    }

    #[test]
    fn generated_step_plan_normalizes_numeric_id_and_prose_expected_result() {
        let plan = parse_generated_step_plan_json(
            r#"{"goal":"g","steps":[{"id":1,"kind":"setup","instruction":"Assess workspace","expected_result":"Workspace state assessed."},{"id":2,"kind":"verify","instruction":"Add red test","expected_result":"The focused test should fail before implementation.","verify":["cargo test"]}]}"#,
            "g",
        )
        .unwrap();
        assert_eq!(plan.steps[0].id, "step-1");
        assert_eq!(plan.steps[0].expected_result, "pass");
        assert_eq!(plan.steps[1].id, "step-2");
        assert_eq!(plan.steps[1].expected_result, "fail");
    }

    #[test]
    fn generated_step_plan_normalizes_string_lists() {
        let plan = parse_generated_step_plan_json(
            r#"{"goal":"g","steps":[{"id":"s1","kind":"implement","expected_result":"pass","instruction":"Create README","expected_paths":"README.md","verify":"test -f README.md"}]}"#,
            "g",
        )
        .unwrap();
        assert_eq!(plan.steps[0].expected_paths, vec!["README.md"]);
        assert_eq!(plan.steps[0].verify, vec!["test -f README.md"]);

        let empty = parse_generated_step_plan_json(
            r#"{"goal":"g","steps":[{"id":"s1","kind":"inspect","expected_result":"pass","instruction":"Inspect","expected_paths":"","verify":""}]}"#,
            "g",
        )
        .unwrap();
        assert!(empty.steps[0].expected_paths.is_empty());
        assert!(empty.steps[0].verify.is_empty());
    }
}
