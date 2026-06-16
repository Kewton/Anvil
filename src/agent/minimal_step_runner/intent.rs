use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WorkIntent {
    Create,
    Fix,
    Investigate,
    Enhance,
    Document,
    Refactor,
    #[default]
    Unknown,
}

impl WorkIntent {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Create => "create",
            Self::Fix => "fix",
            Self::Investigate => "investigate",
            Self::Enhance => "enhance",
            Self::Document => "document",
            Self::Refactor => "refactor",
            Self::Unknown => "unknown",
        }
    }
}

impl fmt::Display for WorkIntent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl FromStr for WorkIntent {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().replace('_', "-").as_str() {
            "create" | "new" | "build" => Ok(Self::Create),
            "fix" | "bugfix" | "bug-fix" => Ok(Self::Fix),
            "investigate" | "investigation" | "triage" | "research" => Ok(Self::Investigate),
            "enhance" | "feature" | "add-feature" => Ok(Self::Enhance),
            "document" | "docs" | "documentation" => Ok(Self::Document),
            "refactor" | "refactoring" => Ok(Self::Refactor),
            "unknown" | "auto" | "default" => Ok(Self::Unknown),
            other => Err(format!(
                "unknown work intent `{other}`; expected create, fix, investigate, enhance, document, refactor, or unknown"
            )),
        }
    }
}

pub(in crate::agent::minimal_step_runner) fn detect_work_intent(goal: &str) -> WorkIntent {
    let lower = goal.to_ascii_lowercase();
    let contains_any = |items: &[&str]| items.iter().any(|item| lower.contains(item));
    if contains_any(&[
        "investigate",
        "triage",
        "research",
        "root cause",
        "原因",
        "調査",
        "分析",
    ]) {
        WorkIntent::Investigate
    } else if contains_any(&["fix", "bug", "repair", "修正", "不具合", "直して", "直す"])
    {
        WorkIntent::Fix
    } else if contains_any(&[
        "document",
        "documentation",
        "docs",
        "readme",
        "ドキュメント",
        "文書",
    ]) {
        WorkIntent::Document
    } else if contains_any(&["refactor", "refactoring", "リファクタ"]) {
        WorkIntent::Refactor
    } else if contains_any(&["enhance", "feature", "add ", "追加", "改善", "拡張"]) {
        WorkIntent::Enhance
    } else if contains_any(&[
        "create", "build", "develop", "new ", "scaffold", "作成", "開発", "構築", "新規",
    ]) {
        WorkIntent::Create
    } else {
        WorkIntent::Unknown
    }
}
