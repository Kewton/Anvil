# Blocked Script-Run Triage

Date: 2026-06-13 JST

Scope:

- Basis: [Blocked mkdir trap triage](blocked-mkdir-trap.md)
- Basis: [Cycle 3 narrow seeded rerun](../cycle3-narrow-seeded-rerun.md)
- Task25 root:
  `/Users/maenokota/share/work/github_kewton/Anvil-develop/workspace/task25-narrow-seeded-rerun/.anvil/benchmarks/20260613T003335-98414`
- Model: `qwen3.6:27b-coding-nvfp4`
- Engine: `minimal`

This is investigation only. No minimal-loop, parser, prompt, success-check, or
Bash policy code was changed.

## Findings

### 1. `cd ... && python/node ...` falls through to `General`

`src/tools/bash.rs::classify_command` checks the whole normalized command in
this order:

1. dangerous
2. env setup
3. network
4. read-only
5. build-test
6. script-run
7. mutating
8. general

`is_script_run_command` only accepts commands whose full string starts with one
of:

- `python `
- `python3 `
- `python -m `
- `python3 -m `
- `node `
- `deno run `
- `bun run `
- `ruby `
- `perl `
- `sh `
- `bash `

Therefore a command shaped as:

```text
cd /path/to/workdir && python3 -c "..."
```

does not classify as `ScriptRun`. It also does not classify as `ReadOnly`,
`BuildTest`, `Mutating`, or `Network`, so it becomes `General`. In offline mode,
`enforce_offline_policy` rejects `General`, even though the error text says
offline allows local script-run shell commands.

This is a classifier limitation around simple working-directory wrappers, not a
model capability issue.

### 2. Task25 blocked script-run events

The Task25 report counted 8 canonical `cd ... && python/node ...` blocked
events. They occurred in three successful runs:

| scenario | run | count | success_check | should policy allow? | judgment |
|---|---:|---:|---|---|---|
| `fix-js-date-helper` | 10 | 2 | `true` / `ok` | yes | local `node` validation inside the run workdir |
| `fix-python-slugify` | 1 | 2 | `true` / `ok` | yes | local `python3 -c` validation inside the run workdir |
| `fix-python-slugify` | 10 | 4 | `true` / `ok` | yes | local `python` / `python3` validation inside the run workdir |

All 8 are local script execution attempts under the benchmark run workdir. None
fetch dependencies, call package managers, mutate outside the workdir, or run a
dangerous command. From the YAML intent and real use, they should have been
allowed or normalized into a directly allowed script-run form.

Representative raw commands, one per observed shape:

```text
cd /Users/maenokota/share/work/github_kewton/Anvil-develop/workspace/task25-narrow-seeded-rerun/.anvil/benchmarks/20260613T003335-98414/qwen3.6-27b-coding-nvfp4/minimal/fix-js-date-helper/default/run-10/workdir && node test_dateRange.js
```

```text
cd /Users/maenokota/share/work/github_kewton/Anvil-develop/workspace/task25-narrow-seeded-rerun/.anvil/benchmarks/20260613T003335-98414/qwen3.6-27b-coding-nvfp4/minimal/fix-python-slugify/default/run-1/workdir && python3 -c "from src.slugify import slugify; print(slugify('Hello, World!')); print(slugify('---Hello   World---')); print(slugify('café')); print(slugify('  Hello,   World!  '))"
```

```text
cd /Users/maenokota/share/work/github_kewton/Anvil-develop/workspace/task25-narrow-seeded-rerun/.anvil/benchmarks/20260613T003335-98414/qwen3.6-27b-coding-nvfp4/minimal/fix-python-slugify/default/run-10/workdir && python3 test_slugify.py
```

The inline `-c` payloads were longer in the raw logs, but they follow the same
shape: a `cd` to the current run workdir followed by local `node`/`python`
execution.

### 3. `fix-js-date-helper` semantic failures were not caused by this block

Task25 produced 9 `fix-js-date-helper` failures. Eight were semantic
`command_failed:1` after the file existed, and one was missing-file.

For the eight semantic failures:

| run | success_check reason | tool sequence | blocked script-run count |
|---:|---|---|---:|
| 1 | `command_failed:1` | `Write -> Read` | 0 |
| 2 | `command_failed:1` | `Glob -> Bash -> Write` | 0 |
| 3 | `command_failed:1` | `Bash -> Glob -> Write` | 0 |
| 4 | `command_failed:1` | `Bash -> Glob -> Write` | 0 |
| 5 | `command_failed:1` | `Bash -> Write` | 0 |
| 6 | `command_failed:1` | `Bash -> Write` | 0 |
| 7 | `command_failed:1` | `Bash -> Write` | 0 |
| 8 | `command_failed:1` | `Bash -> Glob -> Bash -> Write` | 0 |

The blocked `node` validation attempts appeared only in run 10, which passed.
So the hypothesis "verification execution was blocked, therefore the model
could not enter the repair loop" is not supported for the `fix-js-date-helper`
semantic failures in Task25.

The semantic failures instead show a simpler pattern: the model writes a buggy
implementation and then stops or says it will verify/fix without issuing the
next tool call. That remains a separate no-tool / semantic-continuation problem,
not evidence for a verifier subsystem yet.

## Judgment

Primary classification:

- **Classifier bug / policy affordance mismatch** for simple
  `cd <run-workdir> && python/node ...` wrappers around local script execution.

Secondary classification:

- **Not the main cause of `fix-js-date-helper` semantic failures** in Task25.
  It is a real policy bug, but the current evidence points to wasted iterations
  in successful runs rather than the direct reason for the 8 semantic failures.

The block is not a correct security decision for these observed commands. The
offline policy already permits local script-run commands; the classifier simply
does not recognize the common shell wrapper form.

## Suggested Fix

Do not broadly allow arbitrary `&&` chains. Keep the offline policy tight and
handle only a narrow deterministic shape.

Candidate implementation:

1. In `run_with_outcome`, before `classify_command`, recognize a leading
   `cd <dir> && <tail>` segment using the existing quote-aware
   `split_shell_control_segments`.
2. Allow only the exact three-part shape: `cd <dir>`, `&&`, `<tail>`.
3. Resolve `<dir>` relative to the current `cwd`, reject symlinks/path escapes,
   and require it to be equal to `cwd` or a descendant of `cwd`.
4. Classify `<tail>` with `classify_command`.
5. Treat the whole command as `ScriptRun` or `BuildTest` only if `<tail>` is
   `ScriptRun` or `BuildTest`; keep `EnvSetup`, `Network`, `Mutating`, and
   `Dangerous` blocked in offline mode.

Unit fixtures to add in the fix PR:

- `cd <cwd> && python3 -c "print(1)"` is allowed offline.
- `cd <cwd> && node test.js` is allowed offline.
- `cd <cwd> && python3 test_slugify.py` is allowed offline.
- `cd /tmp && python3 -c "print(1)"` is rejected when `/tmp` is outside the
  project/workdir.
- `cd <cwd> && python3 test.py && rm out` remains rejected.
- `cd <cwd> && npm install` remains rejected as env setup.
- `cd <cwd> && curl https://example.com` remains rejected as network.

No fix is made in this PR.
