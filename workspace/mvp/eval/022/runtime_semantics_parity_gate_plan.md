# Runtime Semantics Parity Gate Plan

作成日: 2026-06-29

## 0. 目的

021-4 までの監査と 021-5〜021-9 の修正で、MVP `anvilminimal` には runtime semantics の一部が戻った。

しかし直近評価では、`mvp-smoke` は前回比で小幅改善した一方、以下が残っている。

- `failure_kind` が空欄の失敗が残っている。
- OpenAI planner の verify command policy 違反が `step-plan` / `plan-run` / `ultra-plan-run` に波及している。
- verify repair が実装変更へ戻れないケースが残っている。
- Next.js ultra で dependency/setup/build lifecycle が成功まで運び切れないケースが残っている。
- 実装項目は完了しても、manual UAT 相当の成果物品質が安定していない。

本計画の目的は、`runtime semantics source parity` を単なる監査文書ではなく、**通らなければ完了扱いにしない gate** に変えることである。

ここでの gate とは、各移植 gap / runtime lifecycle stage について、以下が機械的に確認できない限り完了にしない仕組みを指す。

```text
source trace
 -> parity matrix
 -> MVP mapping
 -> deterministic fixture
 -> live provider probe where needed
 -> targeted eval
 -> anvildev comparison
 -> UAT-equivalent acceptance
 -> failure-kind coverage
```

## 0.1 レビュー結果と反映内容

本計画をレビューした結果、当初案は方向性としては正しいが、以下の不足があった。

1. gate の実行レベルが曖昧だった。
   - 毎回 live provider / anvildev 比較 / UAT 相当を要求すると重すぎる。
   - 一方で軽量 gate だけでは再び文書完了になる。
   - 反映: `local / network / comparative / release` の gate level を追加した。
2. anvildev 比較の「許容範囲」が曖昧だった。
   - 「同等」「改善」を人間判断にすると、過去と同じく都合よく解釈される。
   - 反映: warn / fail の比較閾値と、correct failure detection の例外条件を追加した。
3. trace の再現性と証跡形式が不足していた。
   - source trace / MVP trace が存在しても、commit、binary、provider、model、env、redaction がないと比較不能になる。
   - 反映: trace manifest と gate report の必須 schema を追加した。
4. `failure_kind` 空欄禁止の例外が曖昧だった。
   - dry-run / skipped / success まで禁止すると運用不能になる。
   - 反映: process failure / acceptance failure の空欄は禁止、success / diagnostic skipped は対象外と明記した。
5. UAT-equivalent acceptance がまだ弱かった。
   - browser oracle が使えない場合も pass できるように読めた。
   - 反映: release gate では browser readiness / interaction evidence を必須にし、使えない場合は partial までにした。
6. CI / preflight が「設計する」止まりだった。
   - gate を文書で終わらせないには、machine-readable report と pytest が必要である。
   - 反映: `parity_gate_report.json` と新規 pytest / eval-preflight 連携を完了条件に追加した。

この反映により、022 は単なる監査計画ではなく、段階導入可能な完了判定 gate の設計計画として扱う。

## 1. 背景と問題認識

### 1.1 これまで繰り返した失敗

過去の移植では、何度も以下を繰り返した。

- source 側のファイルや機能を確認した。
- MVP 側に似た部品を追加した。
- unit test や eval が一部通った。
- しかし手動 UAT では成果物品質が戻っていなかった。

これは、洗い出し方法が「機能一覧ベース」であり、「runtime semantics lifecycle ベース」ではなかったためである。

source の強さは、個々のファイルではなく以下の接続で成立している。

```text
task contract
  -> plan / phase contract
  -> prompt
  -> tool execution
  -> verify
  -> repair
  -> final acceptance
  -> diagnostics / handoff
```

この接続が同じ意味で戻っていることを gate しない限り、また「部品はあるが成果物品質が戻らない」状態になる。

### 1.2 直近評価から見た現状

直近 `mvp-smoke` 比較:

| 項目 | 前回 | 今回 | 差分 |
| --- | ---: | ---: | ---: |
| 全体成功率 | 37/48 | 38/48 | +1 |
| minimal-loop | 11/12 | 10/12 | -1 |
| step-plan | 11/12 | 11/12 | 0 |
| plan-run | 8/12 | 8/12 | 0 |
| ultra-plan-run | 7/12 | 9/12 | +2 |
| bridge failure | 6 | 4 | -2 |
| planning failure | 3 | 4 | +1 |

主なスコア変化:

| 指標 | 前回 | 今回 | 差分 |
| --- | ---: | ---: | ---: |
| execution_score | 77.1 | 79.2 | +2.1 |
| executable_plan_score | 67.2 | 69.3 | +2.1 |
| verify_strength_score | 69.9 | 73.5 | +3.6 |
| runtime_friction_score | 47.4 | 51.6 | +4.2 |
| phase_completion_score | 79.9 | 83.2 | +3.3 |
| ultra_runtime_health_score | 77.2 | 81.9 | +4.7 |
| execution_shape_readiness_score | 65.6 | 64.0 | -1.6 |
| execution_contract_adherence_score | 96.5 | 95.5 | -1.0 |

評価としては小幅改善だが、以下のため「runtime semantics parity が完了した」とは言えない。

- planning / bridge / runtime の失敗がまだ混在している。
- summary の `failure_kind` 空欄が残り、診断 gate として弱い。
- `anvildev` 比較を必須 gate として扱っていない。
- UAT-equivalent acceptance が完了判定に強制されていない。

## 2. Gate 化の原則

### 2.1 完了条件を文書から検査へ移す

今後は、移植 gap を「対応済み」とする条件を以下へ変更する。

| 従来 | Gate 化後 |
| --- | --- |
| source を確認した | source trace が保存されている |
| MVP に似た処理を追加した | MVP trace で同じ lifecycle stage が観測できる |
| unit test がある | positive / negative fixture が揃っている |
| eval が一部改善した | targeted eval と anvildev 比較がある |
| 失敗理由を人間が読める | `failure_kind` / lifecycle stage が summary に出る |
| UAT は別途確認 | UAT-equivalent acceptance が gate に入る |

### 2.2 Gate は main score ではない

Gate は `overall_score` を上げるための指標ではない。

Gate の役割は以下である。

- 移植漏れを完了扱いにしない。
- 意味論ズレを早期に検出する。
- eval false positive を抑制する。
- `anvildev` との差分を trace と event で説明可能にする。
- 修正単位を小さくし、rollback 可能にする。

したがって、score 改善と gate 通過は分けて扱う。

### 2.3 MVP の小さい API 境界を維持する

Gate 化は full `TaskContract` / full `RepairJob` / full actor loop を丸ごと移植する口実ではない。

各 source semantics は、MVP の既存境界へ写像する。

- `CompletionContract`
- `RuntimeAcceptanceReport`
- `RepairTarget`
- `StepPlan`
- `UltraPlan`
- `eval_events`
- `build_verifier`
- `dependency_setup`
- `planner::verify`

新しい抽象を追加する場合は、以下を gate に含める。

- 既存型では表現できない理由
- 重複 classifier を増やしていないこと
- mode/profile 横断の責任境界
- rollback plan

### 2.4 Gate level を分ける

全 gate を常に必須にすると開発速度が落ち、逆に軽量 gate だけでは parity 完了を誤判定する。

そのため、gate を以下の level に分ける。

| Level | 用途 | 必須内容 | API/network |
| --- | --- | --- | --- |
| local | 通常開発 / PR 前の軽量確認 | matrix completeness、fixture、failure taxonomy、dry eval contract | 不要 |
| network | provider / prompt-sensitive 修正 | local + provider probe + cloud targeted eval | 必要 |
| comparative | runtime semantics 修正 | network + anvildev 同条件比較 + trace diff | 必要 |
| release | manual UAT 相当の品質確認 | comparative + browser readiness / interaction evidence + UAT-equivalent acceptance | 必要 |

判定ルール:

- prompt / provider / repair prompt / tool-call parsing を変える修正は `network` 以上を必須にする。
- plan-run / ultra-plan-run / minimal-loop runtime semantics を変える修正は `comparative` 以上を必須にする。
- Next.js / TUI / manual UAT 失敗に関係する修正は `release` gate を最終確認にする。
- 通常 unit test では API key / network を必須にしない。

### 2.5 Machine-readable report を必須にする

gate は markdown だけでは完了扱いにしない。

各 gate 実行は、最低限以下の JSON report を出す。

```json
{
  "schema_version": "1",
  "gate_level": "comparative",
  "baseline_commit": "...",
  "source_trace_manifest": "...",
  "mvp_trace_manifest": "...",
  "matrix_path": "...",
  "summary_paths": [],
  "required_gate_ids": [],
  "passed_gate_ids": [],
  "failed_gate_ids": [],
  "partial_gate_ids": [],
  "failure_kind_blank_count": 0,
  "anvildev_comparison": {
    "success_rate_delta_pp": 0.0,
    "stage_regressions": []
  },
  "uat_equivalent": {
    "status": "pass",
    "evidence_paths": []
  }
}
```

`parity_gate_report.json` が存在しない修正は、runtime semantics parity 完了扱いにしない。

## 3. Gate 対象の lifecycle stage

以下の stage を parity gate の最小単位とする。

| ID | Stage | Gate で確認する責務 |
| --- | --- | --- |
| G-S01 | request understanding | intent / profile / required artifact / capability の抽出 |
| G-S02 | task contract | setup / scaffold / implementation / verify / acceptance の役割分離 |
| G-S03 | plan generation | prompt、schema、retry、lint、invalid fail-fast |
| G-S04 | ultra phase generation | phase の粒度、責任境界、profile rules |
| G-S05 | phase context continuity | prior phase outcome、failure、repair target の継続 |
| G-S06 | step prompt construction | overall goal、expected paths、verify、expected_result の伝達 |
| G-S07 | tool execution policy | tool args shape、workspace confinement、recoverable validation |
| G-S08 | deterministic verify | command policy、expected_result、dependency boundary |
| G-S09 | dependency/setup/build lifecycle | missing dependency、setup authority、install、build rerun |
| G-S10 | repair targeting | failure から repair target への写像、target follow-through |
| G-S11 | scaffold fallback | scaffold-only を completion にしない |
| G-S12 | final acceptance | artifact / build / capability / postcheck / browser evidence |
| G-S13 | recovery handoff | repair exhaustion 時の prompt 保存と suggested command |
| G-S14 | diagnostics | lifecycle stage、failure kind、event、summary への反映 |
| G-S15 | provider behavior | OpenAI/Gemini/Ollama の tool-call / prompt-sensitive 挙動 |
| G-S16 | TUI/manual run observability | 通常 TUI 実行で run events と停止理由が残る |

## 4. 必須成果物

`workspace/mvp/eval/022` に以下を作る。

| ファイル | 目的 |
| --- | --- |
| `runtime_semantics_parity_gate_plan.md` | 本計画 |
| `runtime_semantics_gate_matrix.md` | G-S01〜G-S16 の gate matrix |
| `source_mvp_trace_manifest.md` | source / MVP trace の対応表 |
| `parity_gate_fixture_plan.md` | positive / negative fixture 一覧 |
| `parity_gate_eval_protocol.md` | targeted eval / anvildev 比較 / provider probe 手順 |
| `parity_gate_failure_taxonomy.md` | failure kind 空欄禁止と lifecycle mapping |
| `parity_gate_uat_acceptance.md` | manual UAT 相当 acceptance の自動化方針 |
| `parity_gate_ci_contract.md` | pytest / cargo / eval preflight への組み込み方針 |
| `parity_gate_rollout_plan.md` | 段階導入、閾値、rollback 方針 |
| `parity_gate_trace_schema.md` | source / MVP trace の必須 metadata と redaction 方針 |
| `parity_gate_report_schema.md` | `parity_gate_report.json` の machine-readable schema |
| `parity_gate_review.md` | レビュー指摘、反映、defer 理由 |

## 5. Gate Matrix の必須列

`runtime_semantics_gate_matrix.md` は以下の列を必須にする。

