# plan-run repaired YAML score experiment

## 目的

既存 `/private/tmp/anvilminimal-eval-008-smoke-predictiveness-1` の plan-run 失敗例について、同じ課題を保ったまま YAML の実行形状を修正し、実際に plan-run が成功する YAML を用意した上で、元 YAML と repaired YAML のスコア差を確認する。

検証したい仮説は以下。

- 従来の静的な `plan_quality_score` / `executable_plan_score` / `verify_strength_score` だけでは plan-run 成否を十分に分離できない。
- 後付け算出した execution shape / runtime event 系、特に tool call 摩擦、artifact progress、finalization は成否をかなり分離できる。

## 入力

- 元 eval run: `/private/tmp/anvilminimal-eval-008-smoke-predictiveness-1`
- repaired plan:
  - `workspace/mvp/eval/009/repaired_plans/js-date-helper-compact.yaml`
  - `workspace/mvp/eval/009/repaired_plans/python-markdown-linter-compact.yaml`
  - `workspace/mvp/eval/009/repaired_plans/rust-cli-compact.yaml`
  - `workspace/mvp/eval/009/repaired_plans/readme-compact.yaml`
- repaired run workdir:
  - `/private/tmp/anvilminimal-plan-repair-experiment/js`
  - `/private/tmp/anvilminimal-plan-repair-experiment/python`
  - `/private/tmp/anvilminimal-plan-repair-experiment/rust`
  - `/private/tmp/anvilminimal-plan-repair-experiment/readme`

## repaired YAML の方針

失敗例の YAML は、`inspect` / `report` のような空の段階や、artifact を作る前の確認ステップが多く、OpenAI runtime が `Read` / `Glob` / no-tool response に流れやすい形だった。

repaired YAML では以下だけを変更した。

- required artifact を所有する実装 step を明確化する。
- `expected_paths` を concrete path にする。
- 実装 step の instruction に「作るファイル」と「検証ファイル」を同時に書く。
- 空の `inspect` / `report` step を削る。
- verify command は残す。ただし `--run-plan` 直実行では eval harness の postcheck とは別なので、生成後に手動で verify command も実行した。

この修正は、現在の eval だけに合わせた文字列条件ではなく、plan-run の実行摩擦を減らす一般的な形状修正である。ただし、今回の repaired YAML は比較のために意図的に短くしているため、「すべての planner が常に 1 step を出すべき」という結論にはしない。

## 実行結果

すべて OpenAI runtime `gpt-5.4-mini` で `--run-plan` 直実行した。

| case | repaired plan-run | generated verify |
|---|---:|---:|
| js-date-helper | pass | `node smoke-check.js` pass |
| python-markdown-linter | pass | `python -m unittest test_markdown_lint.py` pass |
| rust-cli | pass | `cargo test` pass |
| readme | pass | `grep -i "usage" README.md` pass |

## スコア比較

`plan_quality_score` などの静的スコアは `mvp/anvilminimal/scripts/eval_lib/plan_scoring.py` で再計算した。runtime event 系は `anvil-events.jsonl` / `events.jsonl` から後付け算出した。

| case | variant | success | failure kind | plan quality | executable | verify strength | pre-run shape | runtime friction | artifact progress | finalization | policy compat | posthoc predictive |
|---|---:|---:|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| js | original | false | max_iterations | 89.0 | 82.0 | 77.0 | 86.7 | 0.0 | 10.0 | 20.0 | 100.0 | 26.5 |
| js | repaired | true |  | 60.0 | 100.0 | 77.0 | 90.5 | 84.0 | 100.0 | 100.0 | 100.0 | 96.0 |
| python | original | false | tool_validation_error | 79.0 | 82.0 | 67.0 | 83.3 | 0.0 | 30.0 | 20.0 | 65.0 | 32.2 |
| python | repaired | true |  | 51.0 | 100.0 | 67.0 | 84.7 | 92.0 | 100.0 | 100.0 | 100.0 | 98.0 |
| rust | original | false | missing_tool_call | 92.0 | 100.0 | 92.0 | 98.4 | 12.0 | 100.0 | 20.0 | 100.0 | 58.0 |
| rust | repaired | true |  | 58.0 | 100.0 | 92.0 | 93.4 | 92.0 | 100.0 | 100.0 | 100.0 | 98.0 |
| readme | original | true |  | 84.0 | 60.0 | 51.0 | 76.2 | 20.0 | 80.0 | 50.0 | 100.0 | 62.5 |
| readme | repaired | true |  | 68.0 | 100.0 | 51.0 | 90.2 | 100.0 | 80.0 | 100.0 | 100.0 | 95.0 |

