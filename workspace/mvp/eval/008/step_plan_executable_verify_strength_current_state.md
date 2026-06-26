# step-plan executable/verify strength current state

## 目的

MVP 版 anvilminimal の step-plan eval について、次の2指標を優先改善対象として現状を整理する。

- `executable_plan_score`
- `verify_strength_score`

比較対象として、移植元 `anvildev --engine minimal` も同一条件で実行した。

## 実行条件

- suite: `mvp/anvilminimal/eval/suites/mvp-smoke.yaml`
- profile: `speed-cloud`
- modes: `step-plan`
- local LLM: 未使用
- runs: 1
- parallel: 4
- context budget: 65536

実行結果:

- MVP: `/tmp/anvilminimal-step-plan-speed-cloud-20260626-005`
- anvildev: `/tmp/anvildev-step-plan-speed-cloud-20260626-004`
- compare: `/tmp/anvildev-step-plan-speed-cloud-20260626-004/compare_vs_mvp.md`

## 現在の事象

### MVP

| metric | value |
|---|---:|
| success | 12/12 |
| plan_quality_score avg | 85.1 |
| executable_plan_score avg | 78.3 |
| constraint_coverage_score avg | 89.5 |
| verify_strength_score avg | 62.2 |
| artifact_ownership_score avg | 98.0 |
| lint_repair_score avg | 87.4 |
| overall_score avg | 83.6 |

MVP は step-plan 生成自体は全件成功している。required artifact ownership は高く、生成された YAML は plan-run に渡せる形として概ね成立している。

一方で `verify_strength_score` が平均 62.2 と低い。低い行は以下。

| scenario | main/planner | verify_strength | executable_plan | success |
|---|---|---:|---:|---|
| nextjs-space-invaders-large | gemini/openai | 23.0 | 82.0 | true |
| repair-exhausted-report-large | openai/gemini | 45.0 | 100.0 | true |
| docs-heading-update-small | openai/gemini | 47.0 | 78.0 | true |
| docs-heading-update-small | gemini/openai | 48.0 | 60.0 | true |
| nextjs-space-invaders-large | openai/gemini | 63.0 | 82.0 | true |

### anvildev

| metric | value |
|---|---:|
| success | 7/12 |
| plan_quality_score avg | 76.1 |
| executable_plan_score avg | 78.3 |
| constraint_coverage_score avg | 92.9 |
| verify_strength_score avg | 47.0 |
| artifact_ownership_score avg | 47.9 |
| lint_repair_score avg | 100.0 |
| overall_score avg | 48.3 |

anvildev は 5/12 が invalid generated step plan で失敗している。失敗行は YAML 保存前に拒否されるため、成功行だけを見るよりも実運用上の不安定性が大きい。

失敗理由:

- `node smoke-check.js` が safe allowlist 外
- `python -m unittest discover ...` が safe allowlist 外
- `python -m unittest` が safe allowlist 外
- `npm run build` が `package.json` と Next.js entry path 作成前扱いで lint 失敗

## 問題点

### P1. MVP の verify が弱い

MVP は成功率が高いが、`verify_strength_score` が低いケースがある。

代表例:

- docs 系: `test -f`, `test -s`, `grep`, `cat` 中心になりやすい
- Next.js 系: required artifact の存在確認はできるが、`npm run build` まで含まれない、または profile/postcheck に依存しすぎる
- recovery 系: 成果物は作るが、実際の unittest 実行や失敗入力ケースの確認が弱い

これは step-plan YAML としては妥当でも、plan-run の実装品質を保証する力が不足する。

重要なのは、これは単純な prompt 不足ではないこと。現状の問題は「invalid plan が出ている」ではなく、「valid だが弱い plan が deterministic check を通過している」ことにある。したがって、planner に強い verify を出させるだけでなく、作成済み plan を自己チェックし、明確に弱い場合だけ bounded retry に戻す閉ループが必要。

### P2. executable_plan_score は平均だけでは差が見えにくい

