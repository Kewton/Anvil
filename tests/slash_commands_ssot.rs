//! Single-source-of-truth tests for the slash command inventory.
//!
//! These guard the contract declared in Issue #427: the `/help` output and
//! the rustyline tab-completion list must stay in sync, alias /
//! placeholder commands must never leak into the completion set, and
//! `SlashHelper::complete()` itself behaves as documented (returns all
//! candidates for `/`, filters by prefix, and declines to complete at the
//! argument position).

use anvil::agent::loop_run::slash_commands::{SLASH_COMMANDS, SlashHelper, help_line};
use rustyline::Context;
use rustyline::completion::Completer;
use rustyline::history::DefaultHistory;

/// Helper: invoke `SlashHelper::complete()` with a dummy, empty history so
/// tests don't need to stand up a real editor. The completer ignores
/// history — it only looks at `(line, pos)` — so any `Context<'_>` works.
fn complete(line: &str, pos: usize) -> (usize, Vec<String>) {
    let helper = SlashHelper;
    let history = DefaultHistory::new();
    let ctx = Context::new(&history);
    let (start, candidates) = helper
        .complete(line, pos, &ctx)
        .expect("SlashHelper::complete must not error");
    let replacements: Vec<String> = candidates.into_iter().map(|p| p.replacement).collect();
    (start, replacements)
}

#[test]
fn help_line_matches_slash_commands() {
    assert_eq!(help_line(), SLASH_COMMANDS.join(" "));
}

#[test]
fn slash_commands_contains_expected_10() {
    assert_eq!(
        SLASH_COMMANDS,
        &[
            "/help", "/status", "/model", "/yes", "/no", "/plan", "/approve", "/compact", "/logs",
            "/exit",
        ]
    );
}

#[test]
fn slash_commands_excludes_aliases() {
    for excluded in [
        "/act",
        "/quit",
        "/checkpoint",
        "/rollback",
        "/watch",
        "/autotest",
        "/skills",
        "/skill",
        "/mcp",
        "/parallel",
    ] {
        assert!(
            !SLASH_COMMANDS.contains(&excluded),
            "{excluded} must not appear in the completion set"
        );
    }
}

/// A single `/` at pos=1 must expand to the full inventory (10 commands) so
/// Tab-after-`/` shows everything. Guards design policy Section 8.2.
#[test]
fn completer_returns_all_candidates_for_single_slash() {
    let (start, replacements) = complete("/", 1);
    assert_eq!(start, 0, "completion must replace from column 0");
    assert_eq!(
        replacements.len(),
        SLASH_COMMANDS.len(),
        "expected all {} commands, got {replacements:?}",
        SLASH_COMMANDS.len()
    );
    // Order-independent set equality against the SSOT.
    for cmd in SLASH_COMMANDS {
        assert!(
            replacements.iter().any(|r| r == cmd),
            "{cmd} missing from completion set {replacements:?}"
        );
    }
}

/// `/pl` at pos=3 must narrow to `/plan` only. Guards design policy Section 8.2.
#[test]
fn completer_filters_by_prefix() {
    let (_, replacements) = complete("/pl", 3);
    assert_eq!(replacements, vec!["/plan".to_string()]);
}

/// Once the user has typed past the first whitespace token, the completer
/// must stop offering slash-command candidates — that position is an argument
/// (e.g. `/logs path`, `/logs path <session>`), not a command. Guards design
/// policy Section 8.2 / 12.1 (`completer_returns_empty_after_word_boundary`).
#[test]
fn completer_returns_empty_for_args_position() {
    let line = "/logs path";
    let pos = line.len(); // pos = 10, inside the argument token
    let (_, replacements) = complete(line, pos);
    assert!(
        replacements.is_empty(),
        "argument position must not produce slash-command candidates, got {replacements:?}"
    );
}
