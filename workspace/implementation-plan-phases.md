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

テストモデル方針:

- Ollama を使う実機確認では `qwen3.5:35b` を基準モデルとして使用する
- provider adapter の unit test は fake server を使うが、Milestone 受け入れテストでは `qwen3.5:35b` を実際に使う

## レビュー結果

この計画は大枠では十分だが、初版には以下の不足があったため本版で反映した。

- Phase 1 と仕様の不整合
  - `ANVIL.md` 読み込み
  - `ANVIL-MEMORY.md` 読み込み
  - `/memory add`
  が仕様上は Phase 1 だったが、初版では Phase 2 に寄っていた
- 監査ログの versioning と backward compatibility テストが不足していた
- parser / provider / policy の単体テストに比べ、session / redaction / audit recovery の観点が弱かった
- UI 強化 Phase に対して renderer のみで、TTY 非依存の event contract テストが不足していた
- 拡張フェーズに対して conformance test の前提整備が弱かった

以下の計画は上記を反映済みである。

## Phase 0: 土台整備

目的:

- [x] プロジェクト骨格
- [x] テスト基盤
- [x] CI 最低限
- [x] TDD を回せる状態の確立

先に書くテスト:

- [x] `cargo test` が通る最小 smoke test
- [x] CLI 起動の smoke test
- [x] config 読み込みの基本テスト
- [x] `PermissionMode` / `PermissionPolicy` の表駆動テスト
- [x] `AuditEvent` serialize / deserialize テスト
- [x] `AuditEvent` schema version 後方互換テスト
- [ ] fake HTTP / fake FS / fake clock の基盤テスト
- [x] `cargo fmt --check` / `clippy` を通す最小 quality gate テスト

実装:

- [x] Cargo workspace / module 骨格
- [x] `src/main.rs`
- [x] `src/cli.rs`
- [x] `src/policy/permissions.rs`
- [x] `src/state/audit.rs`
- [x] `src/config.rs`
- [ ] tracing 初期化
- [ ] test support module (`fake_server`, `fake_fs`, `fixtures`)

完了条件:

- [x] テスト基盤が動く
- [x] `PermissionPolicy` の表駆動テストがすべて通る
- [x] 監査イベント型が JSONL 化できる
- [x] CI 上で単体テストと最小統合テストが実行できる
- [x] `fmt` / `clippy` を常時回せる

## Phase 1: Ollama MVP

目的:

- [x] 単一エージェント
- [x] Ollama 接続
- [x] 基本ツール
- [x] ストリーミング
- [x] 権限確認
- [x] `ANVIL.md`
- [x] `ANVIL-MEMORY.md`
- [x] `/memory add`

先に書くテスト:

- [ ] Ollama provider の health / list_models / chat / chat_stream のモックテスト
- [ ] NDJSON ストリーム正規化テスト
- [x] 壊れた tool call を fail-closed で拒否するテスト
- [ ] `Read` / `Write` / `Edit` / `Exec` / `Glob` / `Search` / `Diff` の単体テスト
- [x] `--permission-mode ask|accept-edits|bypass-permissions` の統合テスト
- [x] `anvil -p` の one-shot 統合テスト
- [x] append-only audit log 出力テスト
- [ ] audit log redaction テスト
- [x] `ANVIL.md` nearest-only 読み込みテスト
- [x] `ANVIL-MEMORY.md` load と `/memory add` の統合テスト
- [x] permission flow の回帰テスト
- [x] session persistence の回帰テスト
- [x] `qwen3.5:35b` を使った Ollama 実機疎通テスト

実装:

- [x] `src/models/ollama.rs`
- [ ] `src/models/stream.rs`
- [x] `src/agent/mod.rs` の single-loop MVP
- [x] `src/tools/*`
- [x] `src/main.rs` / `src/agent/mod.rs` の plain interactive UI
- [x] `src/cli.rs`
- [x] `src/state/session.rs`
- [x] `src/state/audit.rs`
- [x] `src/instructions/mod.rs`
- [x] `src/state/memory.rs`
- [x] `src/agent/mod.rs` の `/memory add`
- [x] Ollama 実機確認用 fixture / smoke prompt

TDD の観点:

- まず provider を fake server で固定する
- 次に parser を赤→緑にする
- その後 agent loop を最小実装する
- 最後に CLI へつなぐ

完了条件:

- [x] `anvil` で対話起動できる
- [x] `anvil -p "..."` が動く
- [x] 基本ツールが使える
- [x] 権限確認と audit log が破綻しない
- [x] `ANVIL.md` と `ANVIL-MEMORY.md` の基本機能が動く
- [x] Ollama だけで MVP が完結する
- [x] 壊れた tool call を誤実行せず fail-closed で停止できる

## Phase 2: 実用化

目的:

- [ ] LM Studio 対応
- [ ] Plan / Act
- [ ] `/memory show`
- [ ] `/memory edit`
- [ ] カスタム slash command
- [ ] 単一サブエージェント

先に書くテスト:

