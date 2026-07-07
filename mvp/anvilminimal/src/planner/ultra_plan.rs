use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UltraPlan {
    pub goal: String,
    pub profile: String,
    pub style: String,
    pub intent: String,
    pub phases: Vec<UltraPhase>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UltraPhase {
    pub id: String,
    pub prompt: String,
}

impl UltraPlan {
    pub fn deterministic(goal: &str, profile: &str, style: &str, intent: &str) -> Self {
        Self {
            goal: goal.to_string(),
            profile: profile.to_string(),
            style: style.to_string(),
            intent: intent.to_string(),
            phases: vec![
                UltraPhase {
                    id: "scaffold".to_string(),
                    prompt: format!("Scaffold the project for: {goal}"),
                },
                UltraPhase {
                    id: "implement".to_string(),
                    prompt: format!("Implement the main behavior for: {goal}"),
                },
                UltraPhase {
                    id: "verify".to_string(),
                    prompt: format!("Verify and repair the project for: {goal}"),
                },
            ],
        }
    }
}

pub fn render_ultra_plan(plan: &UltraPlan) -> String {
    let mut out = format!(
        "goal: {}\nprofile: {}\nstyle: {}\nintent: {}\nphases:\n",
        quote_yaml_string(&plan.goal),
        quote_yaml_string(&plan.profile),
        quote_yaml_string(&plan.style),
        quote_yaml_string(&plan.intent)
    );
    for phase in &plan.phases {
        out.push_str(&format!("  - id: {}\n", quote_yaml_string(&phase.id)));
        out.push_str(&format!(
            "    prompt: {}\n",
            quote_yaml_string(&phase.prompt)
        ));
    }
    out
}

pub(crate) fn quote_yaml_string(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| format!("{value:?}"))
}

pub fn parse_ultra_plan(text: &str) -> anyhow::Result<UltraPlan> {
    let text = extract_yaml(text);
    let mut goal = String::new();
    let mut profile = "generic".to_string();
    let mut style = "default".to_string();
    let mut intent = "create".to_string();
    let mut phases = Vec::new();
    let mut current: Option<UltraPhase> = None;
    for raw in text.lines() {
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed == "phases:" {
            continue;
        }
        if let Some(value) = trimmed.strip_prefix("goal:") {
            goal = unquote(value.trim());
        } else if let Some(value) = trimmed.strip_prefix("profile:") {
            profile = unquote(value.trim());
        } else if let Some(value) = trimmed.strip_prefix("style:") {
            style = unquote(value.trim());
        } else if let Some(value) = trimmed.strip_prefix("intent:") {
            intent = unquote(value.trim());
        } else if let Some(value) = trimmed.strip_prefix("- id:") {
            if let Some(phase) = current.take() {
                phases.push(phase);
            }
            current = Some(UltraPhase {
                id: unquote(value.trim()),
                prompt: String::new(),
            });
        } else if let Some(value) = trimmed.strip_prefix("prompt:")
            && let Some(phase) = current.as_mut()
        {
            phase.prompt = unquote(value.trim());
        }
    }
    if let Some(phase) = current {
        phases.push(phase);
    }
    if goal.is_empty() {
        anyhow::bail!("UltraPlan missing goal");
    }
    if phases.is_empty() {
        anyhow::bail!("UltraPlan has no phases");
    }
    Ok(UltraPlan {
        goal,
        profile,
        style,
        intent,
        phases,
    })
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
    fn ultra_yaml_round_trip() {
        let plan = UltraPlan::deterministic("goal", "nextjs", "default", "create");
        assert_eq!(parse_ultra_plan(&render_ultra_plan(&plan)).unwrap(), plan);
    }

    #[test]
    fn ultra_yaml_round_trip_preserves_escaped_and_multiline_prompts() {
        let plan = UltraPlan {
            goal: "厚いゲーム: quotes \"inside\", slash \\, and multiline\nsecond line"
                .to_string(),
            profile: "nextjs".to_string(),
            style: "default".to_string(),
            intent: "recover".to_string(),
            phases: vec![
                UltraPhase {
                    id: "inspect-current-state".to_string(),
                    prompt: "Inspect files with escaped quotes like \\\"boss\\\" and Windows path C:\\tmp\\game.\n日本語の行も保持する。".to_string(),
                },
                UltraPhase {
                    id: "repair-final-acceptance".to_string(),
                    prompt: "Repair the generated thick game without restarting.\nFailure evidence:\n- ./src/app/game.ts\n- Error:\n-   x Expected ',', got '}'\n- > Build failed because of webpack errors\nKeep template literals such as `Score: ${score}` intact.".to_string(),
                },
                UltraPhase {
                    id: "verify-recovery".to_string(),
                    prompt: "Run `npm run build`; if it fails, keep the exact SWC frame and repair only the syntax error.".to_string(),
                },
            ],
        };

        let rendered = render_ultra_plan(&plan);
        let parsed = parse_ultra_plan(&rendered).unwrap();
        assert_eq!(parsed, plan);
        assert_eq!(parse_ultra_plan(&render_ultra_plan(&parsed)).unwrap(), plan);
    }

    #[test]
    fn fenced_yaml_is_supported() {
        let plan = UltraPlan::deterministic("goal", "nextjs", "default", "create");
        let text = format!("```yaml\n{}\n```", render_ultra_plan(&plan));
        assert_eq!(parse_ultra_plan(&text).unwrap(), plan);
    }
}
