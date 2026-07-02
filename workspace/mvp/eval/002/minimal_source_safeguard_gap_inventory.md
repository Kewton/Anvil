# 移植元 minimal 安全装置の取りこぼし棚卸し

作成日: 2026-06-25

## 結論

MVP には source の minimal 系から移植された基本 loop / plan / verify はありますが、収束性・成果物契約・repair 誘導・plan 品質を守る安全装置が複数落ちています。

今回の `minimal loop reached max_iterations (12)` に直接効いている最重要の取りこぼしは次の 4 点です。

1. `early_success_paths` による tool 実行直後の成功停止がない
2. `minimal-loop --prompt` に期待成果物契約を入れる経路がない
3. source の no-tool / completion-without-write / requested-artifact feedback が弱化している
4. step repair の進捗分析と bounded repair policy が大幅に簡略化されている

ただし、`Glob` / `Read` / `Bash` の同一呼び出し反復を汎用検出する仕組みは、直接移植元の minimal 系にも存在しません。これは旧 `src/agent/loop_run/*` 系の no-progress recovery に近い領域であり、本棚卸しでは「移植元 minimal からの取りこぼし」には含めません。

## 対象範囲

直接移植元として確認した範囲:

- `src/agent/minimal_loop/*`
- `src/agent/minimal_repl.rs`
- `src/agent/minimal_step_runner.rs`
- `src/agent/minimal_step_runner/*`
- 上記から呼ばれる minimal 実行時の `src/tools/registry.rs`
- 上記から呼ばれる workspace read policy: `src/util/workspace_paths.rs`
- Bash build/test 出力整形の直接依存: `src/tools/test_output.rs`
- provider/tool protocol の確認補助: `src/agent/minimal_llm.rs`, `src/agent/planner_llm.rs`

MVP 側の照合対象:

- `mvp/anvilminimal/src/minimal_loop/*`
- `mvp/anvilminimal/src/planner/*`
- `mvp/anvilminimal/src/tools/*`
- `mvp/anvilminimal/src/providers/*`
- `mvp/anvilminimal/src/tui/*`
- `mvp/anvilminimal/src/repl.rs`
- `mvp/anvilminimal/src/cli.rs`
- `mvp/anvilminimal/src/state.rs`
- `mvp/anvilminimal/src/eval_events.rs`
- `mvp/anvilminimal/scripts/eval_lib/*`
- `mvp/anvilminimal/tests/*`

除外:

- 旧 `src/agent/loop_run/*` の artifact ledger / no-progress recovery / verifier orchestration
- 旧 heavy loop の CaseRecord / Photon / mechanism-ledger 系

## 取りこぼし一覧

