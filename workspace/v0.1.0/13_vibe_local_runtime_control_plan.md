# vibe-local思想を取り込むための実装計画

## 目的

この文書は、`vibe-local` から取り込むべき考え方を Anvil の `Plan` / `Act` 実行系へ落とし込むための計画です。

対象にする考え方は次の5点です。

- tool-first、短い応答、余計な質問をしない
- ユーザーにやらせず、自分でコマンド実行する
- 失敗したら別案を試すが、長く考えさせない
- Planは必要最小限にし、実装に早く入る
- local LLM前提では、promptよりruntimeの単純な制御を優先する

主眼はプロンプト改善ではなく、runtime側の分岐、上限、fallback、成功判定を単純化することです。

---

## 現状整理

### Anvilの現在の強み

- Rust実装で `config` / `session` / `loop_run` / `recovery` / `quality` が分離されている。
- `Plan` / `Act` 状態、session保存、compaction、footer、interrupt、checkpointを持つ。
- E2Eで発生した低品質出力に対し、deterministic fallbackとquality gateを導入済み。
- local LLMが詰まるケースに対して、timeout、missing section、focused edit、placeholder検出などの復旧経路がある。

### 現在の弱点

- `Plan` が重く、local LLMに複数sectionの完全な計画作成を長く背負わせている。
- `Act` のrecoveryが増え、状態持ち越しや制約矛盾が起きやすくなっている。
- tool-firstを促す文言はあるが、no-tool応答後のruntime制御がまだ回り道になる。
- 失敗時に再プロンプトで長考させる経路が残っている。
- 「ユーザーにやらせない」方針はあるが、実装後の検証コマンド実行までruntimeが一貫して保証しているわけではない。

---

## 基本方針

### 方針1: Planを短くする

Plan modeの目的を「完璧な計画作成」から「Actが安全に開始できる最小実行契約」へ寄せる。

- 大規模なセクション埋めをやめる。
- 1回または2回のtool turnで計画を完成させる。
- 探索は原則1回までにする。
- scaffoldや明確なアプリ生成タスクは、定型Planを即materializeしてActへ進める。

### 方針2: Actはtool-firstをruntimeで強制する

モデルが説明だけ返した場合に、再度「お願いします」と促すのではなく、runtimeで次の具体行動を狭める。

- RepoChange期待時のno-tool応答は短い上限で打ち切る。
- 初回no-toolで対象ファイルまたはscaffold actionを決める。
- 2回目no-toolでdeterministic fallbackまたはMissingRepoEditsへ進める。

### 方針3: 失敗時は長考させず、代替経路を選ぶ

local LLMに「考え直し」を長くさせない。

- timeout後のsidecar再試行を最小化する。
- 既知の失敗モードは、LLMに再生成させずruntime fallbackへ短絡する。
- `Bash` 失敗時は、同じコマンドの反復を止め、別の検証コマンドまたはファイル検査へ移る。

### 方針4: ユーザー作業を要求しない

最終応答で「ユーザーが実行してください」と書かせない。

- 実装タスクでは、可能な範囲でinstall / build / test / dev server起動確認をruntimeが促す。
- sandboxやnetwork制約で実行できない場合だけ、実行不能理由を明示する。
- コマンド未実行のまま成功扱いにしない。

### 方針5: promptより単純な制御を優先する

recovery noteを増やすより、状態遷移と許可ツールを明確にする。

- recovery stateは最新ユーザーターンに限定する。
- 1つのrecovery状態につき、許可ツールと期待成果を1つに絞る。
- 複雑な「Read or Edit or Bash」指示を避ける。

---

## 実装計画

### Phase 1: Plan最小化

#### 目的

Plan modeを「実装に入るための最小契約」に変更し、長考や探索ループを減らす。

#### 実装内容

