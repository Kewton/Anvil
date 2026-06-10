# WP-A EvidenceObservation Validation

作成日: 2026-06-10

参照:

- `workspace/v0.6.11/hypothesis-validation-work-plan-20260610.md`
- `workspace/v0.6.11/eval-runs/wp-a-evidence-observation-smoke2-20260610/summary.md`
- `workspace/v0.6.11/eval-runs/wp-a-evidence-observation-smoke2-20260610/results.csv`

## 実装内容

WP-A として、terminal projection の入力を将来一箇所へ集約するための最小型 `EvidenceObservation` を追加した。

追加した観測 shape:

- `kind`: `ObjectiveEvidenceKind`
- `status`: `passed` / `failed` / `missing`
- `bound_artifacts`: artifact path list
- `runner`: `EvidenceRunnerKind`
- `source`: `completion_evidence` / `evidence_runner` / `verifier`
- `diagnostic_summary`: verifier class / bound count / schema columns / failure reason

既存 terminal 判定は変更していない。今回の変更は既存 `CompletionEvidence` から `EvidenceObservation` を作る adapter と、以下の観測ログ追加に限定した。

- repo edit -> `file_layout_check`
- Bash/verifier exit zero -> `test_run`
- task evidence runner command -> objective evidence kind

ログイベント:

- `agent.evidence_observation.observed`

## 最小検証

実行したコマンド:

```bash
cargo test --lib evidence_observation -- --nocapture
cargo test --lib evidence_runner -- --nocapture
cargo test --lib verifier_exit_zero -- --nocapture
cargo test --lib repo_edit -- --nocapture
python3 -m py_compile workspace/v0.6.11/wp_eval_matrix.py
cargo build
```

結果:

- `evidence_observation`: 9/9 pass
- `evidence_runner`: 11/11 pass
- `verifier_exit_zero`: 14/14 pass
- `repo_edit`: 46/46 pass
- `cargo build`: pass
- `wp_eval_matrix.py` syntax check: pass

補足:

- sandbox 内の `cargo test --lib repo_edit` は mockito の local server 起動が `Operation not permitted` で失敗した。
- 同じコマンドを権限付きで再実行し、46/46 pass を確認した。

## 実 LLM 検証

最終コードで以下を実行した。

```bash
python3 workspace/v0.6.11/wp_eval_matrix.py \
  --suite wp10 \
  --case-sequence python_sales,python_sales,toml_merge,toml_merge,docs_runbook \
  --run-id wp-a-evidence-observation-smoke2-20260610 \
  --anvil-bin target/debug/anvil \
  --max-iterations 20 \
  --chat-timeout-secs 180 \
  --timeout-secs 420
```

結果:

| metric | result |
| --- | ---: |
| total | 5 |
| pass | 5/5 |
| high_quality | 5/5 |
| verification_pass | 5/5 |
| false_done | 0 |
| false_missing | 2 |
| repair_exhausted | 0 |
| max_iterations | 0 |

ケース別:

| case | pass | high_quality | total |
| --- | ---: | ---: | ---: |
| python_sales | 2 | 2 | 2 |
| toml_merge | 2 | 2 | 2 |
| docs_runbook | 1 | 1 | 1 |

## 観測ログの確認

ログから以下を確認した。

- Python sales:
  - `main.py` / `tests/test_main.py` が `file_layout_check passed` として記録された。
  - structured verifier 成功が `test_run passed` / `runner=coding_build_test` / `bound_test_artifacts_count=1` として記録された。
- Docs runbook:
  - `runbooks/local-agent-triage.md` が `file_layout_check passed` として記録された。
- TOML merge:
  - `merge_toml.py` / `tests/test_merge_toml.py` が `file_layout_check passed` として記録された。
  - terminal は `safe_stop_verifier_missing` のままだった。

## インサイト

WP-A の仮説 H1 は一部支持された。

`EvidenceObservation` により、artifact/deliverable 観測と verifier 観測を同じログ shape で追えるようになった。特に TOML では、外部グレードとローカル pytest は成功しているのに、Anvil terminal は `safe_stop_verifier_missing` になった。この差分は以下の形で明確になった。

- deliverable 側: `file_layout_check passed`
- terminal 側: `safe_stop_verifier_missing`
- verifier observation 側: `test_run passed` が存在しない

つまり、TOML false-missing の主因は「成果物がない」ではなく、「成果物/テストは作られたが、Anvil の verifier binding/projection に `test_run` として入っていない」ことに寄っている。

## 次の判断

次は個別 TOML ルールを増やすのではなく、計画どおり以下に進むのが妥当。

- `EvidenceRunnerSelection` と `EvidenceBinding` の分離を深める。
- 外部的に実行可能なテストが存在するが `test_run` observation がないケースを、typed observation gap として扱う。
- terminal projection を `ArtifactLedger + EvidenceObservation + ObjectiveContract` へ寄せる。ただし WP-A 時点では behavior change を入れない。

なお、WP-A は成功率改善を主張する変更ではない。今回の 5-run は回帰なしと診断可能性向上の smoke であり、改善主張には 20-run / 50-run が必要。