| 列 | 内容 |
| --- | --- |
| gate_id | `G-S01` 形式 |
| lifecycle_stage | stage 名 |
| source_refs | source file / line / function |
| source_trace_path | anvildev 実行 trace |
| source_semantics | source の意味論 |
| mvp_refs | MVP file / line / type |
| mvp_trace_path | anvilminimal 実行 trace |
| mvp_semantics | MVP の現在挙動 |
| parity_status | `pass / partial / fail / intentionally_different` |
| difference_kind | `missing / simplified / semantic_drift / intentional / unknown` |
| affected_modes | minimal-loop / step-plan / plan-run / ultra-plan-run / TUI |
| affected_profiles | generic / nextjs / python / rust / docs / data |
| positive_fixture | 通るべき fixture |
| negative_fixture | 落ちるべき fixture |
| live_probe_required | provider probe 要否 |
| gate_level | `local / network / comparative / release` |
| targeted_eval | 対象 suite / scenario / mode |
| anvildev_comparison | 比較 run path |
| comparison_threshold | warn / fail 閾値 |
| uat_acceptance | UAT-equivalent acceptance |
| failure_kind_coverage | summary に具体 failure kind が出るか |
| trace_schema_version | trace schema version |
| evidence_hash | trace / fixture / eval 証跡の hash または stable id |
| gate_report_field | `parity_gate_report.json` の対応 field |
| complexity_impact | `none / low / medium / high` |
| rollback_plan | rollback 方法 |
| owner_phase | 対応 phase |
| status | `todo / in_progress / gated / complete` |

## 6. Gate 判定ルール

### 6.1 Pass

以下をすべて満たす場合のみ `pass`。

- source trace がある。
- MVP trace がある。
- lifecycle stage が同じ意味で観測できる。
- positive fixture が通る。
- negative fixture が落ちる。
- eval summary に failure kind が空欄で残らない。
- 必要な場合、provider probe が通る。
- `anvildev` 比較で差分が許容範囲内、または intentional difference として説明されている。
- UAT-equivalent acceptance が通る。
- `parity_gate_report.json` に gate 結果が machine-readable に出ている。
- required gate level の全必須項目が満たされている。

anvildev 比較の初期閾値:

- MVP 成功率が anvildev を 5pt 以上下回る場合は `warn`。
- MVP 成功率が anvildev を 10pt 以上下回る場合は `fail`。
- 同一 lifecycle stage の failure が anvildev より増えた場合は `warn`。
- `correct failure detection` による成功率低下を主張する場合は、false positive が減った証跡、failure kind、acceptance evidence を必須にする。
- `anvildev` の方が明らかに false positive を許している場合は、success rate だけで fail にしない。ただし、その判断は `intentionally_different` として gate matrix に記録する。

### 6.2 Partial

以下のいずれかがある場合は `partial`。

- source / MVP trace の片方が不足している。
- fixture はあるが positive / negative の片方だけ。
- eval は通るが UAT-equivalent acceptance が未確認。
- events には診断があるが summary に failure kind が出ない。
- `anvildev` 比較が未実施。
- machine-readable report がない。
- required gate level より低い確認しかしていない。

### 6.3 Fail

以下は `fail`。

- source semantics が MVP で未対応。
- scaffold-only / setup-only / style-only が success になる。
- capability が成果物に反映されないのに success になる。
- repair exhaustion が handoff なしで終了する。
- failure kind が空欄のまま gate を通ろうとする。
- provider-specific 挙動に依存する修正なのに live probe がない。
- process failure / acceptance failure の `failure_kind` が空欄である。
- release gate で browser readiness / interaction evidence がないのに full pass 扱いする。

`failure_kind` 空欄禁止の対象外:

- success row
- dry-run row
- diagnostic skipped row
- user cancellation / explicit abort として分類済みの row

上記以外の `rc != 0`、`process_success=false`、`acceptance_success=false` は、具体 failure kind を必須にする。

### 6.4 Intentionally Different

source と違う挙動を採用する場合は、以下を必須にする。

