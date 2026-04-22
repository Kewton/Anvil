# UAT — Issue #431: assistant markdown rendering

Manual acceptance checklist for the terminal markdown renderer. The automated
integration test covers session-store invariants, chunk splitting, think-block
handling, overflow, and sanitization; this document covers the four behaviors
that only reproduce in a real terminal: footer coexistence (DECSTBM), SGR
color rendering, `is_terminal()` signaling when piped, and UTF-8 bullet
fallback.

## Prerequisites

- Local Ollama instance responding on `http://127.0.0.1:11434`
- `anvil` built: `cargo build --release`
- Wide terminal (80+ cols) in at least one test cell
- Prompt used for all "interactive" cells:

  ```
  Explain how to sort a list in Python with a small code example. Use a heading, a **bold** word, `inline` code, a bulleted list, and a fenced code block.
  ```

## Checklist (4 environments × footer on/off = 8 cells)

Legend: `fresh` = `--fresh`, `pipe` = pipe stdout through `| cat`.

| # | Environment                                              | Expected                                                                                                                                                                        | Pass? |
|---|----------------------------------------------------------|---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|-------|
| 1 | TTY + color + footer on (default)                        | Full decoration: code block green with 2-space indent, inline code cyan, H1 magenta, `**bold**` bold, `- ` → `● `. Footer stays painted, no tear. No DECSTBM leaks into body.   |       |
| 2 | TTY + color + `ANVIL_FOOTER=0`                           | Full decoration as in #1, no footer line.                                                                                                                                        |       |
| 3 | TTY + `NO_COLOR=1` + footer on                           | Plain text with markdown symbols removed (no `**`, no backticks, no `#`). List bullet still `● ` (UTF-8). No ANSI escapes in body. Footer stays painted.                        |       |
| 4 | TTY + `NO_COLOR=1` + `ANVIL_FOOTER=0`                    | Plain text as in #3, no footer.                                                                                                                                                  |       |
| 5 | TTY + `ANVIL_NO_MARKDOWN=1` + footer on                  | Legacy behavior: raw chunks pass through `print!` verbatim (asterisks, backticks, hashes are visible). Footer stays painted. `~/.anvil/sessions/<id>/session.json` ANSI-free.   |       |
| 6 | TTY + `ANVIL_NO_MARKDOWN=1` + `ANVIL_FOOTER=0`           | Legacy behavior as in #5, no footer.                                                                                                                                             |       |
| 7 | `cargo run -- run \| cat` (piped stdout, footer on)       | No ANSI anywhere on stdout (`is_terminal()` returns false → color disabled). Plain text with markdown symbols removed. Footer disabled automatically when stdout is not a TTY. |       |
| 8 | `cargo run -- --oneshot -p "…" \| cat`                    | No ANSI on stdout. Oneshot stderr banner visible separately. `session.json` ANSI-free.                                                                                          |       |

## Per-cell reproduction

Cell 1 — TTY + color + footer:
```
cargo run --release -- run
```

Cell 2 — TTY + color + no footer:
```
ANVIL_FOOTER=0 cargo run --release -- run
```

Cell 3 — TTY + NO_COLOR + footer:
```
NO_COLOR=1 cargo run --release -- run
```

Cell 4 — TTY + NO_COLOR + no footer:
```
NO_COLOR=1 ANVIL_FOOTER=0 cargo run --release -- run
```

Cell 5 — TTY + ANVIL_NO_MARKDOWN + footer:
```
ANVIL_NO_MARKDOWN=1 cargo run --release -- run
```

Cell 6 — TTY + ANVIL_NO_MARKDOWN + no footer:
```
ANVIL_NO_MARKDOWN=1 ANVIL_FOOTER=0 cargo run --release -- run
```

Cell 7 — Piped REPL:
```
echo "Explain markdown briefly" | cargo run --release -- run | cat
```

Cell 8 — Oneshot pipe:
```
cargo run --release -- --oneshot -p "Explain bold markdown with an example" | cat
```

## Post-session invariant check (all cells)

After running any cell, verify `session.json` stays ANSI-free:

```
SESSION_ID=$(ls -t ~/.anvil/sessions/ | head -1)
! grep -q $'\x1b\\[' ~/.anvil/sessions/$SESSION_ID/session.json && echo OK
```

## AC coverage

- AC1–AC9, AC17–AC19: automated (`cargo test --lib tui::markdown` + `cargo test --test markdown_stream_integration`)
- AC10: CI (`cargo fmt --check` / `cargo clippy --all-targets -- -D warnings` / `cargo test`)
- AC11: cells 1/3/5 (non-stream behavior matches stream behavior)
- AC12: cells 7, 8
- AC13: cell 1 + `session.json` grep (above)
- AC14: automated (`cargo test --lib session::compact`)
- AC15: automated (`cargo test --test markdown_stream_integration find_last_user_prompt_has_no_ansi_after_roundtrip`)
- AC16: cell 1 (footer + long stream coexistence)
- AC17: automated
- AC18: automated
- AC19: automated
