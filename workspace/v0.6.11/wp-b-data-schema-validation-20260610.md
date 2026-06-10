# WP-B Data Schema Evidence Validation

作成日: 2026-06-10

参照:

- `workspace/v0.6.11/hypothesis-validation-work-plan-20260610.md`
- `workspace/v0.6.11/eval-runs/wp-b-data-schema-smoke-20260610/summary.md`
- `workspace/v0.6.11/eval-runs/wp-b-data-schema-smoke-20260610/results.csv`

## 実装内容

WP-B の最小 slice として、既存 `StructuredRecordSchema` を拡張し、列の扱いを typed policy にした。

追加した型:

- `StructuredColumnPolicy::RequiredOnly`
- `StructuredColumnPolicy::Exact`

方針:

- 既存挙動は `RequiredOnly` のまま維持する。
- user request が `exactly/same/only/no extra` のように列の完全一致を明示した場合だけ `Exact` にする。
- CSV/JSON などの benchmark 名ではなく、`StructuredRecordSchema` の属性として exactness を扱う。
- JSON object は従来どおり top-level fields を exact 扱いにする。

今回直した false-done path:

- `Read input/orders.csv and create output/order-summary.csv with exactly the same columns id,total ...`
- 旧挙動: `id,total,same` でも required columns を含むため `done` になり得た。
- 新挙動: `StructuredColumnPolicy::Exact` により extra column を schema mismatch とする。

## 最小検証

実行したコマンド:

```bash
cargo test --lib data_schema -- --nocapture
cargo test --lib data_task -- --nocapture
cargo test --lib evidence_observation -- --nocapture
python3 -m py_compile workspace/v0.6.11/wp_eval_matrix.py
cargo build
```

結果:

- `data_schema`: 7/7 pass
- `data_task`: 7/7 pass
- `evidence_observation`: 9/9 pass
- `wp_eval_matrix.py` syntax check: pass
- `cargo build`: pass

追加した focused coverage:

- exact column policy rejects extra CSV columns.
- required-only policy preserves existing superset acceptance.
- data-only prompt with `exactly the same columns id,total` does not treat `same` as a column.
- extra-column `output/order-summary.csv` returns `Continue { DataOutput }`, not `Done`.
- exact-column artifact can still reach `Done`.

## 実 LLM 検証

最終コードで以下を実行した。

```bash
python3 workspace/v0.6.11/wp_eval_matrix.py \
  --suite wp10 \
  --case-sequence data_csv,data_csv,data_csv,data_csv,data_csv,data_csv,data_json,data_json,data_json,docs_runbook,docs_runbook,python_sales,python_sales \
  --run-id wp-b-data-schema-smoke-20260610 \
  --anvil-bin target/debug/anvil \
  --max-iterations 20 \
  --chat-timeout-secs 180 \
  --timeout-secs 420
```

結果:

| metric | result |
| --- | ---: |
| total | 13 |
| pass | 13/13 |
| high_quality | 13/13 |
| verification_pass | 13/13 |
| false_done | 0 |
| false_missing | 1 |
| repair_exhausted | 0 |
| max_iterations | 0 |

ケース別:

| case | pass | high_quality | total |
| --- | ---: | ---: | ---: |
| data_csv | 6 | 6 | 6 |
| data_json | 3 | 3 | 3 |
| docs_runbook | 2 | 2 | 2 |
| python_sales | 2 | 2 | 2 |

## インサイト

H3 は支持された。

data CSV の false-done は、data task 全体の能力不足ではなく、列 schema の exactness が typed contract に存在しないことが主因だった。`StructuredColumnPolicy::Exact` を追加すると、`id,total,same` のような余分列を deterministic schema mismatch として扱える。

一方で、今回の実 LLM run では model が全 data case を正しく作成したため、実 LLM 上で「不正 artifact を repair して pass する」挙動までは観測していない。これは unit/focused tests が担保している。

## 残課題

data run の runtime log では、現時点では主に `file_layout_check` observation が出ている。schema pass/fail 自体を `EvidenceObservation(kind=schema_check)` として明示記録するところまでは未実装。

次に進めるなら、個別 CSV rule を足すのではなく、以下のどちらかを小さく検証する。

- artifact excerpt evaluation から `schema_check` EvidenceObservation を生成する。
- terminal projection が `StructuredRecordSchema` の pass/fail を typed observation として参照できるようにする。

また、coding 側では Python sales 1/2 が外部高品質にもかかわらず `safe_stop_verifier_missing` になった。これは WP-A で見えた TOML と同じ evidence binding/projection gap であり、WP-B の data schema 問題とは別系統。
