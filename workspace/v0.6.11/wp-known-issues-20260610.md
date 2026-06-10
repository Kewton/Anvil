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

## WP7: RepairTargetDecision Typed Delta

- Typed delta and ledger facts are now emitted on the live repair path, but this is primarily observability/prompt-boundary improvement. It does not by itself make hard repairs converge.
- A verification-only coding request with existing source/test artifacts still stops at `missing_repo_edits` because the WP6 fresh-edit guard cannot distinguish "verify existing artifacts" from "build/modify/fix source". This should be addressed by a typed verification-only objective or a more precise edit-obligation predicate.
- A Python repair smoke successfully fixed the implementation, but then stopped at `repair_safe_stop: verifier_unavailable`; manual `PYTHONPATH=. pytest -q tests/test_calculator.py` passed. The remaining issue is verifier rerun binding/import environment, not target selection.
- `tool_failure` is part of the delta vocabulary, but live tool-failure recovery paths are not fully migrated to emit it through `RepairTargetDecision` yet.

## WP8: Actor Loop Phase Refactor

- No new behavior regression was observed in docs, Python, or hard Node smoke.
- `run_actor_loop` remains large. WP8 only moved phase-event construction and prepared-tool counter mutation; future slices should keep extracting local transition/data builders instead of adding semantic task branches.
