# Ultra Phase Context Continuity Source Parity Plan

作成日: 2026-06-29

## 0. 目的

`/ultra-plan-run` の移植不備を少しずつ戻すため、Phase 021-2 では **phase 間の文脈継続** のみに対象を絞る。

対象は以下に限定する。

- ultra-plan-run 経由で phase をまたぐ作業履歴の保持
- phase 1 で作成した artifact、phase 2 で発生した verify failure、repair が試した内容を後続 phase に渡す仕組み
- 移植元 anvil の `run_ultra_plan` が同じ `SessionSnapshot` を使い回す意味論の MVP 化
- 文脈継続の有無を eval / event / unit test で観測できるようにする

対象外は以下とする。

- UltraPlan 生成 prompt の追加変更
- profile runtime contract / Next.js snapshot の詳細化
- repair exhaustion から `/ultra-plan-run` recovery prompt を保存する仕組み
- profile verify の `Invariant / Final` 分離
- Next.js / Space Invaders 専用テンプレート追加
- `RepairJob` や source scaffold pipeline の丸ごと移植

021-1 では UltraPlan 生成だけを戻した。021-2 では、その plan を phase 実行へ運ぶときの文脈継続だけを戻す。

## 0.1 レビュー結果と反映内容

本計画を以下の観点でレビューし、指摘を反映した。

- Source parity: 移植元の本質は `SessionSnapshot` 共有であり、bounded summary だけでは source parity と言い切れない。shared session を主、bounded summary を補助に位置づける。
- 安定性: shared session は context bloat と古い失敗の引きずりを招く可能性があるため、bounded summary、上限、event 観測、fallback 方針を明記する。
- API 影響: `plan-run` 単体の public behavior を変えないことを受入条件に明記する。
- 実装可能性: context 更新には `run_step_plan_with_ui_inner` の文字列戻り値だけでは不足するため、内部用の structured outcome を計画に追加する。
- 検証可能性: 「phase prompt に含まれる」だけでなく、「shared session の message history に残る」ことも検証対象にする。
- 過適応防止: Space Invaders / Next.js 固有のキーワードではなく、artifact / verify failure / repair target / changed path という汎用情報だけを引き継ぐ。
- 段階導入: profile final repair の shared session 化は影響が広いため、021-2 の必須範囲は phase step execution に限定し、profile repair は context 記録のみを必須にする。
- Planner / execution 分離: shared session の対象は execution model の step 実行であり、planner の StepPlan 生成会話を共有しない。planner には bounded phase context を prompt として渡す。
- 失敗時 partial outcome: `anyhow::Result<String>` のままでは失敗時に changed paths / repair target / verifier failure を失うため、partial outcome を保持できる内部 error / result 形を必須にする。
- 観測信頼性: event の `shared_session: true` だけでは自己申告に近いため、phase 前後の `session_message_count` など、実際に同じ session が伸びていることを検証できる field を追加する。
- 情報漏えい対策: bounded summary には raw stderr / prompt 全文 / secret-like value を入れない。path、command 名、failure kind、短い redacted snippet に限定する。

## 1. 現状

### 1.1 移植元 anvil の挙動

移植元は `src/agent/minimal_step_runner.rs::run_ultra_plan` で、呼び出し元から受け取った `SessionSnapshot` を ultra 全体で使い回す。

該当箇所:

- `src/agent/minimal_step_runner.rs:580`
- `src/agent/minimal_step_runner.rs:586`
- `src/agent/minimal_step_runner.rs:604`
- `src/agent/minimal_step_runner.rs:610`

挙動:

- `run_ultra_plan` の引数に `session: &mut SessionSnapshot` がある。
- 各 phase の `generate_and_run_step_plan` に同じ `session` を渡す。
- phase 1 の計画・実行・repair の会話履歴が、phase 2 以降の LLM 呼び出しにも残る。
- ファイルシステム上の成果物だけでなく、LLM との対話上の作業履歴も継続する。

この仕組みにより、後続 phase は以下を暗黙に参照しやすい。

- すでに作成したファイル
- 直前 phase の作業意図
- 失敗した verify command
- repair で試した変更
- まだ残っている未完了 artifact

### 1.2 MVP の現在挙動

MVP は `mvp/anvilminimal/src/planner/runner.rs::run_ultra_plan_with_ui` で phase ごとに step-plan を生成し、その step-plan を `run_step_plan_with_ui_inner` に渡す。

該当箇所:

- `mvp/anvilminimal/src/planner/runner.rs:837`
- `mvp/anvilminimal/src/planner/runner.rs:865`
- `mvp/anvilminimal/src/planner/runner.rs:915`
- `mvp/anvilminimal/src/planner/runner.rs:264`
- `mvp/anvilminimal/src/planner/runner.rs:276`

現在の問題点:

- `run_step_plan_with_ui_inner` 内で毎回 `let mut session = SessionSnapshot::new();` している。
- ultra phase が変わるたびに、実行モデルの会話 session がリセットされる。
- ファイルは残るが、LLM への作業履歴、失敗履歴、repair 履歴は残りにくい。
- phase 2 以降は、compact workspace snapshot と step prompt から再推論する必要がある。

### 1.3 実害

`test0628_001` / `test0628_002` や targeted eval で見えた問題は、単に planner が悪いだけではない。

特に以下の挙動が起きやすい。

- phase 1 で scaffold した内容を、phase 2 が十分に前提化できない。
- phase 2 の build failure が repair prompt に入っても、実行モデルが同じ失敗に対して具体 edit をしない。
- repair が何を試したかが後続 phase や final phase に伝わらない。
- 「前 phase で作った app を完成させる」ではなく、各 phase が孤立した小タスクになりやすい。
- plan-run 単体の指標は高くても、ultra-plan-run の accepted artifact まで到達しにくい。

## 2. あるべき挙動

### 2.1 Source parity として戻す意味論

MVP でも ultra-plan-run 中は、phase 間で作業文脈が継続されるべきである。

あるべき挙動:

- ultra-plan-run 開始時に、ultra 全体用の実行 session を作り、補助として bounded context を持つ。
- phase 1 の step-plan 実行で発生した会話履歴、tool 実行、verify 結果、repair 結果を phase 2 以降へ渡す。
- shared session の対象は execution model の step 実行である。planner の StepPlan 生成は従来どおり各 phase prompt を入力とする独立した planner call とし、必要な prior context は bounded summary として prompt に含める。
- 後続 phase の prompt には、少なくとも bounded summary として以下を含める。
  - completed phases
  - created / edited paths
  - expected final artifacts の達成状況
  - unresolved verify failures
  - repair attempts and whether they changed files
  - pending repair targets
- step-plan 単体実行では従来どおり新規 session を使ってよい。
- 文脈継続は `/ultra-plan-run` 経由の実行に限定して有効にする。

### 2.2 MVP としての設計制約

移植元と同じ `SessionSnapshot` を単純に全 phase で共有するだけでも parity は上がる。ただし MVP では以下を守る。

- 無制限な会話履歴肥大を避ける。
- step-plan 単体 API の挙動を変えない。
- plan-run の単体成功率を下げない。
- LLM に過去履歴を渡しすぎて、古い失敗に過剰適応しない。
- eval の特定シナリオ名や Space Invaders 固有語に依存しない。
- source の巨大な repair graph は移植しない。

したがって、候補は2段階で検討する。

1. **Shared Session 型**
   - `run_step_plan_with_ui_inner` に optional `&mut SessionSnapshot` を渡せるようにする。
   - ultra-run だけ shared session を渡す。
   - 最も source parity に近い。

2. **Bounded UltraRunContext 型**
   - phase 結果を構造化 summary にして次 phase prompt に挿入する。
   - 会話履歴そのものは共有しない。
   - context bloat と過去失敗の引きずりを制御しやすい。

今回の採用方針は **Shared Session を source parity の本体とし、Bounded Summary を制御・観測の補助として併用する** ことである。

- source parity のため、ultra-run 経由では同じ `SessionSnapshot` を使う。
- 安定性と診断性のため、phase event から `UltraRunContext` も更新し、次 phase prompt に短い summary を入れる。
- shared session と bounded summary は重複してよいが、役割を分ける。
  - shared session: LLM 会話履歴の継続。
  - bounded summary: 重要 artifact / failure / repair target を明示し、eval で観測可能にする。
- context budget 超過や品質劣化が見えた場合は、shared session の compaction または bounded summary への縮退を検討する。ただし初期実装では silent fallback せず event に残す。

## 3. 問題点

### 3.1 実装上の問題箇所

主な問題箇所:

- `mvp/anvilminimal/src/planner/runner.rs:264`
  - `run_step_plan_with_ui_inner` が session を内部生成している。
- `mvp/anvilminimal/src/planner/runner.rs:276`
  - `let mut session = SessionSnapshot::new();`
