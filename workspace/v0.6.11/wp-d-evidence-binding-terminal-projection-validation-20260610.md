# WP-D EvidenceBinding / Terminal Projection Validation

Date: 2026-06-10

## Change

- Narrowed generated-test non-ASCII rejection to assertion contexts.
  - This prevents docstrings/prose in generated tests from being treated as unsupported contract assertions.
  - The existing unsupported non-ASCII assertion guard remains active for actual assertion lines.
- Added `OwnedTestVerifierProjection`.
  - Preserves the existing admitted test artifact API.
  - Carries candidate/rejected counts so all-candidate preflight rejection is distinguishable from no test artifact.
- Projected all-candidate preflight rejection to `structured_weak` rather than `structured_missing`.
  - This keeps binding/preflight failure out of verifier-missing semantics.

## Static Validation

- `cargo test --lib generated_test_guard -- --nocapture`
  - 23 passed
- `cargo test --lib verifier_driver -- --nocapture`
  - 15 passed
- `cargo test --lib evidence_binding -- --nocapture`
  - 35 passed
- `cargo build`
  - passed

## Real LLM Validation

Command:

```sh
python3 workspace/v0.6.11/wp_eval_matrix.py --suite wp10 --case-sequence toml_merge,toml_merge,toml_merge,toml_merge,python_markdown,node_json,rust_word --variant no_pam --run-id wp-d-evidence-binding-smoke2-20260610 --timeout-secs 240 --chat-timeout-secs 180
```

Result:

- pass: 7/7
- high_quality: 7/7
- verification_pass: 7/7
- false_done: 0
- false_missing: 0
- repair_exhausted: 0
- max_iterations: 0
- TOML: 4/4 pass, 4/4 high_quality

## Interpretation

The original TOML false-missing was caused by generated-test preflight rejecting a non-ASCII docstring/prose literal as an unsupported assertion, which removed the explicit test artifact from verifier binding. After narrowing the predicate and carrying preflight rejection metadata, TOML tests bind to the structured pytest verifier and complete as `done`.

This is not yet a broad success-rate claim. It is a focused WP-D smoke with four TOML runs plus Python/Node/Rust regression guards.

## Known Issues

- Real LLM validation still relies on local Ollama availability; sandboxed execution without localhost access fails before model invocation.
- All-candidate preflight rejection is now observable as weak binding, but this WP does not implement a dedicated repair operator for rejected generated-test content.