| ID | 重要度 | source minimal の安全装置 | MVP の状態 | 影響 |
|---|---:|---|---|---|
| SG-01 | P0 | `early_success_paths` を `MinimalLoopConfig` に持ち、tool 実行直後に期待 path が揃えば成功停止する | `required_paths` は no-tool 応答時だけ確認。tool を出し続けると停止できない | 成果物作成済みでも max_iterations に到達する |
| SG-02 | P0 | `minimal_step_runner` が step の `expected_paths` を `early_success_paths_for_step` 経由で loop に渡す | plan-run は `expected_paths` を渡すが、loop 内停止条件が弱いため同等でない | plan-run でも同じ max_iterations が波及する |
| SG-03 | P0 | `extract_requested_artifact_paths(user_prompt)` で prompt 内の明示 path を抽出し、不足時に feedback | MVP の `--prompt` は `required_paths=[]`。eval suite の `expected_artifacts` も渡らない | minimal-loop eval が成果物契約なしで走る |
| SG-04 | P1 | `completion_without_write_feedback`: Act mode で書き込み前に final/no-tool を返した場合、1 回だけ作業継続を促す | MVP は `looks_like_progress_without_tool` に該当した時だけ feedback | 「完了」と言って終わる/曖昧な no-tool 応答の検出漏れ |
| SG-05 | P1 | `planned_action_without_tool`: "Now let me..." 等の未来行動を no-tool で返したら assistant 応答を破棄して再試行。反復時は error | MVP に類似 feedback はあるが語彙が少なく、反復上限後の専用 error がない | no-tool planning 反復が max_iterations まで延びやすい |
| SG-06 | P1 | `requested_artifacts_missing`: 明示要求 path が未作成なら final を破棄して作成/阻害理由を促す | MVP は caller が `required_paths` を渡した時のみ | `--prompt` / eval direct task で不足成果物を loop 内に戻せない |
| SG-07 | P1 | frontend source の relative import を検査し、未解決 import があれば final 前に修正 feedback | MVP には該当検査なし | Next.js/TS/JS で import 先未作成のまま成功扱いまたは postcheck 失敗 |
| SG-08 | P1 | Edit anchor mismatch を検出し、Read し直して小さい Edit を促す feedback | MVP は missing arg / unknown tool 以外の tool error を原則 hard error | 修復可能な Edit 失敗が一発終了、または eval 上の粗い失敗になる |
| SG-09 | P2 | tool 実行 error を tool result `ERROR: ...` として会話に戻し、LLM に別手段を試させる | MVP は一部 recoverable のみ feedback、他は hard error | path / edit / command の修復余地が狭い |
| SG-10 | P2 | no-tool 失敗応答は破棄し、feedback は ephemeral user message として次 request のみに注入する。tool error 自体は source/MVP とも tool result として履歴に残る | MVP も pending feedback はあるが、source と同じ feedback state / failed assistant discard の網羅性がない | no-tool 失敗応答や簡易 feedback が後続 prompt を汚しやすい |
| SG-11 | P1 | `WorkspacePolicy::for_task_request` により通常タスクでは controller metadata 等の read/discovery を抑制 | MVP の `workspace_policy` は `Normal` placeholder で loop/tool context に接続されていない | `.anvil` 等の内部生成物を LLM が読んで迷走する可能性 |
| SG-12 | P2 | source prompt は「tool を使うべき場面」「final は予定で終えない」「観測していない結果を捏造しない」を明文化 | MVP prompt は短く、同等の禁止事項が弱い | provider 差により no-tool final / 未観測成功宣言が増える |
| SG-13 | P1 | verify command は長さ・shell control syntax・許可コマンドを厳格 allowlist | MVP は `blocked_reason` と一部 path validation 中心で、許可コマンド種別の制約が弱い | verify に setup/network/複合 shell が混入しやすい |
| SG-14 | P1 | `PlanStep.kind` は enum、`expected_result` は pass/fail 型。TDD red step を表現/検証できる | MVP は `kind: String`、`expected_result` がない | plan の責任境界と red/green 検証契約が弱い |
| SG-15 | P1 | `validate_step_kind_contract`: inspect/setup/verify/report の契約違反を拒否 | MVP lint は step 数・id・path・verify command 中心 | setup step が verify したり、verify step が install したりする plan を弾きにくい |
| SG-16 | P1 | `lint_plan_with_workspace`: 複数 expected path の具体性、dependency setup before verify、Next.js build order を検査 | MVP には同等の意味 lint がない | plan 分解品質が落ち、verify が早すぎる/曖昧になる |
| SG-17 | P1 | plan/ultra plan generation は invalid output に対して最大 3 回 corrective prompt を返す | MVP は step plan parse 失敗時に `StepPlan::single(goal)` へ fallback | planner 出力不良が単一巨大 step になり、minimal loop へ過負荷 |
| SG-18 | P1 | step 実行は `STEP_TURN_MAX_ITERATIONS=8`、repair は `STEP_REPAIR_MAX_ITERATIONS=6` に cap | MVP は原則 `config.max_iterations` をそのまま使用 | 1 step が長く回りすぎ、失敗単位が粗くなる |
| SG-19 | P0 | repair は最大 4 turn / file-changing repair 2 回まで。進捗なしなら missing path / repeated Write/Edit を repair prompt に入れる | MVP repair は 1 回の汎用 prompt のみ | expected path が減らない/同じ file を直し続ける問題を誘導修正できない |
| SG-20 | P1 | repair exhausted report に missing paths、changed files、repeated edits、verify failures、suggested replan command を出す | MVP の repair report は `{:?}` の status 中心 | eval/人間レビューで直接原因を特定しづらい |
| SG-21 | P1 | `verify_step` は missing expected paths と verify failures を複数蓄積して返す | MVP は `VerifyStatus` 単一で先頭 failure で止まる | 修復 prompt / eval 診断が情報不足になる |
| SG-22 | P2 | `verifier_precondition_failure`: `npm run build` 前に `node_modules/.bin/next` 不在を dependency_missing と明示 | MVP は `not found` / `No such file` 文字列依存 | Next.js dependency missing の分類が不安定 |
| SG-23 | P1 | ultra phase prompt に original ultra goal、required artifacts、profile/style/intent、workspace snapshot、runtime contract を注入 | MVP は基本的に phase.prompt をそのまま step planner に渡す | phase 間で original goal / port / profile contract が薄まる |
| SG-24 | P1 | source は profile verification を各 phase 後に実施し、profile contract 違反をその phase で止める | MVP は non-final の `ProfileContractFailed` を許容して最終 phase まで進める | 早期 drift を見逃し、最後に大きく壊れる |
| SG-25 | P2 | Next.js profile contract が Tailwind toolchain、`tsconfig.rootDir`、`@/*` alias、build/dev script 弱体化を検査 | MVP は deps/build/dev/entry/layout/alias 中心。Tailwind/rootDir 等は薄い | Next.js app の build 不能・構成不整合を postcheck まで残しやすい |
| SG-26 | P2 | source plan/run summary は step ごとに `completed` / `repaired_after_max_iterations` / `verification_failed_after...` を記録 | MVP の成功出力は `plan-run complete: N steps` 中心 | eval failure classification に loop 内の stop reason が届きにくい |
| SG-27 | P1 | `empty_response` feedback: 空の assistant 応答を 1 回だけ再試行させる | MVP は no-tool かつ空 content を成功応答として返し得る | provider の空応答が silent success / 後続 postcheck failure になる |
| SG-28 | P1 | `missing_tool_call(user_prompt)`: action prompt なのに tool 未使用なら tool 呼び出しを促す | MVP は未来行動らしい文面だけを見る簡易判定 | `done` / `完了しました` のような no-tool action failure を検出しにくい |
| SG-29 | P1 | XML fallback 時は system prompt を `PromptToolMode::XmlFallback` に切り替え、XML tool call 例を明示する | MVP は native tools disabled 後も system prompt がほぼ同じで、feedback も XML 形式を明示しない | fallback 後に model が有効な tool call 形式へ復帰しづらい |
| SG-30 | P2 | assistant が tool call と一緒に出した preamble content は次 request では消す | MVP は assistant content + tool_calls をそのまま履歴化する | "I'll create..." 等の古い前置きが再promptされ、no-tool/loop誘発要因になる |
| SG-31 | P1 | compaction は最新 user、直近 Read/Edit tool result、既存 summary の置換を保護する | MVP は文字数超過時に `messages[1]` から機械的に削る | Edit anchor / Read evidence / 最新依頼を失い、修復品質が落ちる |
| SG-32 | P2 | Bash safety は typed dangerous command classifier、timeout、process group、cancel flag、structured outcome を持つ | MVP Bash は簡易 substring block + `Command::output()` 中心 | 長時間 command、ESC中断、危険 command の取りこぼし、分類不能が残る |
| SG-33 | P1 | `Required final artifacts` block を抽出し、plan generation / step prompt / repair prompt / ultra phase prompt に継承する | MVP は eval `expected_artifacts` を scoring/postcheck で見るだけで、prompt契約として継承しない | phase/step をまたぐと最終成果物 path が薄まり、別名/別場所に作る |
| SG-34 | P1 | data profile は raw/input data を read-only とし、snapshot した protected data の削除/サイズ変更を phase 後 verify で検出する | MVP の data profile verify は常に pass | data-analysis/data-pipeline で入力データを壊しても検出できない |
| SG-35 | P1 | Read/Glob/Grep は workspace policy と ignore walker を使い、large Read / Grep 出力を要約・上限化する | MVP は `.git` 以外を単純再帰し、Read/Grep 出力上限や metadata filtering が弱い | node_modules/target/.anvil 等の探索、context膨張、内部metadata参照を招く |
| SG-36 | P2 | Edit は already-applied/no-op 検出、normalized-line fallback、token-anchor fallback を持つ | MVP Edit は exact anchor mismatch のみ | 軽微な空白差・既適用 edit で失敗し、repair/iteration を消費する |
| SG-37 | P1 | plan/ultra validation は duplicate id、id文字種、goal/instruction長、shell-command風 instruction、REPL command風 phase を拒否する | MVP lint は step数、空id、path、verify command中心 | planner不良が巨大/曖昧/危険な step として実行に流れる |
| SG-38 | P2 | Bash output は build/test summary、large `cat` 要約、出力 truncate を通して tool result を小さく診断向きにする | MVP Bash は stdout/stderr をほぼそのまま返す | build/test失敗の要点が埋もれ、context budgetを圧迫する |

