# fix-js-date-helper Vibe-Local Branchpoint Triage

Date: 2026-06-14 JST

Purpose: inspect only `fix-js-date-helper` after the vibe-local comparison,
identify where vibe-local succeeds and minimal fails, and decide whether a new
M002 mechanism is justified.

## Inputs

| field | value |
|---|---|
| minimal root | `workspace/eval-artifacts/cycle3-task29-20260613/20260613T124024-63887` |
| vibe-local root | `workspace/eval-artifacts/cycle3-task29-20260613/20260613T163056-25350-vibe-local` |
| model | `qwen3.6:27b-coding-nvfp4` |
| scenario | `fix-js-date-helper` |
| prompt | `Create src/dateRange.js with a buggy inclusiveDays(start, end), then fix it so same-day ranges return 1 and reversed ranges return 0.` |
| success check | `inclusiveDays('2024-01-01', '2024-01-01') === 1` and `inclusiveDays('2024-01-02', '2024-01-01') === 0` |

## Outcome Snapshot

| engine | success | failed runs | successful runs |
|---|---:|---|---|
| minimal + M001 | 1/5 | 1, 2, 3, 5 | 4 |
| vibe-local | 2/5 | 1, 2, 3 | 4, 5 |

Vibe-local is better on this scenario, but not broadly correct. Runs 1-3 write
fixed-looking code that expects `Date` objects and fails the benchmark's string
input check.

## Vibe-Local Successful Branches

### run-4

Observed sequence from `stdout.log`:

1. `Write src/dateRange.js` with a 16-line buggy implementation.
2. Assistant says it will apply the fix.
3. `Edit src/dateRange.js`.
4. Assistant says it will verify.
5. `Bash cd <workdir> && node -e ...`.
6. Final summary reports same-day, reversed, and normal cases passing.

Branchpoint: after the first `Write`, the assistant converts "apply the fix" to
an `Edit` tool call instead of stopping at a natural-language progress note.

### run-5

Observed sequence from `stdout.log`:

1. `Write src/dateRange.js`.
2. `Edit` fails because `old_string` is not found.
3. `Read src/dateRange.js`.
4. `Edit` fails again.
5. `Bash cat -A ...` fails on macOS.
6. Assistant rewrites the whole file with `Write`.
7. `Bash cd <workdir> && node -e ...`.
8. Final summary reports all checked cases passing.

Branchpoint: vibe-local keeps translating failed or pending work into more tool
calls. It does not rely on a single "continue" nudge.

## Minimal Failed Branches

| run | sequence | first bad branch | final artifact |
|---:|---|---|---|
| 1 | `Bash -> Write -> no-tool` | after `Write`, assistant says "verify ... and then fix" without a tool call | buggy; returns raw day diff |
| 2 | `Bash -> M001 feedback -> Write -> no-tool` | after `Write`, assistant says it will verify/read back without a tool call | buggy; comments still mark both bugs |
| 3 | `Bash -> Write -> Read -> no-tool` | after `Read`, assistant identifies both bugs and says it will fix them without a tool call | buggy; comments still mark both bugs |
| 5 | `Glob + Bash -> Write -> no-tool` | after `Write`, assistant explains both bugs and says it will fix them without a tool call | buggy; returns raw day diff |

All four minimal failures are semantic continuation failures after a file
exists. There is no parser failure, no blocked shell command, and no missing
file. The direct bad branch is natural-language continuation being accepted as
the final response.

The minimal success run-4 is also informative: it reaches
`Bash -> Edit -> Read`, then ends with "Step 4: Verify the fix" and no final
verification tool call. The artifact passes external `success_check`, but the
conversation still stops on a progress heading. So the no-tool completion issue
is present even in the successful run.

## Classification

| factor | classification | evidence |
|---|---|---|
| prompt | contributing | minimal only says "Make small coherent changes and verify when practical"; vibe-local prompt says "TOOL FIRST", "multi-step tasks ... complete ALL steps", and "If a tool fails ... immediately try a fix". |
| tool catalog | not the blocker | minimal failures use `Write` successfully and have no tool errors. |
| completion acceptance | direct branchpoint | minimal accepts no-tool progress notes as completion after `Write`/`Read`. |
| observability | adequate for minimal, weak for vibe-local | minimal has `session.json` and `llm-io`; vibe-local comparison preserved only `stdout.log`, enough for tool sequence but not prompt payloads. |
| benchmark prompt | major ambiguity | the scenario is classified as `existing_code_fix` but starts from an empty workdir and asks the model to create a buggy file before fixing it. This artificial two-phase prompt creates a premature stopping point. |

## Subtraction Before Mechanism

The ambiguity to remove is in the benchmark shape, not in minimal-loop runtime
code.

`fix-js-date-helper` was converted from:

```text
Create src/dateRange.js with a buggy inclusiveDays(start, end), then fix it...
```

to a true existing-code fixture in this change:

1. `scripts/bench.sh` now supports `setup_files` entries in benchmark YAML.
2. The scenario pre-seeds `src/dateRange.js` in the run workdir with the buggy
   implementation.
