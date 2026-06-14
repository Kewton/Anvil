# Vibe-Local Win Factor Triage

Date: 2026-06-14 JST

Purpose: inspect the three scenarios where the Task29-inclusive comparison
showed a material vibe-local advantage over Anvil minimal, then identify what
should be copied, avoided, or removed from minimal.

## Inputs

| field | value |
|---|---|
| Anvil minimal root | `workspace/eval-artifacts/cycle3-task29-20260613/20260613T124024-63887` |
| vibe-local root | `workspace/eval-artifacts/cycle3-task29-20260613/20260613T163056-25350-vibe-local` |
| model | `qwen3.6:27b-coding-nvfp4` |
| Anvil commit | `b27b79d048ec4b72c485a63556b6c263fac0e6f4` |
| vibe-local commit | `31a750d968e006fc1093897925bd043cf4f92adc` |

The roots are not perfectly equivalent execution environments. Anvil used
`scripts/bench.sh` with seed support and the minimal-loop tool policy.
vibe-local used `/Users/maenokota/share/work/github_kewton/vibe-local/vibe-coder.py`
without seed support and with different prompt, tool policy, and agent loop.

## Scenario Snapshot

| scenario | minimal | vibe-local | vibe mean elapsed | main observed difference |
|---|---:|---:|---:|---|
| `long-session-data-report` | 1/5 | 5/5 | 770.6s | vibe-local completes both artifacts and often verifies or rewrites report data |
| `long-session-read-edit` | 1/5 | 3/5 | 254.6s | vibe-local writes both files before final answer; two failures are 29/30-line border misses |
| `new-large-react-kanban` | 3/5 | 5/5 | 123.0s | vibe-local writes the target path directly with a large single artifact |

## Evidence By Scenario

### `long-session-data-report`

Task: create `data/sample-sales.csv` and `reports/sales-analysis.md`.

Minimal tool sequences:

| run | tool sequence | outcome |
|---:|---|---|
| 1 | `Bash -> Glob -> Write` | wrote only CSV, then said it would create report |
| 2 | `Bash -> Glob` | no files |
| 3 | `Read -> Glob` | no files |
| 4 | `Bash -> Glob -> Write` | wrote only CSV, then said it would create report |
| 5 | `Bash -> Glob -> Write -> Write -> Read -> Read` | wrote both files; success_check passed |

Representative minimal bad branch:

```text
Now let me create the sales analysis report based on the data I just created.
```

That was a no-tool assistant response accepted as completion, leaving
`reports/sales-analysis.md` missing.

vibe-local successful branches:

- run-1 wrote CSV, ran a local analysis command, then wrote a 95-line report.
- run-4 wrote CSV and report, ran a local validation command, then rewrote the
  report with verified numbers.
- run-3 used parallel subagents for the two files, then read both files back.

Factor classification:

| factor | assessment |
|---|---|
| verification continuation | helpful in runs 1 and 4; report data was grounded by a local command |
| retry/rewrite | helpful in run 4; report rewritten after validation |
| sub-agent / parallel | helpful in run 3 because the two artifacts are genuinely separable |
| prompt | likely helpful: vibe-local strongly says multi-step tasks must complete all steps |
| tool affordance | not the blocker for minimal; Write can create parent directories |
| minimal failure class | accepted progress statement before all requested artifacts existed |

### `long-session-read-edit`

Task: create an 80+ line `docs/notes/architecture.md`, then create
`src/summary.ts` exporting `summarizeArchitectureNotes`.

Minimal tool sequences:

| run | tool sequence | outcome |
|---:|---|---|
| 1 | `Read -> Glob -> Bash` | no files |
| 2 | `Read -> Glob` | no files |
| 3 | `Read -> Glob` | no files |
| 4 | `Write -> Bash` | wrote only architecture notes, then said it would create summary |
| 5 | `Write -> Write` | wrote both files; success_check passed |

Representative minimal bad branch:

```text
The file has 139 lines, which exceeds the 80-line minimum. Now let me create the TypeScript summary file.
```

That was accepted as completion, leaving `src/summary.ts` missing.

vibe-local branches:

- All five runs wrote both files before the final response.
- Runs 1, 2, and 5 passed.
- Runs 3 and 4 failed only because `src/summary.ts` had 29 lines against a
  30-line check. The task behavior was mostly correct; the failure is a
  borderline check artifact, not a missing-work failure.

Factor classification:

| factor | assessment |
|---|---|
| verification continuation | useful but not essential; vibe-local mostly succeeded by writing both files first |
| retry/rewrite | not the main factor |
| sub-agent / parallel | not necessary; sequential `Write -> Write` works |
| prompt | likely helpful: vibe-local explicitly says complete all multi-step tasks |
| tool affordance | not the blocker |
| minimal failure class | accepted progress statement after first artifact or after exploration |

### `new-large-react-kanban`

Task: create `src/components/KanbanBoard.tsx` with a self-contained React
Kanban board, three columns, cards, add-card form, move buttons, and local state.

Minimal tool sequences:

| run | tool sequence | outcome |
|---:|---|---|
| 1 | `Glob -> Bash -> Bash -> Bash -> Bash -> Bash -> Bash` | no target file |
| 2 | `Glob -> Bash -> Bash -> Bash -> Write` | pass |
| 3 | `Glob -> Bash -> Bash -> Bash -> Bash -> Bash -> Write` | pass |
| 4 | `Bash -> Glob -> Bash -> Write` | pass |
| 5 | `Glob -> Bash -> Bash -> Bash` | no target file |

