# WP4: BehaviorDeltaObligation Shadow

Date: 2026-06-11

## Scope

WP4 added a shadow-only `BehaviorDeltaObligation` for existing feature-change
tasks.

The important boundary is intentional:

- The projection reads the sealed `TaskContract`.
- It does not rescan the raw user prompt.
- It does not grant completion authority.
- It does not add benchmark-specific branches such as `discounts.py`.
- Non-coding docs/data tasks do not receive behavior delta obligations.

## Implementation

Added:

- `src/agent/loop_run/behavior_delta_obligation.rs`
- `agent.behavior_delta.shadow` audit event from task classification

The shadow projection currently requires:

- `TaskKind::Coding`
- `TaskIntent::Modify` or `TaskIntent::Fix`
- a finite required-behavior confidence above the existing low-confidence
  threshold
- a sealed required-behavior goal

The log payload intentionally contains only safe structural metadata:

- whether a behavior goal exists
- whether an affected surface exists
- expected observable behavior count
- authority=`shadow_only`
- completion_authority=`false`

## Deterministic Verification

Executed:

```text
cargo fmt
cargo test --lib behavior_delta_obligation -- --nocapture
cargo build
```

Result:

- `behavior_delta_obligation` focused tests passed.
- `cargo build` passed.

Focused test coverage:

- feature modify request projects a shadow behavior delta
- docs/data requests do not project a behavior delta
- explain-only request does not project a behavior delta

## Real LLM Verification

Executed:

```text
python3 workspace/v0.6.11/wp_eval_matrix.py \
  --suite wp11 \
  --case-sequence feature_discount,feature_discount,python_sales,docs_runbook \
  --variant no_pam \
  --run-id wp4-behavior-delta-obligation-shadow-20260611 \
  --anvil-bin target/debug/anvil \
  --timeout-secs 600 \
  --chat-timeout-secs 240
```

Result:

| Case | Runs | Pass | HQ | Notes |
| --- | ---: | ---: | ---: | --- |
| feature_discount | 2 | 0 | 0 | existing `missing_repo_edits`; only pycache changed |
| python_sales | 1 | 1 | 1 | no regression observed |
| docs_runbook | 1 | 1 | 1 | no behavior delta shadow emitted |

Aggregate:

- pass: 2/4
- high_quality: 2/4
- false_done: 0
- false_missing: 0
- repair_exhausted: 0
- max_iterations: 0

`agent.behavior_delta.shadow` was emitted for both `feature_discount` runs and
not for the docs run, which matches the intended WP4 boundary.

## Interpretation

WP4 improves observability only. It does not claim success-rate improvement.

The repeated `feature_discount` failure is useful evidence for WP5: the
controller can now see that a current feature-change behavior delta exists, but
it still has no authority to require current-turn source edits and behavior-bound
evidence. That adoption must be handled separately and narrowly so false-done
remains zero.

## Known Issues Carried Forward

- `feature_discount` still exits `missing_repo_edits` after no meaningful source
  edit.
- The only observed changed file in the failed runs was Python bytecode under
  `tests/__pycache__`, which must not count as behavior evidence.
- WP5 should bind behavior delta to source edits and evidence without introducing
  file-name-specific or benchmark-specific rules.
