# WP10 20-Run Regression Guard

Date: 2026-06-10

## Scope

WP10 ran a 20-row local LLM regression guard after WP1-WP9. The matrix used the same local model across coding and non-coding tasks:

- Python sales CLI
- docs runbook
- TOML merge
- Rust word counter
- Rust NDJSON
- Node JSON formatter
- Node CSV to JSON
- FastAPI notes API
- Python markdown lint
- data CSV output
- research brief
- ops command observation

The run used real Anvil invocations with `qwen3.6:27b-coding-nvfp4`; each row used an isolated workdir and state dir. The grader then ran local deterministic checks such as `pytest`, `cargo test`, `node --test` / `npm test`, file content checks, and JSON/CSV checks.

## Command

```bash
python3 workspace/v0.6.11/wp_eval_matrix.py \
  --suite wp10 \
  --run-id wp10-20run-20260610 \
  --anvil-bin target/debug/anvil \
  --model qwen3.6:27b-coding-nvfp4 \
  --sidecar-model qwen3-coder:30b \
  --max-iterations 20 \
  --chat-timeout-secs 180 \
  --timeout-secs 420
```

## Result

- pass: 14/20
- high quality: 14/20
- verification pass: 15/20
- false-done: 1
- false-missing: 2
- repair exhausted: 2
- max-iterations: 0

By case:

| Case | Pass | Total |
| --- | ---: | ---: |
| Python sales CLI | 3 | 3 |
| Docs runbook | 2 | 2 |
| TOML merge | 2 | 2 |
| Rust word counter | 2 | 2 |
| Rust NDJSON | 0 | 1 |
| Node JSON formatter | 2 | 2 |
| Node CSV to JSON | 2 | 2 |
| FastAPI notes API | 0 | 2 |
| Python markdown lint | 1 | 1 |
| Data CSV output | 0 | 1 |
| Research brief | 0 | 1 |
| Ops command observation | 0 | 1 |

## Interpretation

WP10 shows meaningful recovery from the v0.6.11 Python sales regression:

- Python sales was 3/3 in this matrix.
- Node JSON and Node CSV were both 2/2.
- TOML manually passed 2/2, which is a strong improvement signal against the previous 0% wall.

However, the strict regression guard is not passed because false terminal alignment issues remain:

- Data CSV exited `done` but produced an extra `same` column.
- TOML passed the external deterministic grader but exited `safe_stop_verifier_missing`.
- FastAPI failed 2/2 with `repair_exhausted` around 422 response shape mismatch.
- Research and ops did not create the requested artifact and stopped at `missing_repo_edits`.
- Rust NDJSON failed with a generated test/import binding mismatch.

## Architecture Assessment

The direction remains valid: contract-bound generation, style separation, and typed repair signals improved several hard coding paths without benchmark-specific branches. The run also confirms that Python sales recovered in this sample.

The remaining failures are not random:

- non-coding research/ops can still be incorrectly gated by repo-edit expectations;
- data tasks can still produce semantically wrong but superficially complete files;
- TOML can satisfy the real artifact/evidence contract while internal verifier binding reports missing evidence;
- FastAPI repair still lacks enough typed API/request-schema reconciliation;
- Rust NDJSON still has source/test binding drift.

Therefore WP10 is a useful regression measurement, but it does not justify a broad success-rate improvement claim by itself. WP11 should still be run because the user requested WP4-WP11 completion, but any WP11 improvement claim must be conditional and must preserve these WP10 failures as known issues.

## Artifacts

- `workspace/v0.6.11/eval-runs/wp10-20run-20260610/results.csv`
- `workspace/v0.6.11/eval-runs/wp10-20run-20260610/summary.md`
- `workspace/v0.6.11/eval-runs/wp10-20run-20260610/summary.json`