Representative minimal bad branches:

```text
I'll use Write to create all the necessary files. Let me start by creating the project structure and the KanbanBoard component.
```

and:

```text
I see, I need to create the file. Let me create the `KanbanBoard.tsx` component now.
```

Both were no-tool progress statements accepted as completion. run-1 also hit a
blocked `mkdir -p` path before failing to recover to `Write`.

vibe-local branches:

- All five runs wrote `src/components/KanbanBoard.tsx` directly.
- Generated files were 294, 380, 328, 420, and 353 lines.
- Runs did little exploration: usually `Bash ls` followed by one `Write`, or
  just one `Write`.

Factor classification:

| factor | assessment |
|---|---|
| verification continuation | not central; success_check is mostly path/line/name based |
| retry/rewrite | not central |
| sub-agent / parallel | not used; single large direct Write is enough |
| prompt | likely helpful: vibe-local pushes immediate action instead of planning text |
| tool affordance | relevant; minimal still tries `mkdir`, then may or may not recover to `Write` |
| minimal failure class | over-exploration plus accepted progress statement; one blocked mkdir trap residue |

## Cross-Cutting Findings

### What vibe-local does better here

1. It converts multi-artifact instructions into tool calls more reliably.
   `Write -> Write` is the decisive pattern for both long-session scenarios.

2. It sometimes validates generated artifacts with local commands and then
   rewrites the artifact. This helped `long-session-data-report`, especially
   when report figures had to match the CSV.

3. It is willing to spend much more time. The three win scenarios averaged
   123s to 771s under vibe-local, compared with failed minimal runs often
   ending in single-digit or tens of seconds. Some wins are real, but they are
   bought with a large latency budget.

4. Its stronger prompt contains useful operational constraints: tool-first,
   complete all multi-step tasks, and recover from tool failures immediately.

### What should not be copied wholesale

1. Do not import vibe-local's automatic `ParallelAgents` behavior into minimal.
   It helped one separable two-artifact run, but the fixture-backed
   `fix-js-date-helper` rerun showed the same decomposition can split one
   tightly coupled edit request into advice-only subagents and fail 0/5.

2. Do not add an open-ended "keep trying until success" loop. It explains some
   vibe-local wins, but it also creates very expensive failures:
   `non-coding-runbook` 0/5 at 428s mean, `non-coding-research-brief` 0/5 at
   380s mean, `multi-file-python-package` 0/5 at 410s mean.

3. Do not add scenario-specific success_check knowledge to the runtime. The
   interesting runtime problem is not "know the benchmark expected paths"; it
   is "do not accept a progress statement as final completion."

## Subtraction Candidates Before New Mechanisms

These remove ambiguity or bad affordances before adding more control machinery.

| candidate | rationale | affected evidence | risk |
|---|---|---|---|
| tighten Bash catalog wording | Bash currently looks like a general setup tool; for file creation, models still try `mkdir`. State up front that Bash is not for creating directories/files and that `Write` creates parents. | Kanban failed run-1/run-5 pattern, earlier mkdir trap | low; factual tool affordance |
| reduce empty-workdir exploration pressure | For explicit `Create <path>` tasks, exploration often wastes turns and creates progress-text stops. Replace broad "prefer tools for repository facts" pressure with "if the user gives exact output paths, write them directly unless existing context is needed." | all three scenarios, especially `long-session-read-edit` runs 1-3 | medium; prompt change may affect real repo edits |
| avoid "verify when practical" as an excuse for progress text | The phrase is weak and often becomes "Let me verify..." with no tool call. Consider replacing it with "If you say you will verify or create something, call the tool in the same turn." | `long-session-read-edit` run-5, Kanban successes and failures | medium; prompt snapshot churn |

These are prompt/tool-catalog clarifications, not mechanism admissions. They
should be measured as prompt ablations before new runtime feedback is added.

## Additive Candidates, After Subtraction

### Candidate M002: progress-statement completion guard

Trigger idea: if the assistant returns a no-tool response that is structurally a
forward-looking progress statement, inject one neutral feedback turn instead of
accepting completion.

Examples from this triage:

- "Now let me create the sales analysis report..."
- "Now let me create the TypeScript summary file."
- "Let me start by creating the project structure..."
- "Let me verify both files were created correctly."

Suggested neutral feedback:

```text
Your last message described a next action but did not execute it. If work remains, call the appropriate tool now. If the task is actually complete, provide a final answer without saying you will do more.
```

Why this is narrower than the rejected post-write continuation experiment:

- It does not trigger merely because a file was written.
- It targets the exact bad branch: no-tool text that says work is about to
  happen.
- It does not assert that file changes are required.
- It can be single-use per session and behind an off flag.

Risks:

- Needs a conservative classifier. A loose natural-language detector can become
  another legacy-style repair loop.
- Must be measured on non-file informational tasks to avoid forcing tools when
  the user only asked a question.

### Candidate: bounded local validation continuation

This is less ready than the progress guard. vibe-local's strongest
`long-session-data-report` behavior was "write data -> run local summary ->
write report -> sometimes rewrite". However, minimal should not grow a generic
verifier loop until a stable loss cluster proves that missing local validation,
not completion acceptance, is the direct cause.

