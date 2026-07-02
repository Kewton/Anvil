# 022-1 Failure Taxonomy Gate Implementation Result

作成日: 2026-06-30

## 実装内容

022-1 に従い、failure taxonomy / report / preflight gate を実装した。

主な変更:

- `eval-summary-v3-failure-gate` へ schema version を更新。
- `summary.eval.tsv` に top-level `failure_kind` 列を追加。
- process/runtime classification と acceptance failure を統合して `failure_kind` へ落とすようにした。
- `failure_classification.py` に以下を追加。
  - `normalize_failure_kind`
  - `failure_kind_required_for_row`
  - `blank_failure_kind_gate_violations`
- `report.py` の failure summary / failure layer / stop reason が top-level `failure_kind` と acceptance failure を見るようにした。
- `parity_gate.py` を追加し、`parity_gate_report.json` の schema / gate partition / blank failure count を検査できるようにした。
- pytest を追加。
  - `test_parity_gate_report.py`
  - `test_failure_classification.py` の gate cases
  - `test_summary_schema.py` の top-level `failure_kind` case

## 確認結果

実行:

```bash
python3 -m pytest mvp/anvilminimal/tests/eval
cargo test --manifest-path mvp/anvilminimal/Cargo.toml
```

結果:

- `pytest mvp/anvilminimal/tests/eval`: 166 passed, 1 skipped
- `cargo test --manifest-path mvp/anvilminimal/Cargo.toml`: passed

## 既存最新 smoke の gate 確認

新しい normalizer で既存 summary を確認した。

| Summary | Rows | Blank failure kind gate violations |
| --- | ---: | ---: |
| `/private/tmp/anvilminimal-eval-0219-mvp-smoke/summary.eval.tsv` | 48 | 0 |
| `/private/tmp/anvilminimal-eval-0219-provider-smoke/summary.eval.tsv` | 24 | 0 |

正規化後の failure kind:

`mvp-smoke`:

- `phase_scaffold_error`: 2
- `plan_final_contract_failure`: 1
- `plan_output_missing_required_capabilities`: 6
- `step_verify_failure`: 3
- `verify_command_policy_error`: 2
- `verify_repair_no_change`: 2

`mvp-provider-smoke`:

- `phase_scaffold_error`: 3
- `plan_output_missing_required_capabilities`: 4
- `verify_command_policy_error`: 2

## Gate 判定

G-S14 は `fail` から `partial` に変更した。

理由:

- blank failure kind gate は helper / pytest / existing summary normalization で満たした。
- ただし、fresh eval summary はまだ `eval-summary-v3-failure-gate` として再生成していない。
- normalized source/MVP trace diff は 022-2 の対象であり、まだ未実装。

したがって現時点では `pass` ではなく `partial` が妥当。
