# v0.6.11 Residual Baseline

作成日: 2026-06-10

参照:

- `workspace/v0.6.11/residual-hypothesis-validation-work-plan-20260610.md`
- `workspace/v0.6.11/wp-known-issues-20260610.md`
- `workspace/v0.6.11/eval-runs/wp10-20run-20260610/results.csv`
- `workspace/v0.6.11/eval-runs/wp11-50run-20260610/results.csv`

## 1. 目的

RWP-1 以降の改善が本当に残課題へ効いたか判断するため、代表 failure signature と今後の smoke suite を固定する。

この RWP-0 では product behavior は変更しない。追加したのは評価 harness の `feature_discount` ケースのみで、既存機能改善カテゴリを今後の regression guard に含めるためのもの。

## 2. 既存 20/50-run から見える失敗分布

### WP10 20-run

- pass: 14/20
- high_quality: 14/20
- false_done: 1
- false_missing: 2
- repair_exhausted: 2

代表行:

| case | signature | observed terminal | interpretation |
| --- | --- | --- | --- |
| `data_csv` | false-done | `done` | artifact exists but CSV content/schema mismatch |
| `toml_merge` | false-missing | `safe_stop_verifier_missing` | external grader passed but internal terminal missed evidence |
| `fastapi_notes` | repair convergence | `repair_exhausted` | HTTP request/response mismatch not repaired |
| `research_cache` | non-coding leakage | `missing_repo_edits` | report artifact not created, coding edit pressure leaked |
| `ops_health` | non-coding leakage | `missing_repo_edits` | command report artifact not created |
| `rust_ndjson` | source/test binding drift | `verifier_failed` | generated tests imported from wrong root path |

### WP11 50-run

- pass: 38/50
- high_quality: 35/50
- false_done: 2
- false_missing: 5
- repair_exhausted: 8

Representative failures:

| case | high_quality | total | dominant signature |
| --- | ---: | ---: | --- |
| `data_csv` | 0 | 2 | false-done / schema evidence gap |
| `research_cache` | 0 | 2 | non-coding deliverable not created |
| `ops_health` | 0 | 2 | non-coding deliverable not created |
| `fastapi_notes` | 1 | 6 | API contract repair convergence |
| `toml_merge` | 4 | 6 | false-missing / verifier binding drift |
| `python_markdown` | 1 | 2 | artifact externally passes but terminal may report repair exhaustion |

## 3. RWP-0 実 LLM baseline smoke

### Focused residual smoke

Command:

```sh
python3 workspace/v0.6.11/wp_eval_matrix.py \
  --suite wp11 \
  --case-sequence data_csv,toml_merge,fastapi_notes,research_cache,ops_health,python_sales,docs_runbook \
  --variant no_pam \
  --run-id rwp0-residual-baseline-smoke-20260610 \
  --timeout-secs 420 \
  --chat-timeout-secs 180
```

Result:

- pass: 6/7
- high_quality: 6/7
- false_done: 0
- false_missing: 0
- repair_exhausted: 0
- PAM availability: `no_pam:disabled`

Rows:

| case | result | terminal | observation |
| --- | --- | --- | --- |
| `data_csv` | pass | `done` | simple data case passed in this single run |
| `toml_merge` | pass | `done` | false-missing did not reproduce in this single run |
| `fastapi_notes` | fail | `verifier_failed` | POST note returned JSON without `id`; API contract/evidence issue reproduced |
| `research_cache` | pass | `done` | non-coding leakage did not reproduce in this single run |
| `ops_health` | pass | `done` | command report passed in this single run |
| `python_sales` | pass | `done` | coding regression guard passed |
| `docs_runbook` | pass | `done` | docs regression guard passed |

Interpretation:

- A single smoke is not enough to claim improvement or disappearance of previous failures.
- FastAPI remains a stable hard case even after WP-E.
- Data/research/ops failures are probabilistic or condition-sensitive, so RWP-9/10 must keep multi-run coverage.

### Current-turn / TDD smoke

Command:

```sh
python3 workspace/v0.6.11/wp_f_turn_authority_eval.py \
  --run-id rwp0-current-turn-tdd-smoke-20260610 \
  --timeout-secs 360 \
  --chat-timeout-secs 180
```

Result:

- pass: 3/4
- high_quality: 3/4
- verification_pass: 3/4

Rows:

