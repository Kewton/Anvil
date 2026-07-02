# Runtime Semantics Parity Gate: Implementation Order

作成日: 2026-06-29

## 0. 目的

この README は、`workspace/mvp/eval/022` で洗い出した runtime semantics parity gate を、実装・検証の順序へ落とし込む。

重要なのは、再び「部品を移植したので完了」としないこと。各順序では、source refs / MVP refs / fixture / eval / UAT / report を揃え、`parity_gate_report.json` で pass/partial/fail を機械的に確認する。

## 0.1 レビュー結果と反映内容

本 README をレビューした結果、初版は「実装順序と参照ファイル」は整理できていたが、以下が不足していた。

| 指摘 | 問題 | 反映 |
| --- | --- | --- |
| 順序間の依存関係が弱い | 022-3 以降を先に進めると、また診断不足のまま runtime を直す危険がある | 022-1/022-2 を hard prerequisite として明記 |
| 1回の修正範囲が膨らむリスク | 021 で起きたように、複数 semantics をまとめて戻すと劣化原因を切り分けにくい | 各 order は原則 1 lifecycle group のみ、例外時は rollback plan 必須とした |
| comparative/release gate の具体コマンドが薄い | anvildev 同条件比較が実施者依存になる | MVP / anvildev / provider smoke の標準コマンドを追加 |
| 完了条件が各 order 内に閉じすぎている | matrix/report/trace の更新漏れが起きる | 共通完了条件として matrix/report/trace 更新を必須化 |
| source trace 未取得時の扱いが曖昧 | コード参照だけで pass と誤判定される可能性がある | source trace がない gate は `partial` 以下に固定するルールを追記 |
| UAT と eval の関係が弱い | eval 改善後も manual UAT の品質が戻らない可能性が残る | release gate では browser/interaction evidence なしに full pass 不可と再明記 |

## 1. 対応順序の全体像

| Order | 主対象 gate | 目的 | 優先理由 |
| --- | --- | --- | --- |
| 022-1 | G-S14, G-S08 | failure taxonomy / report / preflight を実装し、空 failure_kind をなくす | 診断が粗いまま runtime を直すと、劣化と正しい失敗検知を区別できない |
| 022-2 | G-S01〜G-S16 | source/MVP trace capture と normalized event diff を実装する | source parity の判断をコード読解から trace 比較へ移す |
| 022-3 | G-S08, G-S03 | deterministic verify / planner verify policy を source 寄りに戻す | step-plan/plan-run/ultra-plan-run の planning failure が横展開している |
| 022-4 | G-S09 | dependency/setup/build lifecycle を source 寄りに戻す | Next.js などで good plan が build/setup 成功まで運ばれない |
| 022-5 | G-S10, G-S13 | repair targeting / follow-through / handoff を戻す | repair no-change、target not followed、bounded repair exhausted を正しく扱う |
| 022-6 | G-S01, G-S02, G-S11 | TaskContract-lite obligation tracking / scaffold continuation を固める | setup-only/scaffold-only/style-only false positive を防ぐ |
| 022-7 | G-S12, G-S16 | final acceptance / UAT-equivalent / TUI observability を強制する | build 成功だけの false positive と manual UAT 不安定を潰す |
| 022-8 | G-S07, G-S15 | provider probe gate と tool args recovery を gate に接続する | OpenAI/Gemini/Ollama の prompt-sensitive 変更を fake fixture だけで済ませない |
| 022-9 | all | comparative/release gate を運用化し、anvildev 閾値比較を固定する | parity complete を人間判断ではなく gate report で判定する |

## 1.1 実施順序のゲート

実装は以下の制約で進める。

| Rule | 内容 |
| --- | --- |
| hard prerequisite | 022-1 と 022-2 が完了するまで、022-3 以降を完了扱いにしない |
| one lifecycle group | 1つの PR/作業単位で複数 order を同時に大きく変更しない |
| source trace first | source code refs だけでは pass 不可。source/MVP trace の片方が欠ける gate は `partial` 以下 |
| report update | 各 order 完了時に `runtime_semantics_gate_matrix.md` と `parity_gate_report.json` を更新する |
| regression split | 成功率低下は `correct failure detection` と単純 regression に分けて report に残す |
| no false positive relaxation | 成功率改善のために final acceptance / failure taxonomy を甘くしない |