Current evidence says completion acceptance is the primary blocker in these
three scenarios. Validation continuation should stay behind M002 in priority.

## Recommended Minimal Roadmap

1. First PR: tool-catalog/prompt subtraction.
   - Clarify Bash is not the directory creation path.
   - Clarify exact-path create tasks should use `Write` directly.
   - Replace weak "verify when practical" wording with a tool-action invariant.
   - Snapshot tests only; no runtime mechanism.

2. Narrow ablation:
   - Run these three scenarios plus a small set of known-good generation tasks.
   - Check that `long-session-data-report`, `long-session-read-edit`, and
     `new-large-react-kanban` improve without hurting previously strong
     multi-file tasks.

3. Only if failures remain stable: admit M002 as the progress-statement guard.
   - Trigger on no-tool forward-looking statements.
   - Single-use per session.
   - Off flag required.
   - Measure on the three target scenarios plus non-coding question-like tasks.

4. Defer verifier/long-retry machinery.
   - vibe-local proves it can help, but also proves the cost and failure modes
     are large.
   - Do not import ParallelAgents or broad retry loops into minimal.

## Bottom Line

The vibe-local wins are real on the Task29 matrix, but the transferable lesson
is not "add subagents" or "keep trying for minutes." The strongest minimal
improvement path is to remove ambiguity that lets the model narrate a next
action without taking it, then measure a narrow progress-statement guard only if
that subtraction is insufficient.

## Bash Catalog Subtraction Rerun

Date: 2026-06-14 JST

Change measured:

- `Bash` tool description now says it is for read-only inspection, build/test,
  and local script validation.
- It explicitly says not to use Bash to create files or directories.
- It points file creation to `Write` and states that `Write` creates parent
  directories automatically.

Validation:

| check | result |
|---|---|
| `cargo test --lib prompt_snapshot` | pass |
| `cargo test --lib tool_description` | pass |
| `cargo build --release` | pass |

Rerun command:

```bash
scripts/bench.sh minimal-loop-expanded --engine minimal --model qwen3.6:27b-coding-nvfp4 --cases long-session-data-report,long-session-read-edit,new-large-react-kanban --runs 5 --max-iterations 12 --no-auto-test --no-precautions --no-case-memory --bench-no-debug
```

Rerun root:

```text
.anvil/benchmarks/20260614T103905-8684
```

### Result

| scenario | Task29 minimal | catalog-subtraction rerun | vibe-local reference | observation |
|---|---:|---:|---:|---|
| `long-session-data-report` | 1/5 | 3/5 | 5/5 | improved; 3 runs wrote both CSV and report |
| `long-session-read-edit` | 1/5 | 0/5 | 3/5 | all runs wrote only `docs/notes/architecture.md` |
| `new-large-react-kanban` | 3/5 | 1/5 | 5/5 | one pass; failures mostly explored and never wrote target |

Mean elapsed:

| scenario | mean elapsed |
|---|---:|
| `long-session-data-report` | 79.6s |
| `long-session-read-edit` | 31.4s |
| `new-large-react-kanban` | 32.4s |

### Log Findings

No measured run used `mkdir`, `cat >`, `tee`, or `touch` through Bash. The
catalog change achieved the narrow affordance goal: the model no longer treated
Bash as the directory/file creation path in these runs.

The remaining failures shifted to the clearer completion-acceptance pattern:

| scenario | representative no-tool stop |
|---|---|
| `long-session-data-report` | `Now let me create the analysis report based on the sample data.` |
| `long-session-read-edit` | `Now let me create the TypeScript summary file.` |
| `new-large-react-kanban` | `I'll start by exploring the project structure... then create the KanbanBoard component.` |

For `long-session-read-edit`, all five runs wrote
`docs/notes/architecture.md` and then stopped before `src/summary.ts`.

### Interpretation

The Bash catalog subtraction is useful but insufficient. It removed the shell
creation affordance trap and helped `long-session-data-report`, but it did not
solve the core failure mode: the runtime still accepts a forward-looking
progress statement as final completion.

Next best candidate remains the narrow progress-statement completion guard, not
subagents or broad retry machinery.

## Final-Answer Contract Prompt Rerun

Date: 2026-06-14 JST

Change measured on top of the Bash catalog subtraction:

- Replaced weak rule `Make small coherent changes and verify when practical`.
- Added a same-response tool-action contract:
  `If you say you will create, edit, read, or verify something, call the tool in that same response.`
- Added a final-answer contract:
  `Final answers must describe completed work, not planned next steps. Do not end with phrases like "I will create", "Let me verify", or "Now I'll edit".`

Validation:

| check | result |
|---|---|
| `cargo test --lib prompt_snapshot` | pass |
| `cargo build --release` | pass |

Rerun command:

```bash
scripts/bench.sh minimal-loop-expanded --engine minimal --model qwen3.6:27b-coding-nvfp4 --cases long-session-data-report,long-session-read-edit,new-large-react-kanban --runs 5 --max-iterations 12 --no-auto-test --no-precautions --no-case-memory --bench-no-debug
```

Rerun root:

```text
.anvil/benchmarks/20260614T110217-33113
```

### Result

