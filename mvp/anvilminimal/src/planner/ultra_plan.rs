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
        "goal: {:?}\nprofile: {:?}\nstyle: {:?}\nintent: {:?}\nphases:\n",
        plan.goal, plan.profile, plan.style, plan.intent
    );
    for phase in &plan.phases {
        out.push_str(&format!("  - id: {:?}\n", phase.id));
        out.push_str(&format!("    prompt: {:?}\n", phase.prompt));
    }
    out
}

pub fn parse_ultra_plan(text: &str) -> anyhow::Result<UltraPlan> {
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
}
