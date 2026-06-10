# RWP-4 ApiContractObservation Validation

Date: 2026-06-10

## Scope

RWP-4 adds an evidence-side `ApiContractObservation` for generic HTTP API tasks. The intent is to avoid FastAPI-specific repair rules while giving the active diagnostic and repair prompts a compact typed delta for request body binding and status drift.

Implemented:

- `ApiContractObservation` with observation kind, method, path, declared JSON body fields, request binding issue, and status policy.
- API contract payload projection for both diagnostic prompt and verifier repair prompt.
- Active repair guidance that treats `api_contract` as controller-computed provenance.
- `node_notes_api` smoke case to verify the mechanism is not FastAPI-only.

## Deterministic Verification

Commands:

- `python3 -m py_compile workspace/v0.6.11/wp_eval_matrix.py`
- `cargo test --lib api_contract -- --nocapture`
- `cargo build`
- `cargo fmt --check`
- `git diff --check`

Result:

- `api_contract` focused tests: 16 passed.
- Build and formatting checks passed.

Covered assertions:

- POST JSON body fields are observed as `request_schema_mismatch`.
- `json_body_fields_not_bound` is emitted for 422-style body/query binding failures.
- unspecified status policy does not force exact `201`.
- explicitly requested status is preserved.
- active verifier diagnostic and repair prompts include `api_contract`.

## Real LLM Validation

### Full RWP-4 Smoke

Run:

- `workspace/v0.6.11/eval-runs/rwp4-api-contract-observation-smoke2-20260610/results.csv`

Case sequence:

- `fastapi_notes` x8
- `node_notes_api` x3
- `python_sales` x3
- `docs_runbook` x1
- `data_csv` x1
- `data_json` x1

Result:

- Overall: 14/17 pass, 11/17 high_quality.
- FastAPI: 5/8 high_quality.
- Python/docs/data regression cases: 6/6 high_quality.
- `false_done`: 0.
- `false_missing`: 0.
- `shadow_terminal_conflict`: 0.
- `repair_exhausted`: 3.

Observed improvement:

- The first RWP-4 smoke before wiring the payload into the active diagnostic/repair path was 9/17 pass and 6/17 high_quality, with FastAPI 0/8.
- After adding `api_contract` to the active verifier diagnostic and repair payloads, FastAPI improved to 5/8 high_quality.

### Node HTTP-Style Harness Check

Run:

- `workspace/v0.6.11/eval-runs/rwp4-node-notes-api-harness-smoke-20260610/results.csv`

Result:

- `node_notes_api`: 3/3 pass, 3/3 high_quality.

This confirms the API observation work is not only tied to FastAPI. The harness now allows a minimal `package.json` for Node test execution while still treating README creation as leakage.

## Residual Issues

RWP-4 is directionally useful but not sufficient by itself.

- FastAPI failures still include malformed or non-convergent repairs after the API observation reaches the prompt.
- Two failed FastAPI rows are state/list-order expectation drift (`Another Note` vs `Test Note`), which is not an API request-body binding problem.
- One failed FastAPI row still returns 422 after repair attempts, indicating that typed observation improves target guidance but does not guarantee executable edit convergence.
- Additional benchmark-specific string rules should not be added here. The remaining failures fit RWP-6 typed repair action admissibility and broader authority/repair convergence work.

## Complexity Assessment

The new deterministic logic is bounded inside API contract observation and projection. It uses generic HTTP concepts (`method`, `path`, JSON request body fields, status policy) instead of framework-specific branches. Prompt text increased slightly, but it is driven by typed `api_contract` fields rather than benchmark names.

RWP-4 is complete. Continue with RWP-5/RWP-6 rather than adding more API-specific repair rules.