| scenario | Task29 minimal | Bash catalog rerun | final-contract rerun | vibe-local reference | observation |
|---|---:|---:|---:|---:|---|
| `long-session-data-report` | 1/5 | 3/5 | 0/5 | 5/5 | regressed; all runs stopped before report creation |
| `long-session-read-edit` | 1/5 | 0/5 | 4/5 | 3/5 | improved; all runs wrote both files, one failed only `min_lines:26<30` |
| `new-large-react-kanban` | 3/5 | 1/5 | 4/5 | 5/5 | improved; four runs wrote the target component |

Mean elapsed:

| scenario | mean elapsed |
|---|---:|
| `long-session-data-report` | 27.6s |
| `long-session-read-edit` | 92.4s |
| `new-large-react-kanban` | 89.8s |

### Log Findings

The prompt change strongly improved the two coding scenarios:

- `long-session-read-edit`: all five runs wrote both
  `docs/notes/architecture.md` and `src/summary.ts`; four passed.
- `new-large-react-kanban`: four runs wrote `src/components/KanbanBoard.tsx`
  and passed.

The data-report scenario exposed the limit of prompt-only wording. All five
runs still ended with forward-looking progress text:

| run | final no-tool stop |
|---:|---|
| 1 | `I'll create both files now — the CSV data file and the Markdown analysis report.` |
| 2 | `I'll create both files now — the CSV data and the analysis report.` |
| 3 | `I'll create both files now — the CSV data file and the Markdown analysis report.` |
| 4 | `I'll create both files now — the CSV data file and the Markdown analysis report.` |
| 5 | `I see the directory is empty. Let me create both files now.` |

Runs 1-4 wrote only `data/sample-sales.csv`; run 5 wrote no files. The prompt
contract did not catch the contraction `I'll create...`, and the runtime still
accepted the no-tool progress statement as completion.

### Interpretation

Prompt subtraction is useful but brittle:

- It can materially improve scenarios where the model is close to the right
  tool sequence.
- It can also shift wording rather than eliminate the bad branch.
- Adding more phrase examples would become prompt whack-a-mole.

The next defensible step is no longer more prompt examples. It is the narrow
runtime guard described above: detect forward-looking no-tool progress
statements and give one neutral feedback turn. This should be measured as M002
with an off flag and with the same three target scenarios plus a small
non-regression set.

## Lightweight Non-Regression Rerun

Date: 2026-06-14 JST

Before moving to M002, five lightweight scenarios with high Task29 baseline
success were rerun with the Bash catalog and final-answer prompt changes.

Rerun root:

```text
.anvil/benchmarks/20260614T112702-72007
```

Selected scenarios:

- `fix-json-normalizer`
- `fix-readme-command`
- `new-markdown-release-notes`
- `new-python-csv-small`
- `new-typescript-formatter`

### Result

| scenario | Task29 baseline | prompt-contract rerun | delta | mean elapsed |
|---|---:|---:|---:|---:|
| `fix-json-normalizer` | 5/5 | 5/5 | +0 | 10.2s |
| `fix-readme-command` | 5/5 | 4/5 | -1 | 11.8s |
| `new-markdown-release-notes` | 5/5 | 5/5 | +0 | 9.4s |
| `new-python-csv-small` | 5/5 | 5/5 | +0 | 17.6s |
| `new-typescript-formatter` | 5/5 | 0/5 | -5 | 5.4s |
| total | 25/25 | 19/25 | -6 | 10.9s |

### Findings

Three scenarios showed no regression:

- `fix-json-normalizer`
- `new-markdown-release-notes`
- `new-python-csv-small`

`fix-readme-command` had one false-negative-like miss:

- run-4 wrote `README.md`, but it was 17 lines against `min_lines:20`.

`new-typescript-formatter` regressed on the current success check:

- all five runs wrote `src/formatTitle.ts`;
- all five implementations were 5 lines against `min_lines:8`;
- the implementation was concise:

```typescript
export function formatTitle(input: string): string {
  return input
    .trim()
    .replace(/\s+/g, ' ')
    .replace(/\b\w/g, (char) => char.toUpperCase());
}
```

The old Task29 successes used a longer `split/map/join` implementation that
passed the line-count check. This looks more like a check-shape sensitivity
than a clear functional regression, because the scenario only checks for
`export function formatTitle` plus line count. Still, for admission discipline
this is a non-regression failure: the prompt change cannot be treated as safe
until either the check is made semantic or the prompt change is measured without
this regression.

### Interpretation

The final-answer prompt contract improves the targeted long/multi-artifact
coding scenarios, but it is not clean enough to promote as-is:

- target wins: `long-session-read-edit` and `new-large-react-kanban` improved
  to 4/5;
- target miss: `long-session-data-report` regressed to 0/5;
- lightweight non-regression miss: `new-typescript-formatter` dropped to 0/5
  on a line-count-sensitive check.

Before M002, either:

1. convert `new-typescript-formatter` to a semantic check and recheck, or
2. treat the prompt contract as too unstable and move to the narrower runtime
   guard without adding more prompt pressure.

## TypeScript Formatter Semantic Recheck

Date: 2026-06-14 JST

`new-typescript-formatter` was converted from a line-count check to a semantic
check:

- removed `min_lines:8` for `src/formatTitle.ts`;
- kept the `export function formatTitle` grep;
- added a Node-based semantic check that loads the generated TypeScript source,
  strips the simple type annotations used by the fixture, and verifies trim,
  space collapse, and title-casing behavior on three inputs.

