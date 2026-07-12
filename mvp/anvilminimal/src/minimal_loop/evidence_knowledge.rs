use std::sync::OnceLock;

use serde::Deserialize;

const KNOWLEDGE_TOML: &str = include_str!("evidence_knowledge.toml");

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub(crate) struct EvidenceKnowledge {
    pub(crate) vocabulary: EvidenceVocabulary,
    pub(crate) goal_hints: GoalHintKnowledge,
    pub(crate) repair_targets: Vec<EvidenceRepairTargetKnowledge>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
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
#[allow(dead_code)]
pub(crate) struct GoalHintKnowledge {
    pub(crate) ascii_stopwords: Vec<String>,
    pub(crate) japanese_stopwords: Vec<String>,
    pub(crate) katakana_prefix_stopwords: Vec<String>,
    pub(crate) translations: Vec<GoalHintTranslation>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub(crate) struct GoalHintTranslation {
    pub(crate) source: String,
    pub(crate) targets: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub(crate) struct EvidenceRepairTargetKnowledge {
    pub(crate) evidence_kinds: Vec<String>,
    pub(crate) path_candidates: Vec<String>,
}

#[allow(dead_code)]
pub(crate) fn get() -> &'static EvidenceKnowledge {
    static KNOWLEDGE: OnceLock<EvidenceKnowledge> = OnceLock::new();
    KNOWLEDGE.get_or_init(|| {
        toml::from_str(KNOWLEDGE_TOML).expect("embedded evidence_knowledge.toml must parse")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const LEGACY_GENERIC_VISIBLE_SURFACE: &[&str] = &[
        "button",
        "form",
        "input",
        "textarea",
        "select",
        "screen",
        "surface",
        "view",
        "onclick",
        "click",
        "tap",
        "submit",
        "画面",
        "フォーム",
        "入力",
        "ボタン",
    ];
    const LEGACY_USER_INPUT_HANDLER_CASE_SENSITIVE: &[&str] = &["addEventListener"];
    const LEGACY_USER_INPUT_HANDLER_LOWER: &[&str] = &[
        "onkeydown",
        "onkeyup",
        "onclick",
        "onpointer",
        "onmousedown",
        "onmouseup",
        "ontouch",
        "onsubmit",
        "onchange",
        "keydown",
        "keyup",
        "pointerdown",
        "touchstart",
        "keypressed",
        "mousepressed",
        "inputhandler",
        "input_handler",
        "handleinput",
        "handle_input",
    ];
    const LEGACY_GENERIC_STATE_UPDATE: &[&str] = &[
        "state",
        "status",
        "update(",
        "update_",
        "render(",
        "rerender",
        "refresh",
        "set_",
        "set(",
        "items",
        "notes",
        "todos",
        "table.insert",
        "push(",
        "append(",
        "score",
        "count",
        "counter",
    ];
    const LEGACY_ADVERSARY_ENTITY: &[&str] = &[
        "enemy",
        "enemies",
        "adversary",
        "opponent",
        "obstacle",
        "hazard",
        "invader",
        "alien",
        "ufo",
        "asteroid",
        "monster",
        "zombie",
        "mob",
        "wave",
        "spawn",
        "target",
        "challenge",
        "boss",
        "brick",
        "bricks",
        "block",
        "blocks",
        "paddle",
        "ball",
        "puck",
        "meteor",
        "barrier",
        "timer",
        "countdown",
        "敵",
        "ブロック",
        "パドル",
        "ボール",
        "障害物",
        "インベーダー",
        "エイリアン",
        "モンスター",
    ];
    const LEGACY_ADVERSARY_ENTITY_CONTEXT: &[&str] = &[
        "x:",
        "y:",
        ".x",
        ".y",
        "array.from",
        ".map(",
        ".foreach(",
        ".filter(",
        "setenemies(",
        "setinvaders(",
        "enemy =",
        "enemy=",
        "enemies =",
        "enemies=",
        "invader =",
        "invader=",
        "invaders =",
        "invaders=",
        "const enemy",
        "const enemies",
        "const invader",
        "const invaders",
        "let enemy",
        "let enemies",
        "let invader",
        "let invaders",
    ];
    const LEGACY_POSITION_OR_MOTION: &[&str] = &[
        "position",
        "positions",
        "velocity",
        "speed",
        "move",
        "movement",
        "direction",
        ".x",
        ".y",
        "x:",
        "y:",
        "left",
        "top",
        "translate",
    ];
    const LEGACY_MOTION_UPDATE: &[&str] = &[
        "+=",
        "-=",
        "map(",
        "filter(",
        "set",
        "update",
        "tick",
        "frame",
        "requestanimationframe",
        "setinterval",
    ];
    const LEGACY_SCORE_OR_PROGRESSION: &[&str] = &[
        "score",
        "points",
        "level",
        "stage",
        "wave",
        "combo",
        "lives",
        "life",
        "health",
        "progress",
        "スコア",
    ];
    const LEGACY_FAILURE_OR_COLLISION: &[&str] = &[
        "collision",
        "collide",
        "hit",
        "damage",
        "gameover",
        "game over",
        "lives",
        "life",
        "health",
        "intersect",
        "overlap",
        "bounds",
        "lose",
        "fail",
        "衝突",
        "当たり",
    ];
    const LEGACY_RESTART_OR_RECOVERABLE_STATE: &[&str] = &[
        "start",
        "restart",
        "reset",
        "pause",
        "resume",
        "gameover",
        "game over",
        "play again",
        "try again",
        "initgame",
        "initstate",
        "resetstate",
        "newgame",
        "newstate",
        "newlevel",
        "スタート",
        "開始",
    ];
    const LEGACY_PERSISTENCE: &[&str] = &[
        "localstorage",
        "sessionstorage",
        "indexeddb",
        ".setitem(",
        ".getitem(",
        "navigator.storage",
        "caches.open(",
    ];
    const LEGACY_ASCII_STOPWORDS: &[&str] = &[
        "application",
        "browser",
        "build",
        "canvas",
        "client",
        "component",
        "create",
        "develop",
        "development",
        "feature",
        "game",
        "games",
        "implement",
        "implementation",
        "interactive",
        "next",
        "nextjs",
        "page",
        "playable",
        "port",
        "project",
        "react",
        "screen",
        "shooting",
        "space",
        "typescript",
        "using",
        "with",
    ];
    const LEGACY_JAPANESE_STOPWORDS: &[&str] = &[
        "アプリ",
        "ゲーム",
        "シューティング",
        "スペース",
        "ネクスト",
        "ブラウザ",
        "ページ",
        "ポート",
        "実装",
        "作成",
        "開発",
    ];
    const LEGACY_TRANSLATIONS: &[(&str, &[&str])] = &[
        ("ブロック", &["block", "brick"]),
        ("ボール", &["ball"]),
        ("パドル", &["paddle"]),
        ("敵", &["enemy"]),
        ("インベーダー", &["invader", "invaders"]),
        ("ミサイル", &["missile"]),
        ("シューティング", &["shooter"]),
        ("エイリアン", &["alien"]),
        ("障害物", &["obstacle", "barrier"]),
        ("隕石", &["meteor"]),
        ("タイマー", &["timer"]),
        ("カウントダウン", &["countdown", "timer"]),
    ];
    const LEGACY_BEHAVIORAL_EVIDENCE: &[&str] = &[
        "restart_or_recoverable_state_evidence",
        "challenge_or_adversary_evidence",
        "failure_or_collision_evidence",
        "user_input_handler_evidence",
        "stateful_update_evidence",
        "visible_interactive_surface_evidence",
        "interactive_ui_source_evidence",
        "non_static_screen_evidence",
        "score_or_progression_evidence",
    ];
    const LEGACY_NEXTJS_ENTRYPOINTS: &[&str] = &[
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
    ];

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    #[test]
    fn embedded_evidence_vocabulary_matches_legacy_arrays() {
        let vocabulary = &get().vocabulary;
        assert_eq!(
            vocabulary.generic_visible_surface,
            strings(LEGACY_GENERIC_VISIBLE_SURFACE)
        );
        assert_eq!(
            vocabulary.user_input_handler_case_sensitive,
            strings(LEGACY_USER_INPUT_HANDLER_CASE_SENSITIVE)
        );
        assert_eq!(
            vocabulary.user_input_handler_lower,
            strings(LEGACY_USER_INPUT_HANDLER_LOWER)
        );
        assert_eq!(
            vocabulary.generic_state_update,
            strings(LEGACY_GENERIC_STATE_UPDATE)
        );
        assert_eq!(
            vocabulary.adversary_entity,
            strings(LEGACY_ADVERSARY_ENTITY)
        );
        assert_eq!(
            vocabulary.adversary_entity_context,
            strings(LEGACY_ADVERSARY_ENTITY_CONTEXT)
        );
        assert_eq!(
            vocabulary.position_or_motion,
            strings(LEGACY_POSITION_OR_MOTION)
        );
        assert_eq!(vocabulary.motion_update, strings(LEGACY_MOTION_UPDATE));
        assert_eq!(
            vocabulary.score_or_progression,
            strings(LEGACY_SCORE_OR_PROGRESSION)
        );
        assert_eq!(
            vocabulary.failure_or_collision,
            strings(LEGACY_FAILURE_OR_COLLISION)
        );
        assert_eq!(
            vocabulary.restart_or_recoverable_state,
            strings(LEGACY_RESTART_OR_RECOVERABLE_STATE)
        );
        assert_eq!(vocabulary.persistence, strings(LEGACY_PERSISTENCE));
    }

    #[test]
    fn embedded_goal_hints_match_legacy_values_and_translate_both_directions() {
        let goal_hints = &get().goal_hints;
        assert_eq!(goal_hints.ascii_stopwords, strings(LEGACY_ASCII_STOPWORDS));
        assert_eq!(
            goal_hints.japanese_stopwords,
            strings(LEGACY_JAPANESE_STOPWORDS)
        );
        assert_eq!(goal_hints.katakana_prefix_stopwords, strings(&["スペース"]));
        assert_eq!(goal_hints.translations.len(), LEGACY_TRANSLATIONS.len());
        for (translation, (source, targets)) in goal_hints
            .translations
            .iter()
            .zip(LEGACY_TRANSLATIONS.iter())
        {
            assert_eq!(translation.source, *source);
            assert_eq!(translation.targets, strings(targets));
        }

        let block = goal_hints
            .translations
            .iter()
            .find(|translation| translation.source == "ブロック")
            .expect("block translation must exist");
        assert_eq!(block.targets, strings(&["block", "brick"]));
        assert!(goal_hints.translations.iter().any(|translation| {
            translation.source == "ブロック"
                && translation.targets.iter().any(|target| target == "brick")
        }));
    }

    #[test]
    fn adversary_golden_contains_breakout_tokens() {
        let adversary = &get().vocabulary.adversary_entity;
        for token in ["brick", "block", "paddle", "ball", "ブロック"] {
            assert!(adversary.iter().any(|candidate| candidate == token));
        }
    }

    #[test]
    fn embedded_repair_target_mapping_matches_legacy_values() {
        let mappings = &get().repair_targets;
        assert_eq!(mappings.len(), 1);
        assert_eq!(
            mappings[0].evidence_kinds,
            strings(LEGACY_BEHAVIORAL_EVIDENCE)
        );
        assert_eq!(
            mappings[0].path_candidates,
            strings(LEGACY_NEXTJS_ENTRYPOINTS)
        );
    }
}
