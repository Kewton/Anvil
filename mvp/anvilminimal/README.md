# anvilminimal

Copyable MVP for a minimal local-first coding loop, YAML step plans, plan run,
ultra plan run, and deterministic verification.

## Build

```bash
cargo build
cargo test
cargo run -- --help
```

## Run

```bash
anvilminimal --yes --context-budget 65536 --model qwen3.6:27b-coding-nvfp4 --planner-model gemini-3.5-flash --planner-provider gemini --provider ollama
```

From the REPL:

```text
/ultra-plan-run --profile nextjs あなたが考える最高に面白くかっこいいスペースインベーダーゲームを3011ポートで起動可能なnext.jsアプリとして開発してください。
```

CLI equivalent:

```bash
anvilminimal --provider ollama --model qwen3.6:27b-coding-nvfp4 \
  --planner-provider gemini --planner-model gemini-3.5-flash \
  --ultra-plan-run --profile nextjs \
  "あなたが考える最高に面白くかっこいいスペースインベーダーゲームを3011ポートで起動可能なnext.jsアプリとして開発してください。"
```

## TUI

Interactive TTY mode uses the same `anvil>` prompt and slash commands, plus
terminal-only markdown rendering, spinner, ESC boundary interrupt, and a fixed
footer. Disable paths:

```bash
ANVIL_NO_SPINNER=1 anvilminimal --yes
ANVIL_NO_FOOTER=1 anvilminimal --yes
ANVIL_NO_INTERRUPT=1 anvilminimal --yes
ANVIL_NO_MARKDOWN=1 anvilminimal --yes
NO_COLOR=1 anvilminimal --yes
anvilminimal --yes --no-footer
```

Release/manual TTY smoke:

```bash
ANVIL_PTY_TESTS=1 cargo test tui_pty_smoke -- --ignored
anvilminimal --help
anvilminimal --yes --context-budget 65536 --model qwen3.6:27b-coding-nvfp4 --planner-model gemini-3.5-flash --planner-provider gemini --provider ollama
```

## UAT

Before a UAT run, verify the binary provenance:

```bash
anvilminimal --version
command -v anvilminimal
```

`anvilminimal --version` should show the intended commit, dirty marker, and
build timestamp. `command -v anvilminimal` should resolve to the expected
`target/` binary or install path for the run.

## API Keys

`OPENAI_API_KEY` and `GEMINI_API_KEY` are read from process env first, then
from `.env` in the active workspace. Values are redacted from logs.

Live provider tests are gated:

```bash
ANVIL_LIVE_PROVIDER_TESTS=1 cargo test live_ -- --ignored
```

Smoke model IDs can be overridden with `ANVIL_OPENAI_SMOKE_MODEL` and
`ANVIL_GEMINI_SMOKE_MODEL`.

## Local Symlink

```bash
cargo build --release
ln -sfn "$(pwd)/target/release/anvilminimal" "$HOME/.local/bin/anvilminimal"
anvilminimal --help
```

The symlink is a local convenience and is not part of the copy artifact.

## Copy Validation

```bash
tmp=$(mktemp -d)
cp -R mvp/anvilminimal "$tmp/"
cd "$tmp/anvilminimal"
cargo test
cargo run -- --help
```