Recheck root:

```text
.anvil/benchmarks/20260614T112702-72007
```

Recheck output:

```text
.anvil/benchmarks/20260614T112702-72007/summary.recheck.tsv
```

### Result

| scenario | prompt-contract original check | semantic recheck | note |
|---|---:|---:|---|
| `fix-json-normalizer` | 5/5 | 5/5 | unchanged |
| `fix-readme-command` | 4/5 | 4/5 | run-4 remains `min_lines:README.md:17<20` |
| `new-markdown-release-notes` | 5/5 | 5/5 | unchanged |
| `new-python-csv-small` | 5/5 | 5/5 | unchanged |
| `new-typescript-formatter` | 0/5 | 5/5 | all five concise implementations are semantically valid |
| total | 19/25 | 24/25 | remaining miss is unrelated to formatter |

### Interpretation

The `new-typescript-formatter` drop was a success-check false negative, not a
functional regression from the prompt-contract change. The lightweight
non-regression picture after semantic recheck is therefore 24/25, with the only
remaining miss being the known `fix-readme-command` line-count case.

This does not resolve the target-scenario instability:

- `long-session-read-edit` and `new-large-react-kanban` still support the
  direction of removing prompt ambiguity;
- `long-session-data-report` still shows that prompt-only wording is brittle
  and can be bypassed by forward-looking no-tool completion text.

The next M002 candidate should still be judged on a narrow runtime guard rather
than more prompt examples.

## Runtime Guard Rerun

Date: 2026-06-14 JST

Implemented a narrow runtime guard for forward-looking no-tool completions:

- if an Act-mode assistant response has no tool call;
- and the response looks like a next-action statement such as `I'll create...`,
  `Let me verify...`, or `Now I'll run...`;
- inject one ephemeral feedback turn instead of accepting completion;
- keep the guard one-shot per session to avoid infinite loops;
- add off flag `ANVIL_NO_MINIMAL_FORWARD_PROGRESS_FEEDBACK`.

The concrete prompt examples from the previous final-answer prompt contract
were removed from the fixed system prompt. The prompt now keeps only the general
rule, while completion acceptance is handled by the runtime guard.

Rerun root:

```text
.anvil/benchmarks/20260614T120118-21544
```

Command shape:

```text
scripts/bench.sh minimal-loop-expanded --engine minimal \
  --model qwen3.6:27b-coding-nvfp4 \
  --cases long-session-data-report,long-session-read-edit,new-large-react-kanban,fix-json-normalizer,fix-readme-command,new-markdown-release-notes,new-python-csv-small,new-typescript-formatter \
  --runs 5 --max-iterations 12 --no-auto-test --no-precautions \
  --no-case-memory --bench-no-debug
```

### Result

| scenario | previous checked baseline | runtime guard rerun | delta |
|---|---:|---:|---:|
| `long-session-data-report` | 0/5 | 1/5 | +1 |
| `long-session-read-edit` | 4/5 | 4/5 | +0 |
| `new-large-react-kanban` | 4/5 | 1/5 | -3 |
| `fix-json-normalizer` | 5/5 | 2/5 | -3 |
| `fix-readme-command` | 4/5 | 5/5 | +1 |
| `new-markdown-release-notes` | 5/5 | 5/5 | +0 |
| `new-python-csv-small` | 5/5 | 5/5 | +0 |
| `new-typescript-formatter` | 5/5 | 5/5 | +0 |
| total | 32/40 | 28/40 | -4 |

Runtime guard firing:

| scenario | forward-progress guard hits | success in hit runs |
|---|---:|---:|
| `long-session-data-report` | 5/5 | 1/5 |
| `fix-json-normalizer` | 2/5 | 1/2 |
| `fix-readme-command` | 2/5 | 2/2 |
| all other selected scenarios | 0/25 | n/a |

### Findings

The guard caught real target behavior in `long-session-data-report`, but one
neutral feedback turn was not enough. Four failed runs ended with another
forward-looking no-tool statement after the guard:

- `I'll create both files now. Let me start by writing...`
- `I'll create both files now. Let me first create...`
- `Now let me create the analysis report...`

The guard also missed some clear forward-looking variants because the classifier
was intentionally narrow:

- `Now let me create the source files...`
- `I'll start by exploring the project structure...`

Those misses explain why `new-large-react-kanban` did not benefit from the
runtime guard despite still showing the same no-tool planning shape.

The non-regression set is mixed. `fix-readme-command`,
`new-markdown-release-notes`, `new-python-csv-small`, and
`new-typescript-formatter` were fine, but `fix-json-normalizer` regressed to
2/5. The failing runs mostly stopped before writing `src/normalizeJson.ts`,
and only one of the three failures involved the new guard directly.

### Interpretation

This M002 shape should not be admitted as-is:

- it improves the primary failing target only from 0/5 to 1/5;
- it fails to recover repeated no-tool future-action responses after one
  feedback turn;
- it has a visible lightweight regression in `fix-json-normalizer`;
- broadening the phrase classifier would move toward the same phrase-list
  overfitting problem that made the prompt-only approach brittle.

