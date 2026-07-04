use regex::Regex;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortSource {
    Goal,
    Plan,
}

impl PortSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Goal => "goal",
            Self::Plan => "plan",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequestedPort {
    pub port: u16,
    pub source: PortSource,
}

pub fn requested_port_from_text(text: &str) -> Option<u16> {
    let patterns = [
        (r"(?i)\bport\s+(\d{2,5})\b", 0usize),
        (r"-p\s*(\d{2,5})\b", 1usize),
        (r":(\d{2,5})\b", 2usize),
        (r"ポート\s*(\d{2,5})", 3usize),
        (r"(\d{2,5})\s*番?\s*ポート", 4usize),
    ];
    patterns
        .iter()
        .filter_map(|(pattern, order)| {
            let regex = Regex::new(pattern).ok()?;
            regex
                .captures_iter(text)
                .filter_map(move |captures| {
                    let matched = captures.get(0)?;
                    let raw = captures.get(1)?.as_str();
                    let port = raw.parse::<u32>().ok()?;
                    if (1024..=65535).contains(&port) {
                        Some((matched.start(), *order, port as u16))
                    } else {
                        None
                    }
                })
                .min_by_key(|(start, order, _)| (*start, *order))
        })
        .min_by_key(|(start, order, _)| (*start, *order))
        .map(|(_, _, port)| port)
}

pub fn requested_port(goal: &str, plan_text: Option<&str>) -> Option<RequestedPort> {
    requested_port_from_text(goal)
        .map(|port| RequestedPort {
            port,
            source: PortSource::Goal,
        })
        .or_else(|| {
            plan_text
                .and_then(requested_port_from_text)
                .map(|port| RequestedPort {
                    port,
                    source: PortSource::Plan,
                })
        })
}

pub fn contains_canvas_token(text: &str) -> bool {
    contains_any(text, CANVAS_TOKENS)
}

pub fn contains_game_token(text: &str) -> bool {
    contains_any(text, GAME_TOKENS) || contains_canvas_token(text)
}

pub fn contains_persistence_token(text: &str) -> bool {
    contains_any(text, PERSISTENCE_TOKENS)
}

pub fn contains_interactive_token(text: &str) -> bool {
    contains_any(text, INTERACTIVE_TOKENS) || contains_persistence_token(text)
}

pub fn contains_app_like_token(text: &str) -> bool {
    contains_any(text, APP_LIKE_TOKENS)
        || contains_game_token(text)
        || contains_interactive_token(text)
}

pub fn matched_app_intent_token(text: &str) -> Option<&'static str> {
    matched_app_intent(text, APP_INTENT_TOKENS)
}

pub fn contains_app_intent_token(text: &str) -> bool {
    matched_app_intent_token(text).is_some()
}

pub fn contains_browser_probe_token(text: &str) -> bool {
    contains_any(text, BROWSER_PROBE_TOKENS) || contains_game_token(text)
}

pub fn contains_setup_token(text: &str) -> bool {
    contains_any(text, SETUP_TOKENS)
}

pub fn contains_nextjs_goal_token(text: &str) -> bool {
    contains_any(text, NEXTJS_GOAL_TOKENS)
}

pub fn contains_python_cli_goal_token(text: &str) -> bool {
    contains_any(text, PYTHON_CLI_GOAL_TOKENS)
}

pub fn plan_adherence_stopword(token: &str) -> bool {
    STOPWORDS.contains(&token)
}

fn contains_any(text: &str, tokens: &[&str]) -> bool {
    matched_any(text, tokens).is_some()
}

