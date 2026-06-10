# RWP-2: Terminal Shadow Limited Adoption Validation

作成日: 2026-06-10

## 1. 仮説

RWP-1 の shadow terminal projection を評価 taxonomy に limited adoption すれば、`done` と evidence projection が矛盾したケースを成功扱いし続けない足場になる。

この RWP では runtime の `final_outcome` は変えない。評価ログ上の `evaluation_taxonomy` と harness summary に shadow conflict を反映する。

## 2. 実装範囲

- `build_evaluation_taxonomy` に `shadow_terminal_projection` を入力として追加。
- `shadow_terminal_projection.conflict=true` かつ `final_outcome=done` の場合:
  - `evaluation_taxonomy.anvil_terminal_class=shadow_<class>`
  - `evaluation_taxonomy.failure_authority=<shadow class>`
- actor loop の eval record refresh order を `completion_reason -> terminal_diagnostics -> shadow_terminal_projection -> evaluation_taxonomy` に変更。
- `wp_eval_matrix.py` に shadow terminal columns を追加。
  - `shadow_terminal_class`
  - `shadow_terminal_conflict`
  - `shadow_missing_evidence`
  - `shadow_failed_evidence`
- summary に `shadow_conflict` と `By Shadow Terminal` を追加。

## 3. Deterministic Verification

- `cargo test --lib shadow_terminal_projection -- --nocapture`: passed
- `cargo test --lib evaluation_taxonomy_adopts_shadow_conflict_for_done -- --nocapture`: passed
- `python3 -m py_compile workspace/v0.6.11/wp_eval_matrix.py`: passed
- `cargo build`: passed
- `git diff --check`: passed

Key unit assertion:

- synthetic `done` record + unsatisfied `schema_evidence` with `verification_failure` becomes:
  - `shadow_terminal_projection.conflict=true`
  - `evaluation_taxonomy.anvil_terminal_class=shadow_evidence_failed`
  - `evaluation_taxonomy.failure_authority=evidence_failed`

## 4. Real LLM Validation

### Main smoke

Command:

```sh
python3 workspace/v0.6.11/wp_eval_matrix.py \
  --suite wp11 \
  --case-sequence data_csv,data_csv,data_csv,data_csv,data_csv,data_csv,toml_merge,toml_merge,toml_merge,toml_merge,toml_merge,toml_merge,docs_runbook,docs_runbook,docs_runbook,python_sales,python_sales,python_sales \
  --variant no_pam \
  --run-id rwp2-terminal-shadow-adoption-smoke-20260610 \
  --timeout-secs 420 \
  --chat-timeout-secs 180
```

Result:

- pass: 18/18
- high_quality: 17/18
- verification_pass: 17/18
- false_done: 0
- false_missing: 0
- repair_exhausted: 0
- shadow_conflict: 0

By case:

| case | pass | high_quality | total |
| --- | ---: | ---: | ---: |
| `data_csv` | 6 | 6 | 6 |
| `toml_merge` | 6 | 5 | 6 |
| `docs_runbook` | 3 | 3 | 3 |
| `python_sales` | 3 | 3 | 3 |

By shadow terminal:

| shadow_terminal_class | pass | high_quality | total |
| --- | ---: | ---: | ---: |
| `success` | 18 | 17 | 18 |

### TDD/current-turn companion smoke

Command:

```sh
python3 workspace/v0.6.11/wp_f_turn_authority_eval.py \
  --run-id rwp2-current-turn-tdd-smoke-20260610 \
  --timeout-secs 360 \
  --chat-timeout-secs 180
```

Result:

- pass: 3/4
- high_quality: 3/4
- verification_pass: 3/4
- `data_to_tdd`: pass/high_quality
- `docs_to_coding`: failed with unicode slugify expectation drift

## 5. Interpretation

RWP-2 successfully wires shadow terminal projection into evaluation taxonomy and the matrix summary without changing runtime completion behavior.

The main smoke had no shadow conflicts. This is expected because current shadow input still comes from terminal diagnostics, and terminal diagnostics does not yet include structured data schema failures or API behavior observations. Therefore RWP-2 is an adoption path, not a standalone quality improvement.

The one non-high-quality main row was `toml_merge`: external pass was true, terminal was `done`, but `verification_pass=false`. Shadow still showed `success` because terminal diagnostics saw verifier evidence as satisfied. This confirms that richer observation is needed before terminal alignment can improve.

## 6. Known Issues

- RWP-2 cannot detect data false-done unless RWP-3 emits schema evidence failure into the typed obligation/observation path.
- RWP-2 cannot detect API mismatch unless RWP-4 emits API contract observations.
- `docs_to_coding` unicode expectation drift persists and should remain in current-turn/TDD companion smoke.
- Because runtime `final_outcome` is unchanged, user-facing terminal behavior is not fixed yet. This is intentional for a limited adoption slice.

## 7. Next Step

Proceed to RWP-3: `StructuredDataObservation`.

RWP-3 should feed schema mismatch into the same shadow/adoption path so that `data_csv` false-done can become a visible `shadow_evidence_failed` or equivalent before any broader runtime terminal change is attempted.
