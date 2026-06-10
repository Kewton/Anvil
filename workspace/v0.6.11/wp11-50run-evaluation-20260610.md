# WP11 50-Run Improvement Claim

Date: 2026-06-10

## Scope

WP11 ran a 50-row local LLM evaluation after WP10:

- 25 no-PAM rows
- 25 PAM-enabled rows
- same case distribution across both variants
- real `qwen3.6:27b-coding-nvfp4` Anvil invocations
- local deterministic graders for artifacts and verification

Important caveat:

- PAM env was enabled for the `pam` variant, but Photon context/evaluate calls failed and no memory was injected (`agent.photon_context_pack.completed failed=true`, `adoption_status=not_injected`). Therefore this run cannot claim a positive or negative PAM-memory effect. It only compares normal execution against "PAM attempted but unavailable".

## Command

```bash
python3 workspace/v0.6.11/wp_eval_matrix.py \
  --suite wp11 \
  --run-id wp11-50run-20260610 \
  --anvil-bin target/debug/anvil \
  --model qwen3.6:27b-coding-nvfp4 \
  --sidecar-model qwen3-coder:30b \
  --max-iterations 20 \
  --chat-timeout-secs 180 \
  --timeout-secs 420
```

## Result

- pass: 38/50
- high quality: 35/50
- verification pass: 37/50
- false-done: 2
- false-missing: 5
- repair exhausted: 8
- max-iterations: 0

By variant:

| Variant | Pass | High Quality | Total |
| --- | ---: | ---: | ---: |
| no-PAM | 19 | 18 | 25 |
| PAM attempted | 19 | 17 | 25 |

By case:

| Case | Pass | High Quality | Total |
| --- | ---: | ---: | ---: |
| Python sales CLI | 8 | 8 | 8 |
| Docs runbook | 4 | 4 | 4 |
| TOML merge | 6 | 4 | 6 |
| Rust word counter | 4 | 4 | 4 |
| Rust NDJSON | 3 | 3 | 4 |
| Node JSON formatter | 6 | 6 | 6 |
| Node CSV to JSON | 4 | 4 | 4 |
| FastAPI notes API | 1 | 1 | 6 |
| Python markdown lint | 2 | 1 | 2 |
| Data CSV output | 0 | 0 | 2 |
| Research brief | 0 | 0 | 2 |
| Ops command observation | 0 | 0 | 2 |

## Improvement Assessment

The result is a strong raw success signal for the current architecture:

- Python sales recovered to 8/8 in this run.
- Node JSON and Node CSV were both 100%.
- Rust word was 100%.
- Rust NDJSON improved to 3/4.
- TOML produced externally valid artifacts in 6/6, which is a major signal against the previous 0% wall.

However, this is not a clean, unconditional architecture improvement claim:

- The WP11 harness is a focused v0.6.11 matrix, not exactly the historical v0.6.10/v0.6.11 performance harness.
- false-done is not zero.
- false-missing is not zero.
- FastAPI repair convergence is still poor.
- non-coding research/ops admission is still wrong.
- PAM was unavailable, so memory/advisory impact was not actually measured.

Therefore the defensible claim is:

- success-rate signal improved materially in this focused matrix;
- the architecture direction is still valid;
- terminal authority and non-coding objective admission are not yet good enough to declare the system broadly stable.

## Failure Composition

### False-Done

Data CSV failed in both variants:

- Anvil exited `done`;
- the file existed;
- but content added an extra `same` column.

This is a content/schema acceptance failure. It should be handled by stronger typed data schema evidence, not by a CSV-specific branch.

### False-Missing

TOML often produced externally valid artifacts but exited `safe_stop_verifier_missing` or `repair_exhausted`.

This means artifact/evidence reality and internal verifier binding still disagree. The next work should focus on making EvidenceRunner observations bind to declared artifacts and external deterministic checks more precisely.

### Repair Exhausted

FastAPI failed 5/6, mostly with 422 response mismatches. The controller/repair loop still does not reconcile API request schema, Pydantic model shape, and tests strongly enough.

Python markdown had one externally passing artifact with `repair_exhausted`, another terminal alignment mismatch.

### Non-Coding Admission

Research and ops failed 0/2 each with `missing_repo_edits` and no artifact. This is a direct signal that non-coding objective admission still inherits coding/edit-obligation assumptions in some paths.

## Architecture Direction

Continue the current direction, but shift the next work to the remaining authority gaps:

1. ObjectiveContract construction must be current-turn authoritative for non-coding tasks.
2. Data tasks need typed schema/content evidence, not surface file-existence acceptance.
3. EvidenceRunner binding must be able to accept real externally passing checks without `safe_stop_verifier_missing`.
4. FastAPI-style request/response schema repair needs generic API contract reconciliation.
5. PAM should remain advisory; no success claim should depend on it until context injection succeeds in the logs.

## Artifacts

- `workspace/v0.6.11/eval-runs/wp11-50run-20260610/results.csv`
- `workspace/v0.6.11/eval-runs/wp11-50run-20260610/summary.md`
- `workspace/v0.6.11/eval-runs/wp11-50run-20260610/summary.json`
