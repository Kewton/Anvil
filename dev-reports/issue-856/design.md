# Issue 856 Design

## Context

The current verifier path already has structured owned-test binding, repair jobs, failure signatures, and terminal diagnostics. The missing pieces are smaller policy gaps: docs-only tasks can still inherit a generic verification requirement from words like "check"; generated tests are executed directly without an explicit compile/preflight phase; repeated signatures are tracked but the repair prompt does not surface a narrow invariant; eval diagnostics do not expose a concise verifier status / last signature summary.

## Design

- Treat `CompletionProjectIntent::DocsOnly` as document/no-verifier completion. Once the usage-docs artifact obligation is satisfied, docs-only tasks should reach `Done` without asking the coding verifier to run.
- Add a structured preflight in `AutoTestRunner::run_structured` before the main verifier:
  - Python owned tests run `python -B -m py_compile <bound test files>`.
  - Rust top-level integration tests run `cargo test --no-run --test <name>`.
  - Other runners currently skip preflight and keep the existing execution path.
- Keep failure-signature storage in the existing `RepairJob` fields and enrich the repair prompt payload with a deterministic repeated-signature invariant when the same signature recurs.
- Extend eval terminal diagnostics additively with satisfied/missing obligation lists, verifier status, and an optional last failure signature. For `repair_exhausted`, classify likely model-output, verifier-environment, and control-loop failures from available terminal evidence instead of always returning one bucket.

## Scope

This change avoids introducing provider abstractions or replacing the repair driver. It adds focused policy helpers and tests around the existing verifier contracts.
