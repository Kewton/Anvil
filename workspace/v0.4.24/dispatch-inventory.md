# v0.4.24 Dispatch Inventory

## Purpose

Responsibility-cleanup work starts by freezing where verifier / recovery dispatch decisions are currently made. The immediate regression target is a false-positive `done`: a Rust-library request completed after creating only Python test/docs artifacts.

## Current Dispatch Sources

### Artifact / Contract Flow

- `task_contract.rs`
  - Derives required artifact roles from the user request.
  - Knows whether test execution evidence is required.
- `turn.rs::run_task_contract_verifier_once`
  - Runs the in-turn verifier after required artifacts appear.
  - Previously selected `ProjectUnit` without considering the requested stack.

### Success / Post-Loop Flow

- `success.rs::should_run_auto_test_for_success`
  - Decides whether a successful-looking turn needs an auto verifier.
  - Previously accepted any current-turn verifier candidate in scope.
- `success.rs::run_post_loop_success_verifier`
  - Builds `VerifierInputs` for the verifier skill.
  - Previously passed a request-unaware `ProjectUnit`.

### Project / Verifier Detection

- `project_probe.rs::probe_completion`
  - Request-aware, but only used by the completion probe path.
  - Already rejects obvious implementation stack mismatch.
- `project_probe.rs::probe_project_unit`
  - Request-unaware project facts for internal and legacy callers.
  - Still useful where no active request exists.
- `auto_test.rs`
  - Converts a selected `ProjectUnit` into allowlisted verifier commands.

## Identified Gap

The request-aware logic existed only in `probe_completion`. The verifier dispatch paths still accepted request-unaware project units. Therefore a Rust request could bind to a Python `tests/test_main.py` verifier if the model wrote a passing Python dummy test.

## v0.4.24 First Fix Scope

- Add `probe_project_unit_for_request`.
- Keep `probe_project_unit` for contexts without an active request.
- Route task-contract verifier and post-loop verifier through the request-aware probe when request text exists.
- Reject or filter verifier candidates whose project family conflicts with an explicit requested stack.
- Preserve TypeScript / JavaScript package verifier compatibility.

## Non-Goals

- Do not add FastAPI / CRUD / ToDo-specific rules.
- Do not replace RepairJob in this step.
- Do not delete legacy paths until request-bound verifier selection is covered by tests and smoke evaluation.