The useful finding is narrower: completion acceptance is indeed part of the
problem, but a one-shot future-phrase guard is not the right standalone
mechanism. The next candidate should avoid accumulating phrase examples and
instead look for a more deterministic acceptance fact, such as "assistant
claims it is about to create/read/verify a specific artifact and no tool call is
present" or "a required benchmark artifact remains missing after a no-tool
completion" before adding more feedback.

## Runtime Guard Rollback Rerun

Date: 2026-06-14 JST

The forward-progress runtime guard implementation was removed after the failed
rerun above. The retained changes are:

- `new-typescript-formatter` semantic success check;
- Bash/tool catalog wording that makes Write the file creation affordance;
- the final-answer prompt contract from the pre-runtime-guard pass.

The M002-specific code path and off flag were removed:

- no `ANVIL_NO_MINIMAL_FORWARD_PROGRESS_FEEDBACK`;
- no forward-progress no-tool classifier;
- no one-shot runtime feedback for future-action prose.

Rerun root:

```text
.anvil/benchmarks/20260614T141146-13274
```

Command shape:

```text
scripts/bench.sh minimal-loop-expanded --engine minimal \
  --model qwen3.6:27b-coding-nvfp4 \
  --cases long-session-data-report,long-session-read-edit,new-large-react-kanban,fix-json-normalizer,fix-readme-command,new-markdown-release-notes,new-python-csv-small,new-typescript-formatter \
  --runs 5 --max-iterations 12 --no-auto-test --no-precautions \
  --no-case-memory --bench-no-debug
```

### Result

| scenario | M002 runtime guard | rollback rerun | delta |
|---|---:|---:|---:|
| `long-session-data-report` | 1/5 | 1/5 | +0 |
| `long-session-read-edit` | 4/5 | 3/5 | -1 |
| `new-large-react-kanban` | 1/5 | 4/5 | +3 |
| `fix-json-normalizer` | 2/5 | 5/5 | +3 |
| `fix-readme-command` | 5/5 | 5/5 | +0 |
| `new-markdown-release-notes` | 5/5 | 5/5 | +0 |
| `new-python-csv-small` | 5/5 | 5/5 | +0 |
| `new-typescript-formatter` | 5/5 | 5/5 | +0 |
| total | 28/40 | 33/40 | +5 |

Failures after rollback:

| scenario | failed runs | direct reason |
|---|---:|---|
| `long-session-data-report` | 4 | missing `reports/sales-analysis.md` |
| `long-session-read-edit` | 2 | `src/summary.ts` below `min_lines:30` |
| `new-large-react-kanban` | 1 | missing `src/components/KanbanBoard.tsx` |

All five lightweight non-regression scenarios passed:

- `fix-json-normalizer`: 5/5;
- `fix-readme-command`: 5/5;
- `new-markdown-release-notes`: 5/5;
- `new-python-csv-small`: 5/5;
- `new-typescript-formatter`: 5/5.

### Interpretation

Rolling back M002 recovered the non-regression set and restored
`new-large-react-kanban` to the pre-M002 level. The forward-progress guard
should remain rejected.

The remaining heavy-task failures are now narrower:

- `long-session-data-report` still needs a separate triage path; neither prompt
  contract nor the rejected runtime guard reliably gets both artifacts written.
- `long-session-read-edit` failures are line-count/check-shape misses rather
  than missing files.
- `new-large-react-kanban` is mostly recovered, with one missing-component run.

This is the best current local state for this slice: retain semantic/check and
affordance fixes, keep M002 out, and use the rollback rerun as the comparison
baseline for the next candidate.

## Explicit Requested Artifact Gate Rerun

Date: 2026-06-14 JST

This pass admits a narrower M002 candidate than the rejected
forward-progress guard. The trigger is deliberately deterministic:

- extract explicit file-like paths from the original user prompt only;
- when the assistant returns a no-tool completion, check whether those files
  exist under the work root;
- if any requested files are missing, inject one ephemeral feedback turn naming
  the missing paths;
- fire at most once per session;
- do not classify task intent, future-tense prose, or semantic completeness.

The off flag is:

```text
ANVIL_NO_MINIMAL_REQUESTED_ARTIFACT_FEEDBACK
```

Validation before rerun:

- `cargo fmt`
- `bash -n scripts/bench.sh`
- `cargo test --lib minimal_loop::loop_run`
- `cargo test --lib minimal_loop::feedback`
- `cargo test --lib prompt_snapshot`
- `tests/scripts/test_bench_smoke.sh`
- `cargo build --release`

Rerun root:

```text
.anvil/benchmarks/20260614T150945-19712
```

Build metadata from `meta.json`:

```text
git_revision: 1244a3b58a29012f8c7c6329d05a88c3bc7ec0d2-dirty
binary_path: /Users/maenokota/share/work/github_kewton/Anvil-develop/target/release/anvil
build_time: 2026-06-14T06:09:25Z
```

`active_flags` did not contain
`ANVIL_NO_MINIMAL_REQUESTED_ARTIFACT_FEEDBACK`, so the candidate was enabled.

Command shape:

```text
scripts/bench.sh minimal-loop-expanded --engine minimal \
  --model qwen3.6:27b-coding-nvfp4 \
  --cases long-session-data-report,long-session-read-edit,new-large-react-kanban,fix-json-normalizer,fix-readme-command,new-markdown-release-notes,new-python-csv-small,new-typescript-formatter \
  --runs 5 --max-iterations 12 --no-auto-test --no-precautions \
  --no-case-memory --bench-no-debug
```