## 今回の max_iterations との対応

`minimal loop reached max_iterations (12)` の直接パスは次です。

- `minimal-loop --prompt`
  - SG-03 / SG-06 により expected artifacts が loop に入らない
  - SG-01 がないため、仮に artifact が作成されても tool 実行直後に止まれない
  - SG-04 / SG-05 が弱いため、final/no-tool へ誘導できず tool 反復が継続する

- `plan-run`
  - SG-02 により source と同じ `expected_paths` 早期成功になっていない
  - SG-18 / SG-19 により step 単位の短い失敗・repair 誘導にならない
  - SG-21 / SG-26 により失敗情報が粗く、eval では `unclassified_process_failure` に落ちやすい

## 漏れ検証

### 完了条件レビュー追補

残SG根本対策計画 `workspace/mvp/eval/002/minimal_source_remaining_sg_completion_plan.md` の作成後、次のSGは「取りこぼしとして追跡可能」ではなく「実装完了まで閉じる対象」として扱う。

- `SG-07`, `SG-08`, `SG-11`
- `SG-14`, `SG-15`, `SG-16`, `SG-18`, `SG-19`, `SG-20`, `SG-21`
- `SG-25`, `SG-31`, `SG-32`, `SG-34`, `SG-35`, `SG-36`, `SG-38`