- `mvp/anvilminimal/src/planner/runner.rs:915`
  - ultra phase 実行時に、shared session を渡す口がない。
- `mvp/anvilminimal/src/planner/runner.rs:2097`
  - `ultra_phase_prompt` が prior phase summary を含まない。

### 3.2 評価上の問題

現在の eval は phase completion や runtime health を見るが、以下を十分に直接観測できていない。

- phase 2 の prompt に phase 1 の成果物が含まれているか。
- repair failure が後続 phase に渡っているか。
- repeated repair/no-change が次 phase の方針変更につながっているか。
- shared session が使われているか。

そのため、plan が良いのに実行が孤立して失敗するケースの診断が粗くなる。

## 4. 根本原因

根本原因は、移植時に `run_ultra_plan` の引数形だけでなく、`session` が持つ実行意味論を移植対象として扱えていなかったこと。

具体的には以下。

- `plan-run` 単体 API を小さくする過程で、`run_step_plan_with_ui_inner` が session を内部生成する形になった。
- ultra-plan-run はその API を再利用したため、phase ごとの session reset が発生した。
- ファイルシステムが残るため一見問題が見えにくく、「LLM 会話履歴の継続」が受入条件から漏れた。
- eval は YAML 品質、build、artifact existence に寄り、phase 間 context の有無を直接検査していなかった。
- source parity 調査が `UltraPlan 生成` と `step-plan 実行` に分断され、両者をつなぐ `session continuity` が見落とされた。

これは個別の Next.js 問題ではなく、ultra-plan-run 全 profile に波及する移植不備である。

## 5. 対策案

### 5.1 API を分離する

`run_step_plan_with_ui_inner` を以下のように分ける。

- public /単体 plan-run 用:
  - `run_step_plan_with_ui(...)`
  - 従来どおり内部で新規 session を使う。
- ultra-run 内部用:
  - `run_step_plan_with_session_with_ui(...)`
  - caller が渡した `&mut SessionSnapshot` を使う。
  - 戻り値は string だけでなく、context 更新に必要な structured outcome を返す。

狙い:

- plan-run 単体の挙動を壊さない。
- ultra-run だけ source parity に寄せる。
- session の所有権と寿命を明確にする。

内部用 outcome の最小構造:

- `summary: String`
- `completed_steps: usize`
- `changed_paths: Vec<String>`
- `verify_failures: Vec<String>`
- `primary_failure: Option<String>`
- `repair_targets: Vec<String>`
- `command_failures: Vec<String>`
- `repair_attempts: usize`
- `repair_changed_paths: Vec<String>`
- `stop_reason: Option<String>`

注意:

- public API は従来どおり `anyhow::Result<String>` を返してよい。
- structured outcome は ultra-run 内部と eval event 用に限定する。
- stdout / stderr の文字列 parse に依存して context を更新しない。
- failure path でも partial outcome を失わない。`StepPlanRunError` などの内部型で `partial_outcome` と original error を保持する。

### 5.2 ultra-run で shared session を持つ

`run_ultra_plan_with_ui` の先頭で、ultra 全体用の session を作る。

期待する構造:

- `let mut ultra_session = SessionSnapshot::new();`
- 各 phase の `run_step_plan_with_session_with_ui(..., &mut ultra_session, ...)` に渡す。
- repair loop も同じ session 上に積む。

注意:

- 021-2 の必須範囲は phase step execution の shared session 化に限定する。
- profile final repair で別 session を作っている箇所は、021-2 では少なくとも repair summary を `UltraRunContext` に残す。
- profile final repair の shared session 化は、phase verification / repair handoff の計画と衝突しやすいため、必要性を評価して次フェーズで扱う。

### 5.3 Bounded UltraRunContext を追加する

shared session だけでは eval で観測しづらいため、構造化 context も持つ。

最小構造:

- `completed_phases: Vec<String>`
- `created_or_changed_paths: Vec<String>`
- `last_failed_phase: Option<String>`
- `last_verify_failures: Vec<String>`
- `last_repair_changed_paths: Vec<String>`
- `pending_final_artifacts: Vec<String>`

使い方:

- phase 開始前に `ultra_phase_prompt` へ `Prior ultra context:` を追加する。
- phase 完了後に event / verify report / changed paths から更新する。
- context は最大行数を制限する。

summary の制約:

- 最大 20 行程度に制限する。
- changed paths は最大 20 件、verify failures は最大 5 件、repair targets は最大 5 件に制限する。
- resolved failure と unresolved failure を分ける。
- scenario 固有語ではなく、path / command / failure kind / repair target のみを保持する。
- raw stderr は保存しない。必要な場合は `eval_events::body_snippet` 相当の短い redacted snippet にする。
- secret-like value、API key、token、環境変数値を context summary に含めない。

### 5.4 eval event を追加する

新規 event:

- `ultra_context_initialized`
  - `shared_session: true`
  - `session_message_count: 0`
- `ultra_phase_context_attached`
  - `phase_id`
  - `shared_session: true`
  - `completed_phase_count`
  - `changed_path_count`
  - `pending_artifact_count`
  - `last_failure_count`
  - `session_message_count`
  - `context_truncated`
- `ultra_phase_context_updated`
  - `phase_id`
  - `changed_paths`
  - `pending_final_artifacts`
  - `last_failure_kind`
  - `session_message_count`
  - `partial_outcome_recorded`
  - `context_truncated`

目的:

- 実行結果から phase context continuity の有無を判断できるようにする。
- eval が「phase 間 context があるのに失敗した」のか「context がないため失敗した」のかを分離できるようにする。

### 5.5 既存テストへの影響を抑える

影響を抑えるため、以下を守る。

- `run_step_plan_with_ui` の public behavior は変えない。
- `run_plan_passes_step_contract_to_execution_client` は維持する。
- ultra-plan-run fake client テストだけ、phase 2 の execution prompt に phase 1 context が入ることを追加検証する。
- shared session の検証では、phase 2 の user prompt 文字列だけでなく、execution client に渡された message history に phase 1 の tool call / assistant response が残ることも確認する。
- failure path の検証では、step execution が Err になっても partial outcome から context update event が出ることを確認する。

### 5.6 非目標

021-2 でやらないことを明確化する。

- phase verification の final / invariant 分離はしない。
- Next.js runtime contract の内容は増やさない。
- repair exhaustion handoff は追加しない。
- profile final repair の session 共有は必須にしない。
- TUI の既存会話 session を slash command 実行へ完全接続することは必須にしない。021-2 の最低目標は ultra-run 内の phase 間 shared session である。
- 成功率改善を唯一の判定軸にしない。まずは context continuity が実装・観測できることを合格条件にする。

## 6. 対策後の期待する挙動

### 6.1 通常挙動

`/ultra-plan-run --profile nextjs ...` を実行した場合:

- phase 1 で scaffold したファイルが phase 2 prompt/context に反映される。
- phase 2 は「空の workspace から始める」のではなく、「既存 Next.js app を完成させる」前提で動く。
- build failure が発生した場合、repair の失敗や変更履歴が次 phase に渡る。
- final phase は過去 phase の成果物と未達 artifact を踏まえて acceptance に向かう。

### 6.2 失敗時挙動

途中で verify / repair が失敗した場合:

- `.anvil` event に context attached / updated が残る。
- failure summary に、どの phase まで完了し、どの artifact / verify failure が残っているかが出る。
- 後続 phase が存在する場合は、直前の unresolved failure / repair target が次 phase context に入る。
- 次の対策である repair handoff へつなげやすくなる。

### 6.3 期待する指標改善

直接改善を期待する指標:

- `phase_completion_score`
- `phase_step_execution_score`
- `runtime_friction_score`
- `finalization_score`
- `ultra_runtime_health_score`
- `execution_contract_adherence_score`

間接改善を期待する指標:

- `plan_output_adherence_score`
- `postcheck_stability_score`
- `acceptance_success`

ただし、021-2 の目的は「成功率を一気に上げること」ではなく、phase 間文脈が切れる移植不備を是正し、失敗原因をより正しく分離できるようにすることである。

## 7. 検証方法

### 7.1 Unit tests

追加または更新するテスト:

1. `run_step_plan_with_ui` は従来どおり単体 session を使う。
   - public API の backward compatibility を確認する。

2. ultra-run は phase 間で shared session を使う。
   - fake planner / fake execution client を使う。
   - phase 1 で `Write app.txt`。
   - phase 2 の execution client messages に phase 1 の履歴が残ることを確認する。
   - phase 2 の prompt に bounded context が含まれることを確認する。

3. `ultra_phase_prompt` は prior context を含む。
   - completed phase
   - changed path
   - pending artifact
   - last failure

4. context は bounded である。
   - changed paths が多い場合も上限件数で切られる。
   - prompt が無制限に肥大しない。

5. final contract verification は従来どおり final phase のみで走る。
   - phase context 追加で final/non-final の境界が壊れないことを確認する。

