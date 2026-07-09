# UAT App Corpus

The regression corpus lives in `tests/corpus/apps/<case-id>`. Each case is a
source-only snapshot of a generated app plus `expectations.toml`.

The corpus is the fixture side of the M6 generality declaration in
[generality.md](generality.md). It is mandatory regression coverage for probe,
evidence, or profile changes.

Harvest a UAT workspace with:

```sh
mvp/anvilminimal/scripts/snapshot-uat-corpus.sh \
  /Users/maenokota/share/work/localwork/commandagent_mvp/01/test0704_003 \
  test0704_003
```

The first ambiguous no-profile harvest from G3 is:

```sh
mvp/anvilminimal/scripts/snapshot-uat-corpus.sh \
  /Users/maenokota/share/work/localwork/commandagent_mvp/01/test0704-4030444542434647484814950515354_001 \
  test0704-4030444542434647484814950515354_001
```

That case intentionally records an unknown Vite stack as a generic static-tier
fallback. It should not be rewritten into a Next.js fixture unless a future
profile explicitly supports that stack.

The Q1-final quality-track harvests are:

```sh
mvp/anvilminimal/scripts/snapshot-uat-corpus.sh \
  /Users/maenokota/share/work/localwork/commandagent_mvp/01/test0704-999-Q1-62_001/tool_a \
  q1-final-tool-a-persistence-not-evaluated

mvp/anvilminimal/scripts/snapshot-uat-corpus.sh \
  /Users/maenokota/share/work/localwork/commandagent_mvp/01/test0704-999-Q1-62_001/game_b \
  q1-final-game-b-rendered-hidden-probe-limit
```

`q1-final-tool-a-persistence-not-evaluated` records a behavioral partial where
the interaction probe executed and observed Todo mutation, but
`persistence_after_reload=not_evaluated` carried the rendered reason
`no_mutation_observed`.

`q1-final-game-b-rendered-hidden-probe-limit` records an HTTP-200 page whose
hydrated served DOM had a visible canvas and primary action, but the probe's
surface wait selected a hidden leading `data-anvil-state` node before later
visible controls. This is a known probe calibration limit and must remain an
honest failed fixture until the probe selection is changed deliberately.

The local-tier Q1 round harvests are:

```sh
mvp/anvilminimal/scripts/snapshot-uat-corpus.sh \
  /Users/maenokota/share/work/localwork/commandagent_mvp/01/test0704-999-Q1-6264656667686970_001/game_b \
  local-q1-game-b-artifact-stagnation

mvp/anvilminimal/scripts/snapshot-uat-corpus.sh \
  /Users/maenokota/share/work/localwork/commandagent_mvp/01/test0704-999-Q1-6264656667686970_001/tool_b \
  local-q1-tool-b-output-pipe-verify
```

`local-q1-game-b-artifact-stagnation` records the missing-component recovery
shape where `src/components/GameBoard.tsx` and `src/hooks/useGame.ts` were
requested but artifact recovery exhausted before a complete route-bound game
surface existed.

`local-q1-tool-b-output-pipe-verify` records the Todo app source state from the
run whose runtime verifier attempted `npm run build 2>&1 | tail -80`; that
shape must be normalized to the base build command rather than rejected or
allowed to mask the build exit status.

The local single-model GAME golden harvest is:

```sh
mvp/anvilminimal/scripts/snapshot-uat-corpus.sh \
  /Users/maenokota/share/work/localwork/commandagent_mvp/01/test0707_009 \
  local-single-qwen36-game-full-pass
```

`local-single-qwen36-game-full-pass` records the instruction 81-87 closure run
for the single-model `qwen3.6:27b-coding-nvfp4` GAME track. It pins browser
readiness, interaction pass, start transition, in-play recovery/restart
transition, and score/enemy state mutation. Its `fits_viewport=false` browser
fixture is informational presentation-quality evidence, not a release gate.

`test0708_009` records the `gemma4:31b-cloud` zero-edit compile repair shape:
a route-bound page destructures `movePlayer`/`shoot` from an imported
`useGameEngine` hook whose returned API exposes different members, and compact
repair exhausted without source edits or a rollback snapshot. It is the corpus
fixture for the repair-as-regeneration rung.

The post-95/96 residual harvests are:

```sh
mvp/anvilminimal/scripts/snapshot-uat-corpus.sh \
  /Users/maenokota/share/work/localwork/commandagent_mvp/01/test0708_010 \
  test0708_010

mvp/anvilminimal/scripts/snapshot-uat-corpus.sh \
  /Users/maenokota/share/work/localwork/commandagent_mvp/01/test0708_011 \
  test0708_011

mvp/anvilminimal/scripts/snapshot-uat-corpus.sh \
  /Users/maenokota/share/work/localwork/commandagent_mvp/01/test0708_012 \
  test0708_012
```