## 1.2 共通完了条件

各 022-x の完了条件は、個別の受入条件に加えて以下を満たす必要がある。

- 対象 gate の source refs / MVP refs / trace / fixture / eval / UAT の不足が matrix に反映されている。
- `parity_gate_report.json` の `partial_gate_ids` / `failed_gate_ids` / `passed_gate_ids` が更新されている。
- 新規または変更した failure は blank `failure_kind` にならない。
- `pytest mvp/anvilminimal/tests/eval` と `cargo test --manifest-path mvp/anvilminimal/Cargo.toml` の結果を記録する。
- comparative gate 以上では MVP と `anvildev --engine minimal` の同条件比較を実施する。
- release gate では browser/interaction evidence がない限り full pass にしない。
- 変更範囲と rollback plan を作業結果に残す。

## 2. 022-1: Failure Taxonomy / Report / Preflight

### 対象 gate

- G-S14 diagnostics
- G-S08 deterministic verify

### 参照ファイル

Source:

- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/verifier.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/verifier_driver.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/verifier_repair_targeting.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/repair_lifecycle.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/emit_verifier_events.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/minimal_step_runner/repair.rs`

MVP:

- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/eval_events.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/verify.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/runner.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/scripts/eval_lib/failure_classification.py`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/scripts/eval_lib/runtime_scoring.py`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/scripts/eval-run.py`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/tests/eval`

022 artifacts:

- `/Users/maenokota/share/work/github_kewton/Anvil-develop/workspace/mvp/eval/022/parity_gate_failure_taxonomy.md`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/workspace/mvp/eval/022/parity_gate_report_schema.md`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/workspace/mvp/eval/022/parity_gate_ci_contract.md`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/workspace/mvp/eval/022/parity_gate_report.json`

### Codex 指示

```text
workspace/mvp/eval/022/README.md の 022-1 に従って、failure taxonomy / report / preflight gate を実装してください。

目的:
- process failure / acceptance failure の failure_kind 空欄を gate failure にする。
- events に出ている lifecycle stage / failure kind を eval summary へ落とす。
- parity_gate_report.json の schema validation と gate partition validation を pytest で検査する。
- deterministic verify / planner verify policy の失敗を具体 failure kind に分類する。

必ず参照する source:
- /Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/verifier.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/verifier_driver.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/verifier_repair_targeting.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/repair_lifecycle.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/emit_verifier_events.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/minimal_step_runner/repair.rs

主な編集候補:
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/eval_events.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/verify.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/runner.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/scripts/eval_lib/failure_classification.py
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/scripts/eval_lib/runtime_scoring.py
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/scripts/eval-run.py
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/tests/eval

制約:
- success / dry-run / explicit user abort / provider probe skip は blank failure_kind 禁止対象外にしてよい。
- rc != 0、process_success=false、acceptance_success=false、postcheck_failure は具体 failure_kind 必須にする。
- main success score と diagnostic score を混同しない。
- unclassified_process_failure を完全禁止せず、raw stderr/events 付きの最終 fallback に限定する。

受入条件:
- pytest mvp/anvilminimal/tests/eval が通る。
- cargo test --manifest-path mvp/anvilminimal/Cargo.toml が通る。
- 最新 smoke/provider smoke の失敗で failure_kind 空欄が 0 になる、または残る場合は gate failure として report に出る。
- parity_gate_report.json を検証する pytest が追加される。
- G-S14 の status を fail から partial/pass へ動かせる根拠が workspace/mvp/eval/022 に残る。
```

## 3. 022-2: Source/MVP Trace Capture And Normalized Diff

### 対象 gate

- G-S01〜G-S16 全体
- 特に G-S05, G-S06, G-S16

### 参照ファイル

Source:

- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/minimal_step_runner.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/minimal_step_runner/profile.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/minimal_step_runner/repair.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/task_contract.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/task_contract_core.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/task_contract_completion_policy.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/verifier.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/scaffold_pipeline.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/project_probe.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/project_verifier.rs`

MVP:

- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/eval_events.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/runner.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/minimal_loop/loop_run.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/minimal_loop/evidence.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/scripts/eval-run.py`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/tests/eval`

022 artifacts:

- `/Users/maenokota/share/work/github_kewton/Anvil-develop/workspace/mvp/eval/022/parity_gate_trace_schema.md`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/workspace/mvp/eval/022/source_mvp_trace_manifest.md`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/workspace/mvp/eval/022/runtime_semantics_gate_matrix.md`

### Codex 指示

```text
workspace/mvp/eval/022/README.md の 022-2 に従って、source/MVP trace capture と normalized event diff を実装してください。

目的:
- runtime semantics parity をコード読解ではなく trace で比較できるようにする。
- anvildev --engine minimal と anvilminimal の同条件 run を trace manifest に登録する。
- events を normalized lifecycle stage に写像し、G-S01〜G-S16 の差分を出せるようにする。
- TUI/manual run でも最低限の run events と停止理由を残す。

必ず参照する source:
- /Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/minimal_step_runner.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/minimal_step_runner/profile.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/minimal_step_runner/repair.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/task_contract.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/task_contract_core.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/task_contract_completion_policy.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/verifier.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/scaffold_pipeline.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/project_probe.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/project_verifier.rs

主な編集候補:
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/eval_events.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/runner.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/minimal_loop/loop_run.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/scripts/eval-run.py
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/tests/eval

制約:
- raw prompt/API key/full provider response を trace に保存しない。
- normalized event は source/MVP の event 名を無理に一致させず、lifecycle stage に写像する。
- trace が欠けている gate を pass にしない。
- manual TUI trace は release gate の証跡として扱う。

受入条件:
- source_mvp_trace_manifest.md を更新できる trace writer または補助 script がある。
- normalized event sequence の fixture test がある。
- anvildev と MVP の同条件 eval 結果を trace manifest/report に追加できる。
- TUI/manual run の silent exit が gate failure として検出される。
- pytest mvp/anvilminimal/tests/eval が通る。
```

## 4. 022-3: Deterministic Verify / Planner Verify Policy

### 対象 gate

- G-S08 deterministic verify
- G-S03 plan generation

### 参照ファイル

Source:

- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/verifier_command_policy.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/verifier.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/verifier_driver.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/minimal_step_runner.rs`

MVP:

- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/verify.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/lint.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/step_plan.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/runner.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/scripts/eval_lib/failure_classification.py`

### Codex 指示

```text
workspace/mvp/eval/022/README.md の 022-3 に従って、deterministic verify / planner verify policy を source semantics に寄せて修正してください。

目的:
- StepPlan/UltraPlan の verify command policy を source の verifier_command_policy と比較し、不足を埋める。
- shell control syntax、dependency manifest/setup before build、expected_result 不足を具体 failure kind にする。
- planner が荒い verify を出した場合、必要なら schema/lint retry で修復し、修復不能なら fail-fast する。

必ず参照する source:
- /Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/verifier_command_policy.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/verifier.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/verifier_driver.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/minimal_step_runner.rs

主な編集候補:
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/verify.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/lint.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/step_plan.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/runner.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/scripts/eval_lib/failure_classification.py

制約:
- shell control syntax を許可する方向に緩めない。
- provider/prompt に過適応せず、verify command の一般 policy として実装する。
- build/test verify を package manifest/setup authority より前に置く plan を成功扱いしない。

受入条件:
- verify command policy の positive/negative fixture がある。
- step-plan / plan-run / ultra-plan-run の planning failure が具体 failure kind になる。
- pytest mvp/anvilminimal/tests/eval が通る。
- cargo test --manifest-path mvp/anvilminimal/Cargo.toml が通る。
- eval smoke で G-S08 の fail が改善したか、改善しない場合は理由が report に出る。
```

## 5. 022-4: Dependency / Setup / Build Lifecycle

### 対象 gate

- G-S09 dependency/setup/build lifecycle

### 参照ファイル

Source:

- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/node_request_helpers.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/node_runner_manifest.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/project_probe.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/project_verifier.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/verifier_command_policy.rs`

