# WP5: BehaviorDeltaObligation Limited Adoption

Date: 2026-06-11

## Scope

WP5 adopted `BehaviorDeltaObligation` narrowly for existing feature-change
coding tasks.

The change is intentionally limited:

- It only affects coding `Modify` / `Fix` requests with a sealed required
  behavior delta and required artifact identities.
- It does not add benchmark-specific file-name branches.
- It does not make behavior delta alone completion authority.
- It only blocks pre-verifier promotion until a current-turn implementation
  edit is observed.
- Existing verifier/evidence success paths remain unchanged once source edit
  evidence exists.

## Implementation

Added required behavior-delta projection:

- `project_required_behavior_delta_obligation`
- `required_behavior_delta_missing_source_edit`
- `agent.behavior_delta.required` audit event

Changed recovery planning:

- When `task_contract_recovery_action` would return `RunVerifier`, it now first
  checks whether a required behavior delta is missing current source-edit
  evidence.
- If source-edit evidence is missing, it returns existing
  `ArtifactRecoveryAction::Continue` for `Implementation` and lets the existing
  artifact recovery target/tool-policy path drive the model to edit the source.

This fixes the observed WP4 failure mode where pre-existing tests passed before
the model edited the requested behavior, then the turn exited
`missing_repo_edits`.

## Deterministic Verification

Executed:

```text
cargo fmt
cargo test --lib behavior_delta_obligation -- --nocapture
cargo test --lib behavior_delta_blocks_preexisting_verifier_until_source_edit -- --nocapture
cargo build
```

Result:

- `behavior_delta_obligation`: 7/7 passed.
- `behavior_delta_blocks_preexisting_verifier_until_source_edit`: passed.
- `cargo build`: passed.

The focused integration test confirms that existing `discounts.py` and
`tests/test_discounts.py` are not enough to run verifier first; the controller
selects `Implementation` recovery until a current source edit is observed.

## Real LLM Verification

Executed:

```text
python3 workspace/v0.6.11/wp_eval_matrix.py \
  --suite wp11 \
  --case-sequence feature_discount,feature_discount,feature_discount,feature_discount,python_sales,python_sales,docs_runbook,data_json \
  --variant no_pam \
  --run-id wp5-behavior-delta-adoption-20260611 \
  --anvil-bin target/debug/anvil \
  --timeout-secs 600 \
  --chat-timeout-secs 240
```

Result:

| Case | Runs | Pass | HQ | Notes |
| --- | ---: | ---: | ---: | --- |
| feature_discount | 4 | 4 | 4 | changed `discounts.py` and `tests/test_discounts.py` |
| python_sales | 2 | 2 | 2 | no regression observed |
| docs_runbook | 1 | 1 | 1 | no regression observed |
| data_json | 1 | 1 | 1 | no regression observed |

Aggregate:

- pass: 8/8
- high_quality: 8/8
- verification_pass: 8/8
- false_done: 0
- false_missing: 0
- repair_exhausted: 0
- max_iterations: 0

Observed logs:

- `agent.behavior_delta.required` emitted for feature runs before source edit.
- feature runs completed with `exit_reason=done`.
- feature runs changed the requested implementation and test files.
- docs/data did not regress in this smoke.

## Interpretation

WP5 directly addressed the WP4 failure signature. The controller no longer lets
pre-existing verifier success advance ahead of current-turn behavior change
evidence for feature-modification tasks.

This is a strong targeted improvement, not yet a broad improvement claim. The
next required confidence step is a 20-run guard after the remaining active WPs,
because a small smoke can still be lucky and does not cover TOML, Rust, Node, API
and TDD enough.

## Known Issues Carried Forward

- `agent.behavior_delta.required` can be emitted multiple times before source
  edit because recovery action is projected in more than one phase. This is
  redundant observability, not a behavior correctness issue.
- Behavior delta currently requires an implementation edit, but it does not yet
  bind semantic content of that edit. Semantic correctness is still proven by
  verifier/evaluation, not by the delta type itself.
- Broader regression risk must be checked in WP9 20-run guard.