完了判定は残SG計画の SG別 Test Coverage Matrix と R8 の最終受け入れ条件に従う。つまり、対象SGは `defer:` や別計画移管では完了扱いにしない。

2026-06-25 実施結果:

- 対象SGは `mvp/anvilminimal/tests/safety_parity_traceability.rs` 上で `defer:` ではなく実テスト名へ置換済み。
- 追加ゲート `safety_traceability_has_no_defer_after_remaining_sg_completion` により、今後 `defer:` が残る場合は default Rust test suite で失敗する。
- 実装範囲は direct minimal safety に限定し、旧 heavy loop の汎用 mechanism ledger / no-progress detector は引き続き本棚卸し範囲外とする。

必須の判定軸:

- unit / integration / eval のいずれかではなく、SGごとに定義された全レイヤのテストが存在する。
- `mvp/anvilminimal/tests/safety_parity_traceability.rs` から対象SGの `defer:` が消える。
- SG別 fixture matrix で `unknown=0`、pass rate 100% を満たす。
- deterministic fake eval で `unclassified_process_failure=0`、`max_iterations=0`、required artifact postcheck pass 100% を満たす。

### 0. 過不足レビュー追補

再確認の結果、前回版には以下の過不足があった。

- 不足:
  - SG-27 `empty_response` feedback が漏れていた。
  - SG-28 action prompt 向け `missing_tool_call` feedback が漏れていた。
  - SG-29 XML fallback prompt mode の明示が漏れていた。
  - SG-30 tool-call assistant preamble の履歴除去が漏れていた。
  - SG-31 context compaction の evidence 保護が漏れていた。
  - SG-32 Bash 実行安全性の差分が漏れていた。
- 追加調査で見つかった不足:
  - SG-33 Required final artifacts の plan/repair/phase 継承が漏れていた。
  - SG-34 data profile の protected raw input 検証が漏れていた。
  - SG-35 Read/Glob/Grep の出力上限・ignore/policy filtering が漏れていた。
  - SG-36 Edit tool の fallback / already-applied 検出が漏れていた。
  - SG-37 plan/ultra validation の重複ID・長さ・自然言語性検査が漏れていた。
  - SG-38 Bash output shaping / test summary が漏れていた。
