use std::sync::OnceLock;

use serde::Deserialize;

const KNOWLEDGE_TOML: &str = include_str!("evidence_knowledge.toml");

#[derive(Debug, Deserialize)]
pub(crate) struct EvidenceKnowledge {
    pub(crate) vocabulary: EvidenceVocabulary,
    pub(crate) goal_hints: GoalHintKnowledge,
    pub(crate) repair_targets: Vec<EvidenceRepairTargetKnowledge>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct EvidenceVocabulary {
    pub(crate) generic_visible_surface: Vec<String>,
    pub(crate) user_input_handler_case_sensitive: Vec<String>,
    pub(crate) user_input_handler_lower: Vec<String>,
    pub(crate) generic_state_update: Vec<String>,
    pub(crate) adversary_entity: Vec<String>,
    pub(crate) adversary_entity_context: Vec<String>,
    pub(crate) position_or_motion: Vec<String>,
    pub(crate) motion_update: Vec<String>,
    pub(crate) score_or_progression: Vec<String>,
    pub(crate) failure_or_collision: Vec<String>,
    pub(crate) restart_or_recoverable_state: Vec<String>,
    pub(crate) persistence: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct GoalHintKnowledge {
    pub(crate) ascii_stopwords: Vec<String>,
    pub(crate) japanese_stopwords: Vec<String>,
    pub(crate) katakana_prefix_stopwords: Vec<String>,
    pub(crate) translations: Vec<GoalHintTranslation>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct GoalHintTranslation {
    pub(crate) source: String,
    pub(crate) targets: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct EvidenceRepairTargetKnowledge {
    pub(crate) evidence_kinds: Vec<String>,
    pub(crate) path_candidates: Vec<String>,
}

pub(crate) fn get() -> &'static EvidenceKnowledge {
    static KNOWLEDGE: OnceLock<EvidenceKnowledge> = OnceLock::new();
    KNOWLEDGE.get_or_init(|| {
        toml::from_str(KNOWLEDGE_TOML).expect("embedded evidence_knowledge.toml must parse")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_evidence_vocabulary_keeps_required_body() {
        let vocabulary = &get().vocabulary;
        for values in [
            &vocabulary.generic_visible_surface,
            &vocabulary.user_input_handler_case_sensitive,
            &vocabulary.user_input_handler_lower,
            &vocabulary.generic_state_update,
            &vocabulary.adversary_entity,
            &vocabulary.adversary_entity_context,
            &vocabulary.position_or_motion,
            &vocabulary.motion_update,
            &vocabulary.score_or_progression,
            &vocabulary.failure_or_collision,
            &vocabulary.restart_or_recoverable_state,
            &vocabulary.persistence,
        ] {
            assert!(!values.is_empty());
        }
        assert!(
            vocabulary
                .failure_or_collision
                .contains(&"collision".to_string())
        );
        assert!(
            vocabulary
                .score_or_progression
                .contains(&"score".to_string())
        );
        assert!(
            vocabulary
                .restart_or_recoverable_state
                .contains(&"restart".to_string())
        );
    }

    #[test]
    fn adversary_golden_contains_breakout_tokens() {
        let vocabulary = &get().vocabulary;
        for token in ["brick", "block", "paddle", "ball", "ブロック"] {
            assert!(
                vocabulary
                    .adversary_entity
                    .iter()
                    .any(|candidate| candidate == token)
            );
        }
    }

    #[test]
    fn embedded_goal_hint_translation_is_bidirectional() {
        let goal_hints = &get().goal_hints;
        assert!(goal_hints.ascii_stopwords.contains(&"nextjs".to_string()));
        assert!(
            goal_hints
                .japanese_stopwords
                .contains(&"ゲーム".to_string())
        );
        assert_eq!(goal_hints.katakana_prefix_stopwords, vec!["スペース"]);

        let block = goal_hints
            .translations
            .iter()
            .find(|translation| translation.source == "ブロック")
            .expect("block translation must exist");
        assert_eq!(block.targets, vec!["block", "brick"]);
        assert!(goal_hints.translations.iter().any(|translation| {
            translation.source == "ブロック"
                && translation.targets.iter().any(|target| target == "brick")
        }));
    }

    #[test]
    fn embedded_repair_target_mapping_keeps_candidate_order() {
        let mappings = &get().repair_targets;
        assert_eq!(mappings.len(), 1);
        assert_eq!(
            mappings[0].evidence_kinds,
            vec![
                "restart_or_recoverable_state_evidence",
                "challenge_or_adversary_evidence",
                "failure_or_collision_evidence",
                "user_input_handler_evidence",
                "stateful_update_evidence",
                "visible_interactive_surface_evidence",
                "interactive_ui_source_evidence",
                "non_static_screen_evidence",
                "score_or_progression_evidence",
                "browser_interaction_failed",
            ]
        );
        assert_eq!(
            mappings[0].path_candidates,
            vec![
                "src/app/page.tsx",
                "src/app/page.jsx",
                "src/app/page.ts",
                "src/app/page.js",
                "app/page.tsx",
                "app/page.jsx",
                "app/page.ts",
                "app/page.js",
                "pages/index.tsx",
                "pages/index.jsx",
                "pages/index.ts",
                "pages/index.js",
                "src/pages/index.tsx",
                "src/pages/index.jsx",
                "src/pages/index.ts",
                "src/pages/index.js",
            ]
        );
    }
}