- source と違う理由
- MVP の小さい API 境界との整合性
- リスク
- 代替 gate
- anvildev 比較で差分をどう読むか
- manual UAT への影響

## 7. 実施 Phase

### Phase 0: Baseline 固定

目的:

- 現在の 021-9 後状態を baseline として固定する。

作業:

- 最新 commit SHA を記録する。
- 直近評価 run を記録する。
  - `/private/tmp/anvilminimal-eval-0219-mvp-smoke`
  - `/private/tmp/anvilminimal-eval-0219-provider-smoke`
- 前回比較 run を記録する。
  - `/private/tmp/anvilminimal-0213-postcommit-smoke`
  - `/private/tmp/anvilminimal-eval-rollback-provider-smoke`
- 直近の失敗10件を lifecycle stage へ仮分類する。
- `parity_gate_trace_schema.md` と `parity_gate_report_schema.md` の初版を作る。

受入条件:

- `source_mvp_trace_manifest.md` に baseline run path が記録されている。
- baseline 成功率と主要スコアが記録されている。
- failure kind 空欄が既知問題として登録されている。
- baseline commit、binary path、provider pair、model、suite、modes、run root が trace manifest に記録されている。
- secret / prompt payload / API key を trace artifact に保存しない redaction 方針がある。

### Phase 1: Source Trace Capture

目的:

- anvildev の runtime semantics をコード読解ではなく trace として保存する。

対象 source:

- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/minimal_step_runner.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/minimal_step_runner/repair.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/minimal_step_runner/profile.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/task_contract.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/task_contract_core.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/task_contract_completion_policy.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/verifier.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/verifier_driver.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/verifier_repair_targeting.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/repair_lifecycle.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/scaffold_pipeline.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/project_probe.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/project_verifier.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/ollama/client.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/ollama/xml_fallback.rs`

作業:

- representative tasks を定義する。
  - simple file write
  - docs content
  - JS deterministic test
  - Python unittest
  - Rust cargo test
  - Next.js minimal app
  - Next.js interactive game
  - repair exhaustion
  - dependency missing
- `anvildev --engine minimal` で trace を保存する。
- 各 trace から lifecycle event sequence を抽出する。
- trace には source commit、binary path、command、cwd、state dir、provider pair、model pair、env redaction status を保存する。
- trace artifact は再実行できる command と、比較に使う normalized event sequence を分けて保存する。

受入条件:

- 各 representative task に source trace がある。
- G-S01〜G-S16 の source semantics 欄を埋められる。
- source trace がない stage は `unknown` として明示される。
- trace に secret-like value が含まれていない。
- source trace が provider 揺れで不安定な場合、同じ task の複数 run または deterministic fixture で補強する。

### Phase 2: MVP Trace Capture

目的:

- 同じ representative tasks を MVP `anvilminimal` で実行し、source trace と比較可能にする。

作業:

- `anvilminimal` で同じ prompt / profile / provider pair を使う。
- `ANVIL_EVAL_EVENTS` または通常 `.anvil/runs/<run-id>/events.jsonl` を保存する。
- plan artifact、repair prompt、completion contract、postcheck result を trace manifest に紐付ける。
- source trace と同じ normalized event sequence を生成する。
- missing event は MVP diagnostics gap として扱い、手動推測だけで matrix を埋めない。

受入条件:

- 各 representative task に MVP trace がある。
- trace 欠落がある場合、MVP diagnostics gap として登録する。
- TUI 経由実行でも少なくとも one run の events が残る。
- source trace と MVP trace の stage diff が自動または半自動で出せる。

### Phase 3: Gate Matrix 作成

目的:

- G-S01〜G-S16 を gate matrix として固定する。

作業:

- `runtime_semantics_gate_matrix.md` を作成する。
- `source_refs` / `mvp_refs` / `trace_path` / `fixtures` / `eval` / `uat_acceptance` を列挙する。
- `pass / partial / fail / intentionally_different` を仮判定する。

受入条件:

- G-S01〜G-S16 の行がすべて存在する。
- `unknown` がある場合は root cause と調査計画がある。
- `partial` / `fail` のまま完了扱いにしない。

### Phase 4: Fixture Gate 設計

目的:

- source parity を deterministic fixture で落とせるようにする。

作業:

- `parity_gate_fixture_plan.md` を作成する。
- stage ごとに positive / negative fixture を定義する。
- fixture が scenario 固有文言に依存していないことを確認する。

必須 negative fixture:

- scaffold-only app is not success
- setup-only is not success
- docs-only output does not satisfy app/game task
- missing test artifact blocks deterministic-test task
- shell control syntax verify is classified
- dependency missing without setup authority is blocked
- repair no-change is classified and handoff is saved
- final artifact exists but required capability missing is not success

受入条件:

- 各 lifecycle stage に少なくとも 1 positive / 1 negative fixture がある。
- fixture 名に特定 eval scenario の id を使わない。
- `cargo test` または `pytest` で実行できる形に落とす方針がある。

### Phase 5: Failure Taxonomy Gate

目的:

- `failure_kind` 空欄を gate failure にする。

作業:

- `parity_gate_failure_taxonomy.md` を作成する。
- lifecycle stage と failure kind を対応づける。
- events には出ているが summary に転記されていない failure を洗い出す。

最低限追加・確認する failure kind:

- `planner_verify_command_policy_error`
- `phase_scaffold_error`
- `step_verify_repair_no_change`
- `plan_final_contract_failure`
- `dependency_setup_missing`
- `repair_target_not_followed`
- `capability_evidence_missing`
- `source_semantic_acceptance_failure`
- `provider_tool_args_shape_error`
- `provider_function_call_schema_error`

受入条件:

- 最新 smoke の失敗で `failure_kind` 空欄が 0 になる見込みが示されている。
- lifecycle stage と failure layer が矛盾しない。
- eval main score と diagnostic score を混同しない。

### Phase 6: Eval Protocol Gate

目的:

- targeted eval と anvildev 比較を gate 化する。

作業:

- `parity_gate_eval_protocol.md` を作成する。
- stage ごとに最小 eval を定義する。
- `mvp` と `anvildev --engine minimal` を同じ条件で比較する。

最小 eval set:

- provider smoke
- mvp-smoke full modes
- targeted Next.js ultra
- targeted plan-run bridge
- targeted minimal-loop repair
- targeted failure-taxonomy dry fixture

受入条件:

- 各 gate stage に eval command がある。
- local LLM 未使用時は cloud 並列 5 を使う。
- provider probe 必須条件が明記されている。
- anvildev 比較なしに parity complete としない。
- MVP / anvildev の比較閾値が `comparison_threshold` として matrix に入っている。
- 比較結果が `parity_gate_report.json` に出る。
- `correct failure detection` と単純な regression を区別するため、acceptance false positive / failure kind / UAT evidence を併記する。

### Phase 7: UAT-equivalent Acceptance Gate

目的:

- manual UAT 相当を自動 gate に近づける。

作業:

- `parity_gate_uat_acceptance.md` を作成する。
- Next.js game / CLI / docs / library / data の acceptance を定義する。

Next.js interactive game の最低 gate:

- `package.json` がある。
- `src/app/page.tsx` がある。
- `src/app/layout.tsx` がある。
- `npm install --ignore-scripts` が通る。
- `npm run build` が通る。
- `npm run dev` が 3011 で HTTP 200 を返す。
- source semantic evidence がある。
  - stateful interaction
  - player control
  - adversary/challenge
  - projectile or collision
  - score/progression
  - start/restart/failure flow
- static title-only は failure。

受入条件:

- build だけでは acceptance success にならない。
- release gate では browser readiness / interaction evidence を必須にする。
- browser oracle が使えない環境では full pass にせず、source semantic gate の partial までとする。
- source semantic gate は scenario id や Space Invaders 固有語ではなく、interactive game capability contract に基づく。
- UAT-equivalent acceptance の証跡を `parity_gate_report.json` に保存する。

### Phase 8: CI / Preflight Gate 設計

目的:

- parity gate を手作業ではなく test / script へ組み込む。

作業:

- `parity_gate_ci_contract.md` を作成する。
- `parity_gate_report.json` を検証する pytest を設計する。
- 新規 pytest を設計する。
  - matrix completeness
  - source trace presence
  - fixture coverage
  - failure kind coverage
  - eval summary contract
  - provider probe metadata
  - gate report schema
  - anvildev comparison threshold
  - UAT-equivalent evidence presence
- `scripts/eval-preflight.py` への gate warning / failure 方針を決める。

受入条件:

- `pytest mvp/anvilminimal/tests/eval` へ追加する test 名が明記されている。
- gate failure と warning の境界がある。
- 通常 test は API key / network を必須にしない。
- local gate は network なしで通せる。
- network / comparative / release gate は明示 opt-in とし、実行ログを残す。

### Phase 9: Rollout / Threshold Gate

目的:

- 一度に厳格化して開発不能にしない。

作業:

- `parity_gate_rollout_plan.md` を作成する。
- gate を段階導入する。

導入順:

1. document completeness gate
2. fixture coverage gate
3. failure kind non-empty gate
4. provider probe metadata gate
5. targeted eval comparison gate
6. UAT-equivalent acceptance gate
7. anvildev parity threshold gate

受入条件:

- 各 gate に `warn -> fail` へ切り替える条件がある。
- gate 追加で既存 eval の意味が変わる場合は baseline を更新する。
- rollback 方法がある。

### Phase 10: Review / Freeze

目的:

- gate 計画自体をレビューし、抜けを潰す。

レビュー観点:

- 実行意味論ベースになっているか。
- source trace なしで完了扱いにしていないか。
- MVP の小さい API 境界を壊していないか。
- eval 過適応になっていないか。
- UAT-equivalent acceptance が成果物品質を見ているか。
- provider 固有挙動を fixture だけで済ませていないか。
- failure kind 空欄を許していないか。
- `anvildev` 比較が必須になっているか。

受入条件:

- レビュー指摘を `parity_gate_review.md` に残す。
- 指摘ごとに対応または明示 defer がある。
- Phase 0〜9 の成果物が揃っている。

## 8. テスト計画

### 8.1 Unit / Fixture

- `cargo test --manifest-path mvp/anvilminimal/Cargo.toml`
- `python3 -m pytest mvp/anvilminimal/tests/eval`
- 新規:
  - `test_runtime_semantics_gate_matrix.py`
  - `test_parity_gate_fixture_coverage.py`
  - `test_failure_kind_coverage_gate.py`
  - `test_source_mvp_trace_manifest.py`
  - `test_uat_acceptance_contract.py`
  - `test_parity_gate_report_schema.py`
  - `test_parity_gate_comparison_threshold.py`
  - `test_trace_redaction_contract.py`

### 8.2 Provider Probe

通常 test では skip / fixture only。

live probe:

```bash
ANVIL_PROVIDER_PROBE=1 \
ANVIL_PROVIDER_PROBE_OUT=/private/tmp/anvil-provider-probe-gate.jsonl \
cargo test --manifest-path mvp/anvilminimal/Cargo.toml --test live_provider provider_probe -- --nocapture
```

受入条件:

- key がなければ skip。
- key があれば OpenAI / Gemini live probe が通る。
- JSONL が 1 行 1 event として壊れない。

### 8.3 Targeted Eval

MVP:

```bash
python3 scripts/eval-run.py \
  --suite eval/suites/mvp-smoke.yaml \
  --model-profile speed-cloud-5x \
  --modes minimal-loop,step-plan,plan-run,ultra-plan-run \
  --runs 1 \
  --parallel 5 \
  --provider-limit 5 \
  --binary target/release/anvilminimal \
  --binary-kind anvilminimal
