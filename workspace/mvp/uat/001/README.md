# test0630_001 UAT Repair Work Instructions

作成日: 2026-06-30

この README は、`test0630_ultra_plan_run_repair_roadmap.md` の Phase 0〜9 を、実装依頼としてそのまま Codex に渡せる粒度へ具体化したもの。

## 共通コンテキスト

作業ディレクトリ:

`/Users/maenokota/share/work/github_kewton/Anvil-develop`

共通参照ファイル:

- `/Users/maenokota/share/work/github_kewton/Anvil-develop/workspace/mvp/uat/001/test0630_ultra_plan_run_quality_root_cause.md`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/workspace/mvp/uat/001/test0630_ultra_plan_run_repair_roadmap.md`
- `/Users/maenokota/share/work/localwork/commandagent_mvp/01/test0630_001/.anvil/plans/ultra-plan-019f1754-2d9c-71d1-8475-afd83b610f28.yaml`
- `/Users/maenokota/share/work/localwork/commandagent_mvp/01/test0630_001/.anvil/runs/019f1753-ff27-76c3-bcf2-c1ecbf62851f/events.jsonl`
- `/Users/maenokota/share/work/localwork/commandagent_mvp/01/test0630_001/.anvil/runs/019f1753-ff27-76c3-bcf2-c1ecbf62851f/summary.md`
- `/Users/maenokota/share/work/localwork/commandagent_mvp/01/test0630_001/.anvil/repairs/repair-phase-web-audio-synth-and-ui-019f1758-94ab-75f2-b835-64513f49ddce.md`
- `/Users/maenokota/share/work/localwork/commandagent_mvp/01/test0630_001/src/app/page.tsx`
- `/Users/maenokota/share/work/localwork/commandagent_mvp/01/test0630_001/src/app/globals.css`
- `/Users/maenokota/share/work/localwork/commandagent_mvp/01/test0630_001/package.json`

共通制約:

- Space Invaders 固有の文字列対応にしない。
- Next.js interactive app / game の一般 runtime contract として実装する。
- success を甘くしない。false positive 削減を優先する。
- recovery は無制限自動再実行にしない。
- recovery `.md` は診断/replan prompt、recovery `.yaml` は手動確認・編集・再実行可能な plan として分離する。
- 通常 TUI 実行でも未完了状態と次の回復手段が分かるようにする。
- 既存の dirty/untracked 変更を巻き戻さない。

レビュー反映ガード:

- source の TaskContract / RepairJob / browser runner 全体を丸ごと移植しない。既存の `VerificationReport`、`RuntimeAcceptanceReport`、`RepairContext`、`UltraPlan`、event/eval helper の薄い拡張を優先する。
- browser readiness は availability-aware にする。browser unavailable は `partial`、browser HTTP 500 は `fail` と分け、通常 unit test の必須依存にはしない。
- verify command policy は緩めない。shell control syntax は runtime execution では引き続き拒否し、planner/StepPlan normalization で安全に分解できるものだけを扱う。
- Tailwind / Next.js の対策は exact version pinning ではなく、package/config/dev-server evidence に基づく profile contract として扱う。
- 現段階では LLM API 追加実行は不要。provider prompt/tool-call 挙動を変える Phase が出た場合のみ provider probe を実施する。

## 推奨対応順序

1. Phase 0: 現状固定と再現 fixture 化
2. Phase 1: verify command policy violation の deterministic repair
3. Phase 2: verify/planner failure 時の recovery UltraPlan YAML 保存
4. Phase 3: partial artifact 状態の TUI / run summary 明示
5. Phase 4: app/game capability evidence の導出
6. Phase 5: implement step の path-only completion 抑制
7. Phase 6: Next.js browser readiness を通常 final acceptance に接続
8. Phase 7: Tailwind / CSS pipeline の profile verifier 強化
9. Phase 8: release gate evidence content validation
10. Phase 9: 回帰評価と manual UAT gate

## Phase 0: 現状固定と再現 fixture 化

参照ファイル:

- `/Users/maenokota/share/work/github_kewton/Anvil-develop/workspace/mvp/uat/001/test0630_ultra_plan_run_quality_root_cause.md`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/workspace/mvp/uat/001/test0630_ultra_plan_run_repair_roadmap.md`
- `/Users/maenokota/share/work/localwork/commandagent_mvp/01/test0630_001/.anvil/runs/019f1753-ff27-76c3-bcf2-c1ecbf62851f/events.jsonl`
- `/Users/maenokota/share/work/localwork/commandagent_mvp/01/test0630_001/.anvil/plans/ultra-plan-019f1754-2d9c-71d1-8475-afd83b610f28.yaml`
- `/Users/maenokota/share/work/localwork/commandagent_mvp/01/test0630_001/src/app/page.tsx`
- `/Users/maenokota/share/work/localwork/commandagent_mvp/01/test0630_001/src/app/globals.css`