`test0708_010` pins the PostCSS module-format incoherence: `postcss.config.js`
uses `export default` while `package.json` is not `type=module`, producing the
Next/PostCSS `plugins` export build error. `test0708_011` pins repeated
`edit_anchor_not_found` on a route-bound game implementation, motivating
best-match anchor feedback, unique normalized salvage, and full-file Write
escalation. `test0708_012` pins the gate-reached final-acceptance partial:
browser readiness is 200 and interaction probing runs, but final acceptance
exhaustion was not naming pending restart/input evidence honestly.

The post-97 five-run residual harvests are:

```sh
mvp/anvilminimal/scripts/snapshot-uat-corpus.sh \
  /Users/maenokota/share/work/localwork/commandagent_mvp/01/test0708_013 \
  test0708_013

mvp/anvilminimal/scripts/snapshot-uat-corpus.sh \
  /Users/maenokota/share/work/localwork/commandagent_mvp/01/test0708_016 \
  test0708_016

mvp/anvilminimal/scripts/snapshot-uat-corpus.sh \
  /Users/maenokota/share/work/localwork/commandagent_mvp/01/test0708_017 \
  test0708_017
```

`test0708_013` pins the root-anchor path-salvage miss for an absolute
`/Users/.../commandagent_mvp/01/test0708_013/package.json` tool path.
`test0708_016` pins the Bash-tool shell-control boundary shape
`ls -R src/app && node -p "..."`. `test0708_017` pins the
`gemma4:31b-cloud` honest exhaustion datum where restart/input capability
evidence remained pending and the terminal reason names those keys.

The a3b five-run blocker harvest records two live classes and the probe
correlation:

| Class | Fixture | Diagnosis | Probe card predicted class 2? |
| --- | --- | --- | --- |
| setup `model_stagnation:no_progress_recorded` | `test0708_018/fixtures/events-a3b-setup-no-progress.jsonl` | Expected paths were already present (`initially_missing_paths=[]`), but no deterministic step completion fired before the model loop exhausted. | No. `absolute_path_rate` / `corrupted_path_count` do not predict already-satisfied setup steps. |
| stale absolute-path confinement | `test0708_013/fixtures/events-path-salvage-miss.jsonl` and `events-stale-path-remediation.jsonl` | Historical rejected path: `/Users/example/share/work/commandagent_mvp/01/test0708_013/package.json`; only a `root_anchor` field was recorded, required-path fallback was not evaluated, and feedback was the generic `path_outside_workspace`. Remediation fixture records root-anchor fallback evaluated false, required-path fallback evaluated true for `/Users/example/share/work/old-run/package.json` -> `package.json`; rejected path `/Users/example/share/work/old-run/src/app/layout.tsx` reports the current workspace root and nearest expected relative path `src/app/page.tsx`, after which the model retries with `src/app/page.tsx`. | Yes. Elevated `absolute_path_rate` predicts this class; `corrupted_path_count` is adjacent but not required for stale absolute-path reproduction. |

The a3b re-measurement harvest is `test0709_a3b_remeasurement`. It records one
fixture per live class:

| Class | Fixture | Diagnosis | Probe card predicted class 2? |
| --- | --- | --- | --- |
| stale path injection | `test0709_a3b_remeasurement/fixtures/events-stale-path-injection.jsonl` | The first foreign literal was the Bash command argument `cat /Users/<user>/share/work/commandagent_mvp/01/test0709_bs_002/test0709_camp_003/package.json`; no earlier tool-output event introduced it, and guarded path fallback was not reached because this was not a Write/Edit path rejection. | Partially. The class is adjacent to high absolute-path-rate risk, but the harvested run had no pre-run probe card in events and the first occurrence was model-authored. |
| evidence repair follow-through | `test0709_a3b_remeasurement/fixtures/events-evidence-repair-followthrough.jsonl` | Final acceptance repair edited `src/components/SpaceInvaders.tsx`, re-probed, then exhausted on `restart_or_recoverable_state_evidence` without an evidence regeneration decision in the harvested run. | No. `absolute_path_rate` / `corrupted_path_count` do not predict evidence repair follow-through. |
| missing-tool-call ladder coverage | `test0709_a3b_remeasurement/fixtures/events-missing-tool-ladder.jsonl` | Read-only stagnation reached compact restatement; compile repair then compacted with no changes and recorded regeneration failure as `model_stagnation:no_progress_recorded`. Evidence-shaped `missing tool call` now follows the same no-source-change ladder. | No. The probe card is not a no-tool/repair-ladder predictor. |

The confirmed Q1-full diagnosis harvests are:

```sh
# Python CLI verify-boundary fixture, kept outside the Next.js app corpus.
tests/fixtures/q1_full/cli_a_verify_redirect_command.txt

mvp/anvilminimal/scripts/snapshot-uat-corpus.sh \
  /Users/maenokota/share/work/localwork/commandagent_mvp/01/q1-full/tool_a \
  q1-full-tool-a-read-only-stagnation

mvp/anvilminimal/scripts/snapshot-uat-corpus.sh \
  /Users/maenokota/share/work/localwork/commandagent_mvp/01/q1-full/game_b \
  q1-full-game-b-evidence-repair-inspect-only

mvp/anvilminimal/scripts/snapshot-uat-corpus.sh \
  /Users/maenokota/share/work/localwork/commandagent_mvp/01/q1-full/content_b \
  q1-full-content-b-timeout-loop

mvp/anvilminimal/scripts/snapshot-uat-corpus.sh \
  /Users/maenokota/share/work/localwork/commandagent_mvp/01/q1-full/tool_a \
  q1-full-tool-a-orphan-port-in-use
```

Post-101 harvests:

```sh
# Manual reduced fixtures from the post-101 Q1 round:
# - game_a: camelCase/PascalCase gameplay evidence false-negative shape.
# - content_b: invalid semver output and manifest entry remedy shape.
tests/corpus/apps/q1-post101-game-a-camelcase-collision
tests/corpus/apps/q1-post101-content-b-invalid-semver
```

Final multi-model harvests:

```sh
# Manual reduced fixtures from the final gemma4-cloud round:
# - TOOL/GAME: multi-grep verify command reached an unsplit shell-control
#   rejection at verifier lint/contract entry points.
# - CLI: python-cli setup StepPlan lint exhaustion did not substitute the
#   profile scaffold fallback.
tests/corpus/apps/q1-final-multi-grep-shell-control
tests/corpus/apps/q1-final-cli-scaffold-fallback
```

`cli_a` pins the model tendency to combine CSV fixture creation and program
verification in one multi-line verify command with a file-writing redirect; the
live unit fixture asserts this remains rejected with Write-tool remedy text.
`q1-full-tool-a-read-only-stagnation` pins the repeated Read/Grep implement-loop
shape and the K=3/K=5 intervention vocabulary.
`q1-full-game-b-evidence-repair-inspect-only` pins evidence-target repair turns
that inspect without editing until the compact/regeneration rung accepts a
source change.

Post-104 prompt-layout follow-up:

```sh
# Manual reduced fixture from qwen27b GAME test0708_018:
tests/corpus/apps/test0708_018
```

`test0708_018` pins the new qwen27b setup-phase behavior observed immediately
after the prompt-prefix reordering: artifact recovery saw
`no_tool_missing_artifacts` for `src/app/page.tsx` three times while the
project setup scaffold otherwise existed. The source snapshot intentionally
omits `src/app/page.tsx`; the event fixture records the deterministic scaffold
rescue authoring that page at exhaustion. The prompt fixtures pin both
`stable` and `legacy` layouts for the affected StepPlan and step-execution
session families so A/B runs can compare behavior against the same content
ordered differently.

The stableB workspace harvest for the A/B verdict lives in the same case
bundle; `docs/perf-notes.md` records the confirmed stable-layout regression,
the zero cache-benefit measurement, and the default flip back to `legacy`.

The boundedness follow-up harvests are diagnostic fixtures. `content_b` pins the
wide `ls -R && cat package.json` loop: each Bash attempt reached the 180s cap
because recursive listing walked the workspace/dependency tree, and the old
exact error identity was too strict to aggregate variants.
`q1-full-tool-a-orphan-port-in-use` pins the mid-run readiness conflict where
port 3011 was held by a registered Next dev-server child from the same run,
requiring deterministic reap, one retry, and owner-honest vocabulary instead of
empty exhaustion.

The script copies `src/**`, `package.json`, and common Next.js/TypeScript/
Tailwind/PostCSS config files. It intentionally does not copy `node_modules`,
`.next`, `.anvil`, lockfiles, logs, screenshots, or other generated artifacts.

After harvesting, edit `expectations.toml`:

- `required_paths`, `required_capabilities`, `required_evidence`, and
  `required_obligations` define the static acceptance contract for the case.
- `[route_closure]` pins files that must be included or excluded by the
  Next.js route-bound source closure.
- `[evidence]` pins detector tiers as `Strong`, `Weak`, or `Absent`.
- `[weak_evidence]` and `[diagnostics]` pin expected route-unbound or weak
  detector reasons.
- `[probe]` is optional. When `html_fixture` is present, the corpus test runs
  the static HTML version of the interaction probe hook/candidate selector.
- `[json_fields]` is optional. Use
  `fixtures/<file>.json:<field.path> = "<expected>"` to pin harvested runtime
  evidence fields that are not source evidence, such as
  `persistence_after_reload_reason` or a served-DOM calibration note. Keep
  these JSON files under `fixtures/`; do not copy `.anvil` wholesale.

Every UAT anomaly must add one case before changing detector, probe, evidence,
or profile logic, unless the anomaly is explicitly recorded as out of scope.