- 表現修正:
  - SG-10 は「MVPだけが tool error feedback を永続化する」という表現が強すぎた。source も tool error は tool result として残す。正しい差分は、no-tool 失敗応答の破棄・ephemeral feedback・feedback state の網羅性である。
- 過大評価ではないが条件付き:
  - SG-24 は MVP が final phase まで profile repair を遅延する設計になっているため、source parity 基準では不足。source parity の既定は phase ごとの停止であり、final repair 遅延は明示的 opt-in として扱うべき。
  - SG-25 は MVP が `src/app/page.tsx` / `layout.tsx` のように source より強く見ている点もある。過不足は profile contract 全体の同等性ではなく、Tailwind / rootDir / script weakening など source 側の未移植項目に限定する。

### 0.1 調査観点別の結果

今回の追加調査では、次の観点を設定して確認した。

| 観点 | 確認した主な source | 確認した主な MVP | 追加結果 |
|---|---|---|---|
| loop制御 | `minimal_loop/loop_run.rs`, `minimal_repl.rs` | `minimal_loop/loop_run.rs` | 既存 SG-01/02/27/28/29/30 で網羅。追加なし |
| feedback/履歴衛生 | `minimal_loop/feedback.rs`, `build_request_messages` | `minimal_loop/feedback.rs`, `prompt.rs`, `state.rs` | SG-10 の表現を修正。追加なし |
| 成果物契約 | `required_artifact_contract_prompt`, `extract_required_artifacts` | eval `expected_artifacts`, planner prompt | SG-33 を追加 |
| verify/repair | `verify.rs`, `repair.rs`, `plan_lint.rs` | `verify.rs`, `repair.rs`, `lint.rs` | SG-37 を追加。SG-19/21 は維持 |
| profile/ultra | `profile.rs`, `profiles/data.rs`, `profiles/nextjs.rs` | `profile.rs`, `profiles/data.rs`, `profiles/nextjs.rs` | SG-34 を追加。SG-24/25 は条件付き維持 |
| tool安全性 | `tools/read.rs`, `glob.rs`, `grep.rs`, `edit.rs`, `bash.rs`, `registry.rs` | `tools/read.rs`, `glob.rs`, `grep.rs`, `edit.rs`, `bash.rs` | SG-35/36/38 を追加。SG-32 は維持 |
| context/session | `minimal_loop/compact.rs`, prompt history filtering | `minimal_loop/compact.rs`, `state.rs` | SG-31 で網羅。追加なし |
| eval診断/テスト | source unit tests, MVP tests, eval scripts | MVP tests/eval scripts | SG-26 と不足テスト一覧で網羅。追加なし |

### 0.2 観点別レビュー結果

今回のレビューでは、次の観点で過不足を再確認した。

| 観点 | レビュー結果 | 反映 |
|---|---|---|
| SG網羅性 | 棚卸しと Phase 計画の SG-01〜SG-38 は件数・IDとも一致。追加SGは不要 | Phase 計画の Definition of Done に SG差分ゼロ確認を維持 |
| 対象範囲 | `src/tools/test_output.rs`、MVP provider/TUI/eval event 入口が対象範囲に明示されていなかった | 本棚卸しの対象範囲に追記 |
| source parity 境界 | 同一 `Glob` / `Read` / `Bash` 反復の汎用 no-progress detector は direct minimal ではなく旧 heavy loop 領域 | 引き続き本件のSGには含めず、別計画候補として明記 |
| Phase依存 | SG-24 は source parity 既定を「phaseごとに止める」と決めないと、MVP独自の final-only repair が残る | Phase 計画の Phase 6 に既定挙動と opt-in 条件を追加 |
| テスト可能性 | TUI/CLI 操作性は DoD にあるが、Phase 7 smoke に落ちていなかった | Phase 計画の Phase 7 に TUI/CLI smoke を追加 |
| eval観測性 | stop reason だけでは provider schema failure / loop convergence failure / tool-call shape の切り分けが弱い | Phase 計画の raw event 要件を拡張 |
| landing時テスト方針 | Phase 0 の red baseline は TDD中の確認であり、default test suite に失敗テストを残す完了条件ではない | 作業具体化で「red確認後、landing時はgreenまたは明示ignore」に修正 |
| 実行コマンド | Phase計画の eval script path と live provider 有効化条件が、実際のMVP構成とずれていた。さらに `--test live_provider -- --ignored` だけでは Ollama live tests も巻き込む | `mvp/anvilminimal/scripts/eval-run.py` に統一し、OpenAI/Gemini は `live_openai` / `live_gemini` filter付きで実行。Ollama はローカルLLM用の別コマンドに分離 |
| テスト支援コード配置 | fake client helper を production API に露出すると移植MVPのAPIが膨らむ | `tests/common/mod.rs` または `#[cfg(test)]` に限定し、`src/lib.rs` の不要な公開を避ける |
| artifact path安全性 | 成果物契約はあるが、path escape / metadata / symlink の negative test が弱い | SG-03/06/33 の完了条件に path extraction rejection を追加 |
| provider tool-call回帰 | OpenAI arguments string decode と Gemini function calling schema の既知障害を直接固定するテストが弱い | SG-26/29 の完了条件に provider shape regression を追加 |
| TUI実操作性 | slash smoke はあるが、ASCII banner、ESC interrupt、実際の `/ultra-plan-run --profile nextjs ...` 入力の固定が弱い | Phase 7 smoke の完了条件に追加 |
| eval合格基準 | smoke 実行だけでは成功率・分類品質・postcheck pass が曖昧 | deterministic fake suite の `unclassified_process_failure=0`、対象scenarioの `max_iterations=0`、postcheck pass 100% を追加 |
| Bash終了保証 | timeout/cancel はあるが、子プロセス残存なし・structured outcome の判定が弱い | SG-32 の完了条件に child cleanup と structured timeout/cancel result を追加 |