編集候補:

- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/tests/eval`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/scripts/eval_lib`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/workspace/mvp/uat/001`

Codex 指示:

```text
/Users/maenokota/share/work/github_kewton/Anvil-develop/workspace/mvp/uat/001/README.md の Phase 0 に従って、test0630_001 の失敗を再現 fixture として固定してください。

目的:
- phase 4 scaffold failure、verify command policy violation、recovery YAML missing、browser readiness HTTP 500、path-only early stop を再発検知できるようにする。
- correct failure detection と regression を区別できる evidence を残す。

必ず参照するファイル:
- /Users/maenokota/share/work/github_kewton/Anvil-develop/workspace/mvp/uat/001/test0630_ultra_plan_run_quality_root_cause.md
- /Users/maenokota/share/work/github_kewton/Anvil-develop/workspace/mvp/uat/001/test0630_ultra_plan_run_repair_roadmap.md
- /Users/maenokota/share/work/localwork/commandagent_mvp/01/test0630_001/.anvil/runs/019f1753-ff27-76c3-bcf2-c1ecbf62851f/events.jsonl
- /Users/maenokota/share/work/localwork/commandagent_mvp/01/test0630_001/.anvil/plans/ultra-plan-019f1754-2d9c-71d1-8475-afd83b610f28.yaml
- /Users/maenokota/share/work/localwork/commandagent_mvp/01/test0630_001/src/app/page.tsx
- /Users/maenokota/share/work/localwork/commandagent_mvp/01/test0630_001/src/app/globals.css

主な編集候補:
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/tests/eval
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/scripts/eval_lib
- /Users/maenokota/share/work/github_kewton/Anvil-develop/workspace/mvp/uat/001

制約:
- Space Invaders 固有文字列だけで判定しない。
- fixture は phase_scaffold_error / recovery_prompt_saved / browser failure / path-only completion の構造で判定する。

受入条件:
- phase_scaffold_error、recovery_prompt_saved、recovery UltraPlan YAML missing を fixture で検出できる。
- build pass / browser fail の組み合わせを fixture で表現できる。
- pytest mvp/anvilminimal/tests/eval が通る。
```

## Phase 1: verify command policy violation の deterministic repair

参照ファイル:

- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/verify.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/step_plan.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/lint.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/runner.rs`
- `/Users/maenokota/share/work/localwork/commandagent_mvp/01/test0630_001/.anvil/runs/019f1753-ff27-76c3-bcf2-c1ecbf62851f/events.jsonl`

編集候補:

- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/verify.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/step_plan.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/lint.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/runner.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/eval_events.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/tests`

Codex 指示:

```text
/Users/maenokota/share/work/github_kewton/Anvil-develop/workspace/mvp/uat/001/README.md の Phase 1 に従って、verify command policy violation の deterministic repair を実装してください。

目的:
- planner が `npm run build && test -f src/app/page.tsx` のような荒い verify command を出した時、provider retry だけに依存しない。
- 安全に分解できる command は複数 verify command に正規化し、unsafe な command は具体 failure kind で fail-fast する。

必ず参照するファイル:
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/verify.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/step_plan.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/lint.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/runner.rs
- /Users/maenokota/share/work/localwork/commandagent_mvp/01/test0630_001/.anvil/runs/019f1753-ff27-76c3-bcf2-c1ecbf62851f/events.jsonl

主な編集候補:
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/verify.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/step_plan.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/lint.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/runner.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/eval_events.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/tests

制約:
- shell control syntax を許可する方向には緩めない。
- `&&` の safe split は allowlist に限定する。
- `;`, `||`, redirection, command substitution, pipe は unsafe として拒否する。
- raw command execution policy は緩めない。分解は planner/StepPlan normalization 専用とし、Bash tool や runtime executor では shell control syntax を引き続き拒否する。

