# P14 Terminal Outcome SSOT Validation

Date: 2026-06-07

## Objective

Reduce architectural drift introduced by generic terminal outcome projections.

P13 added a useful split between:

- legacy compatibility labels, for example `missing_repo_edits`
- generic lifecycle labels, for example `missing_deliverable`

However, the mapping existed in more than one place. That is a maintenance risk as Anvil expands beyond coding tasks because every new task kind and terminal state would invite duplicated projection tables.

## Implementation

Added:

- `src/terminal_outcome.rs`

Moved/centralized:

- `GenericTerminalState`
- `GenericTerminalState::label`
- `GenericTerminalState::from_legacy_terminal_label`
- `generic_label_for_legacy_terminal`

Updated callers:

- `src/agent/loop_run/summary.rs`
- `src/agent/loop_run/evidence_runner.rs`
- `src/agent/loop_run/evidence_binding.rs`
- `src/session/eval_log.rs`

`summary.rs` still owns the richer `RunTerminalOutcome` projection because it depends on loop-local concepts such as `RecoveryJobKind`, `ArtifactRole`, `MissingDeliverable`, and `MissingEvidence`. The crate-level module owns only the reusable generic lifecycle vocabulary and legacy-label projection.

## Unit Validation

Commands:

```bash
cargo fmt
cargo test --offline --lib legacy_terminal_labels_project_to_generic_lifecycle_labels
cargo test --offline --lib terminal_outcome_projects_generic_states_and_legacy_eval_labels
cargo test --offline --lib terminal_diagnostics_project_legacy_outcome_to_generic_lifecycle_outcome
cargo test --offline --lib
```

Results:

- Shared terminal outcome mapping test: passed.
- Summary projection test: passed.
- Eval-log projection test: passed.
- Full lib suite: `3777 passed; 0 failed`.

## Actual Local LLM Validation

Model:

- `qwen3.6:27b-coding-mxfp8`

Command shape:

```bash
cargo run --offline -- --oneshot --fresh-session --no-footer --trace -y \
  --max-iterations 5 \
  --state-dir /private/tmp/anvil-p14-terminal-outcome-ssot-state-json1 \
  -m qwen3.6:27b-coding-mxfp8 \
  -p 'P14 SSOT validation ... required_artifacts summary.json data schema json_fields title,status'
```

Observed CLI summary:

```text
✔ done  iter 1/5  duration 4s  edited 1 files (summary.json)
```

Observed eval log:

```json
{
  "final_outcome": "done",
  "terminal_diagnostics": {
    "outcome": "done",
    "generic_outcome": "completed",
    "classification": "success"
  },
  "evaluation_taxonomy": {
    "failure_authority": "success"
  }
}
```

## Conclusion

The terminal outcome vocabulary is now less coupled to the CLI summary module.

This moves the architecture closer to the intended generic shape:

- compatibility labels stay stable
- generic labels have a single crate-level projection point
- session/eval logging no longer owns a separate mapping table
- evidence binding/runner can reference the lifecycle vocabulary without depending on the summary renderer

## Remaining Risk

Only the generic terminal state vocabulary was moved. Richer loop-local projections such as recovery job kind and missing obligation detail still live in `summary.rs`.

That is acceptable for this slice because those projections depend on loop-local types. A future slice should split those richer projections only if another subsystem needs them; otherwise moving them now would add abstraction without immediate value.