失敗 3件だけを対象にした平均は以下。

| metric | original failed avg | repaired success avg | delta |
|---|---:|---:|---:|
| plan_quality_score | 86.7 | 56.3 | -30.4 |
| executable_plan_score | 88.0 | 100.0 | +12.0 |
| verify_strength_score | 78.7 | 78.7 | 0.0 |
| runtime_friction_score | 4.0 | 89.3 | +85.3 |
| artifact_progress_score | 46.7 | 100.0 | +53.3 |
| finalization_score | 20.0 | 100.0 | +80.0 |
| tool_policy_compatibility_score | 88.3 | 100.0 | +11.7 |
| posthoc_predictive_score | 38.9 | 97.3 | +58.4 |

## 観察

### 静的 plan score はまだ成否を分離しない

元の失敗 YAML は `plan_quality_score` が高い。特に Rust は `plan_quality_score=92.0`, `executable_plan_score=100.0`, `verify_strength_score=92.0` でも `missing_tool_call` で失敗している。

一方、repaired YAML は比較のために短くしたため、scenario の `min_steps` に合わず `plan_quality_score` が下がる。にもかかわらず plan-run は成功した。

つまり、現行の静的 score は「計画として整っているか」は測れているが、「runtime が迷わず artifact 作成まで進めるか」は十分に測れていない。

### runtime event 系は強く分離した

失敗 3件では `runtime_friction_score` が平均 4.0、repaired 成功 3件では 89.3 だった。

主な差分は以下。

- 元 YAML は `Read` / `Glob` / `Grep` が多く、artifact 作成前に調査へ流れる。
- 元 YAML は no-tool response や `tool_validation_error` が混ざり、finalization に失敗する。
- repaired YAML は `Write` が早く出て、`required_artifacts_satisfied_after_tool` で止まる。
- artifact progress と finalization の差が大きい。

### execution shape は静的だけでは弱い

`pre_run_shape_score` は JS/Python では repaired 側が少し上がったが、Rust では下がった。それでも Rust repaired は成功している。

したがって、現時点の `pre_run_shape_score` だけを予測指標として採用するのは危険である。必要なのは、静的 YAML から以下の runtime 摩擦リスクを推定する別指標である。

- artifact を持たない `inspect` / `report` step の比率
- first implement step までの距離
- first expected path owner までの距離
- empty `expected_paths` step の連続
- terminal report step による no-tool finalization risk
- verify-only step と artifact ownership の分離
- read-before-write を誘発する instruction

## 暫定結論

今回の実験では、失敗していた 3 YAML を同等の課題のまま repaired YAML にして、すべて plan-run 成功かつ生成物 verify pass にできた。

この結果から、仮説は以下のように精度を上げるべき。

- `plan_quality_score` は plan-run 成功率の主指標としては不十分。
- `executable_plan_score` は必要条件に近いが、Rust のように高スコア失敗があるため単独では不十分。
- `runtime_friction_score`, `artifact_progress_score`, `finalization_score` は失敗から成功への変化を強く捉えた。
- 次に作るべき pre-run 指標は、汎用的な `execution_shape_risk_score`。これは「この YAML が runtime に Read/Glob/no-tool/validation loop を誘発しやすいか」を静的に推定する。

## 次の対策候補

1. `execution_shape_risk_score` を追加する。
   - `inspect/report` wrapper step penalty
   - empty expected_paths step penalty
   - first artifact owner latency penalty
   - terminal report step penalty
   - verify-only step separated from artifact owner penalty

2. plan-run 成功率との相関を見る。
   - 既存 24件へ後付け算出
   - repaired 3件と readme control を追加した 28件で再計測
   - false positive / false negative を確認

3. planner 修正は score に直結する文字列最適化ではなく、B/C 側に寄せる。
   - B: 生成済み plan の実行形状 risk を checker で検出する。
   - C: risk が高い時だけ plan を縮約または再配置する。
   - A: prompt への強い誘導は最小限にする。

