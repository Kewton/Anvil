# anvilminimal

Copyable MVP for a minimal local-first coding loop, YAML step plans, plan run,
ultra plan run, and deterministic verification.

See [SECURITY.md](SECURITY.md) for the trusted-workspace threat model,
`--yes` guidance, environment allowlist, and symlink policy.
See [docs/dev-guardrails.md](docs/dev-guardrails.md) for the runner growth
tripwire and module-boundary guardrails.

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

Named presets can live in `.anvil/config.toml` or `~/.anvil/config.toml`.
These are examples only; Anvil does not auto-create them:

```toml
# [preset.gemini-tier]
# provider = "gemini"
# model = "gemini-3.5-flash"
# planner_provider = "gemini"
# planner_model = "gemini-3.5-flash"
# context_budget = 65536
# chat_timeout_secs = 180
# profile = "nextjs"
# narration = "normal"
# footer = "on"
#
# [preset.local]
# provider = "ollama"
# model = "qwen3.6:27b-coding-nvfp4"
# planner_provider = "ollama"
# planner_model = "qwen3.6:27b-coding-nvfp4"
# context_budget = 65536
# chat_timeout_secs = 600
# profile = "generic"
# narration = "normal"
# footer = "on"
# plan_preset = "profile" # optional opt-in; omitted defaults to none
#
# [preset.hybrid-a3b]
# provider = "ollama"
# model = "qwen3.6:35b-a3b-coding-nvfp4"
# planner_provider = "gemini"
# planner_model = "gemini-3.5-flash"
# context_budget = 65536
# chat_timeout_secs = 600
# profile = "nextjs"
# narration = "normal"
# footer = "on"
```

```bash
anvilminimal --preset local --ultra-plan-run "Build a small CLI tool"
```

Hybrid presets use the planner provider/model only for planning calls and the
main provider/model for execution calls. With a local a3b executor, budget RAM
for the Ollama model residency separately from any remote planner; Ollama
requests keep `keep_alive=10m` for the executor model between turns.

Plan presets are explicit opt-ins, not globally enabled. Although
test0711_bs_001 showed a strong qwen27 sample, test0711_bs_004 exposed duplicate
setup-step stagnation in preset implementation phases, so qwen27, gemma-family,
and unmatched planner models currently default to `none`. Planner-tier
resolution remains observable and is independent of whether the model came
from a direct CLI option, the executor-model fallback, or a named config preset.
Set `plan_preset = "profile"` in config or pass `--plan-preset profile` to opt in;
an explicit CLI flag always wins over config and tier defaults.

## TUI

Interactive TTY mode uses the same `anvil>` prompt and slash commands, plus
terminal-only markdown rendering, spinner, Esc/Ctrl-C prompt interrupt, and a fixed
footer. Disable paths:

```bash
ANVIL_NO_SPINNER=1 anvilminimal --yes
ANVIL_NO_FOOTER=1 anvilminimal --yes
ANVIL_NO_INTERRUPT=1 anvilminimal --yes
ANVIL_NO_MARKDOWN=1 anvilminimal --yes
NO_COLOR=1 anvilminimal --yes
anvilminimal --yes --no-footer
anvilminimal --yes --footer off
```

Release/manual TTY smoke:

```bash
ANVIL_PTY_TESTS=1 cargo test tui_pty_smoke -- --ignored
anvilminimal --ux-demo
anvilminimal --help
anvilminimal --yes --context-budget 65536 --model qwen3.6:27b-coding-nvfp4 --planner-model gemini-3.5-flash --planner-provider gemini --provider ollama
```

`anvilminimal --ux-demo` is an offline presentation walkthrough for human
review. It exercises the banner, plan card, phase header, activity narration,
live footer interrupt hint, and terminal summary card without contacting a
provider. If a terminal still shows cursor-region artifacts, rerun with
`--footer off`; scrollback breadcrumbs stay enabled without the DECSTBM footer.

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
