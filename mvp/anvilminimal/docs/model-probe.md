# Model Behavior Probe

`anvilminimal --model-probe` and the TUI `/model-probe` command run a
bounded dialect probe against the configured executor and planner model roles.
The probe is measurement-only: results never auto-configure runtime behavior.
The output is a JSON profile plus a markdown card for human review and
model-tier table evidence.

The probe uses a throwaway scratch workspace under the system temp directory,
cleans it after the run, and must not run package installs. Each task is one
bounded session through the normal model/tool chokepoint unless noted below.
The fixed battery version is `model-probe-v2`.

## Battery

1. `write_simple`: create `src/util/math.ts` with a 5-line function.
   Measures relative, absolute, and corrupted path forms.
2. `write_deep`: create a file 5 directories deep with a long filename.
   Stresses path reconstruction.
3. `edit_provided`: edit a file whose exact content is supplied in the prompt.
   Measures anchor-copy fidelity.
4. `edit_own`: edit the file written in task 1 in the same session.
   This is appended-session variant A for memory-anchor fidelity.
5. `verify_exist`: verify that `src/util/math.ts` exists.
   Measures command shape such as `test -f`, `&&`, pipes, and `cd`.
6. `verify_json`: verify that `package.json` declares a build script.
   Measures JSON-verification command dialect such as `node -p`, `grep`,
   pipes, redirects, and shell control.
7. `repair_appended`: repair a planted one-line compile error in a session
   with roughly 2k tokens of prior context.
8. `repair_compact`: repair the same planted error in a fresh minimal session.
   The appended-vs-compact delta is the context-sensitivity signal.
9. `regenerate`: rewrite the full corrected file via `Write`.
   This checks the instruction-96 full-file regeneration capability.
10. `csv_fixture_verify`: create a small CSV fixture, then verify a local
    program can process it.
    This elicits the live combine tendency where models try to create fixture
    files inside verify commands with redirects or heredocs.
11. `json_schema`: respond only with JSON matching the StepPlan-like schema.
    This measures planner parse rate and missing descriptive or semantic fields.

Every task records raw tool calls and raw Bash commands verbatim in the JSON
profile.

## Metrics

The profile computes:

- `absolute_path_rate` and `corrupted_path_count`
- `shell_control_rate` with `&&`, `;`, pipe, redirect, and `cd` breakdowns
- `edit_anchor` as `exact`, `salvageable`, or `miss`
- `repair_follow_through` for appended and compact repair
- `regeneration_follow_through`
- `json_valid_rate` and `missing_field_kinds`
- `empty_response_rate`
- `malformed_tool_call_rate`
- per-call latency stats
- provider token telemetry from the 98C provider-turn events, including
  context-truncation warnings when they fire

The markdown card maps elevated metrics to the absorbing layers they exercise:
path normalization/salvage, bash and verify normalization, edit-anchor salvage,
compact repair rungs, full-file regeneration, schema repair/defaulting, empty
response retries, malformed tool-call recovery, and token-truncation review.

The card states its own scope: N=11 micro-tasks, dialect indicators, not a
capability benchmark.

Calibration note: `model-probe-v1` did not include a fixture-creation plus
verification task, so it could under-sample the live python-cli tendency to
combine CSV fixture creation and verification in one shell command. `v2` records
that divergence honestly by adding `csv_fixture_verify`; old v1 cards remain
valid for their stated N=10 dialect scope but should not be treated as evidence
about redirect/heredoc fixture creation behavior.

## New-Model Procedure

Standard order before scenario UAT:

1. Run `anvilminimal --model-probe` or `/model-probe` with the intended
   provider/model configuration and review the generated card.
2. Run two smoke checks: one CLI task and one TOOL task.
3. Run the full scenario round with landing criteria committed before the run.
4. Add the tier-table entry with a citation to the probe profile.

Re-run the probe when the model version or digest changes. For cloud-hosted
models, re-run it before every measurement campaign because identity is not
pinned.
