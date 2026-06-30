# Parity Gate Report Schema

作成日: 2026-06-29

## 1. 目的

`parity_gate_report.json` は、runtime semantics parity gate の結果を機械的に検証するための report である。Markdown の説明だけでは gate 通過扱いにしない。

## 2. Required Top-level Fields

```json
{
  "schema_version": "1",
  "generated_at": "2026-06-29T00:00:00+09:00",
  "gate_level": "comparative",
  "baseline_commit": "...",
  "source_trace_manifest": "workspace/mvp/eval/022/source_mvp_trace_manifest.md",
  "mvp_trace_manifest": "workspace/mvp/eval/022/source_mvp_trace_manifest.md",
  "matrix_path": "workspace/mvp/eval/022/runtime_semantics_gate_matrix.md",
  "summary_paths": [],
  "required_gate_ids": [],
  "passed_gate_ids": [],
  "partial_gate_ids": [],
  "failed_gate_ids": [],
  "intentionally_different_gate_ids": [],
  "failure_kind_blank_count": 0,
  "anvildev_comparison": {
    "status": "missing_current_same_condition_trace",
    "success_rate_delta_pp": null,
    "stage_regressions": []
  },
  "uat_equivalent": {
    "status": "partial",
    "evidence_paths": []
  },
  "current_success": {},
  "errors": [],
  "warnings": []
}
```

## 3. Field Semantics

| Field | Rule |
| --- | --- |
| `schema_version` | 現時点は `"1"` 固定 |
| `gate_level` | `local` / `network` / `comparative` / `release` |
| `required_gate_ids` | `G-S01`〜`G-S16` を全件含める |
| `passed_gate_ids` | source/MVP trace、fixture、failure taxonomy、eval、UAT 条件を満たすもののみ |
| `partial_gate_ids` | trace 不足、fixture 不足、UAT 不足、comparison 不足の gate |
| `failed_gate_ids` | 現在の失敗、または false positive を許す gate |
| `failure_kind_blank_count` | process/acceptance failure の空 failure kind 件数。success/dry-run/skipped は除外 |
| `anvildev_comparison.status` | `pass` / `warn` / `fail` / `missing_current_same_condition_trace` |
| `uat_equivalent.status` | `pass` / `partial` / `fail` / `not_run` |
| `errors` | gate failure。CI fail 候補 |
| `warnings` | rollout 中の warn |

## 4. Validation Policy

初期導入では以下を pytest で確認する。

- JSON として parse できる。
- `required_gate_ids` が `G-S01`〜`G-S16` と一致する。
- `passed_gate_ids` / `partial_gate_ids` / `failed_gate_ids` / `intentionally_different_gate_ids` の合計が required set を過不足なく覆う。
- `failure_kind_blank_count > 0` の場合、`errors` に failure taxonomy gap が含まれる。
- `gate_level` が `comparative` 以上で `anvildev_comparison.status == missing_current_same_condition_trace` の場合、full pass にしてはいけない。
- `gate_level == release` で browser/interaction evidence がない場合、`uat_equivalent.status` は `pass` になってはいけない。
