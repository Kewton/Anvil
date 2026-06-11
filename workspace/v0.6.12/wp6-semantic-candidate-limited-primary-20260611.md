# WP6: SemanticCandidate Limited Primary Adoption

Date: 2026-06-11

## Scope

WP6 made semantic-candidate admission more explicit and narrower.

The change does not mutate the sealed `TaskContract`. It clarifies when an
equivalent semantic candidate can be treated as limited primary authority for
audit/admission purposes.

Adopted scope:

- `TaskKind::Docs`
- `TaskKind::Data`
- `TaskKind::Ops`

Not adopted in WP6:

- `TaskKind::Coding`
- `TaskKind::Research`
- `TaskKind::Authoring`

This keeps coding/style/runner semantics out of WP6 and leaves them for WP7.

## Implementation

Changed:

- `SemanticCandidateAdmissionReason::EquivalentStableTaskKind`
- equivalent stable admissions now log `reasons=["equivalent_stable_task_kind"]`
- `limited_semantic_candidate_adoption_enabled` now admits only docs/data/ops
- coding with model-robust style authority stays `shadow_only`

The admission event is still emitted through `agent.semantic_candidate.shadow`
for compatibility, but the payload now makes the authority distinction explicit:

- `admission_mode="limited_adoption"`
- `status="admitted"`
- `reasons=["equivalent_stable_task_kind"]`

## Deterministic Verification

Executed:

```text
cargo fmt
cargo test --lib semantic_candidate -- --nocapture
cargo test --lib task_contract_admission -- --nocapture
cargo build
```

Result:

- `semantic_candidate`: 12/12 passed.
- `task_contract_admission`: 6/6 passed.
- `cargo build`: passed.

Focused coverage:

- docs candidate is admitted with typed reason
- data and ops candidates are admitted
- research, authoring, and coding stay shadow-only
- disagreement still rejects admission

## Real LLM Verification

Executed:

```text
python3 workspace/v0.6.11/wp_eval_matrix.py \
  --suite wp11 \
  --case-sequence docs_runbook,docs_runbook,docs_runbook,data_json,data_json,data_csv,ops_health,ops_health,research_cache,research_cache,python_sales \
  --variant no_pam \
  --run-id wp6-semantic-candidate-limited-primary-20260611 \
  --anvil-bin target/debug/anvil \
  --timeout-secs 600 \
  --chat-timeout-secs 240
```

Result:

| Case | Runs | Pass | HQ | Notes |
| --- | ---: | ---: | ---: | --- |
| docs_runbook | 3 | 3 | 3 | admitted as stable equivalent |
| data_json | 2 | 2 | 2 | admitted as stable equivalent |
| data_csv | 1 | 1 | 1 | admitted as stable equivalent |
| ops_health | 2 | 2 | 2 | admitted as stable equivalent; one run hit `max_iterations` after producing acceptable output |
| research_cache | 2 | 2 | 2 | current contract projected docs/content-check semantics |
| python_sales | 1 | 1 | 1 | coding stayed `shadow_only` |

Aggregate:

- pass: 11/11
- high_quality: 11/11
- verification_pass: 11/11
- false_done: 0
- false_missing: 0
- repair_exhausted: 0
- max_iterations: 1

Observed logs:

- docs/data/ops-shaped runs emitted `reasons=["equivalent_stable_task_kind"]`.
- Python coding emitted `status="ignored"` and `reasons=["shadow_only"]`.
- disagreement rejection remains covered by unit tests.

## Interpretation

WP6 improves semantic authority observability and narrows the primary-adoption
surface. It avoids letting coding style/runner inference become authority before
WP7 validates that boundary.

The smoke is positive but not a broad improvement claim. One ops run still ended
with `max_iterations` while the external grader marked the output high-quality,
so terminal projection and evaluator agreement still need WP9 guard coverage.

Research remains ambiguous: the evaluation case is research, but the current
contract projects it as docs/content-check. This was not introduced by WP6, but
it should be considered when future research-specific evidence semantics become
primary.

## Known Issues Carried Forward

- Event name `agent.semantic_candidate.shadow` now carries admitted payloads for
  compatibility. A future cleanup could add a parallel
  `agent.semantic_candidate.admission` event, but WP6 avoided log churn.
- Research tasks can still project as docs when the deliverable is a document.
- One ops run produced acceptable output but reached `max_iterations`.
