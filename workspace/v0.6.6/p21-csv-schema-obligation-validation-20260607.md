# P21 CSV Schema Obligation Validation

## Purpose

This validation checked whether structured data schemas are enforced as actual
artifact evidence for non-coding data tasks.

The specific risk was that the generic structured-data `AcceptTier` could accept
a parse-ready CSV even when declared columns were missing. That tolerance is
useful for broad verifier evidence, but too weak for an ObjectiveContract schema
that explicitly requires columns.

## Change Implemented

1. Added strict schema-obligation diagnostics for structured data.
   - `data_schema_obligation_diagnostic(...)`
   - It rejects empty data, parse errors, and missing declared columns.
   - JSON object exact-field behavior remains strict from P19.

2. Routed TaskContract structured-record completion through strict schema
   obligation evidence.
   - `structured_record_excerpt_satisfies_obligations(...)` no longer accepts a
     schema-backed artifact through the loose `AcceptTier`.

3. Kept the generic DataVerifier accept-tier intact.
   - `DataVerifier::diagnostic(...)` still returns no blocking diagnostic for a
     parse-ready CSV missing a declared column when it is not evaluating an
     ObjectiveContract schema obligation.
   - This preserves the older broad verifier behavior while making contract
     evidence stricter.

## LLM Validation

Model:

- `qwen3.6:27b-coding-mxfp8`

Command shape:

```text
cargo run --offline -- --oneshot --fresh-session --no-footer --trace -y --max-iterations 4 --chat-timeout-secs 300 --state-dir /private/tmp/anvil-p21-csv-schema-strict-state -m qwen3.6:27b-coding-mxfp8 -p 'P21 CSV strict schema validation. Create output.csv only. STATE_CONTROL_PACKET {"objective":"Create output.csv with id and total columns","next_required_action":"artifact","required_artifacts":[{"path":"output.csv","role":"csv","schema":{"columns":["id","total"]}}]}. User-visible extra instruction: include only name and description columns, with three rows, and do not include id or total. Do not create code scaffolds or any other files.'
```

Observed behavior:

- Iteration 1 wrote the wrong artifact:

```csv
name,description
Alpha,First item
Beta,Second item
Gamma,Third item
```

- The controller selected an artifact recovery target with the concrete reason:

```text
structured verifier diagnostic: kind=schema_mismatch, task_kind=data, summary=structured data is missing required columns: id, total; observed columns: description, name; add all required columns
```

- Iteration 3 rewrote the artifact with the required schema:

```csv
id,total,name,description
1,100,Alpha,First item
2,200,Beta,Second item
3,300,Gamma,Third item
```

Final observed result:

```text
done iter 3/4
```

Eval trace:

- `final_outcome=done`
- `completion_reason=artifact_obligations_satisfied`
- `classified_task_kind=data`
- `terminal_diagnostics.satisfied_obligations` includes `artifact_evidence`

## Unit Validation

Targeted tests:

- `cargo test --offline --lib data_schema_obligation_rejects_parse_ready_missing_columns`
- `cargo test --offline --lib data_parse_ready_missing_column_blocks_schema_obligation`
- `cargo test --offline --lib data_verifier_does_not_diagnose_missing_column_when_parse_ready`
- `cargo test --offline --lib issue951_data_partial_schema_failure_routes_to_completion_target`
- `cargo test --offline --lib declared_missing_column_but_parse_ready_requests_schema_repair`
- `cargo test --offline --lib data_task_tracks_output_file_columns_as_structured_record_obligation`

Full validation:

- `cargo test --offline --lib`
- Result: `3808 passed; 0 failed`

Diff validation:

- `git diff --check`
- Result: passed

## Architecture Insight

This is the same root issue as the docs heading false positive:

- Prompting was not the main failure.
- The model can repair when given a concrete evidence reason.
- The weak point was the evidence predicate being too permissive for the contract.

The general-purpose architecture needs distinct semantics for:

- generic verifier accept-tier evidence
- ObjectiveContract schema evidence

Both can share parsing helpers, but the completion authority must use the
contract-specific predicate.