今回の最新 run では MVP と anvildev の `executable_plan_score avg` がどちらも 78.3 だった。

ただし内訳は異なる。

- MVP: 全件 YAML が生成され、artifact ownership が高い
- anvildev: 失敗行は YAML 保存前に落ちるため、score が入らない行がある

つまり `executable_plan_score avg` は「保存された YAML の実行可能性」は見られるが、「YAML を安定して生成できたか」は十分に表現しない。失敗行を含めた総合判断には `success_rate`, `artifact_ownership_score`, `overall_score`, failure kind を併用する必要がある。

### P3. quality warning が修正ループに十分つながっていない

MVP は最終成功しているが、planner lint retry や prompt issue を経由している行がある。

この場合、最終 YAML は出るが planner 出力の一発安定性は低い。加えて、現在の quality warning は主に event として残るだけで、valid だが弱い plan を作り直す力は限定的。

`verify_strength_score` を上げる主対策は、初回 prompt 強化ではなく、以下の閉ループを作ること。

1. 作成済み plan を deterministic に quality self-check する
2. 高信頼な issue だけ `retryable_quality` として corrective retry に戻す
3. retry が悪化した場合は、既に得られている valid plan を保持する

### P4. anvildev は verify policy と suite 要求のずれが大きい

anvildev は `node smoke-check.js`, `python -m unittest`, `python -m unittest discover ...` を拒否している。一方、suite は node smoke check や unittest を要求する。

このため、planner がユーザー要求に沿った verify を出しても safe allowlist で拒否される。step-plan 性能比較では、anvildev の policy mismatch が成功率を下げている。

## 問題箇所

### Eval scoring

- `mvp/anvilminimal/scripts/eval_lib/plan_scoring.py`
  - `score_verify_strength`
  - `command_strength`
  - `score_executable_step_plan`
  - `score_artifact_ownership`

現在の `verify_strength_score` は command の文字列強度を見ている。次の改善が必要。

- docs 系で存在確認だけの `test -f` / `cat` / 汎用的な `grep` に寄る plan を低く評価し、要求された見出しや文言を確認する content assertion は評価する
- Next.js profile では `npm run build` または postcheck delegated build が plan に明確に反映されることを評価する
- Python/Rust では compile-only より unit test を強く評価する
- plan-run 実測との相関で、しきい値と重みを校正する

### MVP plan self-check / quality retry / prompt guidance

- `mvp/anvilminimal/src/planner/runner.rs`
  - `plan_generation_system_prompt`
  - `build_lint_retry_prompt`
  - `lint_retry_hard_constraints`
  - `strengthen_step_plan_for_profile`
- `mvp/anvilminimal/src/planner/lint.rs`
  - `step_plan_quality_warnings`

現状、expected paths と lint 通過に関する指示は強いが、valid plan の品質不足を検出して修正ループに戻す仕組みが弱い。

改善候補:

- `fatal` / `retryable_quality` / `advisory` の quality issue 分類を追加する
- weak verify、verify/artifact coupling、instruction specificity、profile expectation を deterministic に判定する
- 高信頼な `retryable_quality` だけ既存 retry loop に戻す
- retry が fatal lint を増やした場合は、既に得られている valid plan を保持する
- docs: `test -s` だけでなく、要求された見出しや例を `grep` で確認する
- Python: `python3 -m unittest test_*.py`
- Rust: `cargo test`
- Next.js: `npm run build` は dependency setup/postcheck の扱いを明確化したうえで、plan 上に build verification の意図を残す
- file existence verify は最終手段扱いにする

### MVP plan lint / semantic lint

- `mvp/anvilminimal/src/planner/lint.rs`
  - `lint_step_plan_report`
  - `verify command requires dependency setup or package manifest first`
  - Next.js build ordering checks

強い verify を要求すると、lint 側で dependency setup や manifest ordering と衝突する可能性がある。verify_strength を改善する時は、prompt だけでなく lint の許容条件も確認する必要がある。

### Eval report

