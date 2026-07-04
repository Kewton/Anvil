//! Shared lexical gates for user-goal / plan text.
//!
//! Keep bilingual intent tokens here so Japanese and English detection drift is
//! visible in one small table instead of being re-added per caller.

use serde_json::Value;

pub(crate) const CANVAS_TOKENS: &[&str] = &["canvas", "キャンバス", "カンバス"];

const GAME_TOKENS: &[&str] = &[
    "game",
    "ゲーム",
    "シューティング",
    "ブロック崩し",
    "ボール崩し",
    "レンガ崩し",
    "テトリス",
    "落ち物",
    "落ちもの",
    "パズル",
];

const INTERACTIVE_TOKENS: &[&str] = &[
    "interactive",
    "playable",
    "操作",
    "反応",
    "プレイ",
    "入力",
    "クリック",
    "キーボード",
];

const SETUP_TOKENS: &[&str] = &[
    "install",
    "setup",
    "bootstrap",
    "configure",
    "dependency",
    "dependencies",
    "environment",
    "依存",
    "インストール",
    "セットアップ",
    "環境",
];

const SETUP_DISQUALIFIER_TOKENS: &[&str] = &[
    "run",
    "start",
    "build",
    "lint",
    "serve",
    "dev",
    "deploy",
    "verify",
    "validate",
    "check",
    "fix ",
    "add ",
    "create ",
    "write ",
    "implement ",
    "実行",
    "起動",
    "ビルド",
    "修正",
    "動作確認",
    "確認",
];

const COMPLETION_MARKERS: &[&str] = &[
    "done",
    "completed",
    "implemented",
    "finished",
    "ready",
    "作成しました",
    "実装しました",
    "完了",
    "できました",
];

const FUTURE_WORK_MARKERS: &[&str] = &[
    "now i'll",
    "now i will",
    "i'll ",
    "i will ",
    "let me ",
    "you can run",
    "please run",
    "run this yourself",
    "run it yourself",
    "next,",
    "next i",
    "i am going to ",
    "i'm going to ",
    "これから",
    "今から",
    "次に",
    "次は",
    "探してみます",
    "確認します",
    "調べます",
    "見てみます",
    "してみます",
    "実行してください",
    "確認してください",
];

const REPO_ACTION_TOKENS: &[&str] = &[
    "edit",
    "write",
    "create",
    "modify",
    "fix",
    "implement",
    "add",
    "delete",
    "update",
    "refactor",
    "test",
    "修正",
    "実装",
    "作成",
    "追加",
    "変更",
    "削除",
    "更新",
];

const PLANNED_TOOL_VERBS: &[&str] = &[
    "create",
    "write",
    "edit",
    "modify",
    "read",
    "inspect",
    "check",
    "verify",
    "test",
    "run",
    "build",
    "起動",
    "作成",
    "書き",
    "編集",
    "修正",
    "確認",
    "検証",
    "実行",
    "ビルド",
];

const NEXTJS_PROFILE_PHRASES: &[&str] = &["next.js", "nextjs", "web app", "web アプリ"];

const PYTHON_CLI_PROFILE_PHRASES: &[&str] = &[".py", "command line", "コマンドライン"];

pub(crate) fn contains_canvas_token(text: &str) -> bool {
    contains_any_token(text, CANVAS_TOKENS)
}

pub(crate) fn contains_game_token(text: &str) -> bool {
    contains_any_token(text, GAME_TOKENS)
}

pub(crate) fn contains_interactive_token(text: &str) -> bool {
    contains_any_token(text, INTERACTIVE_TOKENS)
}

pub(crate) fn contains_setup_token(text: &str) -> bool {
    contains_any_token(text, SETUP_TOKENS)
}

pub(crate) fn contains_setup_disqualifier_token(text: &str) -> bool {
    contains_any_token(text, SETUP_DISQUALIFIER_TOKENS)
}

pub(crate) fn contains_completion_marker(text: &str) -> bool {
    contains_any_token(text, COMPLETION_MARKERS)
}

pub(crate) fn contains_future_work_marker(text: &str) -> bool {
    contains_any_token(text, FUTURE_WORK_MARKERS)
}

pub(crate) fn contains_repo_action_token(text: &str) -> bool {
    contains_any_token(text, REPO_ACTION_TOKENS)
}

pub(crate) fn contains_planned_tool_verb(text: &str) -> bool {
    contains_any_token(text, PLANNED_TOOL_VERBS)
}

pub(crate) fn contains_nextjs_profile_token(text: &str) -> bool {
    contains_any_token(text, NEXTJS_PROFILE_PHRASES)
        || contains_ascii_word_ci(text, "next")
        || contains_ascii_word_ci(text, "react")
}

pub(crate) fn contains_python_cli_profile_token(text: &str) -> bool {
    contains_any_token(text, PYTHON_CLI_PROFILE_PHRASES)
        || contains_ascii_word_ci(text, "python")
        || contains_ascii_word_ci(text, "python3")
        || contains_ascii_word_ci(text, "cli")
}

pub(crate) fn contains_any_token(text: &str, tokens: &[&str]) -> bool {
    let lower = text.to_ascii_lowercase();
    tokens.iter().any(|token| {
        let token_lower = token.to_ascii_lowercase();
        lower.contains(&token_lower) || text.contains(token)
    })
}