- [ ] LM Studio SSE ストリーム正規化テスト
- [ ] OpenAI 互換レスポンス差分テスト
- [ ] `/memory show`, `/memory edit` の CLI テスト
- [ ] `ANVIL-MEMORY.md` normalize / update テスト
- [ ] schema 付き custom command load / validate / invoke テスト
- [ ] Plan / Act 遷移テスト
- [ ] plan file load / inject テスト
- [ ] subagent report 圧縮テスト
- [ ] subagent 承認イベントの audit log テスト
- [ ] subagent permission leak 回帰テスト
- [ ] custom command schema bypass 回帰テスト

実装:

- [ ] `src/models/lm_studio.rs`
- [ ] `src/slash/registry.rs`
- [ ] `src/slash/builtins.rs`
- [ ] `src/slash/custom.rs`
- [ ] `src/agent/plan.rs`
- [ ] `src/agent/subagent.rs`
- [ ] memory edit/show の更新

TDD の観点:

- provider 差分は adapter テストから始める
- slash command は schema validation テストから始める
- subagent は report schema のテストから始める

完了条件:

- [ ] LM Studio でも基本操作が動く
- [ ] custom slash command を schema 付きで追加できる
- [ ] Plan / Act が安定して動く
- [ ] subagent が report 経由で文脈圧縮に使える

## Phase 3: パフォーマンスと UX 強化

目的:

- [ ] コンテキスト圧縮の高度化
- [ ] Claude Code ライク UI の強化
- [ ] footer UI
- [ ] type-ahead
- [ ] rich diff
- [ ] モデル選択補助

先に書くテスト:

- [ ] summary 発火条件の表駆動テスト
- [ ] token budget 超過時の圧縮テスト
- [ ] 大きい tool output truncate テスト
- [ ] renderer snapshot test
- [ ] UI event sequence test
- [ ] TTY 非依存 renderer contract test
- [ ] file change detection method の回帰テスト
- [ ] audit log volume / rotation の性能テスト
- [ ] summary latency budget テスト
- [ ] subagent 起動時の latency budget テスト

実装:

- [ ] `src/state/summary.rs`
- [ ] `src/ui/interactive.rs`
- [ ] `src/ui/render.rs`
- [ ] `src/config/model_profiles.rs`
- [ ] `src/policy/change_detection.rs`
- [ ] `src/state/artifacts.rs`

TDD の観点:

- UI は snapshot / event ベースで固定する
- パフォーマンス機構は閾値テストを先に書く

完了条件:

- [ ] 長時間セッションでも劣化が抑えられる
- [ ] UI が Claude Code に近い操作感を持つ
- [ ] token budget 制御が機能する
- [ ] change detection が大規模リポジトリでも過負荷にならない
- [ ] summary / subagent / audit log の性能予算が守られる

## Phase 4: 拡張フェーズ

目的:

- [ ] 並列サブエージェント
- [ ] Notebook / Web / RAG
- [ ] 追加 provider / tool
- [ ] 高度な automation

先に書くテスト:

- [ ] 並列 subagent の isolation test
- [ ] 複数 subagent の audit ordering test
- [ ] Notebook / Web / RAG の capability test
- [ ] provider 追加時の conformance test
- [ ] registry 拡張時の backward compatibility test

実装:

- [ ] `src/agent/parallel_subagent.rs`
- [ ] `src/tools/notebook.rs`
- [ ] `src/tools/web.rs`
- [ ] `src/tools/rag.rs`
- [ ] provider conformance test harness
- [ ] extensibility fixtures / golden cases

完了条件:

- [ ] 拡張機能が core を壊さずに追加できる
- [ ] 監査と権限モデルが維持される
- [ ] 新 provider / tool 追加時に conformance test を通せる

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

1. [ ] failing test を追加
2. [ ] 最小実装
3. [ ] refactor
4. [ ] audit / regression test 追加
5. [ ] doc 更新

## 推奨マイルストーン

### Milestone A

- [x] Phase 0 完了
- [x] 権限と監査の中核型が固まる

### Milestone B

- [x] Phase 1 完了
- [x] Ollama MVP が使える
- [x] `qwen3.5:35b` での Ollama 受け入れ確認が完了している
- [x] `./sandbox/<timestamp>/` を作成し、「ブラウザから直接実行可能なカッコ良いスペースインベーダーゲーム」を生成できる
- [x] 生成物をブラウザで実行して動作確認できる

Milestone B 受け入れテスト:

1. `./sandbox` 配下にタイムスタンプ付きディレクトリを作成する
2. `qwen3.5:35b` を使って one-shot または対話モードでゲーム生成タスクを実行する
3. 出力先はブラウザから直接実行可能な静的ファイル構成にする
4. 見た目が明確に作り込まれたスペースインベーダーゲームであることを確認する
5. ブラウザで起動し、少なくとも以下を確認する
   - ゲーム画面が表示される
   - 自機移動ができる
   - 敵が表示される
   - 弾発射ができる
   - 当たり判定またはスコア更新が動く
6. 生成時の監査ログと実行ログを保存する

### Milestone C

- [ ] Phase 2 完了
- [ ] 実用機能が揃う

### Milestone D

- [ ] Phase 3 完了
- [ ] UX と性能が実用域に入る

### Milestone E

- [ ] Phase 4 完了
- [ ] 拡張機能を安全に載せられる
