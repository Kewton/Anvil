# RWP-6 Typed Repair Action Admissibility Validation

Date: 2026-06-10

## Scope

RWP-6 adds a typed `repair_action_space` projection to the active verifier repair prompt. It is intentionally a thin layer over the existing selected target and optional controller-admitted `RepairAction`; it does not add a new repair planner or new provider abstraction.

Implemented:

- `repair_action_space` module.
- `RepairActionPlan` projection with target artifact, allowed tool category, expected evidence delta, allowed change kind, and rejection reason.
- Active verifier repair prompt now receives `repair_action_space` alongside the existing `repair_action`.
- Malformed retry prompt now explicitly keeps the same action-space target, allowed change, and expected evidence delta.

## Deterministic Verification

Commands:

- `cargo test --lib repair_action_space -- --nocapture`
- `cargo test --lib verifier_repair_pass_prompt -- --nocapture`
- `cargo build`
- `cargo fmt --check`
- `git diff --check`

Result:

- `repair_action_space`: 5/5 passed.
- `verifier_repair_pass_prompt`: 7/7 passed.
- Build and formatting checks passed.

Covered assertions:

- Data schema repair projects a data evidence delta.
- API body mismatch can admit an implementation action.
- Setup/verifier binding repair projects setup/evidence-runner delta.
- No selected target is rejected.
- Accepted actions must match selected target.
- Active repair prompt carries `repair_action_space`.
- Previous malformed proposal keeps the same action space.

## Real LLM Validation

Run:

- `workspace/v0.6.11/eval-runs/rwp6-repair-action-space-smoke-20260610/results.csv`

Case sequence:

- `fastapi_notes` x6
- `node_notes_api` x2
- `python_markdown` x2
- `data_csv` x1
- `data_json` x1

Result:

- Overall: 12/12 pass, 11/12 high_quality.
- FastAPI: 6/6 high_quality.
- Node API: 2/2 high_quality.
- Data: 2/2 high_quality.
- Python markdown: 1/2 high_quality; one row passed functional grading but ended with `repair_exhausted`, so high_quality was false.
- `false_done`: 0.
- `false_missing`: 0.
- `shadow_terminal_conflict`: 0.

## Interpretation

This is the strongest local signal so far that bounded action-space guidance helps repair convergence, especially after RWP-4 made API contract deltas available to the active repair path.

However, this is still a 12-run smoke, not a 20-run regression guard or 50-run improvement claim. The result should be treated as hypothesis support, not a final success-rate claim.

Residual issues:

- A harder Python coding case can still end with `repair_exhausted`.
- The current action space is a prompt-side typed projection. Patch execution still depends on `old_string` exactness and existing validation.
- Further work should keep this boundary typed and avoid adding benchmark/framework-specific repair rules.

## Complexity Assessment

The new code is a small pure projection over `RecoveryTargetHint` and `RepairAction`. It uses typed roles and existing `AllowedChangeKind` labels. It does not inspect raw prompts or verifier text and does not change completion authority.

RWP-6 is complete.
