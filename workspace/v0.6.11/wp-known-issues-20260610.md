# WP4-WP11 Known Issues

Date: 2026-06-10

This file tracks known issues observed while executing WP4-WP11. It is separate from per-WP validation notes so that unresolved issues are easy to carry forward.

## WP4: SemanticCandidate Shadow

- Shadow candidates are still deterministic projections of the existing path, not a fully independent LLM semantic candidate.
- Shadow logs help compare contract quality, but they do not by themselves improve completion or repair behavior.

## WP5: SemanticCandidate Limited Adoption

- Limited adoption is intentionally low risk: docs/data/research/ops and styled coding candidates only.
- Coding success-rate improvement cannot be claimed from WP5 alone because most hard failures are still downstream in evidence binding and repair convergence.

## WP6: Contract-Bound Generation

- Hard Python repair still reached `repair_exhausted` after verifier failures. The taxonomy made the failure visible but did not make repair converge.
- Rust integration tests can be generated and pass manually, but internal generated-test preflight may reject them as `unsupported_contract_assertion`, producing `safe_stop_verifier_missing`.
- Node runnable tests can pass manually, but artifact completion may still exhaust because the test shape is not recognized as bindable evidence.
- Existing-project feature improvement exposed a false-done path. This was fixed by requiring fresh repo edit evidence before accepting verifier success for coding change contracts.
- After the false-done guard, the same scenario now fails closed with `missing_repo_edits`; it still needs WP7/WP9 work to recover into the correct implementation edit.
- TDD order is not strictly enforced by the controller; a successful TDD smoke still wrote implementation before tests on one attempt path.
