use crate::planner::signals::contains_bilingual_token;

pub const UNAMBIGUOUS_SIDE_EFFECT_PATHS: &[&str] =
    &["node_modules", ".next", "__pycache__", ".venv", "venv"];
pub const AMBIGUOUS_SIDE_EFFECT_PATHS: &[&str] = &["dist", "build", "target", "coverage", "out"];

const COMPLETION_CONTRACT_BLOCKED_PATHS: &[&str] =
    &[".anvil", ".git", "target", "node_modules", ".next", ".env"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SideEffectPathTier {
    Unambiguous,
    Ambiguous,
}

impl SideEffectPathTier {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unambiguous => "unambiguous",
            Self::Ambiguous => "ambiguous",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SideEffectExpectedPathDiagnosis {
    pub path: String,
    pub token: String,
    pub tier: SideEffectPathTier,
    pub token_mentioned: bool,
    pub blocked_contract_path: bool,
}

impl SideEffectExpectedPathDiagnosis {
    pub fn should_drop(&self) -> bool {
        matches!(self.tier, SideEffectPathTier::Unambiguous)
            || (matches!(self.tier, SideEffectPathTier::Ambiguous) && !self.token_mentioned)
    }

    pub fn lint_message(&self) -> Option<String> {
        if self.should_drop() {
            return Some(format!(
                "expected path '{}' is a {} dependency/build side-effect path owned by the dependency lifecycle, not the artifact contract",
                self.path,
                self.tier.as_str()
            ));
        }
        if self.blocked_contract_path {
            return Some(format!(
                "goal requests '{}' but it is a blocked contract path: {}",
                self.token, self.path
            ));
        }
        None
    }
}

pub fn diagnose_expected_path(
    path: &str,
    goal_or_plan_text: &str,
) -> Option<SideEffectExpectedPathDiagnosis> {
    let components = normalized_path_components(path);
    let token = components
        .iter()
        .find(|component| UNAMBIGUOUS_SIDE_EFFECT_PATHS.contains(&component.as_str()))
        .cloned();
    if let Some(token) = token {
        return Some(SideEffectExpectedPathDiagnosis {
            path: path.to_string(),
            token,
            tier: SideEffectPathTier::Unambiguous,
            token_mentioned: true,
            blocked_contract_path: completion_contract_blocks_path(path),
        });
    }

    let token = components
        .iter()
        .find(|component| AMBIGUOUS_SIDE_EFFECT_PATHS.contains(&component.as_str()))
        .cloned()?;
    Some(SideEffectExpectedPathDiagnosis {
        path: path.to_string(),
        token: token.clone(),
        tier: SideEffectPathTier::Ambiguous,
        token_mentioned: contains_bilingual_token(goal_or_plan_text, &token),
        blocked_contract_path: completion_contract_blocks_path(path),
    })
}

pub fn completion_contract_blocks_path(path: &str) -> bool {
    let raw = path.trim().trim_start_matches("./");
    COMPLETION_CONTRACT_BLOCKED_PATHS
        .iter()
        .any(|name| raw == *name || raw.starts_with(&format!("{name}/")))
}

fn normalized_path_components(path: &str) -> Vec<String> {
    path.trim()
        .trim_start_matches("./")
        .split(['/', '\\'])
        .filter(|component| !component.is_empty() && *component != ".")
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unambiguous_paths_are_always_drop_candidates() {
        let diagnosis =
            diagnose_expected_path("node_modules/next", "Build a Next.js app").expect("diagnosis");

        assert!(diagnosis.should_drop());
        assert_eq!(diagnosis.tier, SideEffectPathTier::Unambiguous);
        assert_eq!(diagnosis.token, "node_modules");
    }

    #[test]
    fn ambiguous_paths_drop_only_without_goal_token() {
        let absent = diagnose_expected_path("dist", "Create a Next.js app").expect("absent");
        let present = diagnose_expected_path("dist", "Create and publish the dist artifact")
            .expect("present");

        assert!(absent.should_drop());
        assert!(!present.should_drop());
        assert!(!present.blocked_contract_path);
    }

    #[test]
    fn ambiguous_blocked_path_reports_policy_conflict_when_goal_mentions_it() {
        let diagnosis =
            diagnose_expected_path("target", "Create the target output").expect("diagnosis");

        assert!(!diagnosis.should_drop());
        assert!(diagnosis.blocked_contract_path);
        assert_eq!(
            diagnosis.lint_message().as_deref(),
            Some("goal requests 'target' but it is a blocked contract path: target")
        );
    }
}