| case | turn kinds | result | observation |
| --- | --- | --- | --- |
| `coding_to_data` | coding -> data | pass | current-turn data artifact produced |
| `coding_to_docs` | coding -> docs | pass | current-turn docs artifact produced |
| `docs_to_coding` | docs -> coding | fail | unicode slugify expectation drift; both turns ended `repair_safe_stop` |
| `data_to_tdd` | data -> coding_tdd | pass | TDD deliverable and tests passed |

Interpretation:

- Current-turn authority remains mostly effective in this small sample.
- TDD must stay in future smoke because it can pass while still needing order/adherence checks.
- `docs_to_coding` shows repair/expectation drift can still appear in continued sessions.

### Feature improvement smoke

The existing matrix did not include a feature-improvement case, so RWP-0 added `feature_discount` to `workspace/v0.6.11/wp_eval_matrix.py`.

Command:

```sh
python3 workspace/v0.6.11/wp_eval_matrix.py \
  --suite wp11 \
  --case-sequence feature_discount \
  --variant no_pam \
  --run-id rwp0-feature-improvement-smoke-20260610 \
  --timeout-secs 420 \
  --chat-timeout-secs 180
```

Result:

- pass: 0/1
- high_quality: 0/1
- terminal: `missing_repo_edits`
- changed files: `tests/__pycache__/test_discounts.cpython-312.pyc`

Observation:

- The existing test passed without requiring the requested behavior change.
- The controller correctly refused `done` because no repository edit was recorded.
- The external direct behavior check failed because `final_price(100, 15)` still returned the old behavior.

Interpretation:

- This is a useful RWP-5/RWP-6 signature: existing-project feature improvement needs both fresh implementation edit evidence and behavior evidence.
- Future fixes must not weaken the fresh-edit guard for coding feature changes.

## 4. Fixed residual smoke suite

RWP-1 through RWP-8 should use this focused suite for fast feedback:

| category | case | source |
| --- | --- | --- |
| data false-done | `data_csv` | `wp_eval_matrix.py` |
| TOML false-missing | `toml_merge` | `wp_eval_matrix.py` |
| API repair | `fastapi_notes` | `wp_eval_matrix.py` |
| research deliverable | `research_cache` | `wp_eval_matrix.py` |
| ops command report | `ops_health` | `wp_eval_matrix.py` |
| neutral coding | `python_sales` | `wp_eval_matrix.py` |
| docs | `docs_runbook` | `wp_eval_matrix.py` |
| feature improvement | `feature_discount` | `wp_eval_matrix.py` |
| current-turn contamination | `coding_to_data`, `coding_to_docs`, `docs_to_coding` | `wp_f_turn_authority_eval.py` |
| TDD | `data_to_tdd` | `wp_f_turn_authority_eval.py` |

Minimum smoke command for RWP-1/RWP-2:

```sh
python3 workspace/v0.6.11/wp_eval_matrix.py \
  --suite wp11 \
  --case-sequence data_csv,toml_merge,fastapi_notes,research_cache,ops_health,python_sales,docs_runbook,feature_discount \
  --variant no_pam \
  --run-id <rwp-run-id> \
  --timeout-secs 420 \
  --chat-timeout-secs 180
```

TDD/current-turn companion smoke:

```sh
python3 workspace/v0.6.11/wp_f_turn_authority_eval.py \
  --run-id <rwp-run-id> \
  --timeout-secs 360 \
  --chat-timeout-secs 180
```

## 5. Hypothesis mapping

| hypothesis | baseline evidence | next WP |
| --- | --- | --- |
| H1: terminal alignment needs EvidenceObservation projection | data false-done, TOML false-missing, Python markdown repair terminal drift | RWP-1, RWP-2 |
| H2: Data/API need evidence-side observation | `data_csv` schema mismatch in WP10/WP11; `fastapi_notes` failed again in RWP-0 | RWP-3, RWP-4 |
| H3: non-coding leakage comes from obligation construction | research/ops `missing_repo_edits` in WP10/WP11 | RWP-5 |
| H4: repair_exhausted comes from diagnosis/action disconnect | FastAPI/Python markdown/Rust NDJSON repair failures | RWP-6 |
| H5: PAM quality impact needs injected availability | WP-G reported `pam:failed`, no injection | RWP-7 |
| H6: complexity guard is required | task_contract/loop responsibilities remain broad | RWP-8 |

## 6. RWP-0 conclusion

RWP-0 fixed the residual baseline and added the missing feature-improvement harness case.

No success-rate improvement is claimed from RWP-0. The important output is a reproducible focused suite and failure-signature map. RWP-1 should start with shadow terminal projection rather than changing completion behavior.