6. structured outcome は stdout parse に依存しない。
   - fake run outcome の changed paths / verify failures が `UltraRunContext` に反映されることを確認する。

7. failure path でも partial outcome が残る。
   - fake execution で verify failure / repair failure を起こし、changed paths / repair targets / primary failure が context update に反映されることを確認する。

8. bounded context は raw secret / raw stderr を含まない。
   - secret-like string を含む fake stderr を入れても、context summary には redacted snippet だけが入ることを確認する。

### 7.2 Integration tests

追加する統合テスト:

1. `tui_ultra_plan_run_smoke_fake_clients` を拡張する。
   - phase 1 と phase 2 の fake execution 呼び出しを検査する。
   - phase 2 が prior context を受け取ることを確認する。

2. Next.js fixture で phase 1 scaffold、phase 2 implementation、phase 3 verify の流れを確認する。
   - phase 2 prompt に phase 1 で作った `package.json` / `src/app/page.tsx` が入る。

3. verify failure fixture。
   - phase 2 で build failure を起こす。
   - bounded repair で回復した場合は、次 phase prompt / event に repair summary が入る。
   - bounded repair で回復不能な場合は、処理を止めたうえで `ultra_phase_context_updated` に failure summary が残る。

4. plan-run standalone fixture。
   - `run_step_plan_with_ui` 単体実行では、複数 plan-run 呼び出し間で session が共有されないことを確認する。

### 7.3 Eval

最低限、以下を実行する。

1. MVP only smoke:
   - `ultra-plan-run`
   - `nextjs-space-invaders-large`
   - local LLM 未使用

2. MVP vs anvildev comparison:
   - `ultra-plan-run`
   - `plan-run`
   - local LLM 未使用
   - 少なくとも 1 run、可能なら 3 runs

3. Event-based assertion:
   - `ultra_context_initialized` が存在する。
   - phase 2 以降に `ultra_phase_context_attached` が存在する。
   - failed run でも `ultra_phase_context_updated` が残る。
   - `shared_session` が true として記録される。
   - phase が進むにつれて attach/update event の `session_message_count` が増える。
   - context summary の item count が上限内である。
   - `context_truncated` が true の場合でも failure kind / repair target は残る。

見るべき指標:

- `phase_completion_score`
- `ultra_runtime_health_score`
- `runtime_friction_score`
- `finalization_score`
- `verify_repair_edit_score`
- `build_repair_effectiveness_score`
- `acceptance_success`

### 7.4 Manual UAT

対象:

- `/Users/maenokota/share/work/localwork/commandagent_mvp/01/test0628_001`
- `/Users/maenokota/share/work/localwork/commandagent_mvp/01/test0628_002`
- 新規空ディレクトリ

実行:

```text
anvilminimal --yes --context-budget 65536 --model qwen3.6:27b-coding-nvfp4 --planner-model gemini-3.5-flash --planner-provider gemini --provider ollama
```

TUI 内:

```text
/ultra-plan-run --profile nextjs あなたが考える最高に面白くかっこいいスペースインベーダーゲームを3011ポートで起動可能なnext.jsアプリとして開発してください。
```

確認:

- `.anvil` 配下に phase context event が残る。
- phase 2 以降で phase 1 の成果物が認識されている。
- 失敗しても、どの phase の何が未達か追跡できる。
- 成功した場合、`src/app/page.tsx` が静的タイトルだけでなく、要求 capability の evidence を含む。

## 8. 受入条件

### 8.1 Source parity

- ultra-run 経由では、phase 間で shared session が継続される。
- bounded context は shared session の代替ではなく、観測と明示補助として継続される。
- 移植元の `run_ultra_plan(..., session, ...)` が持つ「phase をまたぐ作業履歴継続」の意味論が MVP に存在する。
- step-plan 単体実行では従来どおり独立 session を使える。

### 8.2 Observability

- eval event で context continuity を確認できる。
- phase 2 以降の prompt に prior phase summary が含まれることをテストできる。
- phase 2 以降の execution client message history に前 phase の会話履歴が残ることをテストできる。
- failed run でも context update event が残る。
- context が bounded であることを event で確認できる。
- `shared_session: true` だけでなく、session message count の増加で継続を確認できる。

### 8.3 Safety

- prompt / session が無制限に肥大しない。
- old failure を無条件に引きずらない。
- profile-specific hard coding や scenario-specific keyword は入れない。
- plan-run 単体の既存テストが通る。
- shared session 導入後も `max_iterations`、interrupt、yes mode、workspace confinement の挙動を変えない。
- bounded context は raw stderr / prompt 全文 / secret-like value を含まない。