受入条件:
- safe な `&&` verify は複数 verify command に分解される。
- unsafe verify は引き続き拒否される。
- rejected case は `verify_command_policy_error` の具体 reason を保持する。
- 既存の `validate_verify_command("... && ...")` は引き続き拒否し、別 API の normalization 結果だけが複数 command として採用される。
- cargo test --manifest-path mvp/anvilminimal/Cargo.toml verify が通る。
- pytest mvp/anvilminimal/tests/eval が通る。
```

## Phase 2: verify/planner failure 時の recovery UltraPlan YAML 保存

参照ファイル:

- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/repair.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/ultra_plan.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/runner.rs`
- `/Users/maenokota/share/work/localwork/commandagent_mvp/01/test0630_001/.anvil/repairs/repair-phase-web-audio-synth-and-ui-019f1758-94ab-75f2-b835-64513f49ddce.md`

編集候補:

- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/repair.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/ultra_plan.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/runner.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/tui`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/eval_events.rs`

Codex 指示:

```text
/Users/maenokota/share/work/github_kewton/Anvil-develop/workspace/mvp/uat/001/README.md の Phase 2 に従って、verify/planner failure 時に recovery UltraPlan YAML を保存する機構を実装してください。

目的:
- `.anvil/repairs/repair-*.md` だけでなく、手動で確認・編集・再実行できる `.anvil/plans/recovery-ultra-plan-*.yaml` を保存する。
- TUI と summary に「未完了」「保存した recovery YAML」「推奨コマンド」を表示する。

必ず参照するファイル:
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/repair.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/ultra_plan.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/runner.rs
- /Users/maenokota/share/work/localwork/commandagent_mvp/01/test0630_001/.anvil/repairs/repair-phase-web-audio-synth-and-ui-019f1758-94ab-75f2-b835-64513f49ddce.md

主な編集候補:
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/repair.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/ultra_plan.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/runner.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/tui
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/eval_events.rs

制約:
- recovery YAML 保存だけで success 扱いしない。
- 自動再帰実行は入れない。
- `.md` は診断/replan prompt、`.yaml` は focused recovery plan として分離する。
- recovery YAML は `render_ultra_plan` / `parse_ultra_plan` の roundtrip に通る形式だけを保存する。生成不能な場合は success にせず、recovery YAML missing を明示する。

受入条件:
- phase_scaffold_error で `.anvil/repairs/repair-*.md` と `.anvil/plans/recovery-ultra-plan-*.yaml` が両方保存される。
- recovery YAML は original goal、failed phase、missing capability、verify preference を含む。
- TUI summary に incomplete、recovery YAML path、suggested command が表示される。
- recovery YAML は parse/roundtrip 検証に通る。
- cargo test --manifest-path mvp/anvilminimal/Cargo.toml recovery が通る。
- pytest mvp/anvilminimal/tests/eval が通る。
```

## Phase 3: partial artifact 状態の TUI / run summary 明示

参照ファイル:

- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/runner.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/tui`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/eval_events.rs`
- `/Users/maenokota/share/work/localwork/commandagent_mvp/01/test0630_001/.anvil/runs/019f1753-ff27-76c3-bcf2-c1ecbf62851f/summary.md`

編集候補:

- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/runner.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/tui`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/eval_events.rs`

Codex 指示:

```text
/Users/maenokota/share/work/github_kewton/Anvil-develop/workspace/mvp/uat/001/README.md の Phase 3 に従って、partial artifact 状態を TUI と run summary に明示してください。

目的:
- ultra-run 失敗時に、途中成果物を完成品と誤認しないようにする。
- completed phase / failed phase / pending phase / recovery next action を `.anvil/runs/<run-id>/summary.md` と TUI に出す。

必ず参照するファイル:
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/runner.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/tui
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/eval_events.rs
- /Users/maenokota/share/work/localwork/commandagent_mvp/01/test0630_001/.anvil/runs/019f1753-ff27-76c3-bcf2-c1ecbf62851f/summary.md

主な編集候補:
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/runner.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/tui
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/eval_events.rs

制約:
- 成功率を上げるために failure を隠さない。
- summary は人間が読める形式にし、events は機械集計できる形式にする。

受入条件:
- test0630_001 相当の停止で summary が `TUI command failed:` だけにならない。
- completed/pending/failed phase が summary から読める。
- recovery prompt path と recovery YAML path が表示される。
- cargo test --manifest-path mvp/anvilminimal/Cargo.toml tui が通る。
```