### 1. ファイル対応での検証

source minimal と MVP の対応を全ファイルで確認した。

| source | MVP | 判定 |
|---|---|---|
| `src/agent/minimal_loop/loop_run.rs` | `mvp/anvilminimal/src/minimal_loop/loop_run.rs` | loop 本体はあるが SG-01, SG-03, SG-04, SG-06, SG-07, SG-08, SG-09, SG-10, SG-27, SG-28, SG-29, SG-30 が欠落/弱化 |
| `src/agent/minimal_loop/feedback.rs` | `mvp/anvilminimal/src/minimal_loop/feedback.rs` | feedback 種別が大幅に少ない |
| `src/agent/minimal_loop/prompt.rs` | `mvp/anvilminimal/src/minimal_loop/prompt.rs` | system prompt の安全規則が縮退 |
| `src/agent/minimal_loop/compact.rs` | `mvp/anvilminimal/src/minimal_loop/compact.rs` | compaction が source の evidence 保護を持たない |
| `src/agent/minimal_repl.rs` | `mvp/anvilminimal/src/repl.rs`, `mvp/anvilminimal/src/main.rs` | `run_turn_with_early_success_paths` 相当がない |
| `src/agent/minimal_step_runner.rs` | `mvp/anvilminimal/src/planner/runner.rs` | plan/run はあるが generation retry, step caps, required final artifacts, repair policy, profiled phase prompt が弱化 |
| `src/agent/minimal_step_runner/verify.rs` | `mvp/anvilminimal/src/planner/verify.rs` | deterministic verify はあるが allowlist と failure aggregation が弱い |
| `src/agent/minimal_step_runner/repair.rs` | `mvp/anvilminimal/src/planner/repair.rs` | progress-aware repair がほぼ未移植 |
| `src/agent/minimal_step_runner/plan_lint.rs` | `mvp/anvilminimal/src/planner/lint.rs` | semantic lint が未移植 |
| `src/agent/minimal_step_runner/profile.rs` | `mvp/anvilminimal/src/planner/profile.rs` | profile verification はあるが snapshot/intent/runtime contract が弱い |
| `src/agent/minimal_step_runner/profiles/nextjs.rs` | `mvp/anvilminimal/src/planner/profiles/nextjs.rs` | MVP 独自の auto repair はあるが source contract の一部が未移植 |
| `src/agent/minimal_step_runner/profiles/data.rs` | `mvp/anvilminimal/src/planner/profiles/data.rs` | MVP data profile verify は stub で raw input 保護がない |
| `src/tools/read.rs`, `glob.rs`, `grep.rs` | `mvp/anvilminimal/src/tools/read.rs`, `glob.rs`, `grep.rs` | 出力上限・ignore walker・workspace policy の差分がある |
| `src/tools/edit.rs` | `mvp/anvilminimal/src/tools/edit.rs` | source の edit fallback / already-applied 検出がない |
| `src/tools/bash.rs`, `test_output.rs` | `mvp/anvilminimal/src/tools/bash.rs` | timeout/cancel/output shaping/test summary が弱い |
| `src/agent/minimal_llm.rs`, `planner_llm.rs` | `mvp/anvilminimal/src/providers/*` | provider固有のtool call shape / XML fallback / error kind は SG-29 と SG-26 の観測対象 |
| `src/agent/minimal_repl.rs` | `mvp/anvilminimal/src/repl.rs`, `mvp/anvilminimal/src/tui/*`, `mvp/anvilminimal/tests/tui_*` | CLI/TUI入口の操作性は Phase 7 smoke で確認対象 |

