# Cycle 2 Loss Triage

Date: 2026-06-12 JST

Scope:

- Legacy root: `.anvil/benchmarks/20260612T024532-70925`
- Minimal+M001 root: `.anvil/benchmarks/20260612T174200-32077`
- Model: `qwen3.6:27b-coding-nvfp4`

This is investigation only. No minimal-loop, parser, prompt, or success-check
code was changed for this triage.

## Summary

Targets:

| scenario | legacy final | minimal+M001 final | residual gap |
|---|---:|---:|---:|
| `fix-js-date-helper` | 5/5 | 2/5 | -3 |
| `new-python-csv-small` | 5/5 | 3/5 | -2 |
| `non-coding-runbook` | 5/5 | 3/5 | -2 |

Residual minimal failures:

| class | count | runs |
|---|---:|---|
| `no_edit_loop_after_feedback` | 5 | `fix-js-date-helper` run-3/run-4, `new-python-csv-small` run-2/run-5, `non-coding-runbook` run-2 |
| `artifact_quality_after_write` | 1 | `fix-js-date-helper` run-5 |
| `max_iterations_without_session_log` | 1 | `non-coding-runbook` run-3 |
| `check_failed` | 0 | none observed |
| `anchor_mismatch` | 0 | none observed |

Primary finding:

- M001 improves the original no-edit-loop class but does not eliminate it. In
  five residual failures, feedback fired and the model still either emitted no
  tool call or tried blocked `Bash mkdir` instead of `Write`.
- `fix-js-date-helper` also has one distinct failure where the file exists, but
  the model stops after describing the required fix and leaves the buggy code in
  place.
- No remaining target failure is explained by a success-check false negative or
  an Edit anchor mismatch.

## `fix-js-date-helper`

Result:

- legacy: 5/5
- minimal+M001: 2/5

Minimal run outcomes:

| run | success | M001 fired | reason | class |
|---|---:|---:|---|---|
| 1 | yes | no | ok after Task18 semantic recheck | n/a |
| 2 | yes | yes | ok after feedback, `Write`, and repair | n/a |
| 3 | no | yes | `missing_file:src/dateRange.js` | `no_edit_loop_after_feedback` |
| 4 | no | yes | `missing_file:src/dateRange.js` | `no_edit_loop_after_feedback` |
| 5 | no | no | semantic command failed | `artifact_quality_after_write` |

Branch point:

- Legacy reaches `Write:src/dateRange.js` immediately in successful runs.
- Minimal run-3 and run-4 inspect an empty workdir, return prose saying they
  will create `src/dateRange.js`, receive M001 feedback, then call blocked
  `Bash mkdir -p src`. After the blocked command, both runs again say they will
  use `Write` but emit no tool call. The target file is never created.
- Minimal run-5 does create `src/dateRange.js`, but the final file still returns
  `diffDays` without adding one or handling reversed ranges. The model describes
  the fixes in prose and stops without `Edit` or `Write`.

M001 and missing-file relation:

- Missing-file failures: run-3 and run-4.
- M001 fired in both missing-file failures.
- Therefore the remaining missing-file failures are not caused by M001 being
  absent. They are M001-ineffective cases where the follow-up action was either
  blocked `Bash` or another no-tool completion.

Classification:

- `no_edit_loop_after_feedback`: 2
- `artifact_quality_after_write`: 1

Judgment:

- Not a check problem after Task18 semantic recheck.
- Not an anchor problem.
- Mixed implementation/design gap: M001 is too weak for some runs after a
  blocked general shell attempt, and there is no verifier-driven continuation
  when a created file remains semantically wrong.

Admission recommendation:

- No immediate M002 from this scenario alone. The two patterns need separate
  evidence after seeded reruns:
  - blocked-shell-to-Write recovery
  - semantic-failure continuation after a file was written

## `new-python-csv-small`

Result:

- legacy: 5/5
- minimal+M001: 3/5

Minimal run outcomes:

| run | success | M001 fired | reason | class |
|---|---:|---:|---|---|
| 1 | yes | no | ok | n/a |
| 2 | no | yes | `missing_file:tools/csv_stats.py` | `no_edit_loop_after_feedback` |
| 3 | yes | no | ok | n/a |
| 4 | yes | yes | ok after feedback and `Write` | n/a |
| 5 | no | yes | `missing_file:tools/csv_stats.py` | `no_edit_loop_after_feedback` |