3. The prompt now asks only for the fix.
4. The current semantic `success_check` is unchanged.

This is a benchmark/fixture cleanup, not an agent mechanism. It removes an
artificial two-phase instruction that rewards agents for carrying out
"create-buggy -> fix" internally and punishes agents that stop after the first
phase. It also better matches the `existing_code_fix` category.

Do not loosen the success check. Vibe-local runs 1-3 demonstrate why the string
input semantic check is needed.

## M002 Decision

Do not open a new M002 from this evidence.

The original stable minimal failure was real, but it was confounded by
benchmark prompt ambiguity. The previously tested generic post-write
continuation nudge also failed to improve the target set, so repeating that
direction is not justified.

The fixture-backed rerun below succeeded 5/5, so this scenario no longer
provides a reproducible minimal loss for mechanism admission. Candidate families
to keep in mind for other future scenarios:

- Prompt subtraction/clarification: make the fixed system prompt less ambiguous
  about multi-step tool execution without adding scenario-specific reminders.
- Completion acceptance refinement: reject a no-tool response only if it is
  structurally a progress heading, not a completion summary. This needs a much
  stricter trigger than the rejected post-write feedback experiment.
- No candidate: if a fixture-backed rerun succeeds, the original failure was a
  benchmark shape problem, not a minimal-loop gap.

## Recommendation

Include the fixture-backed `fix-js-date-helper` scenario in the next matrix. Do
not admit a minimal-loop mechanism from this scenario.

No minimal-loop mechanism should be admitted from the current data.

## Fixture-Backed Narrow Rerun

After the benchmark ambiguity was removed, `fix-js-date-helper` was rerun with
the updated fixture-backed scenario.

| field | value |
|---|---|
| root | `.anvil/benchmarks/20260614T093231-50388` |
| engine | `minimal` |
| model | `qwen3.6:27b-coding-nvfp4` |
| runs | 5 |
| flags | `--max-iterations 12 --no-auto-test --no-precautions --no-case-memory --bench-no-debug` |
| result | 5/5 success_check |
| elapsed | 11s, 12s, 32s, 11s, 31s |

Observed tool shape:

| run | sequence | result |
|---:|---|---|
| 1 | `Read -> Edit -> no-tool` | pass |
| 2 | `Read -> Edit -> no-tool` | pass |
| 3 | `Read -> Edit -> Edit -> Read -> no-tool` | pass |
| 4 | `Read -> Edit -> no-tool` | pass |
| 5 | `Read -> Edit -> Edit -> Read -> Read -> no-tool` | pass |

The model still sometimes ends with "Let me verify..." without issuing a final
`Bash` verifier, but the edited artifact passed the semantic success check in
all five runs. The previous `1/5` result was therefore primarily a benchmark
shape problem: the empty-workdir "create buggy, then fix" instruction introduced
an artificial first-phase stopping point.

Updated recommendation: do not open M002 from `fix-js-date-helper`. Keep this
scenario fixture-backed in future matrices and look for remaining stable losses
elsewhere.

## Cross-Engine Fixture Rerun

The same fixture-backed task was also measured with legacy and vibe-local.
Minimal and legacy used `scripts/bench.sh`. Vibe-local is not currently a
supported `bench.sh` engine on this branch, so it was run with a temporary
runner that seeded the same `src/dateRange.js` fixture and invoked
`/Users/maenokota/share/work/github_kewton/vibe-local/vibe-coder.py` with the
same model.

| engine | root | success_check | elapsed mean | notes |
|---|---|---:|---:|---|
| minimal + M001 | `.anvil/benchmarks/20260614T093231-50388` | 5/5 | 19.4s | fixture-backed benchmark, `Read/Edit` based fixes |
| legacy | `.anvil/benchmarks/20260614T094149-18644` | 4/5 | 31.8s | run-5 left the original buggy fixture unchanged |
| vibe-local | `.anvil/benchmarks/20260614T004701-vibe-local-fix-js-date-helper` | 0/5 | 181.8s | auto-parallel split produced advice-only subagent outputs; no file edit |

Legacy improved compared with the old ambiguous prompt, but still failed one
run by not changing `src/dateRange.js`.

Vibe-local failed for a different reason than the old comparison: it
auto-detected four parallel tasks from the comma-separated requirements:

1. `Fix src/dateRange.js so inclusiveDays(start`
2. `end) returns 1 for same-day ranges`
3. `0 for reversed ranges`
4. `counts normal ranges inclusively.`

Each subagent returned a textual fix suggestion, and several suggestions were
semantically correct, but the seeded file remained unchanged in all five runs.
This should not be read as a general vibe-local capability result. It is a
measurement of the current vibe-local invocation path on this fixture-backed
single-file edit task, and it exposes a failure mode in automatic parallel
decomposition for tightly coupled edit requirements.

Updated cross-engine conclusion: after removing the artificial "create buggy,
then fix" step, minimal is the strongest engine on this scenario in both
success rate and latency. The remaining issue is not evidence for M002; it is a
benchmark-runner/tooling difference for vibe-local and a residual one-run
legacy miss.
