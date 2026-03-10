#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum ModelStrength {
    Small,
    Medium,
    Large,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct LocalModelProfile {
    pub strength: ModelStrength,
}

impl LocalModelProfile {
    pub fn from_model_name(model: &str) -> Self {
        let normalized = model.to_ascii_lowercase();
        if contains_any(&normalized, &["70b", "72b", "32b", "34b", "35b", "coder-large"]) {
            return Self {
                strength: ModelStrength::Large,
            };
        }
        if contains_any(&normalized, &["14b", "20b", "22b"]) {
            return Self {
                strength: ModelStrength::Medium,
            };
        }
        Self {
            strength: ModelStrength::Small,
        }
    }

    pub fn allows_fast_path(self, prompt: &str) -> bool {
        let normalized = prompt.to_ascii_lowercase();
        let grounded = contains_any(
            &normalized,
            &["repo", "repository", "codebase", "branch", "commit", "diff", "review", "build", "test"],
        ) || contains_any(prompt, &["リポジトリ", "ブランチ", "コミット", "差分", "レビュー", "ビルド", "テスト"]);

        if grounded {
            return false;
        }

        match self.strength {
            ModelStrength::Large => true,
            ModelStrength::Medium => prompt.chars().count() <= 80,
            ModelStrength::Small => prompt.chars().count() <= 40 && !contains_any(&normalized, &["current", "this"]),
        }
    }
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::{LocalModelProfile, ModelStrength};

    #[test]
    fn model_strength_infers_from_local_model_name() {
        assert_eq!(
            LocalModelProfile::from_model_name("qwen3.5:35b").strength,
            ModelStrength::Large
        );
        assert_eq!(
            LocalModelProfile::from_model_name("qwen2.5-coder:14b").strength,
            ModelStrength::Medium
        );
        assert_eq!(
            LocalModelProfile::from_model_name("qwen2.5:7b").strength,
            ModelStrength::Small
        );
    }

    #[test]
    fn small_models_avoid_fast_path_for_contextual_requests() {
        let profile = LocalModelProfile::from_model_name("qwen2.5:7b");
        assert!(profile.allows_fast_path("hi"));
        assert!(!profile.allows_fast_path("what is the current objective?"));
        assert!(!profile.allows_fast_path("review the current diff"));
    }
}
