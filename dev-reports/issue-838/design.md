## Design

Completion policy should be derived once from the same request-backed
`TaskContract` data that already drives artifact recovery. I will add a small
`ProjectIntent` classification and a `CompletionPolicy` value in
`task_contract.rs`, then use that policy from protocol evidence satisfaction.

The policy will cover docs-only, artifact-only, implementation-with-test, and
implementation-without-test shapes. It must stay conservative: docs-only accepts
documentation evidence only, artifact-only accepts requested support artifacts
or matching verifier evidence, and implementation policies do not let a README
edit alone satisfy an implementation request. The existing #651
test-execution/bound-verifier gate remains in `TaskContract::evaluate_*`.

Focused regressions will pin:

- README/docs-only completion accepts docs edits without requiring impl/tests.
- pytest-pass artifact-only evidence does not become `missing_repo_edits`.
- EnvSetup-only evidence still cannot satisfy test-required requests.
- Unbound verifier evidence still cannot satisfy the #651 Done gate.
