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
