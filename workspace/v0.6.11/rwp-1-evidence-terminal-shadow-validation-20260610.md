# RWP-1: EvidenceObservation Terminal Shadow Validation

作成日: 2026-06-10

## 1. 仮説

terminal 判定をすぐ置き換える前に、typed evidence / obligation から shadow terminal を作れば、現行 terminal と evidence projection の差分を観測できる。

RWP-1 では behavior change を入れない。`final_outcome` / `completion_reason` / `evaluation_taxonomy` は既存のままにし、診断用の shadow field だけを追加する。

## 2. 実装範囲

- `EvidenceObservation` から pure projection する `EvidenceShadowTerminalProjection` を追加。
- `EvalRecord` に `shadow_terminal_projection` を追加。
- `shadow_terminal_projection` は `terminal_diagnostics.obligations` から作る diagnostic-only summary。
- projection には次を含める。
  - `class`
  - `current_terminal_class`
  - `conflict`
  - `satisfied_evidence_ids`
  - `missing_evidence_ids`
  - `failed_evidence_ids`
  - `source`
  - `reason`
- actor loop 終端で `refresh_terminal_diagnostics()` 後に `refresh_shadow_terminal_projection()` を呼ぶ。

## 3. Deterministic Verification

- `cargo test --lib evidence_observation -- --nocapture`: passed
- `cargo test --lib shadow_terminal_projection -- --nocapture`: passed
- `cargo test --lib build_eval_record_basic -- --nocapture`: passed
- `cargo build`: passed
- `git diff --check`: passed

追加した unit coverage:

- passed observation -> shadow `success`
- missing observation -> shadow `missing_evidence`
- failed + missing observation -> shadow `evidence_failed`
- empty observations -> shadow `not_observed`
- terminal diagnostics with schema evidence failure under `done` -> `conflict=true`
- `safe_stop_verifier_missing` -> missing evidence projection
- verifier-backed `done` -> success projection

## 4. Real LLM Validation

Command:

```sh
python3 workspace/v0.6.11/wp_eval_matrix.py \
  --suite wp11 \
  --case-sequence data_csv,data_csv,data_csv,toml_merge,toml_merge,toml_merge,docs_runbook,docs_runbook,python_sales,python_sales \
  --variant no_pam \
  --run-id rwp1-terminal-shadow-smoke-20260610 \
  --timeout-secs 420 \
  --chat-timeout-secs 180
```

Result:

- pass: 10/10
- high_quality: 10/10
- verification_pass: 10/10
- false_done: 0
- false_missing: 0
- repair_exhausted: 0
- PAM availability: `no_pam:disabled`

Shadow projection rows:

| case group | rows | shadow class | conflict | satisfied evidence |
| --- | ---: | --- | --- | --- |
| `data_csv` | 3 | `success` | `false` | `artifact_evidence` |
| `toml_merge` | 3 | `success` | `false` | `verification_evidence` |
| `docs_runbook` | 2 | `success` | `false` | `artifact_evidence` |
| `python_sales` | 2 | `success` | `false` | `verification_evidence` |

## 5. Interpretation

RWP-1 succeeded as an observability slice:

- The new field is present in eval logs.
- It does not change runtime terminal behavior.
- Non-coding artifact evidence and coding verifier evidence are visible through the same shadow shape.
- No regression appeared in the focused smoke.

This does not yet solve false-done or false-missing. The shadow projection is only as strong as the typed obligations available in `terminal_diagnostics`. Data schema mismatch and API behavior mismatch still need evidence-side observations before the shadow can expose them reliably.

## 6. Known Issues

- Shadow projection currently derives from `terminal_diagnostics`, not directly from a retained per-turn `EvidenceObservation` list. This keeps RWP-1 small but means RWP-2/RWP-3 should wire richer observations before broad adoption.
- Historical verifier failure signatures may remain in a successful record after recovery. RWP-1 intentionally does not treat `last_failure_signature` alone as failed evidence because it can describe a repaired intermediate state.
- External grader truth is not available inside `EvalRecord`; false-done/false-missing against external checks must still be detected by the evaluation harness.

## 7. Next Step

RWP-2 should only adopt shadow terminal projection for cases where objective-bound evidence is explicitly represented. Based on this smoke, safe initial adoption candidates are:

- docs/data artifact evidence already marked as `artifact_evidence`
- verifier-backed coding success already marked as `verification_evidence`

RWP-2 should not claim data/API correctness until RWP-3/RWP-4 add schema/API observations.
