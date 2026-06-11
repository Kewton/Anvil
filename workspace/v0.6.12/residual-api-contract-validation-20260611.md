# Residual API Contract Validation 2026-06-11

## Context

- Base residual check was fixed at HEAD `c500d89`.
- The stable residual for `fastapi_notes` was not implementation absence, but contract expectation drift in generated tests:
  - `GET /notes returning an empty list` was sometimes expanded into undeclared list mutation/order expectations.
  - `returning the created note` was often expanded into undeclared exact HTTP `201` assertions.
  - one run still bound JSON body fields as top-level handler parameters and returned `422`.

## Change

Implemented a small typed-contract refinement instead of a FastAPI-specific branch:

- `ApiContractExpectation` now emits `response_shape=empty_collection` when a request declares an empty list/array response.
- unspecified API status now emits `status_assertion_policy=no_exact_http_status`.
- contract-bound generation, artifact-directed repair context, and test expectation audit consume those typed facts:
  - JSON fields must be request body object fields, not query/form/separate handler parameters.
  - tests must not compare `status_code` to numeric literals when no exact status was declared.
  - empty collection response shape does not imply cross-endpoint persistence, post-to-list mutation, list length after writes, or ordering.

## Deterministic Verification

Passed:

- `cargo test api_contract --lib`
- `cargo test contract_bound_generation --lib`
- `cargo test contract_generation_expectations --lib`
- `cargo test test_expectation_audit --lib`
- `cargo test artifact_directed_api_context --lib`
- `cargo build`

Notes:

- `cargo test --lib` was also attempted, but sandbox-local mock HTTP server tests failed with `Operation not permitted`; the output also includes existing expectation drift unrelated to this API-contract slice. This run is not used as a regression signal for this change.

## Real LLM Validation

### Before status-policy refinement

Run:

- `workspace/v0.6.11/eval-runs/api-response-shape-20260611-b`

Result:

- total: 13
- pass / high_quality: 9/13
- `fastapi_notes`: 4/8
- guard cases: 5/5

Observed residual after the empty-collection fix:

- POST tests still asserted undeclared `201`.
- one run still used handler parameters instead of JSON body binding.
- no observed failures from POST-to-GET list mutation/order drift.

### After status-policy and body-binding refinement

Run:

- `workspace/v0.6.11/eval-runs/api-status-policy-20260611`

Result:

- total: 13
- pass / high_quality / verification_pass: 13/13
- `fastapi_notes`: 8/8
- guard cases: 5/5
- false_done: 0
- false_missing: 0
- repair_exhausted: 0
- max_iterations: 0
- shadow_conflict: 0

Guard cases:

- `python_sales`: 1/1
- `rust_word`: 1/1
- `toml_merge`: 1/1
- `docs_runbook`: 1/1
- `data_json`: 1/1

## Interpretation

This validates the narrow hypothesis that API expectation drift was caused by underspecified typed facts reaching the LLM as weak prose. Making the contract packet carry positive, generic policies improved the reproduced residual set without adding FastAPI-specific logic.

This is not a broad success-rate claim. The remaining architecture work should continue to prefer typed contract facts plus small policy projections over case-specific prompt branches.
