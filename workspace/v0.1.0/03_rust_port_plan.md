# vibe-local Rust移植計画

作成日: 2026-04-16
対象リポジトリ: `/Users/maenokota/share/work/github_kewton/Anvil-develop`
作業ディレクトリ: `./workspace/v0.1.0`

## 1. 方針

これは Anvil の延長ではなく、**vibe-local 移植版をゼロベースで作る計画**である。

優先順位:

1. ローカルLLMで実際に動くこと
2. 構造が小さいこと
3. 失敗時の原因が追いやすいこと
4. 拡張可能であること

優先しないもの:

- 既存 Anvil アーキテクチャの温存
- 既存 module 名の維持
- 既存 phase machine の再利用
- 既存 integration test 群の延命

## 2. 何を作るか

目標は「Rust 版 vibe-local」であり、v0.1.0 の完成像は次の通り。

- Ollama 直結
- ローカルLLM向け prompt 方針を同梱
- 主要 built-in tools を持つ
- Plan/Act mode を持つ
- Session persistence を持つ
- Sidecar model を optional で持つ
- AutoTest / FileWatcher / GitCheckpoint を段階的に持つ
- TUI は local 実行に向いた軽量実装

## 3. ゼロベース移行の前提

既存コードは「参考資料」として扱い、土台にはしない。

やること:

- 既存ソースは丸ごと削除して再構成する
- 既存テストも丸ごと削除して、移植版の責務に合わせて再設計する
- 既存ドキュメントは参照用に残すが、プロダクト説明は書き換える

やらないこと:

- `src/app/mod.rs` を中心に段階移植する
- 既存 `termination` / `phase_estimator` / `bootstrap lane` 系を生かす
- 既存 provider abstraction を中心に据える

## 4. 削除・作り直し対象

### 4.1 まず削除するディレクトリ

以下は**削除してから作り直す**。

- `./src/`
- `./tests/`

理由:

- 現在の `src/` は Anvil の汎用エージェント構造に最適化されており、local-first な最小構成に向いていない
- 現在の `tests/` は Anvil の既存概念に強く結びついており、移植版の回帰防止資産として使いにくい

### 4.2 削除する個別ファイル

以下は削除対象として扱う。

- `./scripts/local_bootstrap_regression.sh`

理由:

- 現 Anvil の bootstrap regression 専用であり、新アーキテクチャでは前提が変わる

### 4.3 内容を書き換えるファイル

以下は削除ではなく**全面書き換え**。

- `./README.md`
- `./CHANGELOG.md`
- `./ANVIL.md`
- `./CLAUDE.md`

理由:

- プロダクトの前提が「Anvil 改修」から「vibe-local Rust移植」へ変わるため

### 4.4 残すもの

以下は残す。

- `./Cargo.toml`
- `./Cargo.lock`
- `./LICENSE`
- `./.github/`
- `./workspace/`
- `./sandbox/`

ただし `Cargo.toml` は依存関係を大幅に整理する前提で更新する。

## 5. 新しいディレクトリ構成案

v0.1.0 では以下を新設する。

```text
src/
  main.rs
  cli.rs
  config.rs
  logging.rs
  system_prompt.rs
  model_registry.rs
  ollama/
    mod.rs
    client.rs
    streaming.rs
    xml_fallback.rs
  session/
    mod.rs
    store.rs
    compact.rs
  tools/
    mod.rs
    registry.rs
    bash.rs
    read.rs
    write.rs
    edit.rs
    glob.rs
    grep.rs
    web_fetch.rs
    web_search.rs
  agent/
    mod.rs
    loop.rs
    prompting.rs
    permissions.rs
    parallel.rs
  modes/
    mod.rs
    plan_act.rs
  safety/
    mod.rs
    host_validation.rs
    path_guard.rs
  git/
    mod.rs
    checkpoint.rs
  watch/
    mod.rs
    file_watcher.rs
  testloop/
    mod.rs
    auto_test.rs
  mcp/
    mod.rs
    client.rs
  skills/
    mod.rs
    loader.rs
  tui/
    mod.rs
    screen.rs
    scroll_region.rs
    input.rs
tests/
  config_tests.rs
  ollama_client_tests.rs
  xml_fallback_tests.rs
  tool_registry_tests.rs
  session_tests.rs
  plan_act_tests.rs
  git_checkpoint_tests.rs
  file_watcher_tests.rs
  tui_tests.rs
  e2e_local_llm.rs
```

## 6. 実装フェーズ

### Phase 0: リポジトリ初期化

やること:

- `./src` を削除
- `./tests` を削除
- 新しい `src/` と `tests/` のスケルトン作成
- `Cargo.toml` の依存を v0.1.0 用に整理

完了条件:

- `cargo build` が通る最小骨格になる

### Phase 1: 最小 Ollama CLI

実装対象:

