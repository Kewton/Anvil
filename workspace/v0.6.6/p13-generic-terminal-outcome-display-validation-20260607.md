# P13 Generic Terminal Outcome Display Validation

Date: 2026-06-07

## Objective

Separate legacy terminal labels from generic lifecycle terminal labels.

The compatibility requirement is:

- `ExitReason` and `EvalRecord.final_outcome` keep legacy labels such as `missing_repo_edits`.
- New reporting surfaces can show generic labels such as `evidence_repair_exhausted`.
- Eval logs preserve both projections through `terminal_diagnostics.outcome` and `terminal_diagnostics.generic_outcome`.

This keeps existing evaluation compatibility while moving Anvil toward a generic, non-coding-only lifecycle vocabulary.

## Implementation

Changed files:

- `src/agent/loop_run/summary.rs`
  - Added `LoopStats.terminal_outcome_label`.
  - `format_run_summary` uses this generic label when present, while `ExitReason` remains unchanged.
- `src/agent/loop_run/actor_loop_flow.rs`
  - When artifact completion is exhausted by evidence failure, the run summary label is set to `evidence_repair_exhausted`.
- `src/session/eval_log.rs`
  - Added `TerminalDiagnosticsSummary.generic_outcome`.
  - Added legacy-to-generic projection for terminal diagnostics.
  - `mark_artifact_evidence_repair_exhausted` now marks `generic_outcome=evidence_repair_exhausted` while preserving `final_outcome=missing_repo_edits`.
- `src/agent/loop_run/protocol.rs`
  - Updated test helper initialization for the added `LoopStats` field.

## Unit Validation

Commands:

```bash
cargo fmt
cargo test --offline --lib generic_terminal_label_overrides_legacy_summary_label
cargo test --offline --lib terminal_diagnostics_project_legacy_outcome_to_generic_lifecycle_outcome
cargo test --offline --lib terminal_diagnostics_project_artifact_evidence_repair_exhausted
cargo test --offline --lib
```

Results:

- Targeted summary display test: passed.
- Targeted eval-log generic projection test: passed.
- Targeted artifact evidence exhausted projection test: passed.
- Full lib suite: `3776 passed; 0 failed`.

Note: the first full lib run inside the sandbox failed because mockito could not bind local ports. The same command passed when run with the required local-port permission.

## Actual Local LLM Validation

Model:

- `qwen3.6:27b-coding-mxfp8`

### Negative CSV Evidence Exhaustion

Command shape:

```bash
cargo run --offline -- --oneshot --fresh-session --no-footer --trace -y \
  --max-iterations 6 \
  --state-dir /private/tmp/anvil-p13-generic-summary-state-csv2 \
  -m qwen3.6:27b-coding-mxfp8 \
  -p 'Negative validation ... required_artifacts output.csv data schema columns name,score ... keep writing x,y'
```

Observed CLI summary:

```text
✘ evidence_repair_exhausted  iter 5/6  duration 21s  edited 1 files (output.csv)
artifact completion role-specific retry budget exhausted
```

Observed eval log:

```json
{
  "final_outcome": "missing_repo_edits",
  "terminal_diagnostics": {
    "outcome": "missing_repo_edits",
    "generic_outcome": "evidence_repair_exhausted",
    "classification": "evidence_repair_exhausted",
    "missing_obligations": ["repair_convergence", "artifact_evidence"]
  },
  "evaluation_taxonomy": {
    "failure_authority": "artifact_evidence"
  }
}
```

This confirms the intended split:

- Human-facing run summary can use the generic lifecycle label.
- Legacy eval compatibility remains intact.
- The generic failure authority is visible in terminal diagnostics.

### Positive JSON Completion

Command shape:

```bash
cargo run --offline -- --oneshot --fresh-session --no-footer --trace -y \
  --max-iterations 5 \
  --state-dir /private/tmp/anvil-p13-generic-summary-state-json1 \
  -m qwen3.6:27b-coding-mxfp8 \
  -p 'Positive validation ... required_artifacts summary.json data schema json_fields name,score'
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
  }
}
```

### Repeat Negative Probe

A repeated negative CSV probe with nearly the same instruction completed successfully:

```text
✔ done  iter 3/6  duration 22s  edited 1 files (output.csv)
```

The model ignored the negative validation instruction and followed the controller repair signal by writing a valid `name,score` CSV.

This is useful evidence:

- P13 does not force failure display when the controller repairs successfully.
- Local LLM behavior is variable under adversarial instructions.
- The deterministic unit/eval-log tests are necessary because a small number of LLM probes cannot prove every terminal path.

## Conclusion

P13 is working for the targeted architecture slice:

- Legacy terminal outcome compatibility is preserved.
- Generic lifecycle outcome is available for display and diagnostics.
- Artifact evidence exhaustion can be shown as `evidence_repair_exhausted` instead of leaking the coding-centric `missing_repo_edits` label to the CLI summary.
- Successful generic data artifact work remains `done` / `completed`.

## Remaining Risk

This does not yet make all terminal surfaces generic. It only adds the first explicit display override for artifact evidence exhaustion.

Next architectural slices should continue the same pattern:

1. Keep legacy labels at compatibility boundaries.
2. Add generic projections at controller/reporting boundaries.
3. Validate each projection with deterministic tests plus actual local LLM probes.
4. Avoid embedding task-kind-specific rules into the controller loop; use contract/evidence projections instead.