MVP:

- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/minimal_loop/dependency_setup.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/minimal_loop/build_verifier.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/minimal_loop/repair_target.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/minimal_loop/loop_run.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/runner.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/verify.rs`

### Codex 指示

```text
workspace/mvp/eval/022/README.md の 022-4 に従って、dependency/setup/build lifecycle を source semantics に寄せて修正してください。

目的:
- dependency missing -> setup authority -> install/setup -> build rerun -> verification を一つの lifecycle として扱う。
- package manifest がない状態で build/test verify を要求しない。
- setup-only / manifest-only を success 扱いしない。
- plan-run / ultra-plan-run / minimal-loop で同じ dependency lifecycle event taxonomy を使う。

必ず参照する source:
- /Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/node_request_helpers.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/node_runner_manifest.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/project_probe.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/project_verifier.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/verifier_command_policy.rs

主な編集候補:
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/minimal_loop/dependency_setup.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/minimal_loop/build_verifier.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/minimal_loop/repair_target.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/minimal_loop/loop_run.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/runner.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/verify.rs

制約:
- network install を無条件に実行しない。
- dependency setup は authority、workspace state、profile contract に基づいて制御する。
- Next.js 固有ではなく JS framework lifecycle として整理する。
- setup が成功しても task-specific implementation/acceptance がなければ success にしない。

受入条件:
- dependency missing -> setup blocked fixture がある。
- dependency missing -> setup allowed -> build rerun fixture がある。
- setup-only / manifest-only が success にならない fixture がある。
- plan-run / ultra-plan-run / minimal-loop で dependency lifecycle event 名が揃う。
- cargo test --manifest-path mvp/anvilminimal/Cargo.toml が通る。
```

## 6. 022-5: Repair Targeting / Follow-through / Handoff

### 対象 gate

- G-S10 repair targeting
- G-S13 recovery handoff

### 参照ファイル

Source:

- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/verifier_repair_targeting.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/repair_lifecycle.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/verifier_driver.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/minimal_step_runner/repair.rs`

MVP:

- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/minimal_loop/repair_target.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/repair.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/runner.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/minimal_loop/evidence.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/eval_events.rs`

### Codex 指示

```text
workspace/mvp/eval/022/README.md の 022-5 に従って、repair targeting / follow-through / recovery handoff を source semantics に寄せて修正してください。

目的:
- verify/acceptance failure から RepairTarget への分類を改善する。
- repair turn が target に沿った artifact を変更したか確認する。
- no-change repair / unrelated change repair を分類し、bounded repair exhausted 後は recovery handoff を保存する。
- handoff 保存だけで success にしない。

必ず参照する source:
- /Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/verifier_repair_targeting.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/repair_lifecycle.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/verifier_driver.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/minimal_step_runner/repair.rs

主な編集候補:
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/minimal_loop/repair_target.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/repair.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/runner.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/minimal_loop/evidence.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/eval_events.rs

制約:
- 無制限 repair loop を入れない。
- runtime に LLM judge を入れない。
- RepairJob 全体を丸ごと移植せず、MVP の RepairTarget / CompletionContract / RuntimeAcceptanceReport に写像する。
- recovery prompt 保存は failure handoff であり success ではない。

受入条件:
- missing entrypoint repair が expected artifact を作る fixture がある。
- no-change repair が `step_verify_repair_no_change` または同等 kind になる。
- target not followed が分類される。
- repair exhausted で `.anvil/repairs/repair-*.md` と suggested command が保存される。
- cargo test --manifest-path mvp/anvilminimal/Cargo.toml が通る。
```

## 7. 022-6: TaskContract-lite / Obligations / Scaffold Continuation

### 対象 gate

- G-S01 request understanding
- G-S02 task contract
- G-S11 scaffold fallback

### 参照ファイル

Source:

- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/task_contract.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/task_contract_core.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/task_contract_completion_policy.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/task_contract_artifact_contract.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/task_contract_deliverable_lifecycle.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/evidence_binding.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/scaffold_pipeline.rs`