fn matched_any<'a>(text: &str, tokens: &'a [&'a str]) -> Option<&'a str> {
    let lower = text.to_ascii_lowercase();
    tokens.iter().copied().find(|token| {
        if token.chars().any(|ch| ch.is_ascii_alphabetic()) {
            lower.contains(&token.to_ascii_lowercase())
        } else {
            text.contains(token)
        }
    })
}

fn matched_app_intent<'a>(text: &str, tokens: &'a [&'a str]) -> Option<&'a str> {
    let lower = text.to_ascii_lowercase();
    tokens.iter().copied().find(|token| {
        if token.chars().any(|ch| ch.is_ascii_alphabetic()) {
            contains_ascii_intent_token(&lower, &token.to_ascii_lowercase())
        } else {
            text.contains(token)
        }
    })
}

fn contains_ascii_intent_token(lower: &str, token: &str) -> bool {
    lower.match_indices(token).any(|(index, _)| {
        let before_ok = lower[..index]
            .chars()
            .next_back()
            .is_none_or(|ch| !ch.is_ascii_alphanumeric() && ch != '_');
        let after_index = index + token.len();
        let after_ok = lower[after_index..]
            .chars()
            .next()
            .is_none_or(|ch| !ch.is_ascii_alphanumeric() && !matches!(ch, '_' | '.' | '/' | '\\'));
        before_ok && after_ok
    })
}

const CANVAS_TOKENS: &[&str] = &["canvas", "キャンバス", "カンバス"];

const GAME_TOKENS: &[&str] = &[
    "game",
    "playable",
    "player",
    "enemy",
    "enemies",
    "adversary",
    "opponent",
    "obstacle",
    "collision",
    "bullet",
    "lives",
    "game over",
    "ゲーム",
    "シューティング",
];

const PERSISTENCE_TOKENS: &[&str] = &[
    "localstorage",
    "local storage",
    "storage",
    "persist",
    "saved",
    "save",
    "ローカルストレージ",
    "保存",
];

const INTERACTIVE_TOKENS: &[&str] = &[
    "button",
    "form",
    "keyboard",
    "input",
    "interactive",
    "score",
    "todo",
    "markdown",
    "note",
    "notes",
    "editor",
    "edit",
    "delete",
    "filter",
    "preview",
    "操作",
    "追加",
    "完了",
    "削除",
    "フィルタ",
    "編集",
    "一覧",
    "プレビュー",
    "入力",
];

const APP_LIKE_TOKENS: &[&str] = &[
    "app",
    "application",
    "tool",
    "game",
    "ui",
    "form",
    "アプリ",
    "ツール",
    "ゲーム",
    "画面",
    "フォーム",
];

const APP_INTENT_TOKENS: &[&str] = &[
    "アプリ",
    "app",
    "application",
    "ツール",
    "tool",
    "ゲーム",
    "game",
    "UI",
    "画面",
    "フォーム",
    "form",
];

const BROWSER_PROBE_TOKENS: &[&str] = &["interactive", "keyboard", "browser", "操作", "入力"];

const SETUP_TOKENS: &[&str] = &[
    "setup",
    "install",
    "scaffold",
    "init",
    "initialize",
    "initialise",
    "project-setup",
    "project setup",
    "セットアップ",
    "インストール",
];

const NEXTJS_GOAL_TOKENS: &[&str] = &[
    "next.js",
    concat!("next", "js"),
    "react",
    "web app",
    "webアプリ",
    "web アプリ",
    "ウェブアプリ",
];

const PYTHON_CLI_GOAL_TOKENS: &[&str] = &[
    "python",
    "cli",
    "command line",
    "command-line",
    "コマンドライン",
    "コマンド ライン",
];

const STOPWORDS: &[&str] = &[
    "acceptance",
    "add",
    "all",
    "and",
    "app",
    "application",
    "build",
    "component",
    "components",
    "complete",
    "create",
    "current",
    "feature",
    "features",
    "final",
    "for",
    "from",
    "game",
    "goal",
    "implement",
    "implementation",
    "interactive",
    "into",
    "logic",
    "next",
    concat!("next", "js"),
    "page",
    "phase",
    "player",
    "preserve",
    "project",
    "react",
    "screen",
    "setup",
    "state",
    "task",
    "that",
    "the",
    "tsx",
    "typescript",
    "ultra",
    "use",
    "using",
    "verify",
    "with",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requested_port_extracts_supported_forms_in_text_order() {
        assert_eq!(requested_port_from_text("4000番ポートで起動"), Some(4000));
        assert_eq!(requested_port_from_text("ポート4001で起動"), Some(4001));
        assert_eq!(requested_port_from_text("run on port 4002"), Some(4002));
        assert_eq!(requested_port_from_text("next dev -p 4003"), Some(4003));
        assert_eq!(
            requested_port_from_text("http://localhost:4004"),
            Some(4004)
        );
        assert_eq!(
            requested_port_from_text("http://localhost:4005 then port 4006"),
            Some(4005)
        );
    }

    #[test]
    fn requested_port_rejects_out_of_range_ports() {
        assert_eq!(requested_port_from_text("port 80"), None);
        assert_eq!(requested_port_from_text("port 65536"), None);
    }

    #[test]
    fn bilingual_tokens_cover_japanese_goal_gates() {
        assert!(contains_canvas_token("キャンバスを描画"));
        assert!(contains_canvas_token("カンバスを描画"));
        assert!(contains_game_token("シューティングを作る"));
        assert!(contains_interactive_token("入力して追加できる"));
        assert!(contains_setup_token("依存をインストールする"));
    }
}
