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

- [x] Ollama provider の health / list_models / chat / chat_stream のモックテスト
- [x] NDJSON ストリーム正規化テスト
- [x] 壊れた tool call を fail-closed で拒否するテスト
- [x] `Read` / `Write` / `Edit` / `Exec` / `Glob` / `Search` / `Diff` の単体テスト
- [x] `--permission-mode ask|accept-edits|bypass-permissions` の統合テスト
- [x] `anvil -p` の one-shot 統合テスト
- [x] append-only audit log 出力テスト
- [x] audit log redaction テスト
- [x] `ANVIL.md` nearest-only 読み込みテスト
- [x] `ANVIL-MEMORY.md` load と `/memory add` の統合テスト
- [x] permission flow の回帰テスト
- [x] session persistence の回帰テスト
- [x] `qwen3.5:35b` を使った Ollama 実機疎通テスト

実装:

- [x] `src/models/ollama.rs`
- [x] `src/models/stream.rs`
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

- [x] LM Studio 対応
- [x] Plan / Act
- [x] `/memory show`
- [x] `/memory edit`
- [x] カスタム slash command
- [x] 単一サブエージェント

先に書くテスト:

- [x] LM Studio SSE ストリーム正規化テスト
- [x] OpenAI 互換レスポンス差分テスト
- [x] `/memory show`, `/memory edit` の CLI テスト
- [x] `ANVIL-MEMORY.md` normalize / update テスト
- [x] schema 付き custom command load / validate / invoke テスト
- [x] Plan / Act 遷移テスト
- [x] plan file load / inject テスト
- [x] subagent report 圧縮テスト
- [x] subagent 承認イベントの audit log テスト
- [x] subagent permission leak 回帰テスト
- [x] custom command schema bypass 回帰テスト

実装:

- [x] `src/models/lm_studio.rs`
- [x] `src/slash/registry.rs`
- [x] `src/slash/builtins.rs`
- [x] `src/slash/custom.rs`
- [x] `src/agent/plan.rs`
- [x] `src/agent/subagent.rs`
- [x] memory edit/show の更新

TDD の観点:

- provider 差分は adapter テストから始める
- slash command は schema validation テストから始める
- subagent は report schema のテストから始める

完了条件:

- [x] LM Studio でも基本操作が動く
- [x] custom slash command を schema 付きで追加できる
- [x] Plan / Act が安定して動く
- [x] subagent が report 経由で文脈圧縮に使える

## Phase 3: パフォーマンスと UX 強化

目的:

- [x] コンテキスト圧縮の高度化
- [x] Claude Code ライク UI の強化
- [x] footer UI
- [x] type-ahead
- [x] rich diff
- [x] モデル選択補助

先に書くテスト:

- [x] summary 発火条件の表駆動テスト
- [x] token budget 超過時の圧縮テスト
- [x] 大きい tool output truncate テスト
- [x] renderer snapshot test
- [x] UI event sequence test
- [x] TTY 非依存 renderer contract test
- [x] file change detection method の回帰テスト
- [x] audit log volume / rotation の性能テスト
- [x] summary latency budget テスト
- [x] subagent 起動時の latency budget テスト

実装:

- [x] `src/state/summary.rs`
- [x] `src/ui/interactive.rs`
- [x] `src/ui/render.rs`
- [x] `src/config/model_profiles.rs`
- [x] `src/policy/change_detection.rs`
- [x] `src/state/artifacts.rs`

TDD の観点:

- UI は snapshot / event ベースで固定する
- パフォーマンス機構は閾値テストを先に書く

完了条件:

- [x] 長時間セッションでも劣化が抑えられる
- [x] UI が Claude Code に近い操作感を持つ
- [x] token budget 制御が機能する
- [x] change detection が大規模リポジトリでも過負荷にならない
- [x] summary / subagent / audit log の性能予算が守られる

## Phase 3.1: Agent Loop 強化

目的:

- [x] Claude Code / vibe-local 的な核心である、モデル主導のツール実行ループを完成させる
- [x] ルール追加ではなく、tool-calling 中心の agent loop を強化する
- [x] 自然言語の依頼から必要なコンテキスト収集をモデル主導で完走できるようにする

先に書くテスト:

- [x] 自然言語タスクから `Read` / `Exec` / `Diff` を段階実行する統合テスト
- [x] `git branch` / `git status` / `git log` / `git diff` を用いたブランチ説明タスクの統合テスト
- [x] モデルが追加情報不足時に適切なツール呼び出しへ進む回帰テスト
- [x] 同一ツール・同一引数の無限反復を停止するループ防止テスト
- [x] tool result を踏まえた再推論で最終回答へ収束するテスト
- [x] tool call schema validation と fail-closed の統合テスト

実装:

- [x] `src/agent/mod.rs` を単発プロンプト実行から multi-step tool loop へ拡張
- [x] モデル出力から tool call を抽出する parser / validator を追加
- [x] `Read` / `Write` / `Edit` / `Exec` / `Diff` / `Search` / `Glob` を agent loop に接続
- [x] tool 実行結果を履歴へ圧縮注入する state 更新を追加
- [x] 反復上限、重複呼び出し検出、失敗時の別手段誘導を追加
- [x] ブランチ説明のような Git 文脈タスクを一般 tool loop で解けることを確認

TDD の観点:

- まず tool call parser と validation を固定する
- 次に 2-step / 3-step の最小 loop を統合テストで通す
- その後 Git を含む実タスクへ広げる
- 個別ルールではなく、モデルがツールを選んで完走できる形を優先する

完了条件:

- [x] モデルが必要に応じてツールを自律選択して回答に必要な文脈を集められる
- [x] `このブランチを解説して` のような依頼をルール追加なしで処理できる
- [x] tool loop が fail-closed と権限モデルを維持したまま安定動作する
- [x] Claude Code / vibe-local に近い「調べてから答える」挙動を再現できる

## Phase 3.2: ローカルLLM安定化

目的:

- [x] ローカルLLMでの tool calling 不安定性の主因を除去する
- [x] `vibe-local` と比べて不安定になっている構造要因を解消する
- [ ] 生成系タスクでの完走率を実用域まで引き上げる
- [ ] Phase 3.1 の loop を「動く」から「ローカルモデルで安定して使える」へ引き上げる

先に書くテスト:

- [x] Ollama native tool calling adapter の unit test
- [x] native tool calling と text-JSON fallback の切り替え統合テスト
- [ ] `write_file` 大規模 payload を native tool call で壊さず処理する統合テスト
- [x] `list_dir` / `stat_path` / `path_exists` / `mkdir` の単体テスト
- [x] directory を `read_file` せず `list_dir` または `stat_path` へ進む統合テスト
- [ ] create task で空 `glob` を反復せず `mkdir -> write_file` へ進む統合テスト
- [x] empty result cache が探索停止を悪化させない回帰テスト
- [x] `final` 判定が create task / inspect task で適切に分かれる統合テスト
- [x] `loop_turns` compaction が長大 tool result でも劣化しない性能テスト
- [x] tool-use 時の provider options (`temperature`, `num_ctx`, `keep_alive`) 回帰テスト
- [ ] `qwen3.5:35b` での実機受け入れテスト
  - [x] ブランチ説明
  - [ ] 単一 HTML 生成
  - [ ] `./sandbox/<name>/` への静的ゲーム出力

実装:

- [x] `src/models/ollama.rs` に native tool calling 対応を追加
- [x] `src/models/lm_studio.rs` の provider 差分を native tool calling 前提で再整理
- [x] `src/agent/looping.rs` に provider-native tool call path と fallback path を分離実装
- [x] `src/tools/` に `list_dir`, `stat_path`, `path_exists`, `mkdir` を追加
- [x] create / write 系タスク向けの loop policy を追加
- [x] empty `glob` / `search` / empty cached result の扱いを見直す
- [x] `loop_turns` 用 compaction / summarization を追加
- [x] tool-use 時の provider tuning を `model profile` と統合する
- [ ] `write_file` 大規模 content の分割・圧縮方針を整理する

TDD の観点:

- まず provider-native tool calling を adapter test で固定する
- 次に file/dir 判定用ツールを unit test で固める
- その後 create task の統合テストを赤で追加する
- 最後に `qwen3.5:35b` の実機受け入れで完走率を確認する

完了条件:

- [x] Ollama で native tool calling を優先利用できる
- [x] `write_file` の巨大 JSON 本文生成に依存しない
- [x] directory/file の取り違えで loop が破綻しない
- [ ] create task が空 `glob` の反復に陥りにくい
- [ ] `このブランチを解説して` と `ゲームを作って出力して` の両方が安定して完走する
- [ ] `qwen3.5:35b` 実機確認で Phase 3.1 より明確に完走率が改善する

## Phase 3.3: 生成タスク収束性の改善

目的:

- [x] create / output 系タスクでの path 誤読を抑止する
- [x] `mkdir` 後に `stat_path` / `list_dir` を反復する停滞ループを解消する
- [x] 「prepare -> write -> verify -> final」の段階遷移を loop policy として明確化する
- [x] ユーザーから見て、同じ失敗を繰り返していることが分かる可観測性を追加する

先に書くテスト:

- [x] ユーザーが指定した出力先 path と tool call path がズレた場合に `path_mismatch` で差し戻す統合テスト
- [x] `./sandbox/test31_011` のような複雑な path を `./sandbox11` に崩さない回帰テスト
- [x] create task で `mkdir` 成功後に `write_file` へ進む統合テスト
- [x] create task で空 directory に対する `stat_path` / `list_dir` の反復を block する統合テスト
- [x] `prepare -> write -> verify -> final` の段階遷移テスト
- [x] internal `ToolError` が UI event として表示される contract test
- [x] `duplicate_empty_result` / `stalled_pre_write_inspection` / `path_mismatch` の observer event テスト
- [ ] `qwen3.5:35b` 実機回帰テスト
  - [x] 単一 HTML 生成
  - [x] `./sandbox/<name>/` への静的ゲーム出力
  - [ ] 誤 path からの self-correction

実装:

- [x] ユーザー入力から期待出力 path を抽出し loop state に保持する
- [x] tool call path が期待 path と意味的にズレる場合の `path_mismatch` validator を追加する
- [x] create task 用の phase state (`prepare`, `write`, `verify`, `final`) を追加する
- [x] `prepare` 完了後の無意味な `stat_path` / `list_dir` / `path_exists` を block する
- [x] `write` 未実行時は `write_file` を優先する completion hint を強化する
- [x] `ToolError` を UI に表示する observer event を追加する
- [x] one-shot と interactive の両方で同じ create-task policy を使うよう整理する

TDD の観点:

- まず path mismatch の validator を赤で固定する
- 次に create-task phase state を unit test / integration test で固める
- その後 UI event contract を追加する
- 最後に `qwen3.5:35b` 実機でゲーム生成の完走を確認する

完了条件:

- [x] 指定出力 path の誤読が loop 内で自己修正される
- [x] create task が `mkdir` 後の空 directory inspection で停滞しない
- [x] internal retry / tool error がユーザーから追える
- [x] `ブラウザから直接実行可能ないけてるスペースインベーダーゲーム` の生成が `qwen3.5:35b` で完走する

## Phase 3.4: UX と可読性の改善

目的:

- [x] 起動直後に `ANVIL` だと一目で分かるワードマークと起動画面にする
- [x] provider / model / mode / cwd / permission などの状態を視覚的に読みやすくする
- [x] 入力方法、とくに複数行入力のやり方を迷わないようにする
- [x] agent loop の「開始」「途中経過」「回復」「完了」を見分けやすくする
- [x] 最終結果を操作ログから視覚的に分離し、メリハリをつける

先に書くテスト:

- [x] 起動バナーの snapshot test を `ANVIL` 可読ワードマーク前提で更新する
- [x] status header の renderer snapshot test
- [x] 起動ヘルプに複数行入力案内が含まれる contract test
- [x] loop event renderer が `agent / tool / recovery / result` を別スタイルで描く snapshot test
- [x] create task の phase 表示 (`prepare`, `write`, `verify`, `finalize`) contract test
- [x] final response block の snapshot test
- [x] one-shot と interactive で共通の result block を使う renderer test

実装:

- [x] `render_banner()` を読める `ANVIL` ワードマークへ置き換える
- [x] 起動時の status header に記号 / 絵文字 / ラベルを追加する
- [x] 複数行入力方法を起動時ヘルプへ追加する
- [x] `print_loop_event()` を単純ログ列から段階別表示へ変更する
- [x] step ごとに「何をしようとしているか」を表示する
- [x] create / inspect それぞれで phase / intent を UI に出す
- [x] final response を `Result` ブロックとして分離する
- [x] one-shot でも interactive と同じ progress / result 表示方針を使う

TDD の観点:

- まず renderer snapshot を更新して desired UI を固定する
- 次に loop event と result block の contract を通す
- 最後に interactive / one-shot の統合表示を揃える

完了条件:

- [x] 起動画面だけで `ANVIL` / provider / model / mode が瞬時に分かる
- [x] 複数行入力方法をユーザーが迷わない
- [x] `agent loop step` と `tool` と `recovery` の違いが見分けられる
- [x] 最終結果がログの流れに埋もれず、明確に読める

## Phase 3.5: Context Compaction と Session 引き継ぎの実用化

目的:

- [ ] token 使用量が閾値へ近づいたときに、安全に文脈圧縮へ移行する
- [ ] rolling summary を UI 表示用ではなく次ターン prompt に実際に効かせる
- [ ] interactive session の長時間利用でも、重要な文脈が崩れずに引き継がれるようにする
- [ ] local model 向けに `20万` token 前提でも速度と安定性を両立する
- [ ] create task を固定フロー化せず、`Task Contract + Evidence + Phase Gate` で柔軟性と精度を両立する

先に書くテスト:

- [ ] transcript compaction が summary を prompt に注入する integration test
- [ ] summary 発火後も `ANVIL.md` / `ANVIL-MEMORY.md` / 現在 task の優先度が維持される test
- [ ] 長い対話履歴で branch explanation / create task が回帰しない test
- [ ] loop turns compaction が tool evidence を過剰に失わない test
- [ ] token budget 到達時に `summarize -> continue` が発火する contract test
- [ ] summary 後の follow-up 質問で文脈が引き継がれる end-to-end test
- [ ] `Task Contract` が `output_root / deliverable_type / must_review / browser_runnable` を抽出できる test
- [ ] 同じ create task でも複数の tool sequence を許容しつつ、未充足制約だけを差し戻す test
- [ ] evidence-based phase transition が `implement / verify / review / finalize` を過不足なく遷移する test

実装:

- [ ] transcript summary を独立 state として保持し、次ターン prompt の先頭へ注入する
- [ ] `transcript.len() * 1200` の概算依存を減らし、より妥当な budget 管理へ寄せる
- [ ] `loop_turns` compaction と interactive transcript compaction の責務を分離する
- [ ] summary 後に生 transcript を無限増殖させない rotation / pruning を入れる
- [ ] summary に含めるべき内容を `user goals / accepted facts / pending tasks / changed files` に正規化する
- [ ] summary 発火、compaction、carryover を UI と監査ログに出す
- [ ] `qwen3.5:35b` 実機で長めの multi-turn セッション回帰を行う
- [ ] create task 用の `Task Contract` 層を追加し、要求から守るべき制約だけを構造化する
- [ ] `completion hint` を固定手順ではなく `未充足制約` ベースで生成する
- [ ] phase 制御を「推奨フロー」ではなく evidence-based gate として実装する
- [ ] review / verification / output path を phase 名ではなく `required capability` としても扱えるようにする

TDD の観点:

- まず summary carryover の failing integration test を追加する
- 次に transcript summary state と prompt injection を最小実装する
- その後 pruning / rotation と UI event を追加する
- 最後に長時間セッションの実機回帰で精度低下がないことを確認する
- create task については、文言マッチによる固定分岐を増やさず、contract 抽出と evidence 判定から先に赤で固定する

完了条件:

- [ ] summary 発火後の follow-up 質問でも、直前までの task 文脈を保持できる
- [ ] 長い対話でも branch explanation と create task の両方が破綻しにくい
- [ ] transcript が無制限に膨らまず、summary / carryover の状態が追跡できる
- [ ] local model 前提の長時間対話で、速度低下と文脈欠落が実用範囲に収まる
- [ ] create task が固定シナリオに寄らず複数の実装経路を取れて、なお path / review / deliverable 要件を落としにくい

## Phase 3.6: Requirement Set と Progress-Based Loop Control

目的:

- [ ] create task を phase 名中心ではなく `Requirement Set` 中心で制御する
- [ ] 各 tool result が requirement をどれだけ前進させたかを `Evidence` として評価する
- [ ] stalled verification / stalled inspection を固定禁止ルールではなく progress 判定で止める
- [ ] task type と progress に応じた adaptive upper bound を本格化する

先に書くテスト:

- [ ] `output_root_exists / deliverable_written / browser_entry_verified / review_completed` を requirement として抽出する test
- [ ] `mkdir`, `write_file`, `read_file`, `path_exists` が requirement 充足へ変換される evidence test
- [ ] verify phase で `list_dir` を繰り返しても progress 0 と判定される test
- [ ] 進捗が出た create task は loop budget が延長される test
- [ ] stalled create task は loop budget が早めに打ち切られる test
- [ ] 複数の valid tool sequence を許容しつつ、同じ requirement set を満たせる integration test

実装:

- [ ] create task 用 `RequirementSet` 型を追加する
- [ ] tool result を `Evidence` へ正規化する evaluator を追加する
- [ ] `remaining requirements` と `progress score` を loop state に持たせる
- [ ] adaptive upper bound を `base budget + progress extension - stall penalty` で管理する
- [ ] `completion hint` を unmet requirement と low-progress から生成する
- [ ] UI に `remaining requirements` と `this step progress` を表示する
- [ ] verify / review / finalize の遷移を phase 固定ではなく requirement 充足ベースへ寄せる

TDD の観点:

- まず requirement extraction と evidence mapping を unit test で赤にする
- 次に progress score と adaptive budget を table-driven test で固定する
- その後 create task の integration test で複数経路を許容できることを確認する
- 最後に `qwen3.5:35b` 実機で stalled verification が減ることを確認する

完了条件:

- [ ] `list_dir` / `glob` の反復を個別禁止ルールなしで stalled 判定できる
- [ ] create task が複数の tool sequence を取りつつ、同じ requirement set に収束できる
- [ ] progress がある task は完走しやすく、progress のない task は無駄に長引かない
- [ ] verify / review / finalize の遷移が requirement/evidence ベースで説明可能になる

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