MVP:

- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/minimal_loop/completion.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/minimal_loop/evidence.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/minimal_loop/repair_target.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/step_plan.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/runner.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/scripts/eval_lib/plan_capability_contract.py`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/scripts/eval_lib/acceptance_contract.py`

### Codex 指示

```text
workspace/mvp/eval/022/README.md の 022-6 に従って、TaskContract-lite / obligation tracking / scaffold continuation を実装してください。

目的:
- full TaskContract を丸ごと移植せず、MVP に必要な obligation tracking を薄く導入する。
- setup / scaffold / implementation / verify / acceptance の役割を分ける。
- setup-only / scaffold-only / style-only / docs-only app を completion から外す。
- expected artifact と capability evidence の対応を controller 側で追跡する。

必ず参照する source:
- /Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/task_contract.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/task_contract_core.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/task_contract_completion_policy.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/task_contract_artifact_contract.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/task_contract_deliverable_lifecycle.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/evidence_binding.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/scaffold_pipeline.rs

主な編集候補:
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/minimal_loop/completion.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/minimal_loop/evidence.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/minimal_loop/repair_target.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/step_plan.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/runner.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/scripts/eval_lib/plan_capability_contract.py
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/scripts/eval_lib/acceptance_contract.py

制約:
- full TaskContract graph を移植しない。
- 新しい大きな抽象を作る前に CompletionContract / RuntimeAcceptanceReport / RepairTarget の拡張で済むか検討する。
- scenario 固有文字列に過適応しない。
- role は setup, scaffold, implementation, verification, acceptance evidence 程度に留める。

受入条件:
- setup-only は success にならない。
- scaffold-only は success にならない。
- app/game task に対する docs-only output は acceptance を満たさない。
- required capability と artifact evidence の対応が fixture で確認できる。
- pytest mvp/anvilminimal/tests/eval が通る。
- cargo test --manifest-path mvp/anvilminimal/Cargo.toml が通る。
```

## 8. 022-7: Final Acceptance / UAT / TUI Observability

### 対象 gate

- G-S12 final acceptance
- G-S16 TUI/manual run observability

### 参照ファイル

Source:

- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/verifier.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/verifier_driver.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/task_contract_completion_policy.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/actor_loop_flow.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/minimal_step_runner/profile.rs`

MVP:

- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/minimal_loop/evidence.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/minimal_loop/completion.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/runner.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/profiles/nextjs.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/eval_events.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/scripts/eval_lib/acceptance_contract.py`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/scripts/eval_lib/plan_capability_contract.py`

### Codex 指示

```text
workspace/mvp/eval/022/README.md の 022-7 に従って、final acceptance / UAT-equivalent / TUI observability を実装してください。

目的:
- eval だけでなく通常実行でも、plan の capability が成果物に反映されたかを final acceptance で判定する。
- Next.js interactive app/game では build-only / title-only を success にしない。
- browser readiness / interaction evidence を release gate として扱う。
- TUI/manual run でも .anvil/runs/<run-id>/events.jsonl 相当を残し、停止理由を表示/保存する。

必ず参照する source:
- /Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/verifier.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/verifier_driver.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/task_contract_completion_policy.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/actor_loop_flow.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/minimal_step_runner/profile.rs

主な編集候補:
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/minimal_loop/evidence.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/minimal_loop/completion.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/runner.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/profiles/nextjs.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/eval_events.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/scripts/eval_lib/acceptance_contract.py
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/scripts/eval_lib/plan_capability_contract.py

制約:
- runtime に LLM judge を入れない。
- Space Invaders 固有の単語ではなく、interactive game / web app capability contract として評価する。
- browser が使えない環境では full pass にせず partial とする。
- final acceptance を甘くして成功率を上げない。false positive 削減を優先する。