### Result

| scenario | rollback baseline | explicit artifact gate | delta |
|---|---:|---:|---:|
| `long-session-data-report` | 1/5 | 4/5 | +3 |
| `long-session-read-edit` | 3/5 | 2/5 | -1 |
| `new-large-react-kanban` | 4/5 | 5/5 | +1 |
| `fix-json-normalizer` | 5/5 | 5/5 | +0 |
| `fix-readme-command` | 5/5 | 5/5 | +0 |
| `new-markdown-release-notes` | 5/5 | 5/5 | +0 |
| `new-python-csv-small` | 5/5 | 5/5 | +0 |
| `new-typescript-formatter` | 5/5 | 5/5 | +0 |
| total | 33/40 | 36/40 | +3 |

All runs exited with `rc=0`.

Failures after this candidate:

| scenario | failed runs | direct reason |
|---|---:|---|
| `long-session-data-report` | 1 | missing `reports/sales-analysis.md` |
| `long-session-read-edit` | 3 | `src/summary.ts` below `min_lines:30` |

### Feedback Activation

The new feedback fired only in `long-session-data-report`:

| run | outcome | note |
|---:|---|---|
| 4 | fail | feedback detected missing `reports/sales-analysis.md`, but the model had already written the report to a nested absolute-path-like location and did not repair it |
| 5 | pass | feedback detected missing `reports/sales-analysis.md`; the next turn wrote the missing report and verified both files |

The successful run-5 path is the intended recovery case: one requested artifact
already existed, another explicit requested artifact was still missing, and the
model was about to stop with a no-tool "Now I'll create..." response.

The failed run-4 is a separate path-handling failure. The model wrote the
report to:

```text
Users/maenokota/share/work/.../workdir/reports/sales-analysis.md
```

inside the workdir, instead of repository-relative
`reports/sales-analysis.md`. The existence gate correctly noticed the requested
path was still missing, but a single feedback turn was not enough to recover.

### Interpretation

This M002 candidate is materially better than the rejected phrase-based runtime
guard:

- it improves the target heavy scenario, `long-session-data-report`, from 1/5
  to 4/5;
- it improves the 8-scenario slice from 33/40 to 36/40;
- the high-success lightweight scenarios remained 5/5;
- the trigger is based on observable artifact existence, not expanding phrase
  examples.

The `long-session-read-edit` -1 movement is not directly attributable to this
candidate: that scenario's requested path exists, and its remaining failures are
line-count/check-shape misses rather than missing explicit artifacts. With
`n=5`, this should be treated as watchlist noise unless it reproduces in a
targeted rerun.

Admission judgment: admit conditionally as M002 for the missing explicit
artifact class. The admitted scope is narrow: it is not a semantic verifier, not
a general "continue working" guard, and not a phrase classifier. The next
ablation should compare the same heavy/light slice with
`ANVIL_NO_MINIMAL_REQUESTED_ARTIFACT_FEEDBACK=1` to isolate its contribution
from run-to-run variance.

## Explicit Requested Artifact Gate Ablation

Date: 2026-06-14 JST

This run repeats the same 8-scenario slice with the requested-artifact feedback
disabled:

```text
ANVIL_NO_MINIMAL_REQUESTED_ARTIFACT_FEEDBACK=1
```

Rerun root:

```text
.anvil/benchmarks/20260614T164741-32099
```

Build metadata from `meta.json`:

```text
git_revision: 1244a3b58a29012f8c7c6329d05a88c3bc7ec0d2-dirty
binary_path: /Users/maenokota/share/work/github_kewton/Anvil-develop/target/release/anvil
build_time: 2026-06-14T06:09:25Z
```

`active_flags` contained
`ANVIL_NO_MINIMAL_REQUESTED_ARTIFACT_FEEDBACK=1`, and no llm-io log contained
the requested-artifact feedback text. The off flag worked as intended.

Command shape:

```text
scripts/bench.sh minimal-loop-expanded --engine minimal \
  --model qwen3.6:27b-coding-nvfp4 \
  --cases long-session-data-report,long-session-read-edit,new-large-react-kanban,fix-json-normalizer,fix-readme-command,new-markdown-release-notes,new-python-csv-small,new-typescript-formatter \
  --runs 5 --max-iterations 12 --no-auto-test --no-precautions \
  --no-case-memory --bench-no-debug \
  --no-minimal-requested-artifact-feedback
```

### On/Off Result

| scenario | M002 on | M002 off | delta |
|---|---:|---:|---:|
| `long-session-data-report` | 4/5 | 2/5 | +2 |
| `long-session-read-edit` | 2/5 | 2/5 | +0 |
| `new-large-react-kanban` | 5/5 | 4/5 | +1 |
| `fix-json-normalizer` | 5/5 | 5/5 | +0 |
| `fix-readme-command` | 5/5 | 5/5 | +0 |
| `new-markdown-release-notes` | 5/5 | 5/5 | +0 |
| `new-python-csv-small` | 5/5 | 5/5 | +0 |
| `new-typescript-formatter` | 5/5 | 5/5 | +0 |
| total | 36/40 | 33/40 | +3 |

All off-run cases exited with `rc=0`.

Off-run failures:

| scenario | failed runs | direct reason |
|---|---:|---|
| `long-session-data-report` | 3 | missing `reports/sales-analysis.md` |
| `long-session-read-edit` | 3 | `src/summary.ts` below `min_lines:30` |
| `new-large-react-kanban` | 1 | missing `src/components/KanbanBoard.tsx` |

### Direct Failure Shape

The off-run `long-session-data-report` failures were the exact target class.
Each failing run wrote only `data/sample-sales.csv`, then stopped with a
no-tool statement that the report would be created next:

| run | final no-tool statement | observed files |
|---:|---|---|
| 1 | `Now I'll create the sales analysis markdown report.` | `data/sample-sales.csv` only |
| 4 | `Now I'll create the sales analysis report that references the CSV data.` | `data/sample-sales.csv` only |
| 5 | `Now I'll create the sales analysis report.` | `data/sample-sales.csv` only |

With M002 on, the same scenario had only one failure. One on-run was recovered
by the feedback after the CSV was written and `reports/sales-analysis.md` was
still absent. The remaining on-run failure was different: the model wrote the
report to an absolute-path-like nested location, so the requested relative path
still did not exist.

### Admission Update

This ablation strengthens the M002 admission:

- the target scenario improves by +2 with the mechanism on;
- the total 8-scenario slice improves by +3;
- lightweight non-regression scenarios remain 25/25 in both on and off runs;
- the observed off failures match the deterministic trigger exactly.

M002 should be admitted for the explicit requested artifact missing class. The
scope remains intentionally narrow. It does not address line-count misses,
wrong-path writes after a tool call, or semantic insufficiency after a file is
created.

## Long-Session Read/Edit Semantic Recheck

Date: 2026-06-14 JST

The original `long-session-read-edit` check required `src/summary.ts` to have at
least 30 lines while the task requested a "concise summary". This produced a
format-biased failure mode: correct exported functions were marked failed only
because the implementation was concise.

The check was changed from `min_lines: 30` to a semantic Node check:

- `docs/notes/architecture.md` still must have at least 80 lines;
- `src/summary.ts` must contain `summarizeArchitectureNotes`;
- the file is evaluated after stripping simple TypeScript/export syntax;
- `summarizeArchitectureNotes()` must be callable and return a string;
- the returned summary must have meaningful length and include several
  architecture concepts.

GPU-free recheck results:

| root | original | semantic recheck |
|---|---:|---:|
| M002 on `.anvil/benchmarks/20260614T150945-19712` | 2/5 | 5/5 |
| M002 off `.anvil/benchmarks/20260614T164741-32099` | 2/5 | 5/5 |
| vibe-local `20260613T163056-25350-vibe-local` | 3/5 | 5/5 |

Interpretation: this scenario should not drive a new mechanism. The remaining
gap was a benchmark false negative, not a minimal-loop control-flow failure.

## Wrong-Path Write Validation

Date: 2026-06-14 JST

The remaining `long-session-data-report` M002-on failure wrote
`reports/sales-analysis.md` to an absolute-path-like nested location:

```text
Users/maenokota/share/work/.../workdir/reports/sales-analysis.md
```

inside the workdir instead of repository-relative `reports/sales-analysis.md`.
M002 correctly noticed that the requested relative path was still missing, but
one feedback turn did not recover the misplaced write.

This is a deterministic tool affordance issue rather than an admission
candidate. The tool layer now rejects Write/Edit paths that look like the
project root absolute path with the leading slash removed, and returns an error
that tells the model to use a repository-relative path. Normal relative paths
and valid absolute paths under the work root remain accepted.

Validation:

- `cargo test --lib tools::registry`
- `tests/scripts/test_bench_smoke.sh`

## Long-Session Data Report Narrow Rerun

Date: 2026-06-14 JST

After adding the wrong-path Write validation, `long-session-data-report` was
rerun with M002 on and off at 10 runs each.

Roots:

| variant | root |
|---|---|
| M002 on | `.anvil/benchmarks/20260614T173049-31829` |
| M002 off | `.anvil/benchmarks/20260614T174019-59291` |

Build metadata:

```text
git_revision: fdf31ba663117dbf9c5e3ac83f587cda6bd36d75-dirty
binary_path: /Users/maenokota/share/work/github_kewton/Anvil-develop/target/release/anvil
build_time: 2026-06-14T08:30:38Z
```

Result:

| variant | success | rc0 | mean elapsed |
|---|---:|---:|---:|
| M002 on | 6/10 | 10/10 | 55.7s |
| M002 off | 0/10 | 10/10 | 38.9s |

M002 feedback fired in six on-runs. Two of those recovered and four did not:
the model acknowledged that `reports/sales-analysis.md` was still missing and
said it would create it, but returned a second no-tool response. The mechanism
correctly accepts that second no-tool response to avoid an unbounded loop.

The off-run failures were all missing `reports/sales-analysis.md`; every run
left only `data/sample-sales.csv` in the workdir. One off-run hit the new
wrong-path validation, received the repository-relative path hint, and still
ended with a no-tool "Let me create the report file now" response. That confirms
the validation is useful but not a substitute for M002.

Interpretation: M002's target effect is reproduced more strongly at n=10. The
remaining failure class is not missing-path detection; it is a second no-tool
response after the one-shot feedback. Do not expand M002 automatically. If this
class remains important, it should be considered as a separate admission
candidate with its own trigger and ablation.