- CLI 引数
- config 読み込み
- localhost 限定ホスト検証
- Ollama 接続確認
- model 指定
- one-shot prompt

まだ入れないもの:

- TUI
- MCP
- skills
- parallel agents
- file watcher
- auto test

完了条件:

- `vibe-local-rs -p "hello"` 相当で Ollama に接続できる

### Phase 2: コア agent loop

実装対象:

- chat loop
- tool schema
- tool execution
- tool result 追加
- loop 継続/終了
- session 保存

MVP tool:

- Bash
- Read
- Write
- Edit
- Glob
- Grep

完了条件:

- ローカルLLMが上記ツールだけで簡単な変更を完遂できる

### Phase 3: local-LLM特化プロンプト

実装対象:

- local-first system prompt
- tool-first ルール
- no-question ending
- local model 向け失敗回避ルール
- Qwen `<think>` / XML 互換処理

完了条件:

- JSON tool call が崩れても XML fallback で回収できる

### Phase 4: Session compaction と sidecar

実装対象:

- session persistence
- token/size 管理
- sidecar summarization
- compact コマンド

方針:

- sidecar は optional
- まず main model only でも動作可能にする

完了条件:

- 長い対話でも履歴肥大で止まりにくい

### Phase 5: Plan/Act

実装対象:

- read-only Plan mode
- `/approve` で Act mode
- plan ファイル保存
- plan 注入

方針:

- Anvil の複雑な phase machine は持ち込まない
- vibe-local 同様、意味が一目で分かる二段階に留める

完了条件:

- read-only 探索から実行への切替が明確に機能する

### Phase 6: Git checkpoint / rollback / auto-test / watcher

実装対象:

- git checkpoint
- rollback
- file watcher
- auto test loop

完了条件:

- 編集後の local feedback loop が回る

### Phase 7: TUI

実装対象:

- 対話 UI
- ステータス行
- fixed footer
- ESC interrupt
- type-ahead

方針:

- Phase 1-6 のコアが動いてから載せる
- まず headless / one-shot を安定させる

完了条件:

- local LLM の待ち時間に耐える UI になる

### Phase 8: MCP / skills / parallel agents

実装対象:

- MCP client
- skill loader
- parallel agents

方針:

- これは v0.1.0 コアの後段
- 先に載せない

完了条件:

- コアの安定を壊さず拡張可能になる

## 7. Anvil から持ち込まないもの

以下は初期移植対象から外す。

- `termination_fsm`
- `phase_estimator`
- local bootstrap lane
- required-target tracking
- target-specific bootstrap lock
- task-semantics gate
- read-transition guard
- detector profile 群
- worker/fixslice の複雑な委譲制御
- provider abstraction の多層化

理由:

- いずれも Anvil 側で複雑さの源になった
- vibe-local 的な local-first な骨格には不要
- まず成功させる経路を細くするべき

## 8. Rust で意識する実装原則

- モジュールを細かく分けても、責務は単純に保つ
- trait 抽象化は必要になるまで入れない
- provider は当面 Ollama 専用でよい
- tool protocol は最小に絞る
- local LLM 向け fallback を前提機能として入れる
- 失敗時は早く止め、再実行しやすくする
- 「直すための状態機械」より「失敗しにくい一本道」を選ぶ

## 9. テスト計画

### 9.1 Unit test

対象:

- config precedence
- localhost host validation
- model auto-detect / sidecar pick
- tool schema / validation
- XML tool call fallback
- session serialization / compaction
- Plan/Act state transition
- path guard / permission rules

### 9.2 Integration test

対象:

- mocked Ollama server との chat loop
- streamed response の扱い
- tool execution loop
- Bash/Read/Write/Edit/Glob/Grep の統合
- git checkpoint / rollback
- file watcher / auto test

### 9.3 TUI test

対象:

- fixed footer
- resize
- ESC interrupt
- type-ahead
- no-scroll fallback

### 9.4 E2E acceptance test

v0.1.0 以降、必ず以下を回す。

- one-shot で簡単なファイル生成
- interactive で Plan→Act→修正
- 5-run 実アプリ生成テスト
- 3011 ポート起動確認
- 「起動した」だけでなく、依頼内容に沿った UI/ロジックがあるかの意味検査

### 9.5 回帰テストの原則

- unit より E2E を重視する
- 「ファイルがある」ではなく「意味的に正しい」を見る
- scaffold 成功は成功扱いしない
- timeout / no-tool loop / empty follow-up を明示的に失敗とする

## 10. 最終結論

次に作るべきものは、Anvil の簡略版ではない。

作るべきものは、

- vibe-local の local-first な思想
- Rust の型安全性と保守性
- 小さく始めて E2E で磨く開発プロセス

この3つを組み合わせた新しい実装である。

最初の一歩は明確。

- `./src` を削除する
- `./tests` を削除する
- 最小 Ollama CLI から作り直す

そこから、vibe-local 移植版を動かしながら改善していく。