pub(crate) fn requested_port_from_texts<'a>(
    texts: impl IntoIterator<Item = &'a str>,
) -> Option<u16> {
    texts.into_iter().find_map(requested_port)
}

pub(crate) fn requested_port(text: &str) -> Option<u16> {
    let bytes = text.as_bytes();
    let mut index = 0usize;
    while index < bytes.len() {
        if !bytes[index].is_ascii_digit() {
            index += 1;
            continue;
        }
        let start = index;
        while index < bytes.len() && bytes[index].is_ascii_digit() {
            index += 1;
        }
        let end = index;
        let candidate = &text[start..end];
        let Ok(port) = candidate.parse::<u16>() else {
            continue;
        };
        if (1024..=65535).contains(&port) && has_port_context(text, start, end) {
            return Some(port);
        }
    }
    None
}

pub(crate) fn script_declared_port_from_package_json(raw: &str) -> Option<u16> {
    let package: Value = serde_json::from_str(raw).ok()?;
    let scripts = package.get("scripts")?.as_object()?;
    ["dev", "start", "preview"]
        .into_iter()
        .filter_map(|name| scripts.get(name).and_then(Value::as_str))
        .find_map(requested_port)
}

fn has_port_context(text: &str, start: usize, end: usize) -> bool {
    let prefix = bounded_prefix(text, start, 16);
    let suffix = bounded_suffix(text, end, 12);
    let before_colon = start > 0 && text.as_bytes().get(start - 1) == Some(&b':');
    before_colon
        || contains_ascii_word(&prefix.to_ascii_lowercase(), "port")
        || prefix.to_ascii_lowercase().contains("-p")
        || prefix.contains("ポート")
        || contains_ascii_word(&suffix.to_ascii_lowercase(), "port")
        || suffix.contains("ポート")
}

fn bounded_prefix(text: &str, end: usize, max_chars: usize) -> &str {
    let mut start = end;
    for (count, (idx, _)) in text[..end].char_indices().rev().enumerate() {
        if count >= max_chars {
            break;
        }
        start = idx;
    }
    &text[start..end]
}

fn bounded_suffix(text: &str, start: usize, max_chars: usize) -> &str {
    let mut end = text.len();
    for (count, (offset, ch)) in text[start..].char_indices().enumerate() {
        if count >= max_chars {
            end = start + offset;
            break;
        }
        end = start + offset + ch.len_utf8();
    }
    &text[start..end]
}

fn contains_ascii_word(haystack: &str, needle: &str) -> bool {
    let bytes = haystack.as_bytes();
    let mut index = 0usize;
    while index < bytes.len() {
        while index < bytes.len() && !is_ascii_word_byte(bytes[index]) {
            index += 1;
        }
        let start = index;
        while index < bytes.len() && is_ascii_word_byte(bytes[index]) {
            index += 1;
        }
        if start < index && &haystack[start..index] == needle {
            return true;
        }
    }
    false
}

fn is_ascii_word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn contains_ascii_word_ci(text: &str, needle: &str) -> bool {
    contains_ascii_word(&text.to_ascii_lowercase(), needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requested_port_accepts_required_patterns() {
        assert_eq!(requested_port("4000番ポートで起動"), Some(4000));
        assert_eq!(requested_port("ポート4001で起動"), Some(4001));
        assert_eq!(requested_port("port 4002"), Some(4002));
        assert_eq!(requested_port("next dev -p 4003"), Some(4003));
        assert_eq!(requested_port("http://localhost:4004/play"), Some(4004));
    }

    #[test]
    fn requested_port_enforces_range_and_first_match() {
        assert_eq!(requested_port("port 1023 then port 4000"), Some(4000));
        assert_eq!(requested_port("port 4001 then :4002"), Some(4001));
        assert_eq!(requested_port("support 4000 users"), None);
        assert_eq!(requested_port("port 65536"), None);
        assert_eq!(requested_port("port 65535"), Some(65535));
    }

    #[test]
    fn bilingual_tokens_cover_japanese_gates() {
        assert!(contains_canvas_token("HTML5 キャンバスで描画"));
        assert!(contains_canvas_token("カンバスを使う"));
        assert!(contains_game_token("ブラウザゲーム"));
        assert!(contains_interactive_token("操作できる画面"));
        assert!(contains_setup_token("依存をインストール"));
        assert!(contains_future_work_marker("これから実装します"));
        assert!(contains_repo_action_token("修正してください"));
    }

    #[test]
    fn profile_tokens_are_bilingual_and_avoid_cli_substrings() {
        assert!(contains_nextjs_profile_token("Web アプリを作成"));
        assert!(contains_nextjs_profile_token("react dashboard"));
        assert!(contains_python_cli_profile_token("コマンドラインツール"));
        assert!(contains_python_cli_profile_token("python CLI"));
        assert!(!contains_python_cli_profile_token("client dashboard"));
    }

    #[test]
    fn script_declared_port_reads_package_scripts_in_order() {
        let raw = r#"{"scripts":{"dev":"next dev -p 4100","start":"next start -p 4200"}}"#;
        assert_eq!(script_declared_port_from_package_json(raw), Some(4100));
    }
}
