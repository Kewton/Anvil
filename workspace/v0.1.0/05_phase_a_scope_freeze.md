# v0.1.0 Phase A Scope Freeze

作成日: 2026-04-16
対象: `anvil` v0.1.0 local-first rebuild

## 目的

`workspace/v0.1.0` の方針に沿って、v0.1.0 のコア責務と後段機能を固定する。
これ以降の Phase B/C では、この文書を基準に「何を残し、何を外し、何を作り直すか」を判断する。

## 1. v0.1.0 コア責務

v0.1.0 で release candidate 判定の対象にするのは次だけ。

- CLI / config / logging
- Ollama client
- XML fallback
- model registry
- built-in tools
  - `Bash`
  - `Read`
  - `Write`
  - `Edit`
  - `Glob`
  - `Grep`
- small agent loop
- Plan / Act
- session persistence / compaction
- localhost host validation
- workspace path guard
- live Ollama E2E

上記以外は、存在していても v0.1.0 のコア成功条件には含めない。

## 2. v0.1.0 で残す実装単位

コア経路として残す対象:

- `src/main.rs`
- `src/lib.rs`
- `src/cli.rs`
- `src/config.rs`
- `src/logging.rs`
- `src/system_prompt.rs`
- `src/model_registry.rs`
- `src/ollama/mod.rs`
- `src/ollama/client.rs`
- `src/ollama/xml_fallback.rs`
- `src/session/mod.rs`
- `src/session/store.rs`
- `src/session/compact.rs`
- `src/tools/mod.rs`
- `src/tools/registry.rs`
- `src/tools/bash.rs`
- `src/tools/read.rs`
- `src/tools/write.rs`
- `src/tools/edit.rs`
- `src/tools/glob.rs`
- `src/tools/grep.rs`
- `src/agent/mod.rs`
- `src/agent/prompting.rs`
- `src/agent/recovery.rs`
- `src/agent/loop_run.rs`
- `src/modes/mod.rs`
- `src/modes/plan_act.rs`
- `src/safety/mod.rs`
- `src/safety/host_validation.rs`
- `src/safety/path_guard.rs`

ただし `src/agent/loop_run.rs` は現状維持ではなく、Phase C で small loop に作り直す前提とする。

## 3. v0.1.0 コアから外す実装単位

以下は「後で戻す候補」であり、Phase B で接続を外す。

- `src/git/mod.rs`
- `src/git/checkpoint.rs`
- `src/watch/mod.rs`
- `src/watch/file_watcher.rs`
- `src/testloop/mod.rs`
- `src/testloop/auto_test.rs`
- `src/tui/mod.rs`
- `src/mcp/mod.rs`
- `src/mcp/client.rs`
- `src/skills/mod.rs`
- `src/skills/loader.rs`
- `src/agent/parallel.rs`

上記は削除確定ではない。
ただし v0.1.0 コア再建中は、バイナリの起動経路と release acceptance から外す。

## 4. v0.1.0 で作り直す実装単位

最優先で作り直す対象:

- `src/agent/loop_run.rs`
- `src/agent/recovery.rs`
- `src/system_prompt.rs`
- `src/lib.rs`

理由:

- 現在の loop は recovery / guard / completion gate / root switch の責務を持ちすぎている
- local LLM 向けの一本道 loop になっていない
- `workspace/v0.1.0/01_anvil_lessons_and_donts.md` の「状態機械を積み増さない」に反している

## 5. release candidate の意味的成功条件

v0.1.0 の live E2E では、次を成功条件とする。

- Next.js scaffold が成功する
- `page.tsx` が初期テンプレートのままではない
- `3011` で起動できる
- 生成物に依頼内容に沿うゲーム要素がある
- `Write` または `Edit` が実際に発生している

次は成功条件にしない。

- 必要ファイルが存在するだけ
- dev server が起動するだけ
- session が保存されるだけ
- scaffold まで成功しただけ

## 6. Phase B/C への引き継ぎ

Phase B では、コア外モジュールを `lib.rs` / `run_cli()` / 起動経路から外す。

Phase C では、`loop_run.rs` を small loop に置き換える。
その small loop の責務は次に限定する。

- prompt 構築
- Ollama 呼び出し
- tool call 実行
- tool result の in-band 返却
- session 保存
- empty/no-tool の最小 handling

これ以外の高度な recovery や phase guard は、v0.1.0 コア再建中はいったん持ち込まない。