### 2. 実行フェーズでの検証

loop lifecycle ごとに source の安全装置を列挙し、MVP にあるか確認した。

| フェーズ | source の装置 | MVP 判定 |
|---|---|---|
| request build | tool mode native/xml、workspace policy、pending feedback | workspace policy と prompt 規則が弱い |
| model error | parser failure 時に XML fallback と session downgrade | MVP も fallback はあるが provider 実装側中心 |
| no-tool assistant | empty response、completion without write、requested artifact、missing import、planned action、missing tool | MVP は missing paths と簡易 no-tool progress のみ |
| tool execution | Write/Edit tracking、changed source tracking、tool error as feedback、Edit mismatch feedback | tracking と feedback が弱い |
| post-tool | `early_success_paths` 成立で成功停止 | MVP なし |
| step verify | expected paths + verify commands + expected_result | MVP は expected_result なし、単一 status |
| repair | progress-aware multi-turn bounded repair | MVP は 1 回の汎用 repair |
| ultra phase | profiled prompt + per-phase profile verification | MVP は phase prompt が薄く、non-final profile failure を許容 |

### 3. テスト観点での検証

source には次の安全装置テストが存在する。

- `early_success_paths_stop_after_tool_execution`
- `completion_without_write_feedback_then_write_then_complete`
- `requested_artifact_feedback_then_missing_write_then_complete`
- `requested_artifact_path_extraction_is_safe_and_explicit`
- `planned_action_after_feedback_gets_one_more_tool_prompt`
- `planned_action_after_write_gets_tool_prompt`
- `missing_relative_import_gets_repair_prompt_before_final`
- `repeated_planned_action_without_tool_returns_error`
- `verify_step_must_not_prepare_dependencies`
- Next.js build order / profile contract / repair exhausted report 系テスト

MVP 側には次のテストはあるが、source の安全装置全体は覆えていない。

- `fake_write_then_final`
- `missing_tool_argument_feedback_allows_retry`
- `dangerous_command_remains_hard_error`
- basic `verify_step` / `lint_step_plan`
- TUI smoke / eval script tests

不足している MVP テスト:

- tool 実行直後に required path が揃ったら stop する
- `--prompt` で prompt 内 path または eval expected artifacts を loop に渡す
- requested artifact path extraction が `../`, absolute path, symlink escape, `.anvil`, `target`, `node_modules` を拒否する
- no-tool completion without write を feedback して失敗 assistant message を破棄する
- requested artifact が未作成なら final を破棄する
- relative import 未解決を final 前に修復させる
- Edit anchor mismatch が hard error ではなく repair feedback になる
- empty assistant response を success とせず feedback して再試行する
- action prompt なのに no-tool の場合に `missing_tool_call` feedback を出す
- XML fallback 後の system prompt に XML tool call 例が入る
- assistant tool-call preamble が後続 request から除外される
- compaction 後も最新 user / Read evidence / Edit error が残る
- Bash の long-running command が timeout / cancel できる
- Bash timeout / cancel 後に子プロセスが残らず、tool result が `timeout` / `cancelled` の structured outcome になる
- Required final artifacts block が plan/step/repair/ultra phase prompt に保持される
- data profile で raw/input data の削除・改変を phase 後に検出する
- large Read / Grep / Bash output が要約・truncate される
- Glob/Grep が node_modules/target/.anvil 等の無関係/内部パスを拾いにくい
- Edit が already-applied/no-op/空白差 fallback を扱う
- duplicate step/phase id、長すぎる goal/instruction、shell command風 instruction を拒否する
- invalid planner output を single huge step にせず corrective retry する
- npm verify 前に setup step がない plan を lint で拒否する
- Next.js `npm run build` が package/entry 作成前に置かれた plan を拒否する
- repair で missing expected paths が減らない場合に専用 warning を出す
- max_iterations を eval failure kind `max_iterations` に分類する
- OpenAI Responses の `function_call.arguments` が string の場合に JSON object として decode される
- Gemini function calling request / response shape が現行 API の schema に合う
- TUI起動時の ASCII banner 表示、ESC interrupt、実際の `/ultra-plan-run --profile nextjs ...` 入力が PTY smoke で固定される