受入条件:
- static title-only Next.js app が final acceptance failure になる。
- build-only app が interactive app/game acceptance を満たさない。
- successful interactive app は build / route / capability evidence を持つ。
- TUI 実行で run events と failure stage が残る。
- pytest mvp/anvilminimal/tests/eval が通る。
- cargo test --manifest-path mvp/anvilminimal/Cargo.toml が通る。
```

## 9. 022-8: Provider Probe Gate / Tool Args Recovery

### 対象 gate

- G-S07 tool execution policy
- G-S15 provider behavior

### 参照ファイル

Source:

- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/ollama/client.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/ollama/xml_fallback.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/minimal_step_runner.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/minimal_step_runner/repair.rs`

MVP:

- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/providers/openai.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/providers/gemini.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/providers/ollama.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/tools.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/tests/live_provider.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/eval/suites/mvp-provider-smoke.yaml`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/scripts/eval-run.py`

### Codex 指示

```text
workspace/mvp/eval/022/README.md の 022-8 に従って、provider probe gate と tool args recovery を整備してください。

目的:
- prompt/tool-call/provider-sensitive fix を fake client fixture だけで完了扱いにしない。
- OpenAI/Gemini/Ollama の tool args shape / function schema / XML fallback 挙動を小さく観測する。
- recoverable tool args は回復し、unsafe args は拒否する。
- provider probe 結果を parity_gate_report.json と eval summary に接続する。

必ず参照する source:
- /Users/maenokota/share/work/github_kewton/Anvil-develop/src/ollama/client.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/src/ollama/xml_fallback.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/minimal_step_runner.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/minimal_step_runner/repair.rs

主な編集候補:
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/providers/openai.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/providers/gemini.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/providers/ollama.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/tools.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/tests/live_provider.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/eval/suites/mvp-provider-smoke.yaml
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/scripts/eval-run.py

制約:
- provider abstraction を不必要に増やさない。
- probe は任意実行にし、通常 test が API key や network を必須にしない。
- provider 固有の揺れを runtime success 扱いにしない。schema/tool args/repair prompt の観測として記録する。
- unsafe path/workspace confinement 違反は回復せず拒否する。

受入条件:
- OpenAI tool args shape の probe がある。
- Gemini function calling/schema の probe がある。
- Ollama XML fallback/tool-like output の probe がある。
- API key がない環境では skip される。
- provider/prompt-sensitive fix の作業計画に provider probe 要否を明記できる。
- cargo test --manifest-path mvp/anvilminimal/Cargo.toml が通る。
```

## 10. 022-9: Comparative / Release Gate Operation

### 対象 gate

- G-S01〜G-S16 全体

### 参照ファイル

022 artifacts:

- `/Users/maenokota/share/work/github_kewton/Anvil-develop/workspace/mvp/eval/022/runtime_semantics_parity_gate_plan.md`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/workspace/mvp/eval/022/runtime_semantics_gate_matrix.md`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/workspace/mvp/eval/022/source_mvp_trace_manifest.md`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/workspace/mvp/eval/022/parity_gate_eval_protocol.md`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/workspace/mvp/eval/022/parity_gate_uat_acceptance.md`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/workspace/mvp/eval/022/parity_gate_ci_contract.md`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/workspace/mvp/eval/022/parity_gate_rollout_plan.md`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/workspace/mvp/eval/022/parity_gate_report.json`

MVP/eval:

- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/scripts/eval-run.py`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/eval/suites/mvp-smoke.yaml`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/eval/suites/mvp-provider-smoke.yaml`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/tests/eval`

### 標準比較コマンド

MVP:

```bash
python3 mvp/anvilminimal/scripts/eval-run.py \
  --suite mvp/anvilminimal/eval/suites/mvp-smoke.yaml \
  --model-profile speed-cloud-5x \
  --modes minimal-loop,step-plan,plan-run,ultra-plan-run \
  --runs 1 \
  --parallel 5 \
  --provider-limit 5 \
  --binary mvp/anvilminimal/target/release/anvilminimal \
  --binary-kind anvilminimal
```

anvildev:

```bash
python3 mvp/anvilminimal/scripts/eval-run.py \
  --suite mvp/anvilminimal/eval/suites/mvp-smoke.yaml \
  --model-profile speed-cloud-5x \
  --modes minimal-loop,step-plan,plan-run,ultra-plan-run \
  --runs 1 \
  --parallel 5 \
  --provider-limit 5 \
  --binary anvildev \
  --binary-kind anvildev \
  --engine minimal
