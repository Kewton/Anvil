# 015-2 Gate Policy Decision

作成日: 2026-06-28

## 現時点の判断

015-2 の新指標は、初期導入では acceptance hard gate にしない。

理由:

- `prompt_plan_capability_coverage_score` は plan の弱さを示すが、成果物が prompt を満たす可能性を単独では否定できない。
- `plan_verify_coverage_score` は verify の弱さを示すが、成果物自体が良い場合に false negative を起こし得る。
- `acceptance_confidence_score` は成功扱いの信頼度であり、成功/失敗の代替ではない。
- static oracle の inconclusive は deterministic browser oracle で確認すべきで、人間レビューや即 failure にしない。

## Hard Gate として維持するもの

| gate | rationale |
|---|---|
| process success | CLI が正常終了したことは必須。 |
| artifact existence | expected artifacts が無い成果物は受け入れない。 |
| build postcheck | suite が宣言した deterministic build は必須。 |
| launch readiness | suite が宣言した dev server readiness は必須。 |
| source semantic oracle | functional contract の必須 capability が明確に欠ける場合は失敗。 |
| plan-output adherence | YAML plan が約束した capability が成果物に欠ける場合は失敗。 |
| deterministic postcheck | suite postcheck failure は失敗。 |

## Diagnostic / Confidence として扱うもの

| metric | use |
|---|---|
| `prompt_plan_capability_coverage_score` | plan が prompt/profile を拾ったかの診断。 |
| `plan_capability_contract_score` | capability が expected paths / verify と接続しているかの診断。 |
| `plan_verify_declared_coverage_score` | step-plan 時点の verify 予測。 |
| `executed_verify_coverage_score` | plan-run / ultra-plan-run 後の verify 実体診断。 |
| `plan_verify_coverage_score` | build-only / contentless verify の false positive 候補抽出。 |
| `acceptance_confidence_score` | `acceptance_success=true` の信頼度。 |
| `acceptance_confidence_reason` | low confidence の原因分類。 |

## Hard Gate 化の候補条件

次の条件を複数 run で満たした場合のみ hard gate 化を検討する。

| metric | candidate threshold | required evidence |
|---|---:|---|
| `plan_output_adherence_score` | 70 | false positive を安定して検出し、false negative が少ない。 |
| `plan_verify_coverage_score` | 40 | build-only success の false positive と相関し、良い成果物を落としすぎない。 |
| `prompt_plan_capability_coverage_score` | 60-70 | weak plan が plan-run failure と相関する。 |
| `acceptance_confidence_score` | 70 | low confidence success が後続 manual-free oracle で不安定と確認される。 |

## Rollback 条件

- blind suite で false negative が増える。
- docs/CLI/library/data-transform に game-oriented capability が漏れる。
- scenario id 固有の分岐が入る。
- step-plan pre-run score に post-run artifact / success / stderr が混入する。
- anvildev 比較で provider/network failure を agent capability と誤集計する。

## Re-evaluation 後に更新する項目

- MVP / anvildev の `acceptance_success` 差。
- false positive 件数。
- confidence 分布。
- `prompt_plan_gap_kind` / `plan_verify_gap_kind` 分布。
- hard gate 候補を採用するか、diagnostic 維持するか。

## 2026-06-28 Re-evaluation Update

参照:

- `workspace/mvp/eval/015-2/015_2_recalibration_results.md`

実測:

| target | legacy success | acceptance success | false positive |
|---|---:|---:|---:|
| MVP | 36/48 | 17/36 | 9 |
| anvildev | 34/48 | 22/36 | 5 |

判断:

- `plan_output_adherence_success` は hard gate 維持。
  - MVP/anvildev ともに legacy success の false positive を検出できている。
- `prompt_plan_capability_coverage_score` は diagnostic 維持。
  - weak plan の検出には有効だが、単独で acceptance failure にすると false negative リスクがある。
- `plan_verify_coverage_score` は diagnostic/confidence 維持。
  - build-only/contentless verify の検出には有効だが、成果物 semantic success と完全には一致しない。
- `acceptance_confidence_score` は diagnostic 維持。
  - acceptance success の信頼度を見る指標として使い、成功率そのものにはしない。

次に hard gate 化を検討する前提条件:

- blind suite で同傾向が再現する。
- non-game scenarios の plan-output / plan-verify false negative が増えない。
- browser oracle unavailable / static inconclusive が人間レビューなしで deterministic に処理できる。
