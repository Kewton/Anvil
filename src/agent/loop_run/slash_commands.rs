//! Single source of truth for the slash commands surfaced in the REPL.
//!
//! Everything user-visible about which slash commands exist in v0.1.0 lives
//! here. `SLASH_COMMANDS` drives both the rustyline tab completer and the
//! `/help` output, so the two cannot drift.

use rustyline::Helper;
use rustyline::completion::{Completer, Pair};
use rustyline::error::ReadlineError;
use rustyline::highlight::Highlighter;
use rustyline::hint::Hinter;
use rustyline::history::FileHistory;
use rustyline::validate::Validator;
use rustyline::{Config as RlConfig, Context, Editor};

/// The complete list of slash commands surfaced to users in v0.1.0.
///
/// Single source of truth for:
///  1. rustyline tab completion (`SlashHelper::complete`)
///  2. `/help` output rendered by `handle_command`
///
/// Aliases (`/act`, `/quit`) and unavailable placeholders
/// (`/checkpoint /rollback /watch /autotest /skills /skill /mcp /parallel`)
/// are deliberately excluded.
pub const SLASH_COMMANDS: &[&str] = &[
    "/help",
    "/status",
    "/model",
    "/yes",
    "/no",
    "/plan",
    "/approve",
    "/compact",
    // IO/状態確認カテゴリ: /logs と /precautions は機能カテゴリで隣接させる
    "/logs",
    "/precautions",
    "/exit",
];

/// Render the `/help` output line from `SLASH_COMMANDS`.
pub fn help_line() -> String {
    SLASH_COMMANDS.join(" ")
}

/// Minimal rustyline `Helper`: completes slash commands at line start and is
/// a no-op for everything else. No hints, no highlighting, no multi-line
/// validation — kept boring so the REPL stays predictable.
pub struct SlashHelper;

impl Completer for SlashHelper {
    type Candidate = Pair;

    fn complete(
        &self,
        line: &str,
        pos: usize,
        _ctx: &Context<'_>,
    ) -> Result<(usize, Vec<Pair>), ReadlineError> {
        if !line.starts_with('/') {
            return Ok((0, Vec::new()));
        }
        // Only complete inside the first whitespace-delimited token. Once the
        // user typed `/plan <something>`, we do not try to complete
        // sub-arguments (no subcommand inventory exists in v0.1.0).
        let token_end = line.find(char::is_whitespace).unwrap_or(line.len());
        if pos > token_end {
            return Ok((0, Vec::new()));
        }
        let prefix = &line[..pos];
        let candidates: Vec<Pair> = SLASH_COMMANDS
            .iter()
            .filter(|cmd| cmd.starts_with(prefix))
            .map(|cmd| Pair {
                display: (*cmd).to_string(),
                replacement: (*cmd).to_string(),
            })
            .collect();
        Ok((0, candidates))
    }
}

impl Hinter for SlashHelper {
    type Hint = String;
}

impl Highlighter for SlashHelper {}

impl Validator for SlashHelper {}

impl Helper for SlashHelper {}

/// Editor type alias used across REPL and tests so the construction details
/// stay in one place (DRY / SSOT for the config).
pub type AnvilEditor = Editor<SlashHelper, FileHistory>;

/// Construct the rustyline editor used by the REPL. Must be called both by
/// production code and by tests so the two cannot drift (`max_history_size`,
/// `history_ignore_space`, etc.).
pub fn build_editor() -> Result<AnvilEditor, ReadlineError> {
    let rl_config = RlConfig::builder()
        .max_history_size(1000)?
        .history_ignore_space(true)
        .auto_add_history(true)
        .build();
    let mut editor: AnvilEditor = Editor::with_config(rl_config)?;
    editor.set_helper(Some(SlashHelper));
    Ok(editor)
}