- `src/agent/loop_run/turn.rs`
  - `Plan` の最大有効tool turnをタスク種別ごとに制限する。
  - `RepoChange + empty workspace + known framework` は、即 deterministic plan を生成してapprovalへ進める。
  - `Plan` 中の探索toolは原則1回までにし、2回目はplan file更新を強制する。
  - missing sectionを全て埋める方式から、Actに必要な最小項目だけを必須にする。

- `src/agent/loop_run/lifecycle.rs`
  - `current_plan_stage` / `plan_missing_sections` を簡略化する。
  - 必須項目を次に絞る。
    - Goal
    - Constraints
    - First Action
    - Verification

- `src/agent/recovery.rs`
  - Plan recovery noteを短くする。
  - 「探索してもよい」文言を減らし、「次はplan fileに1回だけWrite/Edit」と明示する。

#### 受け入れ条件

- UATのゲーム生成タスクでPlanが2 iter以内にapproval可能になる。
- Plan timeout時にsidecar長考へ流れず、deterministic planへ進む。
- Plan fileが未完成のまま同じセクションを再編集し続けない。

#### テスト

- Plan探索2回目でfallback planへ移る単体テスト。
- known framework taskでdeterministic planが即生成される単体テスト。
- 既存のPlan approval flowテストの更新。

---

### Phase 2: Act tool-first制御

#### 目的

Actでモデルが説明だけ返す、質問する、実行をユーザーに任せる、という挙動をruntimeで抑える。

#### 実装内容

- `src/agent/loop_run/turn.rs`
  - `ActionExpectation::RepoChange` でno-tool応答が出た場合の分岐を単純化する。
  - 初回no-tool:
    - empty workspaceならscaffoldまたはdeterministic app fallback。
    - 既存実装ありならfirst targetを決めてRead/Editへ誘導。
  - 2回目no-tool:
    - deterministic fallback適用可能なら適用してDone。
    - 適用不能ならMissingRepoEdits。
  - final replyが「ユーザーが実行してください」系の場合、未検証として再度tool turnへ戻す。

- `src/agent/recovery.rs`
  - `no_tool_recovery_note` を短縮する。
  - 「説明不要。次は1 tool callのみ」と統一する。

#### 受け入れ条件

- 実装タスクで、assistantが質問だけして終了しない。
- `npm run ...` などの検証をユーザーに依頼するだけでDoneにならない。
- no-tool recoveryが3回以上続かない。

#### テスト

- RepoChangeで質問文だけ返した場合、NoToolCallsではなくrecovery tool turnへ進む。
- 「run this yourself」系final replyをfuture workとして検出する。
- no-tool 2回目でdeterministic fallbackまたはMissingRepoEditsへ進む。

---

### Phase 3: 失敗時の短絡fallback

#### 目的

失敗後に長く考え直させず、既知の失敗モードをruntimeで処理する。

#### 実装内容

- `src/agent/loop_run/turn.rs`
  - timeout後の処理順を整理する。
    1. deterministic recovery可能か確認
    2. 既知の代替コマンドがあるか確認
    3. sidecar retry
    4. failure
  - focused edit timeout時、placeholder / low fidelityなら即quality fallback。
  - Bash失敗時、同一コマンド反復を禁止し、代替actionを選ぶ。

- `src/tools/bash.rs`
  - long-running dev server / install / build / test の検出結果を、recoveryが使いやすい分類で返す。
  - `listen EPERM` のようなsandbox由来エラーを明確に区別する。

#### 受け入れ条件

- timeout後に同じLLM長考へ戻らない。
- focused edit timeoutからplayable fallbackへ移れる。
- Bash失敗時に同じコマンドを繰り返さない。

#### テスト

- timeout errorからquality fallbackへ進む単体テスト。
- repeated bash block後に別action noteが出る単体テスト。
- sandbox由来のlisten失敗を起動失敗と誤分類しないテスト。

---

### Phase 4: 自動検証の標準化

#### 目的

「実装した」だけでなく、起動・build・静的品質確認までruntimeで一貫して扱う。

#### 実装内容