Branch point:

- Legacy emits `Write:tools/csv_stats.py` directly in successful runs.
- Minimal run-2 inspects the empty directory, says it will create the script,
  receives M001, then again says it will create the file without a tool call.
- Minimal run-5 first tries blocked `Bash mkdir -p tools`, then says it will use
  `Write`, receives M001, and again says it will create the file without a tool
  call.

Classification:

- `no_edit_loop_after_feedback`: 2

Judgment:

- The previous `min_lines:25` false negative is gone; this is not a check issue.
- The residual failures are the same M001-ineffective no-edit class seen in
  `fix-js-date-helper`, especially after directory creation gets routed through
  blocked `Bash`.

Admission recommendation:

- Candidate only after Task19 seeded reruns confirm the pattern is stable.
  If stable, the smallest candidate is not task-specific CSV logic; it is a
  general blocked-shell-to-Write feedback or tool-selection repair.

## `non-coding-runbook`

Result:

- legacy: 5/5
- minimal+M001: 3/5

Minimal run outcomes:

| run | success | M001 fired | reason | class |
|---|---:|---:|---|---|
| 1 | yes | no | ok | n/a |
| 2 | no | yes | `missing_file:runbooks/incident-triage.md` | `no_edit_loop_after_feedback` |
| 3 | no | unknown | `rc=1`, max iterations, no copied session log | `max_iterations_without_session_log` |
| 4 | yes | yes | ok after feedback and `Write` | n/a |
| 5 | yes | no | ok after blocked `Bash` then `Write` | n/a |

Branch point:

- Legacy run-1 initially emits prose shaped like `Write(...)`, then reaches an
  actual `Write:runbooks/incident-triage.md` call and succeeds.
- Minimal run-2 checks for the directory, observes it is absent, says it will
  create the runbook, receives M001, then again says it will create the runbook
  without a tool call.
- Minimal run-3 has no copied session log. `stdout.log` records:
  `error: minimal loop reached max_iterations (12)`. The workdir is empty.

Classification:

- `no_edit_loop_after_feedback`: 1
- `max_iterations_without_session_log`: 1

Judgment:

- run-2 matches the residual M001-ineffective pattern.
- run-3 cannot be assigned a model-behavior branch point from llm-io because the
  session was not copied; it should be treated as execution/observability debt
  rather than a mechanism admission signal.

Admission recommendation:

- No standalone admission candidate. Keep this as supporting evidence for the
  broader no-edit-after-feedback family, then re-evaluate after seed support and
  better failed-session log preservation.

## Cross-Scenario Pattern

| pattern | affected runs | interpretation |
|---|---:|---|
| feedback fired, then blocked `Bash mkdir`, then no `Write` | 3 | model chooses the wrong tool after M001 and does not recover from offline policy feedback |
| feedback fired, then no-tool prose again | 2 | M001 is acknowledged linguistically but not converted into a tool call |
| file written but not semantically repaired | 1 | completion acceptance lacks semantic verification/continuation |
| session log missing on rc=1 | 1 | observability gap |

Legacy mechanism hints:

- Legacy successful runs reach `Write` earlier and more consistently.
- The observable advantage is artifact-directed persistence, not Edit anchor
  precision.
- This does not justify importing a broad legacy repair subsystem. The next
  candidate, if any, should be a narrowly measured follow-up to a stable seeded
  failure mode.

## Decision

No M002 should be admitted from this triage yet.

Reasons:

1. Task19 is explicitly addressing run-to-run variance; admission should wait
   for seeded evidence.
2. The residual failures combine at least two mechanisms: tool-selection repair
   after blocked `Bash`, and semantic continuation after a file exists.
3. The remaining `non-coding-runbook` rc=1 case lacks session logs, so it should
   not drive mechanism design.

Recommended next step after Task19:

- Re-run the three target scenarios with deterministic bench seeds.
- If the blocked-shell-to-Write pattern reproduces, consider a single
  deterministic feedback triggered by a blocked general shell command before any
  file change.
- Keep semantic-verifier continuation separate; it is a larger behavior surface
  and should not be bundled with blocked-shell recovery.
