# CLAUDE.md

## Repo Intent

このリポジトリは `workspace/v0.1.0` に基づく Rust 版 local-first coding agent の実装。
旧来の Anvil 拡張ではなく、vibe-local 的な小さい Ollama 専用実装へ振り切っている。

## Current Architecture

- `src/config.rs`
  - CLI / env / `.anvil/config` のマージ
- `src/model_registry.rs`
  - 利用可能モデルとメモリ量から main / sidecar を選択
- `src/ollama/client.rs`
  - `/api/tags` `/api/chat` 呼び出し
- `src/ollama/xml_fallback.rs`
  - `<think>` 除去と XML tool call 回収
- `src/tools/*`
  - built-in tools
- `src/modes/plan_act.rs`
  - Plan / Act の単純な状態
- `src/session/*`
  - セッション保存と compaction
- `src/git/checkpoint.rs`
  - checkpoint / rollback

## Development Expectations

- 旧アーキテクチャへ戻す方向の継ぎ足しはしない
- provider abstraction を増やさない
- local LLM 向けの prompt / loop の単純さを優先する
- 新機能は E2E 寄りの検証を伴わせる

## Release Process

変更しない。

- package / binary は `anvil`
- `.github/workflows/ci.yml` は `fmt` `clippy` `test` `build`
- `.github/workflows/release.yml` は tag push 時に gzip artifact を作って GitHub Release を切る
