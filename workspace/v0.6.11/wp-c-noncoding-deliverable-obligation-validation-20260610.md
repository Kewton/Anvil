# WP-C Non-Coding DeliverableObligation Validation

Date: 2026-06-10

## Scope

WP-C focused on separating non-coding deliverable work from coding edit obligations.

Implemented changes:

- `repo_edit_required` is now derived from WorkMode or ObjectiveContract artifact obligations.
- Answer-only WorkMode no longer suppresses required research/ops artifact creation.
- `do not modify code` / `do not create source code` is treated as a coding-artifact prohibition, not a global file-edit prohibition.
- Command-observation requests such as `run script and write report with stdout/stderr/exit code` are admitted as Ops command-output deliverables, not Coding implementation work.
- Command extraction preserves leading `./` and stops before `and write/create/document/report`.

## Focused Tests

Passed:

- `cargo test --lib workspace_access -- --nocapture`
- `cargo test --lib no_code -- --nocapture`
- `cargo test --lib explicit_no_edit_research_report_stays_answer_only -- --nocapture`
- `cargo test --lib raw_command_observation_request_creates_ops_artifact_obligation -- --nocapture`
- `cargo test --lib confirmed_ops_with_explicit_markdown_path_creates_runbook_obligation -- --nocapture`
- `cargo test --lib data_task -- --nocapture`
- `cargo test --lib evidence_observation -- --nocapture`
- `cargo build`
- `git diff --check`

## Real LLM Validation

Initial mixed smoke:

- Run: `wp-c-noncoding-deliverable-smoke-20260610`
- Cases: research 6, ops 6, docs 3, data 3, coding 3
- Result: pass 15/21, high_quality 15/21
- Research: 6/6 pass
- Docs/Data/Coding regression sentinels: 9/9 pass
- Ops: 0/6, all `missing_repo_edits`

Insight from failed ops:

- The model attempted `Write(reports/health-check.md)`, but the controller had selected `LocalLlmSmallEditAfterRead` against `scripts/health.sh` and exposed only `Edit`.
- Root cause was upstream contract admission: the request was classified as Coding because `command`/script wording was interpreted as implementation work, and `report` was sometimes absorbed by Research before Ops.

Resmoke after command-observation admission fix:

- Run: `wp-c-ops-command-observation-resmoke-20260610`
- Cases: ops 6
- Result: pass 6/6, high_quality 3/6
- `false_done`: 0
- `false_missing`: 0
- `repair_exhausted`: 0

## Outcome

WP-C objective is met for artifact creation and terminal alignment:

- Research artifact creation recovered: 6/6.
- Ops artifact creation recovered from 0/6 to 6/6.
- `missing_repo_edits` for command-observation ops was removed in the resmoke.
- Coding/data/docs sentinels in the mixed smoke did not regress.

Remaining known issue:

- Ops high_quality is 3/6 because some runs create temporary stdout/stderr/exit-code files outside the requested report artifact. This is not a terminal alignment failure, but it is an artifact-scope hygiene issue to carry forward.