- `mvp/anvilminimal/scripts/eval_lib/report.py`
  - `Plan Run Predictiveness`
  - `Additional Plan Metrics`

`executable_plan_score` と `verify_strength_score` を改善するには、step-plan 単体だけでなく `plan-run` 成功率との相関を見る必要がある。`--modes step-plan,plan-run --runs 2+` の評価を追加し、false positive / false negative を追跡する。

## 優先改善方針

### Phase 1: quality self-check を定義する

1. 作成済み plan に対して `fatal` / `retryable_quality` / `advisory` の分類を行う。
2. `test -f`, `cat`, compile-only に寄る plan を弱い verify として検出する。
3. verify が expected artifact と結びついているかを判定する。
4. Next.js など profile がある場合は、profile expectation を自己チェックの根拠にする。

受け入れ目安:

- `success_rate` は 12/12 を維持
- quality issue の category / message / retryability が event と summary で追跡できる
- eval scenario 名に依存した判定を追加していない

### Phase 2: quality issue を bounded retry に戻す

1. 高信頼な `retryable_quality` だけ既存の planner retry loop に戻す。
2. retry prompt は品質不足と hard constraints のみを短く渡す。
3. retry が fatal lint を増やした場合は、既存の valid plan を保持する。
4. 初期実装では quality issue exhausted を新たな失敗扱いにしない。失敗扱いへの昇格は、plan-run predictiveness と blind eval で安定性を確認した後の別判断にする。

受け入れ目安:

- `planner_lint_error` が増えない
- `lint_repair_score avg` が悪化しない
- retry 前後の YAML を目視して、品質改善が自然である
- valid plan がある場合、quality retry の悪化で YAML 生成成功率が下がらない

### Phase 3: prompt / profile guidance を最小補強する

1. planner prompt に task/profile 種別ごとの強い verify 例を短く追加する。
2. file existence verify は fallback と明記する。
3. Next.js は `npm run build` を plan 上に残しつつ、dependency setup/postcheck との衝突を避ける方針を明確化する。
4. `expected_paths` を持つ step の instruction に対象 path を明示させる。

受け入れ目安:

- MVP `verify_strength_score avg` が 62.2 から改善
- MVP `executable_plan_score avg` が 78.3 から改善
- `artifact_ownership_score avg` は 95 以上を維持
- provider 別分岐を追加していない

### Phase 4: 指標の妥当性確認

1. `--modes step-plan,plan-run --runs 2` 以上で eval を実行する。
2. `Plan Run Predictiveness` の false positive / false negative を確認する。
3. 高スコアなのに plan-run 失敗する YAML を目視レビューする。
4. 低スコアなのに plan-run 成功する YAML を目視レビューする。

受け入れ目安:

- `plan_run_predictiveness` の false positive が改善前より減る
- `verify_strength_score` と plan-run 成功率の関係が説明可能
- eval 特化の文字列ルールではなく、一般的な verify 強度として妥当な指標になっている

## 次に確認すべき YAML

MVP 低 verify:

- `/tmp/anvilminimal-step-plan-speed-cloud-20260626-005` の `nextjs-space-invaders-large` / `gemini:gemini-3.1-flash-lite`
- `/tmp/anvilminimal-step-plan-speed-cloud-20260626-005` の `repair-exhausted-report-large` / `openai:gpt-5.4-mini`
- `/tmp/anvilminimal-step-plan-speed-cloud-20260626-005` の `docs-heading-update-small` 2件

anvildev failure:

- `/tmp/anvildev-step-plan-speed-cloud-20260626-004` の失敗 stderr
- 特に Next.js 2件と unittest allowlist 拒否3件

## 現時点の結論

MVP は step-plan 生成安定性と artifact ownership で anvildev より優位。ただし verify の強度はまだ不足している。

まず改善すべきは、MVP planner が生成した valid plan を自己チェックし、弱い verify や曖昧な instruction を限定的に修正ループへ戻す仕組みである。prompt 強化は必要だが主対策ではなく、self-check と bounded retry を支える補助策として扱う。