### 4. eval 症状との照合

直近 eval の代表症状:

- OpenAI: `Glob` / `Read` 反復
- Gemini: `Write` / `Bash` / `Read` 反復
- 直接 stderr: `minimal loop reached max_iterations (12)`
- eval classification: `unclassified_process_failure`

このうち source minimal からの取りこぼしで説明できるもの:

- 成果物が揃っても止まれない: SG-01 / SG-02
- 成果物契約が入っていない: SG-03 / SG-06
- final/no-tool に戻せない: SG-04 / SG-05
- repair が原因を狭めない: SG-19 / SG-20 / SG-21
- eval 分類が粗い: SG-26 と eval 側分類不足

source minimal でも説明できないもの:

- 同一 `Glob` / `Read` / `Bash` の汎用反復検出

これは direct minimal にはなく、旧 heavy loop 側の no-progress recovery から新規設計として持ち込む必要がある。

## 優先対応案

最小の横展開順序:

1. SG-01 / SG-02: `required_paths` を `early_success_paths` として post-tool にも判定する
2. SG-03 / SG-06 / SG-33: eval suite の `expected_artifacts` を `minimal-loop --prompt` と plan/repair/ultra phase prompt に渡す、または prompt path extraction を移植する
3. SG-04 / SG-05 / SG-08 / SG-12 / SG-27 / SG-28 / SG-29 / SG-30: source feedback state と prompt mode を MVP に移植し、failed assistant message を破棄する
4. SG-19 / SG-21: repair prompt に missing paths / changed paths / repeated edits / verify failures を入れる
5. SG-16 / SG-17: planner output corrective retry と semantic lint を追加する
6. SG-11 / SG-35 / SG-38: workspace policy と大出力抑制を入れ、eval中の探索迷走とcontext膨張を抑える
7. SG-26: eval failure classification に `max_iterations` と `verification_failed_after_max_iterations` を追加する

## 完了条件案

この棚卸しを実装へ進める場合、最低限の完了条件は次。

- unit test で source safety regression を MVP 側に追加する
- default test suite に意図的な失敗テストを残さない。red baseline は実装前確認として記録し、landing時はgreen化または理由付き `#[ignore]` にする
- `minimal-loop --prompt` の small/medium task で expected artifacts が loop 内 feedback に入る
- tool-only 反復でも required paths が揃った時点で max_iterations 前に停止する
- plan-run の step が `expected_paths` 充足で stop し、その後 deterministic verify に進む
- repair prompt に missing path と前回進捗が入る
- eval report に `max_iterations` が `unclassified_process_failure` ではなく専用分類で出る
- deterministic fake eval suite で `unclassified_process_failure=0`、Phase 1/4 対象scenarioの `max_iterations=0`、required artifacts postcheck pass rate 100% を満たす
- raw eval event に tool call名、arguments shape、provider error kind、stop reason が残る
- `anvilminimal --prompt` と TUI slash command の smoke が通る
- `anvilminimal --yes --context-budget 65536 --model qwen3.6:27b-coding-nvfp4 --planner-model gemini-3.5-flash --planner-provider gemini --provider ollama` と `/ultra-plan-run --profile nextjs ...` の代表操作が regression として固定されている
- OpenAI/Gemini live smoke は provider filter 付きで実行し、ローカル Ollama smoke を巻き込まない
- 旧 heavy loop の no-progress recovery は本件とは別 item として扱う
