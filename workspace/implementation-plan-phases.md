# Anvil 実装計画

## 方針

Anvil の実装は Phase ごとに進める。  
各 Phase では必ず TDD を採用し、以下の順で進める。

1. 受け入れ条件をテストへ落とす
2. 最小実装でテストを通す
3. リファクタリングする
4. 監査ログ・回帰テストを追加する

共通ルール:

- 仕様追加より先にテストを追加する
- provider / policy / parser は単体テストを厚くする
- agent loop と CLI は統合テストで押さえる
- 危険操作、権限、壊れた tool call は必ず回帰テストを持つ

## Phase 0: 土台整備

目的:

- プロジェクト骨格
- テスト基盤
- CI 最低限
- TDD を回せる状態の確立

先に書くテスト:

- `cargo test` が通る最小 smoke test
- CLI 起動の smoke test
- config 読み込みの基本テスト
- `PermissionMode` / `PermissionPolicy` の表駆動テスト
- `AuditEvent` serialize / deserialize テスト

実装:

- Cargo workspace / module 骨格
- `src/main.rs`
- `src/cli/`
- `src/policy/permissions.rs`
- `src/state/audit.rs`
- `src/config/`
- tracing 初期化

完了条件:

- テスト基盤が動く
- `PermissionPolicy` の表駆動テストがすべて通る
- 監査イベント型が JSONL 化できる

## Phase 1: Ollama MVP

目的:

- 単一エージェント
- Ollama 接続
- 基本ツール
- ストリーミング
- 権限確認

先に書くテスト:

- Ollama provider の health / list_models / chat / chat_stream のモックテスト
- NDJSON ストリーム正規化テスト
- 壊れた tool call を fail-closed で拒否するテスト
- `Read` / `Write` / `Edit` / `Exec` / `Glob` / `Search` / `Diff` の単体テスト
- `--permission-mode ask|accept-edits|bypass-permissions` の統合テスト
- `anvil -p` の one-shot 統合テスト
- append-only audit log 出力テスト

実装:

- `src/models/ollama.rs`
- `src/models/stream.rs`
- `src/agent/loop.rs`
- `src/tools/*`
- `src/ui/plain.rs`
- `src/cli/args.rs`
- `src/state/session.rs`
- `src/state/audit_log.rs`

TDD の観点:

- まず provider を fake server で固定する
- 次に parser を赤→緑にする
- その後 agent loop を最小実装する
- 最後に CLI へつなぐ

完了条件:

- `anvil` で対話起動できる
- `anvil -p "..."` が動く
- 基本ツールが使える
- 権限確認と audit log が破綻しない
- Ollama だけで MVP が完結する

## Phase 2: 実用化

目的:

- LM Studio 対応
- Plan / Act
- `ANVIL.md`
- `ANVIL-MEMORY.md`
- カスタム slash command
- 単一サブエージェント

先に書くテスト:

- LM Studio SSE ストリーム正規化テスト
- OpenAI 互換レスポンス差分テスト
- `ANVIL.md` nearest only ローダーテスト
- `ANVIL-MEMORY.md` load / normalize / update テスト
- `/memory add`, `/memory show`, `/memory edit` の CLI テスト
- schema 付き custom command load / validate / invoke テスト
- Plan / Act 遷移テスト
- subagent report 圧縮テスト
- subagent 承認イベントの audit log テスト

実装:

- `src/models/lm_studio.rs`
- `src/instructions/anvil_md.rs`
- `src/state/memory.rs`
- `src/slash/registry.rs`
- `src/slash/builtins.rs`
- `src/slash/custom.rs`
- `src/agent/plan.rs`
- `src/agent/subagent.rs`

TDD の観点:

- provider 差分は adapter テストから始める
- slash command は schema validation テストから始める
- subagent は report schema のテストから始める

完了条件:

- LM Studio でも基本操作が動く
- `ANVIL.md` と `ANVIL-MEMORY.md` が反映される
- custom slash command を schema 付きで追加できる
- subagent が report 経由で文脈圧縮に使える

## Phase 3: パフォーマンスと UX 強化

目的:

- コンテキスト圧縮の高度化
- Claude Code ライク UI の強化
- footer UI
- type-ahead
- rich diff
- モデル選択補助

先に書くテスト:

- summary 発火条件の表駆動テスト
- token budget 超過時の圧縮テスト
- 大きい tool output truncate テスト
- renderer snapshot test
- UI event sequence test
- file change detection method の回帰テスト

実装:

- `src/state/summary.rs`
- `src/ui/interactive.rs`
- `src/ui/render.rs`
- `src/config/model_profiles.rs`
- `src/policy/change_detection.rs`

TDD の観点:

- UI は snapshot / event ベースで固定する
- パフォーマンス機構は閾値テストを先に書く

完了条件:

- 長時間セッションでも劣化が抑えられる
- UI が Claude Code に近い操作感を持つ
- token budget 制御が機能する

## Phase 4: 拡張フェーズ

目的:

- 並列サブエージェント
- Notebook / Web / RAG
- 追加 provider / tool
- 高度な automation

先に書くテスト:

- 並列 subagent の isolation test
- 複数 subagent の audit ordering test
- Notebook / Web / RAG の capability test
- provider 追加時の conformance test

実装:

- `src/agent/parallel_subagent.rs`
- `src/tools/notebook.rs`
- `src/tools/web.rs`
- `src/tools/rag.rs`
- provider conformance test harness

完了条件:

- 拡張機能が core を壊さずに追加できる
- 監査と権限モデルが維持される

## TDD の運用ルール

### 1. 単体テストを先に書く対象

- permission policy
- parser
- provider adapter
- custom command schema
- memory normalization
- audit event serialization

### 2. 統合テストを先に書く対象

- CLI
- agent loop
- one-shot 実行
- permission flow
- session persistence
- slash command invocation

### 3. 回帰テストを必須にする不具合種別

- 壊れた tool call の誤実行
- 非対話での危険操作許可
- subagent の権限漏れ
- audit log 欠損
- memory 更新時の秘密情報混入
- custom command の schema bypass

### 4. PR 単位の進め方

1. failing test を追加
2. 最小実装
3. refactor
4. audit / regression test 追加
5. doc 更新

## 推奨マイルストーン

### Milestone A

- Phase 0 完了
- 権限と監査の中核型が固まる

### Milestone B

- Phase 1 完了
- Ollama MVP が使える

### Milestone C

- Phase 2 完了
- 実用機能が揃う

### Milestone D

- Phase 3 完了
- UX と性能が実用域に入る

### Milestone E

- Phase 4 完了
- 拡張機能を安全に載せられる