## Phase 4: app/game capability evidence の導出

参照ファイル:

- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/minimal_loop/evidence.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/minimal_loop/completion.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/runner.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/scripts/eval_lib/plan_capability_contract.py`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/scripts/eval_lib/acceptance_contract.py`

編集候補:

- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/minimal_loop/evidence.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/minimal_loop/completion.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/runner.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/scripts/eval_lib/plan_capability_contract.py`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/scripts/eval_lib/acceptance_contract.py`

Codex 指示:

```text
/Users/maenokota/share/work/github_kewton/Anvil-develop/workspace/mvp/uat/001/README.md の Phase 4 に従って、interactive app/game capability evidence の導出を実装してください。

目的:
- `required_final_capabilities` があるのに `required_final_evidence` が空になる状態を減らす。
- prompt / profile / phase task から、generic な app/game capability evidence を controller に渡す。

必ず参照するファイル:
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/minimal_loop/evidence.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/minimal_loop/completion.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/runner.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/scripts/eval_lib/plan_capability_contract.py
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/scripts/eval_lib/acceptance_contract.py

主な編集候補:
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/minimal_loop/evidence.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/minimal_loop/completion.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/runner.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/scripts/eval_lib/plan_capability_contract.py
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/scripts/eval_lib/acceptance_contract.py

制約:
- Space Invaders 固有語に依存しない。
- evidence は visible interactive surface、user input handler、stateful update、challenge、score/progression、failure/collision、restart/recoverable state などの一般観点にする。
- 静的 evidence だけで release-grade full pass にしない。browser/interaction evidence がない場合は partial に留める。

受入条件:
- interactive game task で required_final_evidence が空にならない。
- docs-only / style-only / title-only output は acceptance failure になる。
- evidence 不足は実装 continuation または final acceptance failure として扱い、単なる eval diagnostic に閉じない。
- cargo test --manifest-path mvp/anvilminimal/Cargo.toml evidence が通る。
- pytest mvp/anvilminimal/tests/eval が通る。
```

## Phase 5: implement step の path-only completion 抑制

参照ファイル:

- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/minimal_loop/loop_run.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/minimal_loop/completion.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/minimal_loop/evidence.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/runner.rs`
- `/Users/maenokota/share/work/localwork/commandagent_mvp/01/test0630_001/.anvil/runs/019f1753-ff27-76c3-bcf2-c1ecbf62851f/events.jsonl`

編集候補:

- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/minimal_loop/loop_run.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/minimal_loop/completion.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/minimal_loop/evidence.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/runner.rs`

Codex 指示:

```text
/Users/maenokota/share/work/github_kewton/Anvil-develop/workspace/mvp/uat/001/README.md の Phase 5 に従って、interactive app/game 実装 step の path-only completion を抑制してください。

目的:
- `src/app/page.tsx` など expected paths が存在するだけで playable game completion と判定しない。
- path existence と capability evidence を分離し、capability 不足時には continuation feedback を出す。

必ず参照するファイル:
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/minimal_loop/loop_run.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/minimal_loop/completion.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/minimal_loop/evidence.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/runner.rs
- /Users/maenokota/share/work/localwork/commandagent_mvp/01/test0630_001/.anvil/runs/019f1753-ff27-76c3-bcf2-c1ecbf62851f/events.jsonl

主な編集候補:
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/minimal_loop/loop_run.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/minimal_loop/completion.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/minimal_loop/evidence.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/runner.rs

制約:
- non-interactive file creation task の path-only completion は壊さない。
- app/game profile または interactive capability がある場合に限定して強める。
- setup/scaffold/implementation/verification/acceptance の role を混同しない。setup-only / scaffold-only / style-only を completion にしない。

受入条件:
- interactive app/game では `required_artifacts_satisfied_after_tool` だけで step 完了しない。
- stateful interaction / challenge / failure rule が不足する場合、continuation feedback が出る。
- non-interactive file creation task の既存挙動を保つ fixture がある。
- cargo test --manifest-path mvp/anvilminimal/Cargo.toml completion が通る。
- pytest mvp/anvilminimal/tests/eval が通る。
```

## Phase 6: Next.js browser readiness を通常 final acceptance に接続

参照ファイル:

- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/runner.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/profiles/nextjs.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/scripts/eval_lib/browser_oracle.py`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/scripts/eval_lib/runtime_scoring.py`
- `/Users/maenokota/share/work/localwork/commandagent_mvp/01/test0630_001/src/app/globals.css`

編集候補:

- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/runner.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/profiles/nextjs.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/scripts/eval_lib/browser_oracle.py`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/scripts/eval_lib/runtime_scoring.py`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/tests/eval`

Codex 指示:

```text
/Users/maenokota/share/work/github_kewton/Anvil-develop/workspace/mvp/uat/001/README.md の Phase 6 に従って、Next.js browser readiness を通常 final acceptance に接続してください。

目的:
- Next.js interactive app/game では build-only を full success にしない。
- dev server route / browser render / interaction evidence を final acceptance と eval に反映する。

必ず参照するファイル:
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/runner.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/profiles/nextjs.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/scripts/eval_lib/browser_oracle.py
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/scripts/eval_lib/runtime_scoring.py
- /Users/maenokota/share/work/localwork/commandagent_mvp/01/test0630_001/src/app/globals.css

主な編集候補:
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/runner.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/profiles/nextjs.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/scripts/eval_lib/browser_oracle.py
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/scripts/eval_lib/runtime_scoring.py
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/tests/eval

制約:
- browser unavailable は partial、browser HTTP 500 は fail と区別する。
- Playwright/browser を通常 unit test の必須依存にしない。
- build-only success を full success にしない。
- TUI 実行時に browser check が実行できない場合も full success と表示しない。

受入条件:
- `npm run build` pass / browser HTTP 500 は final acceptance pass にならない。
- browser unavailable と browser failed が区別される。
- static title-only app は interactive game acceptance を満たさない。
- release/UAT gate では browser evidence が必要であることが summary/report に出る。
- pytest mvp/anvilminimal/tests/eval が通る。
```

## Phase 7: Tailwind / CSS pipeline の profile verifier 強化

参照ファイル:

- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/profiles/nextjs.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/minimal_loop/build_verifier.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/minimal_loop/dependency_setup.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/scripts/eval_lib/failure_classification.py`
- `/Users/maenokota/share/work/localwork/commandagent_mvp/01/test0630_001/src/app/globals.css`
- `/Users/maenokota/share/work/localwork/commandagent_mvp/01/test0630_001/package.json`

編集候補:

- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/profiles/nextjs.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/minimal_loop/build_verifier.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/minimal_loop/dependency_setup.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/scripts/eval_lib/failure_classification.py`

Codex 指示:

```text
/Users/maenokota/share/work/github_kewton/Anvil-develop/workspace/mvp/uat/001/README.md の Phase 7 に従って、Next.js Tailwind / CSS pipeline の profile verifier を強化してください。

目的:
- `@tailwind` directive を使う Next.js app で、build pass だが dev route fail になるケースを早期に検出する。
- Tailwind を使うなら toolchain 完備、使わないなら plain CSS に寄せる。

必ず参照するファイル:
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/profiles/nextjs.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/minimal_loop/build_verifier.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/minimal_loop/dependency_setup.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/scripts/eval_lib/failure_classification.py
- /Users/maenokota/share/work/localwork/commandagent_mvp/01/test0630_001/src/app/globals.css
- /Users/maenokota/share/work/localwork/commandagent_mvp/01/test0630_001/package.json

主な編集候補:
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/planner/profiles/nextjs.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/minimal_loop/build_verifier.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/src/minimal_loop/dependency_setup.rs
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/scripts/eval_lib/failure_classification.py

制約:
- Next.js 固有の known-good contract として扱い、Space Invaders 固有にしない。
- plain CSS app に Tailwind toolchain を強制しない。
- exact version pinning ではなく、installed package/config/dev-server error evidence に基づいて判定する。

受入条件:
- `@tailwind` directive と不整合な config/dependency が fixture で fail する。
- plain CSS app は Tailwind toolchain を要求されない。
- dev route HTTP 500 が build success と別 failure kind で集計される。
- package/config/runtime error evidence に基づく failure reason が出る。
- cargo test --manifest-path mvp/anvilminimal/Cargo.toml nextjs が通る。
- pytest mvp/anvilminimal/tests/eval が通る。
```

## Phase 8: release gate evidence content validation

参照ファイル:

- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/scripts/eval_lib/parity_gate.py`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/scripts/eval-run.py`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/workspace/mvp/eval/022/parity_gate_report.json`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/workspace/mvp/eval/022/evidence`

編集候補:

- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/scripts/eval_lib/parity_gate.py`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/scripts/eval-run.py`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/tests/eval`

