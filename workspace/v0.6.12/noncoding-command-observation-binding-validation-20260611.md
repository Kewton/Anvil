# Noncoding Command Observation Binding Validation

Date: 2026-06-11

## Target

Fixed the residual `ops_health` convergence shape from the c500d89 residual check:

- the model wrote the required report before actually running the command,
- the controller then requested Bash evidence,
- Bash succeeded,
- the model rewrote the same report content,
- the normal no-op edit guard discarded that post-command Write/Edit as completion evidence,
- the turn could still end as `max_iterations` despite an externally correct report.

This is a generic command-observation evidence binding issue, not an `ops_health`-specific rule.

## Changes Kept

- `objective_evidence.rs`: command-observation matching now accepts a required command run through a simple shell wrapper, such as `bash ./scripts/health.sh`.
- `repo_edit_observation.rs`: a no-op Write/Edit is still ignored for normal repo-edit and artifact-ledger purposes, but if successful command-observation evidence already exists and the no-op targets the required deliverable identity, it is recorded as turn-local task-contract binding evidence.
- `success.rs`: non-coding `max_iterations` can be reconciled to completion only when the typed `TaskContract` already evaluates to `Done`.
- `actor_loop_flow.rs`: command-observation phase prompts now make the current tool packet authoritative and tell the model not to leave temporary capture files in the workspace.

## Deterministic Checks

Passed:

- `cargo fmt`
- `cargo test no_op_write_after_command_observation_binds_existing_artifact_without_ledger_edit --lib`
- `cargo test command_observation --lib`
- `cargo test completion_credit_reconciliation --lib`
- `cargo build`

## Real LLM Validation

### Mixed Regression Run

Command:

```text
python3 workspace/v0.6.11/wp_eval_matrix.py \
  --suite wp11 \
  --case-sequence ops_health,ops_health,ops_health,ops_health,ops_health,ops_health,docs_runbook,data_json,python_sales,python_markdown,rust_word \
  --variant no_pam \
  --run-id noncoding-maxiter-credit-binding-20260611b \
  --anvil-bin target/debug/anvil \
  --timeout-secs 600 \
  --chat-timeout-secs 240
```

Result:

| Metric | Result |
| --- | ---: |
| total | 11 |
| pass | 11/11 |
| high_quality | 7/11 |
| verification_pass | 10/11 |
| false_done | 0 |
| false_missing | 0 |
| repair_exhausted | 0 |
| max_iterations | 1 |
| shadow_conflict | 1 |

Interpretation:

- Non-coding command-observation completion improved: most `ops_health` rows reached `done`.
- The no-op binding path was exercised in logs as `repo_edit_no_op_command_binding`.
- Remaining non-HQ rows were root-level temporary files in `ops_health` and a separate `python_markdown` verifier conflict.

### Focused Ops Hygiene Run

Command:

```text
python3 workspace/v0.6.11/wp_eval_matrix.py \
  --suite wp11 \
  --case-sequence ops_health,ops_health,ops_health,ops_health,ops_health,ops_health \
  --variant no_pam \
  --run-id ops-command-hygiene-20260611 \
  --anvil-bin target/debug/anvil \
  --timeout-secs 600 \
  --chat-timeout-secs 240
```

Result:

| Metric | Result |
| --- | ---: |
| total | 6 |
| pass | 6/6 |
| high_quality | 6/6 |
| verification_pass | 6/6 |
| false_done | 0 |
| false_missing | 0 |
| repair_exhausted | 0 |
| max_iterations | 0 |
| shadow_conflict | 0 |

All six rows changed only `reports/health-check.md`; no root-level temporary files remained.

## Architecture Assessment

This change stays within the intended direction:

- evidence binding remains typed and contract-local;
- no new provider abstraction was added;
- no task-name-specific rule was introduced;
- normal no-op edits still do not seed the artifact ledger or masquerade as real repo edits;
- the extra prompt text is phase-specific instruction support, not a new lifecycle state.

The remaining risk is that command-observation evidence still does not store stdout/stderr as typed data, so artifact content correctness is still partly external-evaluator dependent. A future improvement should model observed command output as structured `EvidenceObservation` data rather than adding more path or command patterns.