- `src/agent/loop_run/quality.rs`
  - framework別の検証コマンド候補を返すAPIを追加する。
  - Next / Vite / Nuxt のpackage scriptとport整合性を確認する。
  - `"latest"` 依存や重複port指定など、起動不安定要素を検出する。

- `src/agent/loop_run/turn.rs`
  - RepoChange Done前に、軽量なverification planを作る。
  - 実行可能な検証はBashで実行させる。
  - 実行不能な検証は、理由つきでfinal summaryに残す。

#### 受け入れ条件

- UATのアプリ生成タスクで、`package.json` のdev scriptと依存が安定している。
- `npm run dev -- -p 3011` が起動確認対象として扱われる。
- sandbox制約の場合、EPERMを実装品質の失敗と混同しない。

#### テスト

- Next / Vite / Nuxt package検査の単体テスト。
- latest依存検出テスト。
- port script補正テスト。

---

### Phase 5: recovery状態の整理

#### 目的

増えたrecovery状態を減らし、状態持ち越しと矛盾指示を防ぐ。

#### 実装内容

- `src/agent/loop_run/turn.rs`
  - recovery stateを最新ユーザーターン単位で扱う共通helperに集約する。
  - `post_scaffold_*`, `focused_edit_*`, `truncated_tool_call_*` の参照範囲を統一する。
  - `focused_edit_recovery_target` の優先順位を単純化する。

- `src/agent/recovery.rs`
  - recovery noteを棚卸しし、重複文言を削除する。
  - 1 note = 1 action の形へ寄せる。

#### 受け入れ条件

- 新しいユーザー指示に前ターンのscaffold/focused edit/truncated状態が影響しない。
- 既読時にReadを要求するnoteが存在しない。
- recovery noteの許可ツールとruntimeの許可ツールが一致する。

#### テスト

- 最新ユーザーターン以降だけを見るhelperの単体テスト。
- stale active_root / stale scaffold / stale truncated stateを無視するテスト。
- recovery noteとtool filteringの整合テスト。

---

## 実装順序

1. Phase 5: recovery状態の整理
2. Phase 1: Plan最小化
3. Phase 2: Act tool-first制御
4. Phase 3: 失敗時の短絡fallback
5. Phase 4: 自動検証の標準化

理由:

- まず状態持ち越しを減らさないと、Plan/Actの簡略化が別の副作用を生む。
- 次にPlanを短くして、実装フェーズへ早く入る。
- その後Actのtool-firstと失敗時短絡を整理する。
- 最後に検証を標準化し、UATの合格判定を安定させる。

---

## UAT計画

### Smoke

- `cargo fmt --check`
- targeted unit tests
- `cargo build --release`

### E2E

既存の `workspace/v0.1.0/uatspace/v2/uatplan_v2.md` を継続利用する。

段階的に実施する。

1. qwen3.5:122b で1回通し
2. qwen3.5:122b で3回連続通し
3. qwen3.6:27b-coding-nvfp4 で1回通し
4. qwen3.6:27b-coding-nvfp4 で3回連続通し

各回で確認する項目:

- 4指示すべて完了する。
- Planが過剰に長くならない。
- no-tool / focused edit / placeholder loopに入らない。
- package scriptが3011に整合する。
- dependenciesが安定版固定である。
- 起動確認でHTTP 200または明確な環境制約理由が得られる。

---

## 完了条件

- qwen3.5:122b と qwen3.6:27b-coding-nvfp4 の両方で、UAT v2の4指示が3回連続で正常終了する。
- 各runでPlanが短く、Actがtool-firstで進む。
- 実装後の検証がユーザー任せにならない。
- 既知の失敗モードが新しいrecovery note追加ではなく、runtimeの単純な分岐で処理される。

---

## 注意点

- provider abstractionは増やさない。
- 旧Anvil的な大きい設計へ戻さない。
- fallbackを増やす場合も、要求文とframework種別に基づく汎用処理に限定する。
- prompt文言だけで解決しようとしない。
- recovery noteを追加する場合は、対応するruntime制約と単体テストを必ず追加する。