Codex 指示:

```text
/Users/maenokota/share/work/github_kewton/Anvil-develop/workspace/mvp/uat/001/README.md の Phase 8 に従って、release gate evidence content validation を実装してください。

目的:
- release gate が evidence path の存在だけで full pass にならないようにする。
- browser/interaction/TUI evidence の内容を読み、`ok=false` や HTTP 500 を gate failure として扱う。

必ず参照するファイル:
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/scripts/eval_lib/parity_gate.py
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/scripts/eval-run.py
- /Users/maenokota/share/work/github_kewton/Anvil-develop/workspace/mvp/eval/022/parity_gate_report.json
- /Users/maenokota/share/work/github_kewton/Anvil-develop/workspace/mvp/eval/022/evidence

主な編集候補:
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/scripts/eval_lib/parity_gate.py
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/scripts/eval-run.py
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/tests/eval

制約:
- evidence path が存在するだけで pass にしない。
- browser unavailable と browser failed を分ける。
- TUI command failed は release evidence 上も未完了扱いにする。
- evidence JSON が壊れている、読めない、または必須 key がない場合も full pass にしない。

受入条件:
- `browser-readiness.json.ok=false` なら release gate full pass にならない。
- HTTP 500 は release gate fail/partial reason に出る。
- `tui_command_stop.ok=false` は full pass にならない。
- malformed evidence JSON は `evidence_invalid` として分類される。
- pytest mvp/anvilminimal/tests/eval が通る。
```

## Phase 9: 回帰評価と manual UAT gate

参照ファイル:

- `/Users/maenokota/share/work/github_kewton/Anvil-develop/workspace/mvp/uat/001/test0630_ultra_plan_run_quality_root_cause.md`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/workspace/mvp/uat/001/test0630_ultra_plan_run_repair_roadmap.md`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/scripts/eval-run.py`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/eval/suites/mvp-smoke.yaml`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/workspace/mvp/eval/022/parity_gate_report.json`

編集候補:

- `/Users/maenokota/share/work/github_kewton/Anvil-develop/workspace/mvp/uat/001`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/workspace/mvp/eval/022`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/scripts/eval-run.py`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/tests/eval`

Codex 指示:

```text
/Users/maenokota/share/work/github_kewton/Anvil-develop/workspace/mvp/uat/001/README.md の Phase 9 に従って、回帰評価と manual UAT gate を実施してください。

目的:
- 修正が eval だけに過適応していないことを確認する。
- MVP と anvildev --engine minimal の同条件比較に加え、実ブラウザ/TUI の成果物品質を確認する。

必ず参照するファイル:
- /Users/maenokota/share/work/github_kewton/Anvil-develop/workspace/mvp/uat/001/test0630_ultra_plan_run_quality_root_cause.md
- /Users/maenokota/share/work/github_kewton/Anvil-develop/workspace/mvp/uat/001/test0630_ultra_plan_run_repair_roadmap.md
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/scripts/eval-run.py
- /Users/maenokota/share/work/github_kewton/Anvil-develop/mvp/anvilminimal/eval/suites/mvp-smoke.yaml
- /Users/maenokota/share/work/github_kewton/Anvil-develop/workspace/mvp/eval/022/parity_gate_report.json

作業内容:
- cargo build --release --manifest-path mvp/anvilminimal/Cargo.toml を実行する。
- pytest mvp/anvilminimal/tests/eval を実行する。
- MVP 全量 eval を実行する。
- anvildev --engine minimal と同条件比較を実行する。
- test0630_001 相当の manual UAT を新規 workspace で実施する。
- browser `/` HTTP 200、canvas or interactive DOM、keyboard/pointer interaction、TUI run events、recovery artifacts を確認する。

制約:
- 成功率だけで判断しない。
- correct failure detection による成功率低下と単純 regression を分ける。
- browser が使えない場合は full pass にしない。

受入条件:
- release gate が evidence 内容込みで pass または納得できる partial になる。
- build-only false positive が減る。
- recovery `.md` と recovery `.yaml` が保存される。
- successful artifact は実ブラウザで操作可能である。
- 結果を workspace/mvp/uat/001 に記録する。
```
