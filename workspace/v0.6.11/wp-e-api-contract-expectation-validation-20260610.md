# WP-E: API Contract Expectation Validation

Date: 2026-06-10

## Objective

Introduce a small, typed HTTP API contract expectation path without adding framework-specific rules. The target was the recurring FastAPI notes failure where POST `/notes` was implemented as query parameters despite the request saying JSON body.

## Implementation Summary

- Added `api_contract_expectation.rs` as a typed extractor for method, path, JSON request-body fields, optional expected status, and response fields.
- Threaded API expectations into `TaskContract`, semantic candidates, contract-generation expectations, worker contracts, repair deltas, and task-classification shadow logs.
- Added generic API contract context to artifact-directed recovery prompts so the same typed contract reaches both first-write and retry turns.
- Added `expected_status=unspecified` projection and policy text so tests do not invent exact HTTP status assertions when the user did not declare one.

## Local Verification

- `cargo test --lib api_contract -- --nocapture`: passed
- `cargo test --lib contract_bound_generation -- --nocapture`: passed
- `cargo test --lib contract_generation_expectations -- --nocapture`: passed
- `cargo test --lib api_candidate_carries_http_contract_expectation -- --nocapture`: passed
- `cargo test --lib artifact_directed_api_context -- --nocapture`: passed
- `cargo test --lib test_expectation_audit -- --nocapture`: passed
- `cargo build`: passed
- `git diff --check`: passed

## Real LLM Validation

All real LLM runs used local Ollama and the `wp11` smoke matrix with three FastAPI rows plus Node/Python/docs regression rows.

| Run | Result | Observation |
| --- | --- | --- |
| `wp-e-api-contract-smoke2-20260610` | 3/6 high_quality | FastAPI remained 0/3; contract summary reached the prompt but initial implementation often used query parameters. |
| `wp-e-api-contract-smoke3-20260610` | 3/6 high_quality | Adding `request_body=json`/binding policy to contract-bound generation alone did not change FastAPI behavior. |
| `wp-e-api-contract-smoke4-20260610` | 3/6 high_quality | Artifact-directed retry context reached later turns, but not the first implementation write. |
| `wp-e-api-contract-smoke5-20260610` | 3/6 high_quality | First-write API context fixed JSON body binding in 3/3 FastAPI rows, but tests invented exact 201 status. |
| `wp-e-api-contract-smoke6-20260610` | 4/6 high_quality | FastAPI improved to 1/3. Failures shifted from mostly 422 request-schema mismatch to one remaining 422 and one status-drift case. Node, Python, and docs regressions stayed green. |

## Result

WP-E improved the failure mode but did not fully solve FastAPI convergence. The important architectural gain is that API expectations now travel as typed contract data instead of ad hoc FastAPI-specific rules.

## Known Issues

- One FastAPI row still ignored JSON body binding despite typed context being present in the first-write prompt.
- One FastAPI row still generated an exact `201` test expectation even though `expected_status=unspecified` was present.
- Repair diagnosis correctly identified the likely implementation/status issue in some runs, but repair proposals were rejected as malformed or ambiguous before converging.
- This is still prompt/contract-bound behavior, not a deterministic API schema verifier. Future work should add an evidence-side API contract observation rather than adding framework-specific string rules.
