//! Answer-only mode shell-command allowlist + script-execution
//! fallback response builder extracted from `turn.rs`.
//!
//! Hosts:
//!
//! * `ANSWER_ONLY_SCRIPT_ALLOWED_PREFIXES` — prefix whitelist of
//!   allowed script launchers (`bash` / `sh` / `./` / `python` /
//!   `python3` / `node`).
//! * `ANSWER_ONLY_SCRIPT_BLOCKED_CONTAINS` / `_BLOCKED_PREFIXES` —
//!   denylists for shell-control operators (`>` / `>>` / `2>` / `|`
//!   / `&&` / `||` / `;`) and file-mutating commands (`rm` / `mv` /
//!   `cp` / `touch` / `mkdir` / `tee` / `sed -i` / `perl -pi`).
//! * `answer_only_script_command_allowed` — top-level predicate that
//!   walks the optional `cd <dir> && <rest>` chain and applies the
//!   allowlist / denylist checks. Pure / no I/O.
//! * `answer_only_script_execution_fallback_response` — Japanese
//!   templated response shown after a script execution in answer-only
//!   mode (truncates the raw output to 1,600 chars, surfaces
//!   `exit_code=...` if present).
//!
//! `pub(super)` limited / no facade re-export (DR3-001).
//! `turn.rs` is the only in-crate consumer.

pub(super) const ANSWER_ONLY_SCRIPT_ALLOWED_PREFIXES: &[&str] =
    &["bash ", "sh ", "./", "python ", "python3 ", "node "];

pub(super) const ANSWER_ONLY_SCRIPT_BLOCKED_CONTAINS: &[&str] = &[
    " >", ">>", " 2>", " | ", " && ", " || ", ";", " rm ", " mv ", " cp ", " touch ", " mkdir ",
    " tee ", "sed -i", "perl -pi",
];

pub(super) const ANSWER_ONLY_SCRIPT_BLOCKED_PREFIXES: &[&str] =
    &["rm ", "mv ", "cp ", "touch ", "mkdir ", "tee "];

/// UTF-8-safe char-count truncation used by the answer-only script
/// execution fallback response. Returns the original string when its
/// char count is already at or below `max_chars`; otherwise keeps
/// `max_chars - 32` characters and appends a
/// `\n...[truncated N chars]` footer.
pub(super) fn truncate_for_answer(text: &str, max_chars: usize) -> String {
    let total = text.chars().count();
    if total <= max_chars {
        return text.to_string();
    }
    let keep = max_chars.saturating_sub(32);
    let truncated = text.chars().take(keep).collect::<String>();
    format!(
        "{truncated}\n...[truncated {} chars]",
        total.saturating_sub(keep)
    )
}

pub(super) fn answer_only_script_execution_fallback_response(output: &str) -> String {
    let excerpt = truncate_for_answer(output.trim(), 1_600);
    let status = output
        .lines()
        .find_map(|line| line.trim().strip_prefix("exit_code="))
        .map(|code| {
            if code == "0" {
                "コマンドは exit_code=0 で正常終了しています。".to_string()
            } else {
                format!("コマンドは exit_code={code} で終了しています。")
            }
        })
        .unwrap_or_else(|| "コマンドの出力を確認しました。".to_string());

    format!(
        "ファイルは変更せず、指定されたコマンド/スクリプトの実行結果を確認しました。\n\n実行結果:\n```text\n{excerpt}\n```\n\n要約:\n- {status}\n- 上記の stdout/stderr が今回確認できた実行結果です。"
    )
}

fn answer_only_script_contains_any(command: &str, patterns: &[&str]) -> bool {
    patterns.iter().any(|pattern| command.contains(pattern))
}

fn answer_only_script_starts_with_any(command: &str, prefixes: &[&str]) -> bool {
    prefixes.iter().any(|prefix| command.starts_with(prefix))
}

fn answer_only_cd_segment_allowed(segment: &str) -> bool {
    segment.starts_with("cd ") && !answer_only_script_contains_any(segment, &[";", "|", ">"])
}

fn answer_only_cd_chained_script_tail(command: &str) -> Option<&str> {
    let (cd_segment, rest) = command.split_once(" && ")?;
    answer_only_cd_segment_allowed(cd_segment).then_some(rest)
}

fn answer_only_script_has_blocked_operation(command: &str) -> bool {
    answer_only_script_contains_any(command, ANSWER_ONLY_SCRIPT_BLOCKED_CONTAINS)
        || answer_only_script_starts_with_any(command, ANSWER_ONLY_SCRIPT_BLOCKED_PREFIXES)
}

pub(super) fn answer_only_script_command_allowed(command: &str) -> bool {
    let trimmed = command.trim();
    if trimmed.is_empty() {
        return false;
    }
    let lower = trimmed.to_ascii_lowercase();
    if let Some(rest) = answer_only_cd_chained_script_tail(&lower) {
        return answer_only_script_command_allowed(rest);
    }
    if answer_only_script_has_blocked_operation(&lower) {
        return false;
    }
    answer_only_script_starts_with_any(&lower, ANSWER_ONLY_SCRIPT_ALLOWED_PREFIXES)
}