```

anvildev:

```bash
python3 scripts/eval-run.py \
  --suite eval/suites/mvp-smoke.yaml \
  --model-profile speed-cloud-5x \
  --modes minimal-loop,step-plan,plan-run,ultra-plan-run \
  --runs 1 \
  --parallel 5 \
  --provider-limit 5 \
  --binary anvildev \
  --binary-kind anvildev
```

受入条件:

- 同じ suite / model profile / modes で比較する。
- 成功率だけでなく stage 別 failure と UAT-equivalent acceptance を比較する。
- failure kind 空欄が残る場合は gate failure。
- 比較結果は `parity_gate_report.json` に保存する。
- MVP が anvildev を 10pt 以上下回る場合は fail。ただし false positive 抑制による低下は evidence 付きで intentional difference として扱う。

### 8.4 UAT-equivalent

代表 UAT:

```bash
anvilminimal --yes --context-budget 65536 \
  --model qwen3.6:27b-coding-nvfp4 \
  --planner-model gemini-3.5-flash \
  --planner-provider gemini \
  --provider ollama
```

TUI:

```text
/ultra-plan-run --profile nextjs あなたが考える最高に面白くかっこいいスペースインベーダーゲームを3011ポートで起動可能なnext.jsアプリとして開発してください。
```

受入条件:

- 途中停止時に `.anvil/runs/<run-id>/events.jsonl` が残る。
- failure stage と suggested recovery command が出る。
- 成功時は build / launch / capability evidence を満たす。
- static shell / title-only は success にならない。

## 9. リスクと対策

| リスク | 内容 | 対策 |
| --- | --- | --- |
| gate が重すぎる | 毎回 full eval / anvildev 比較が必要になる | lightweight / targeted / release gate を分ける |
| eval 過適応 | fixture 文言に合わせた実装になる | lifecycle stage と capability evidence で定義する |
| 複雑性増大 | gate 用 classifier が増殖する | 既存 failure taxonomy と event schema に統合する |
| provider 揺れ | live API 結果が不安定 | probe は通常 runtime 判定に使わず、prompt-sensitive fix の検証に限定 |
| anvildev との差分解釈 | source と MVP の意図的差分まで fail になる | `intentionally_different` を formal status にする |
| UAT の自動化不足 | 手動確認が残る | source semantic / browser readiness / event evidence へ分解する |
| false positive | build 成功だけで受入成功になる | capability acceptance と plan output adherence を必須にする |
| trace の再現性不足 | trace はあるが commit / command / env がなく比較できない | trace schema と redaction contract を必須化する |
| failure_kind 例外の拡大 | 空欄禁止の例外が増え、診断 gate が形骸化する | 例外は success / dry-run / diagnostic skipped / explicit abort に限定する |
| browser oracle 不在 | release gate でも source semantic だけで pass してしまう | browser 不在時は partial までとし、full pass にはしない |

## 10. 完了条件

本 022 の完了条件:

- `runtime_semantics_gate_matrix.md` が作成され、G-S01〜G-S16 が全て埋まっている。
- `source_mvp_trace_manifest.md` に source / MVP trace が記録されている。
- `parity_gate_fixture_plan.md` に positive / negative fixture が定義されている。
- `parity_gate_failure_taxonomy.md` で `failure_kind` 空欄禁止ルールが定義されている。
- `parity_gate_eval_protocol.md` に MVP / anvildev 比較コマンドがある。
- `parity_gate_uat_acceptance.md` に manual UAT 相当 acceptance がある。
- `parity_gate_ci_contract.md` に pytest / cargo / eval-preflight 組み込み方針がある。
- `parity_gate_rollout_plan.md` に段階導入と rollback plan がある。
- `parity_gate_trace_schema.md` に trace metadata / redaction / normalized event sequence が定義されている。
- `parity_gate_report_schema.md` に `parity_gate_report.json` の必須 field が定義されている。
- `parity_gate_report.json` を検証する pytest の設計がある。
- anvildev 比較閾値が定義されている。
- release gate における browser readiness / interaction evidence の扱いが定義されている。
- 計画レビューで、過去の「機能一覧ベースの洗い出し」へ戻っていないことが確認されている。

## 11. 次アクション

1. 本計画をレビューする。
2. Phase 0〜3 を先に実施し、gate matrix を固定する。
3. その後、Phase 4〜8 で fixture / eval / CI gate を具体化する。
4. Phase 9〜10 で導入順とレビューを確定する。

重要なのは、次の修正へ進む前に「何をもって source parity 完了とするか」を gate として固定することである。
