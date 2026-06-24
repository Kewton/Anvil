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