```

Provider smoke:

```bash
python3 mvp/anvilminimal/scripts/eval-run.py \
  --suite mvp/anvilminimal/eval/suites/mvp-provider-smoke.yaml \
  --model-profile speed-cloud-5x \
  --modes minimal-loop,step-plan,plan-run,ultra-plan-run \
  --runs 1 \
  --parallel 5 \
  --provider-limit 5 \
  --binary mvp/anvilminimal/target/release/anvilminimal \
  --binary-kind anvilminimal
```

注意:

- `eval-run.py` 側で `anvildev` に `--engine minimal` を自動付与する実装にする場合、CLI 引数を二重付与しない。
- full comparison では suite / model profile / modes / runs / provider limit を揃える。
- local LLM を使う条件では `--parallel 5` を既定にしない。

### Codex 指示

```text
workspace/mvp/eval/022/README.md の 022-9 に従って、comparative / release gate operation を整備してください。

目的:
- runtime semantics 修正後に、MVP 単体ではなく anvildev --engine minimal との同条件比較を必須化する。
- release gate では browser readiness / interaction evidence / TUI run events を必須化する。
- parity_gate_report.json を毎回生成し、pass/partial/fail を機械的に判定する。
- warn/fail threshold と rollback 方針を eval-preflight または pytest に組み込む。

必ず参照する計画:
- /Users/maenokota/share/work/github_kewton/Anvil-develop/workspace/mvp/eval/022/runtime_semantics_parity_gate_plan.md
- /Users/maenokota/share/work/github_kewton/Anvil-develop/workspace/mvp/eval/022/runtime_semantics_gate_matrix.md
- /Users/maenokota/share/work/github_kewton/Anvil-develop/workspace/mvp/eval/022/parity_gate_eval_protocol.md
- /Users/maenokota/share/work/github_kewton/Anvil-develop/workspace/mvp/eval/022/parity_gate_uat_acceptance.md
- /Users/maenokota/share/work/github_kewton/Anvil-develop/workspace/mvp/eval/022/parity_gate_rollout_plan.md

主な編集候補:
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/scripts/eval-run.py
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/tests/eval
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/eval/suites/mvp-smoke.yaml
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/eval/suites/mvp-provider-smoke.yaml

制約:
- normal unit test に API key/network/anvildev/browser を必須化しない。
- comparative/release gate は明示 opt-in にする。
- success rate だけでなく lifecycle stage failure、failure kind、acceptance false positive、UAT evidence を比較する。
- correct failure detection による成功率低下と単純 regression を report で分ける。

受入条件:
- MVP と anvildev の同条件比較結果が parity_gate_report.json に入る。
- MVP が anvildev を 10pt 以上下回る場合は fail または intentional difference evidence が必要。
- release gate で browser/interaction evidence がない場合は full pass にならない。
- eval-preflight または pytest で report schema / threshold / gate partition を検査できる。
- rollback 時も blank failure kind と build-only false positive を再許可しない。
```

## 11. 実施上の注意

- 022-1 と 022-2 を飛ばして runtime を直さない。診断と trace が弱いままだと、また「直したように見える」状態になる。
- 022-4〜022-7 は依存関係が強い。dependency lifecycle、repair targeting、obligation tracking、final acceptance は別々に見えるが、実際は plan-run/ultra-plan-run の成功可否を共同で決める。
- 各 step の完了時に `runtime_semantics_gate_matrix.md` と `parity_gate_report.json` を更新する。
- `parity_gate_report.json` の `passed_gate_ids` に移す場合は、source trace / MVP trace / fixture / eval / UAT 証跡の不足がないことを確認する。
- `pass` にできない場合でも、`partial` と `fail` のどちらなのか、次に何が不足しているのかを明示する。
- 成功率が改善しても、title-only / scaffold-only / build-only false positive が増えた場合は regression として扱う。
- `workspace/` は gitignore 対象なので、コミット対象にする場合は明示的な扱いが必要。
