# Cycle 3 Narrow Seeded Rerun

Date: 2026-06-13 JST

Purpose: Task25 narrow rerun to verify whether the blocked-`mkdir` trap remains
after the Write catalog and offline Bash error-message fixes.

## Inputs

Previous root:

- Task15 minimal+M001 root:
  `/Users/maenokota/share/work/github_kewton/Anvil-develop/workspace/task15-rerun/.anvil/benchmarks/20260612T174200-32077`
- Recheck file used for success comparison:
  `summary.recheck.tsv`

New root:

- `/Users/maenokota/share/work/github_kewton/Anvil-develop/workspace/task25-narrow-seeded-rerun/.anvil/benchmarks/20260613T003335-98414`

Execution:

- Commit: `6fbedd0f325e0a88299e9926bb4bd9fb0c4a3468`
- Binary: `target/release/anvil`
- Build mtime: `2026-06-13 00:32:43 JST`
- Model: `qwen3.6:27b-coding-nvfp4`
- Engine: `minimal`
- Runs: 10 per scenario
- Seed: enabled, 50/50 runs recorded `bench_seed`
- Flags: `--max-iterations 12 --bench-no-debug --no-precautions
  --no-case-memory --no-auto-test`

The run used a temporary `benchmarks/cycle3-narrow-seeded.yaml` copied from the
existing `minimal-loop-expanded` cases because `scripts/bench.sh` has no
scenario filter. The temporary file was removed after the run and is not part of
this report PR. As a result, `meta.json` records `build.git_dirty: true`; the
release binary itself was built from a clean worktree before the temporary YAML
was added.

`docs/minimal-loop-plan.md` is not tracked on `develop`; for the required
pre-read, the copy in the original workspace root was used.

## Success Results

| scenario | Task15 recheck | Task25 seeded | failures in Task25 | avg elapsed_s |
|---|---:|---:|---|---:|
| `fix-js-date-helper` | 2/5 | 1/10 | `command_failed:1` x8; `missing_file:src/dateRange.js,command_failed:0,command_failed:1` x1 | 22.1 |
| `new-python-csv-small` | 3/5 | 7/10 | `missing_file:tools/csv_stats.py,command_failed:0` x3 | 12.6 |
| `non-coding-runbook` | 3/5 | 7/10 | `missing_file:runbooks/incident-triage.md,command_failed:0` x3 | 54.3 |
| `fix-python-slugify` | 5/5 | 7/10 | `missing_file:src/slugify.py,command_failed:0` x3 | 21.5 |
| `fix-python-retry-policy` | 4/5 | 10/10 | none | 12.7 |

All 50 Task25 runs exited `rc=0`; success_check is the useful result column.
All 50 runs copied `session.json` and canonical `logs/llm-io.jsonl`, so Task22's
failed-run log preservation path is working for this rerun.

## Blocked Command Results

| root/scope | offline blocks | mkdir blocks | M001 -> block -> no Write | pure no-edit after M001 | notes |
|---|---:|---:|---:|---:|---|
| Task15 full 125 | 44 | 40 | n/a | n/a | 122 canonical logs available |
| Task15 selected 5x5 | 13 | 10 | 2 | 4 | 24 logs available; one run lacked copied session log |
| Task25 selected 5x10 | 12 | 3 | 0 | 10 | 50 logs available |

Task25 blocked command classes:

| command class | count | affected runs | outcome |
|---|---:|---|---|
| `mkdir` | 3 | `fix-js-date-helper` run-8; `fix-python-slugify` run-1/run-6 | all recovered to `Write`; 2/3 passed success_check |
| `cd ... && python/node` | 8 | `fix-js-date-helper` run-10; `fix-python-slugify` run-1/run-10 | validation attempts blocked by offline policy; `fix-python-slugify` run-10 still recovered to success |
| `rm` | 1 | `fix-js-date-helper` run-10 | cleanup command blocked; not a file-creation trap |

Scenario-level Task25 behavior:

| scenario | success | M001 fired | pure no-edit after M001 | block after M001 | any mkdir block |
|---|---:|---:|---:|---:|---:|
| `fix-js-date-helper` | 1/10 | 5/10 | 1 | 0 | 1 |
| `new-python-csv-small` | 7/10 | 8/10 | 3 | 0 | 0 |
| `non-coding-runbook` | 7/10 | 5/10 | 3 | 0 | 0 |
| `fix-python-slugify` | 7/10 | 8/10 | 3 | 1 | 2 |
| `fix-python-retry-policy` | 10/10 | 3/10 | 0 | 0 | 0 |

## Assessment

The harmful blocked-`mkdir` trap from Task21 is effectively gone in this narrow
rerun. The exact prior failure path, `M001 fired -> Bash mkdir blocked -> no
Write`, dropped from 2 cases in the selected Task15 logs to 0 in Task25.

Raw `mkdir` blocks did not disappear completely: 3/50 runs still tried `mkdir`.
Those attempts happened before the M001 feedback path and all recovered to a
later `Write`. This is no longer the same trap; it is residual tool-selection
noise with recovery.

The dominant remaining class is now pure no-edit after M001: 10/50 runs received
the feedback, did not hit the blocked-`mkdir` path, and still ended without a
file-changing tool. This is concentrated in:

- `new-python-csv-small`: 3
- `non-coding-runbook`: 3
- `fix-python-slugify`: 3
- `fix-js-date-helper`: 1

`fix-js-date-helper` is a separate warning sign. It dropped to 1/10, but 8/9
failures are semantic `command_failed:1` cases after a file exists, not missing
file or blocked mkdir. That points at semantic repair/verifier territory and
should not be used to justify a no-edit mechanism.

## Variance

Task24 showed that same-seed Ollama execution on this backend is not
byte-for-byte deterministic. Task25 therefore should be treated as a
variance-reduction run, not deterministic replay.

Compared with the prior `n=5` variance rule, the new `n=10` results still show
large scenario instability:

- `fix-js-date-helper`: 2/5 to 1/10, but failure mode changed toward semantic
  command failure.
- `fix-python-slugify`: 5/5 to 7/10, a three-run miss at `n=10`.
- `new-python-csv-small` and `non-coding-runbook`: both 3/5 to 7/10.

The previous rule remains appropriate for `n=5`: +/-2 runs is noise; +/-3 or
larger is an investigation trigger, not causal proof. With `n=10`, a difference
of 3 runs still appears plausible for unstable scenarios. A future same-seed
duplicate run would be needed to estimate residual backend nondeterminism more
directly.

## Task26 Gate Opinion

Proceeding to Task26 is reasonable with this caveat:

- The specific blocked-`mkdir` trap targeted by Task21 is gone or reduced to a
  recovered, non-dominant residual.
- The remaining no-edit failures are not explained by blocked mkdir.
- The next full matrix should not be interpreted as a pure M001 measurement
  unless it includes the planned M001 on/off ablation.

Task26 was not started here because Task25's completion condition is to record
this report and stop.