### 8.4 Quality

- `cargo test --manifest-path mvp/anvilminimal/Cargo.toml` が通る。
- eval unit tests が通る。
- MVP only ultra-plan-run smoke で context events が記録される。
- 可能であれば、rollback 後 baseline と比較して `phase_completion_score` または `ultra_runtime_health_score` が悪化しない。

## 9. リスクと対策

| リスク | 内容 | 対策 |
| --- | --- | --- |
| Context bloat | shared session で会話履歴が肥大する | bounded summary を併用し、必要なら context compaction / cap を入れる |
| 既存 plan-run への影響 | step-plan 単体の挙動が変わる | public API は新規 session のまま維持し、ultra 内部 API を分ける |
| 古い失敗の引きずり | phase 1 の失敗を phase 3 が過剰に気にする | summary は unresolved / resolved を分け、完了済み失敗は短くする |
| eval 過適応 | Space Invaders 固有の文脈だけを渡す | artifact / failure / repair target の汎用 summary に限定する |
| 複雑性増大 | source の session/repair graph を丸ごと戻す | 021-2 では shared session と bounded context に限定する |
| 二重文脈による矛盾 | shared session と bounded summary が異なる内容を伝える | bounded summary は structured outcome から生成し、event で件数と内容を検証する |
| 実装途中で成功率が悪化 | context が増えて実行モデルが迷う | まず fake/integration で prompt size と message sequence を検証し、eval では成功率だけでなく failure kind を比較する |
| 失敗時に context が更新されない | `Result` の Err で partial outcome を失う | 内部 error 型に partial outcome を持たせ、failure path でも context update event を出す |
| planner session まで共有して挙動が不安定化する | source parity の対象を広げすぎる | shared session は execution model の step 実行に限定し、planner には bounded summary だけを渡す |
| 診断情報の漏えい | raw stderr や secret-like value が context summary に入る | snippet 化、redaction、件数制限を必須にする |

## 10. 実装フェーズ案

### Phase 0: Baseline capture

- 現在の ultra-plan-run event と prompt を保存する。
- phase 2 以降に prior context がないことを fixture で確認する。

### Phase 1: Internal API split

- `run_step_plan_with_session_with_ui` を追加する。
- `run_step_plan_with_ui` は従来の wrapper として残す。
- 内部用 `StepPlanRunOutcome` を追加し、changed paths / verify failures / repair attempts を返せるようにする。
- 内部用 `StepPlanRunError` または同等の型を追加し、failure path でも partial outcome を保持できるようにする。

### Phase 2: Ultra shared session

- `run_ultra_plan_with_ui` に `ultra_session` を追加する。
- phase step execution へ `&mut ultra_session` を渡す。

### Phase 3: Bounded UltraRunContext

- `UltraRunContext` を追加する。
- completed phases / changed paths / failures / pending artifacts を保持する。
- context size cap と resolved / unresolved の区別を入れる。

### Phase 4: Phase prompt attachment

- `ultra_phase_prompt` に `Prior ultra context` を追加する。
- context が空なら `- none yet` にする。

### Phase 5: Event instrumentation

- `ultra_context_initialized`
- `ultra_phase_context_attached`
- `ultra_phase_context_updated`

を追加する。
- message count、context truncated、partial outcome recorded を event field に含める。

### Phase 6: Tests

- unit / integration / eval unit を追加する。
- plan-run 単体 regressions を確認する。
- shared session の message history 継続と bounded context の prompt attachment を別々に検証する。
- failure path の partial outcome と redaction を検証する。

### Phase 7: Eval

- MVP only ultra-plan-run smoke を実行する。
- 可能なら anvildev と比較する。

### Phase 8: Manual UAT

- TUI から `/ultra-plan-run --profile nextjs ...` を実行する。
- `.anvil` events と成果物を確認する。

## 11. 次フェーズへの接続

021-2 は phase 間文脈継続だけを戻す。

021-2 後にまだ失敗が残る場合、次に扱うべき候補は以下。

1. `profile runtime contract / snapshot` を source 寄りに戻す。
2. `repair exhaustion` から `/ultra-plan-run` recovery prompt を保存する。
3. `phase verification` を invariant / final に分ける。
4. build failure repair target を source 寄りに強化する。

これらを同時に入れない。021-2 の効果を測ったうえで、次の小さい移植単位を選ぶ。
