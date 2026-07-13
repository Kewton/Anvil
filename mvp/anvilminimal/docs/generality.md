# Generality Declaration

M6 status: complete on 2026-07-04.

## Scope S

Anvilminimal is generalized within scope S when the same profile, evidence, and
terminal-state mechanisms handle the covered task families without depending on
a single prompt, corpus case, or Next.js game shape.

Scope S is explicit:

| dimension | in scope |
|---|---|
| Profiles | `nextjs`, `python-cli`, and `generic`, including the no-profile start path, static/reduced markers, and known-manifest promotion |
| Scenarios | GAME, TOOL, CONTENT, CLI, and AMBIGUOUS from [uat/scenarios.md](uat/scenarios.md) |
| Models | UAT evidence from `qwen3.6:27b-coding-nvfp4` main execution with the configured planner model used by the runbook |
| Languages | Japanese scenario prompts, TypeScript/TSX Next.js output, Python CLI output, and English/Japanese diagnostic text |
| OS | macOS/Darwin UAT hosts only |

Within S, "generalized" means:

- Contract inference maps task intent to capabilities and evidence without
  requiring scenario IDs or one prompt string.
- Runtime acceptance records full success only when the required profile,
  evidence, and release gate pass.
- Generic app-intent goals bind a minimal static contract
  (`user_input_handler_evidence`, `stateful_update_evidence`,
  `visible_interactive_surface_evidence`) and render static-assurance markers
  only when that contract is verified from source evidence. Generic goals
  without app intent keep the reduced-assurance empty-contract path.
- No-profile app-intent runs start as `generic`, bind the generic static
  contract, then may promote to a known profile after a recognized workspace
  manifest appears. Assurance has three honest tiers: reduced for empty or
  unsupported generic contracts, static for verified generic source evidence,
  and full only after a known promoted profile passes its profile and
  behavioral release gates.
- Profile promotion is intentionally table-limited. The current promotion table
  covers known manifests only, such as Next.js package manifests and
  `python-cli` `pyproject.toml` workspaces. Unknown stacks terminate at the
  generic static tier honestly; that is a correct terminal state, not a hidden
  failure.
- Missing probe evidence, unsupported profile confidence, incomplete behavior
  evidence, or generic goals outside the minimal static contract render
  reduced-assurance markers instead of full success.
- Generic static evidence is source-only. For files outside the comment
  stripper's supported extension set, keyword-tier evidence is accepted as
  `weak_accepted_generic`; co-signal absence on those unsupported languages is
  not a hard failure. This is a stated limit, not behavioral verification.
- Generic contract binding emits `generic_contract_bound` with the inferred
  static evidence keys and the matched application-intent token.
- Every observed UAT anomaly is either explained as out of scope or harvested
  into the corpus before probe, evidence, or profile logic changes.
- The scenario suite is rerun for any probe, evidence, or profile change.
- Any new `DomainProfile` or execution pathway must add a row to
  `tests/conformance/` and pass `cargo test --test conformance`. The
  conformance matrix is the definition of done for shared runner/profile
  interface contracts; new rows reuse the named assertions instead of adding
  pathway-specific copies.

Named guarantees produced by the Generic Assurance Track:

- **Monotonic promotion rebind**: when a generic app-intent run promotes to a
  known profile, the promoted contract is a union. Generic interactive
  requirements remain bound and known-profile requirements are added;
  requirements never decrease during promotion.
- **Earned assurance**: `full` assurance is computed from executed gate
  statuses. A promoted interactive web run cannot earn full assurance from
  disconnected or `not_applicable` browser readiness / interaction gates; those
  gaps fail loudly as `acceptance_gates_disconnected`.
- **Authority symmetry**: dependency needs created by runtime manifests,
  repairs, or promotion are paired with runtime-sanctioned install authority.
  Without that authority, the terminal state is an explicit
  `dependency_setup_authority_required` failure, not a repair loop or full
  success.

## Scenario matrix (nextjs)

The verified `nextjs` profile scenario matrix is Space Invaders / Breakout / Quiz. Quiz is the non-game row: it contains no game vocabulary while still exercising a browser-interactive Next.js app with start, answer/input, score/progress state, and retry behavior. Any change that injects or broadens evidence vocabulary, contract wording, preset planning knowledge, or similar profile-specific guidance must be admitted only after regression measurement across this three-scenario matrix. The current basis is `test0710_bs_006`, where Breakout exposed vocabulary-dependent arbitration, and `test0711_bs_003`, where the first Quiz round reached 2/2 full.

`challenge_or_adversary` is the general category for opposing elements, time pressure, or other sources of difficulty, while `failure_or_collision` is the general category for failure conditions; neither category is intrinsically game-specific. Contract inference scopes whether each category is required from the goal, and scenario-specific repair guidance must follow that inferred contract rather than the category name alone.

## Measured capability bands (nextjs x create, local tier)

The Phase A local-tier band measurement covers the post-hardening window from
`uat-test0711-bs-003` through `uat-test0713-g-001`, including
`uat-test0713-28-001`. The target tier is planner
`qwen3.6:27b-coding-nvfp4` with executors
`qwen3.6:35b-a3b-coding-nvfp4` and `gemma4:31b-cloud`; the all-window
denominator is 78 Next.js create records. Of those records, 74 used the target
qwen27 planner and four Space control rows used `gemma4:31b-cloud` as planner;
the controls are retained because the band rule counts every run in the
measurement window rather than selecting favorable rows.

| scenario family | full rate | denominator | measured characteristic |
|---|---:|---:|---|
| Quiz | 88% | 23/26 | Most stable row. It is the non-game scenario and validates that browser-interactive contracts are not tied to game vocabulary. |
| Breakout | 29% | 5/17 | Middle band. Full success recurs, but the honest non-full frontier remains high across partial, incomplete, and failed terminal states. |
| Space | 9% | 3/35 | Hardest row. Restart/recoverable-state evidence is the local-model capability wall accepted on 2026-07-13; cloud follow-up is recommended. Full is not zero: `gemma4:31b-cloud` produced 2/8 Space full runs, while qwen35 produced 1/27. |

Full-run elapsed time across the measured window was 5m02s minimum, 7m02s
median, and 12m53s maximum. Scenario medians were Space 6m25s, Breakout 7m49s,
and Quiz 7m45s. The false-full count across the window is zero: every
`full_success` row had browser-interaction pass evidence.

These bands are measured facts for this model tier, host class, and
Space/Breakout/Quiz scenario matrix. Future model or mechanism changes must
rerun the same aggregation rather than editing the bands by hand. The source
summary is generated by
[`band_aggregate.py`](../../../workspace/management/scripts/band_aggregate.py)
and recorded at
[`band_summary.md`](../../../workspace/management/runs/band_summary.md).

## Recommended Model Tier

Production-quality implementation outcomes require an implementation model in
the `gemini-3.5-flash` class or above. Lower-tier models remain safe: the
honest-degradation guarantees still require partial, reduced, failed, or
handoff terminal states instead of false full success. They do not, however,
produce the same full-pass rate.

Measured evidence:

- M5 harvest runs used `gemini-3.1-flash-lite` for implementation with
  `gemini-3.5-flash` planning. The harvested distribution was 0 full / 2
  partial / 2 failed / 1 not-checked early harvest: `test0702_008` was an
  early/not-checked harvest, `test0703_002` failed on missing completion
  evidence, `test0703_005_4` was partial because the probe was unavailable,
  `test0704_001` was partial on interaction state-change evidence, and
  `test0704_003` failed on start-transition behavior.
- Q1-final used `gemini-3.5-flash` as the implementation and planner model on
  build `0604a76b` and produced 8/8 honest terminal states: 6 full, 1 reasoned
  partial, and 1 behavioral failed.
- Final local-model compatibility round
  `test0704-999-Q1-62646566676869707172_001` used
  `qwen3.6:27b-coding-nvfp4` as the local planner and `ornith:35b` as the
  local executor. It produced 8/8 honest terminal states and 2/8 full passes:
  CLI 1/2, web 1/6. The web full pass was CONTENT b, including browser
  readiness, interaction evidence, persistence, release-gate pass, and full
  earned assurance. The other 6/8 runs failed with concrete terminal reasons:
  CLI b failed the Python behavior smoke probe, TOOL a hit a dangerous-command
  policy boundary, TOOL b hit the `tsconfig` alias/baseUrl route-closure gap,
  CONTENT a failed dependency setup lifecycle, GAME a failed multi-file
  TypeScript coherence, and GAME b failed compile repair follow-through.
  Verdict: the 27b/35b local pair is below the recommended tier for reliable
  web delivery. CLI is moderately viable; web full-pass delivery is possible
  but low-probability on this distribution. The recommended implementation tier
  remains the `gemini-3.5-flash` class or above.
- The local-model compatibility track left permanent, model-agnostic assets:
  tool-argument path normalization and corrupted-prefix salvage; verifier
  command transforms for shell-control splitting, `cd`/cwd normalization, and
  output-pipe stripping; deterministic substitution after repeated verifier
  policy rejection; deterministic scaffold completion; per-provider timeout
  calibration; planner-call chokepoint instrumentation; and precise
  exhaustion classification. The honest scope limit is that dialect coverage
  was measured against this 27b/35b model pair. New model families may require a
  new dialect-absorption round; that is expected bounded cost, not a portability
  claim for unseen output dialects.
- Local single-model GAME quality track, instructions 81-87, culminated in
  `test0707_009` on build `a9fea8ee`, using
  `qwen3.6:27b-coding-nvfp4` for the single local model configuration. The
  six-run series moved the failure frontier from phase 0, to phase 1, to the
  final acceptance gate, then to a full pass with behavioral verification. The
  full-pass run recorded browser readiness `passed`, interaction evidence
  `passed`, state changes in `aliensRemaining` and `score`, a primary start
  transition, and an in-play recovery/restart transition. This is a distinct
  local data point from the 27b/35b mixed-pair verdict above: single 27b can
  complete the GAME path after the 81-87 guard sequence, but the observation is
  narrow to that track and does not revise the broader recommended tier.
  Residual `fits_viewport=false` / bottom overflow 136px is informational
  presentation quality guidance, not a release gate.

Model-tier observations:

New model-family entries follow the standard order in
[model-probe.md](model-probe.md): model-probe card review, CLI and TOOL smoke
checks, then the full scenario round with pre-committed landing criteria. The
tier table cites the probe profile as dialect evidence, not as a capability
benchmark or an automatic runtime configuration source. For local speed
measurements, the recommended zero-risk default is a single Ollama model used
for both planner and executor with the model kept resident by Ollama-side
keep-alive settings; cloud-hosted providers benefit from prefix stability only
when the server exposes compatible prompt caching.

| configuration | sample | distribution / frontier | verdict |
|---|---|---|---|
| `gemini-3.5-flash` implementation/planner | Q1-final `test0704-999-Q1-62_001`, 8 runs | 6 full / 1 reasoned partial / 1 behavioral failed, 8/8 honest terminal states | Recommended implementation tier baseline. |
| Local mixed pair: `qwen3.6:27b-coding-nvfp4` planner + `ornith:35b` executor | `test0704-999-Q1-62646566676869707172_001`, 8 runs | 2/8 full, 8/8 honest terminal states; web 1/6 | Below recommended tier for reliable web delivery; CLI moderately viable. |
| Local single model: `qwen3.6:27b-coding-nvfp4` | GAME quality track instructions 81-87, six-run series ending at `test0707_009` | frontier advanced phase0 -> phase1 -> final gate -> full pass with behavioral verification, in-play restart/recovery, score and enemy-state mutation | Golden local single-model GAME reference; narrow positive data point, not a broad tier upgrade. |
| Local single model: `gemma4:31b-local` | `test0708_007` GAME run | Reached 4/5 phases with a build-passing, playable-looking app, then exposed restart-hook coverage and pending-evidence exhaustion honesty gaps. | Useful local stress datum for observability coverage; not recommended for reliable web delivery until restart/input evidence follow-through recurs as full pass. |
| Local MoE single model: `qwen3.6:35b-a3b` | `test0707_010` / `test0708_001` | Empty planner responses, missing descriptive StepPlan fields, manifest drift, and dual dependency/compile blockers exercised planner and deterministic-repair ladders. | Dialect-heavy model: viable only with the empty-response ladder, schema defaulting, manifest reconciliation, and compile repair ordering guards engaged. |
| Cloud single model: `gemma4:31b-cloud` | `test0708_009` GAME run | Cross-file hook contract mismatch reached compile repair, but compact zero-edit repair exhausted without regeneration; no last-known-good page snapshot existed for rollback. | Model-family datum for zero-edit repair behavior; motivates regeneration rung and does not change tier recommendation. |
| Cloud single model: `gemma4:31b-cloud` post-97 round | `test0708_016` / `test0708_017` GAME runs | Runtime Bash shell-control at the Bash-tool boundary killed one run before normalization; a later run terminated honestly with pending restart/input capability evidence. | Model-family datum for boundary dialect and honest exhaustion behavior; no tier upgrade. |
| Cloud single model: `gemma4:31b-cloud` Q1 CLI | `cli_b` | First full pass in the CLI domain for this model family, with the generated command-line workflow satisfying the process-profile gates. | Positive domain-specific datum; keep separate from GAME/web reliability because dialect and repair failure modes still recur in app runs. |
| Cloud single model: `gemma4:31b-cloud` post-101 Q1 round | 8-run Q1 round, post-101 | 3/8 full, 8/8 honest terminal states; `tool_b` repeated `model_stagnation:read_only_loop`, while `game_a` exposed camelCase collision evidence and `content_b` exposed invalid semver feedback. | Mixed datum: honesty held, but app reliability remains below recommended web tier; install-substitution, semver remedy, and case-insensitive evidence guards are corpus-backed. |
| Cloud single model: `gemma4:31b-cloud` final multi-model round | final gemma4-cloud round, 2026-07-08, fixture-verified | CLI capable; TOOL unstable; web not recommended. Residuals were multi-grep verify normalization and python-cli scaffold fallback, not live re-round failures. Cloud-hosted models have no identity pinning, so this verdict is valid only with the campaign date and probe card. | Track-closing verdict: use for CLI experiments with review, not as a recommended TOOL/web implementation tier. |

## Contract-Design Principle

Observability contracts may require only invisible instrumentation: data
attributes, state snapshots, and other non-visible probe hooks. Demands on
visible design, including control placement, reachability, or UX layout, are
preferences offered as tradeoffs. When declined or absent, the runner degrades
honestly to unverified or partial evidence instead of forcing design choices.

Two recent applications make this boundary permanent. Instruction 82A' treats
restart reachability as evidence honesty: an in-play restart/recovery path can
earn behavioral verification, while an overlay-only or unreachable restart is
reported as unverified/partial rather than rejected as a required visible
layout. Instruction 86D' treats styling as presence-conditional coherence:
Tailwind artifacts must be coherent when present, but plain CSS with no
Tailwind artifacts is a valid Next.js path and must not be overwritten by
forced stack injection.

## Q1 Final Quality Baseline

Q1 final is the standing quality baseline as of 2026-07-05. The judgment rule
has two parts:

- **Mandatory stranding elimination**: every sampled run must terminate
  honestly with a closed terminal state, concrete status/reason fields, and a
  recovery handoff when the run is not full. Max-iteration, human-interrupt,
  absent terminal status, and false-full exits disqualify the round. A run is
  stranded when its primary termination reason is an iteration or budget label
  instead of the concrete blocker, such as missing artifacts, policy rejection,
  compile failure, probe infrastructure failure, or failed behavioral evidence.
- **Distribution over samples**: after the mandatory condition passes, quality
  is judged by the distribution over at least two samples per scenario.

Q1-final matrix (`test0704-999-Q1-62_001`, implementation/planner model
`gemini-3.5-flash`, build `0604a76b`):

| scenario/run | run id | terminal status | primary reason | key telemetry |
|---|---|---|---|---|
| CLI a | `019f318c-9931-7350-a720-c3840f640968` | full | none | `python-cli`, `contract_origin=initial`, external contract OK, browser/interaction not applicable for non-web, assurance full |
| CLI b | `019f3193-227e-7251-ba45-d32d61762737` | full | none | `python-cli`, `contract_origin=initial`, external contract OK, browser/interaction not applicable for non-web, assurance full |
| TOOL a | `019f319b-aef6-7961-8ff6-15a4ad7a253e` | reasoned partial | `interaction_unverified:not_evaluated:no_mutation_observed` | browser readiness performed/passed, interaction performed/passed, state dimension `todos`, `persistence_after_reload_reason=no_mutation_observed`, recovery prompt/YAML recorded |
| TOOL b | `019f31a1-ff5f-7040-866d-3e405fc43e39` | full | none | browser readiness performed/passed, interaction performed/passed, state dimension `todos`, release gate pass, assurance full |
| CONTENT a | `019f31a9-7700-7c12-b947-9597eca40d16` | full | none | browser readiness performed/passed, interaction performed/passed, state dimension `currentContent`, token echoed, assurance full |
| CONTENT b | `019f31af-8988-7042-bea6-19f7aae0fccf` | full | none | browser readiness performed/passed, interaction performed/passed, state dimension `contentLength`, token echoed, assurance full |
| GAME a | `019f31b4-632c-7413-b322-34f0f8db8e91` | full | none | browser readiness performed/passed, interaction performed/passed, state dimension `bulletsCount`, primary/restart hooks, assurance full |
| GAME b | `019f31c0-14f5-7c53-898f-4612c883da3f` | behavioral failed | `missing_required_evidence:interactive_ui_source_evidence`; `browser_interaction_failed:probe_script_error` | browser readiness HTTP 200, interaction performed_failed at `surface_wait`, recovery prompt/YAML recorded; served-DOM inspection showed visible canvas and primary action after a hidden first `data-anvil-state`, so this is a harvested probe-calibration limit, not a false full |

Result: Q1-final concludes the quality track for the current scoped host/model
pair with 8/8 honest termination and a 6 full / 1 reasoned partial / 1
behavioral failed distribution.

## Clause Evidence

| clause | run evidence | harvested corpus |
|---|---|---|
| Web profile behavior is not Space-Invaders-only; generic interaction, persistence, and content-editing obligations are separately checked. | M2: `test0704-464748_001`, `test0704-464748_002`, and `test0704-48.1 CONTENT re-run` | Not harvested in this source corpus; the regression target is the scenario suite in [uat/scenarios.md](uat/scenarios.md). |
| Contract inference survives renamed or opaque scenario IDs and English/Japanese prompt variation. | M3: `test0704-49_001`, `test0704-50_001` | Covered by required golden tests `tests/eval/test_acceptance_contract.py::AcceptanceContractTest` and `tests/eval/test_completion_contract_snapshots.py::CompletionContractSnapshotTest`. |
| No-profile app-intent runs use generic contract binding and terminate honestly at the generic static tier when the scaffolded manifest is unknown. | Static-tier fallback, live-proven: `test0704-4030444542434647484814950515354_001` scaffolded a Vite/React manifest, emitted no `profile_reinferred`, and ended without pretending to run promoted web gates; report: `../../../workspace/management/runs/uat-test0704-4030444542434647484814950515354-001/uat-report.md`. | [test0704-4030444542434647484814950515354_001](../tests/corpus/apps/test0704-4030444542434647484814950515354_001/expectations.toml) |
| No-profile app-intent runs promote only when a known manifest appears, preserving generic obligations and earning full only through executed web gates. | Promotion path, live-proven: final G3 revalidation `test0704-403044454243464748481495051535455565758_000`, run id `019f30c7-da99-7d83-a715-1db6b6a6a3b6`, recorded `profile_reinferred`, `contract_origin=promoted_union`, dependency reconciliation, browser readiness HTTP 200, interaction probe execution, and `assurance_level=full`; report: `../../../workspace/management/runs/uat-test0704-403044454243464748481495051535455565758-000/uat-report.md`. | [test0704-403044454243464748481495051535455565758_000](../tests/corpus/apps/test0704-403044454243464748481495051535455565758_000/expectations.toml) |
| The runner lifecycle supports a non-web process profile without Next.js probe or port assumptions. | M4: `test0704-51_001` web run and `test0704-51_001` CLI run | The CLI contract is specified in [uat/scenarios.md#cli-python-cli-profile](uat/scenarios.md#cli-python-cli-profile); no app corpus case is expected for the process-only run. |
| App evidence detectors and interaction probe selection are fixture-backed. | M5 Round A four runs | [test0702_008](../tests/corpus/apps/test0702_008/expectations.toml), [test0703_002](../tests/corpus/apps/test0703_002/expectations.toml), [test0703_005_4](../tests/corpus/apps/test0703_005_4/expectations.toml), [test0704_001](../tests/corpus/apps/test0704_001/expectations.toml) |
| The second corpus round confirms the same detectors over a new harvested app snapshot. | M5 Round B | [test0704_003](../tests/corpus/apps/test0704_003/expectations.toml) |
| Q1-final residuals are corpus-backed: persistence `not_evaluated` must render a reason, and the GAME b HTTP-200 hidden-first surface failure is recorded as a probe calibration limit. | Q1-final: `test0704-999-Q1-62_001` TOOL a and GAME b | [q1-final-tool-a-persistence-not-evaluated](../tests/corpus/apps/q1-final-tool-a-persistence-not-evaluated/expectations.toml), [q1-final-game-b-rendered-hidden-probe-limit](../tests/corpus/apps/q1-final-game-b-rendered-hidden-probe-limit/expectations.toml) |
| Local-tier residuals and the first local web full pass are corpus-backed: earlier GAME b records artifact stagnation, earlier TOOL b records output-pipe verifier normalization, final CONTENT b records the complete local web behavioral-gate path, final TOOL b records the `tsconfig` alias/baseUrl gap, and final GAME a records multi-file TypeScript coherence failure. | Local rounds: `test0704-999-Q1-6264656667686970_001` GAME b / TOOL b, and `test0704-999-Q1-62646566676869707172_001` CONTENT b / TOOL b / GAME a | [local-q1-game-b-artifact-stagnation](../tests/corpus/apps/local-q1-game-b-artifact-stagnation/expectations.toml), [local-q1-tool-b-output-pipe-verify](../tests/corpus/apps/local-q1-tool-b-output-pipe-verify/expectations.toml), [local-q1-final-content-b-web-full-pass](../tests/corpus/apps/local-q1-final-content-b-web-full-pass/expectations.toml), [local-q1-final-tool-b-tsconfig-alias-gap](../tests/corpus/apps/local-q1-final-tool-b-tsconfig-alias-gap/expectations.toml), [local-q1-final-game-a-typescript-coherence](../tests/corpus/apps/local-q1-final-game-a-typescript-coherence/expectations.toml) |
| Local single-model GAME full pass is corpus-backed, including behavioral start, input state mutation, in-play recovery/restart, and informational `fits_viewport` overflow. | `test0707_009`, single-model `qwen3.6:27b-coding-nvfp4`, run id `019f3c60-f7d3-7f21-8f8c-6b3b0626370a` | [local-single-qwen36-game-full-pass](../tests/corpus/apps/local-single-qwen36-game-full-pass/expectations.toml) |
| Post-97 residuals are corpus-backed: root-anchor path salvage miss, Bash-tool shell-control normalization, and gemma4-cloud honest capability exhaustion. | `test0708_013`, `test0708_016`, `test0708_017` | [test0708_013](../tests/corpus/apps/test0708_013/expectations.toml), [test0708_016](../tests/corpus/apps/test0708_016/expectations.toml), [test0708_017](../tests/corpus/apps/test0708_017/expectations.toml) |
| Q1 boundedness residuals are corpus-backed: repeated broad Bash timeout loops and same-run dev-server port conflicts. | Q1-full `content_b`, Q1-full `tool_a` | [q1-full-content-b-timeout-loop](../tests/corpus/apps/q1-full-content-b-timeout-loop/expectations.toml), [q1-full-tool-a-orphan-port-in-use](../tests/corpus/apps/q1-full-tool-a-orphan-port-in-use/expectations.toml) |
| Post-101 residuals are corpus-backed: camelCase/PascalCase gameplay evidence no longer false-negatives, and invalid semver feedback names the manifest entry and corrected example. | Q1 post-101 `game_a`, `content_b` | [q1-post101-game-a-camelcase-collision](../tests/corpus/apps/q1-post101-game-a-camelcase-collision/expectations.toml), [q1-post101-content-b-invalid-semver](../tests/corpus/apps/q1-post101-content-b-invalid-semver/expectations.toml) |
| Final multi-model residuals are corpus-backed: multi-grep verify shell-control normalization and python-cli setup fallback substitution. | Final gemma4-cloud fixture round | [q1-final-multi-grep-shell-control](../tests/corpus/apps/q1-final-multi-grep-shell-control/expectations.toml), [q1-final-cli-scaffold-fallback](../tests/corpus/apps/q1-final-cli-scaffold-fallback/expectations.toml) |
| Guardrails are permanent and cheap enough to run in CI. | M6 docs and tests | `cargo test --test corpus_regression`, `cargo test --test generality_guardrails`, `cargo test --test conformance`, plus the scenario contract golden tests named below. |

## Required Gates

These gates are required for M6 branch protection or equivalent release
approval:

- `cargo test --test corpus_regression`
- `cargo test --test generality_guardrails`
- `cargo test --test conformance`
- `cargo test generic_ultra_promotes_to_nextjs_after_workspace_manifest --lib`
- `cargo test generic_ultra_promotes_to_python_cli_after_pyproject_manifest --lib`
- `cargo test generic_ultra_without_manifest_keeps_static_tier --lib`
- `python3 -m unittest tests/eval/test_acceptance_contract.py`
- `python3 -m unittest tests/eval/test_completion_contract_snapshots.py`
- `python3 -m unittest tests/eval/test_false_positive_regression.py`

The corpus harness guards harvested probe/evidence/profile behavior. The
conformance matrix guards shared interface contracts for profiles and
execution pathways. The scenario contract tests are the golden suite for
contract inference. The false-positive regression protects the "static screen
is not a game success" boundary. The generality guardrails protect static- and
reduced-assurance rendering and the Next.js boundary-erosion tripwire.

## Roadmap Completion

| milestone | completion date | evidence |
|---|---|---|
| M0 Baseline scope and runbook | 2026-07-04 | Scope S and UAT runbook fixed in this declaration and [uat/scenarios.md](uat/scenarios.md). |
| M1 Acceptance clauses | 2026-07-04 | Scenario final-acceptance clauses in [uat/scenarios.md](uat/scenarios.md). |
| M2 Web scenario variation | 2026-07-04 | `test0704-464748_001`, `test0704-464748_002`, `test0704-48.1 CONTENT re-run`. |
| M3 Contract inference | 2026-07-04 | `test0704-49_001`, `test0704-50_001` plus required golden tests. |
| M4 Non-web process profile | 2026-07-04 | `test0704-51_001` web and CLI runs. |
| M5 Corpus harvest | 2026-07-04 | Round A four corpus cases plus Round B corpus case. |
| M6 Declaration and guardrails | 2026-07-04 | This document, cross-references, and `generality_guardrails` tests. |

## Generic Assurance Track Completion

Status: complete on 2026-07-05.

| milestone | completion date | evidence |
|---|---|---|
| G0 Scope | 2026-07-04 | Generic assurance scope, limits, and named guarantees recorded in Scope S. |
| G1 Generic contract binding | 2026-07-04 | `generic_contract_bound` event and generic static contract tests. |
| G2 Known-manifest promotion | 2026-07-04 | `generic_ultra_promotes_to_nextjs_after_workspace_manifest`, `generic_ultra_promotes_to_python_cli_after_pyproject_manifest`, and `generic_ultra_without_manifest_keeps_static_tier`. |
| G3 Ambiguous UAT evidence | 2026-07-05 | Two live evidence entries: static-tier fallback `test0704-4030444542434647484814950515354_001` and final promoted full-assurance run `test0704-403044454243464748481495051535455565758_000`. |
| G4 Codification | 2026-07-05 | Default Next.js no-port policy, AMBIGUOUS scenario runbook hardening, G3 corpus harvest, and [mechanism-ledger.md#generic-assurance-track-cross-reference](mechanism-ledger.md#generic-assurance-track-cross-reference). |

## Quality Track Completion

Status: Q1 concluded on 2026-07-05.

| milestone | completion date | evidence |
|---|---|---|
| Q1 model-tier baseline | 2026-07-05 | Recommended model tier and M5/Q1-final distributions recorded in [Recommended Model Tier](#recommended-model-tier). |
| Q1 final round | 2026-07-05 | `test0704-999-Q1-62_001`: 8 runs, 8/8 honest termination, 6 full / 1 reasoned partial / 1 behavioral failed on `gemini-3.5-flash`. |
| Q1 residual corpus harvest | 2026-07-05 | TOOL a persistence reason and GAME b rendered-hidden probe-limit fixtures in [Clause Evidence](#clause-evidence). |
| Q1 boundedness closure | 2026-07-05 | Provider-turn and verify-command wall-clock invariants recorded in [mechanism-ledger.md#boundedness-guarantees](mechanism-ledger.md#boundedness-guarantees). |
| Local single-model GAME closure | 2026-07-07 | Instructions 81-87 concluded with `test0707_009`, a full behavioral GAME pass on single-model `qwen3.6:27b-coding-nvfp4`; corpus fixture [local-single-qwen36-game-full-pass](../tests/corpus/apps/local-single-qwen36-game-full-pass/expectations.toml). |
| Multi-model generalization track | 2026-07-08 | Instructions 89-102 closed the family-adoption loop: cost curve 9 -> 7 -> ~2 -> ~2 instructions as fixes shifted from family dialects to model-agnostic assets; standing protocol is probe -> smoke x2 (CLI + TOOL) -> full round with pre-committed landing criteria -> tier entry citing the probe card; boundedness is five-dimensional: transport, spawn, planner, step wall clock, and server reaping. |

Optional backlog after Q1:

| optional track | trigger |
|---|---|
| `tsconfig` paths alias deterministic invariant repair | Start on recurrence of a route-bound `@/*` import gap or `tsconfig baseUrl/paths missing @/* alias` terminal reason, using [local-q1-final-tool-b-tsconfig-alias-gap](../tests/corpus/apps/local-q1-final-tool-b-tsconfig-alias-gap/expectations.toml) as the fixture. |
| Dangerous-command rejection feedback categories | Start after recurrence analysis shows repeated local-model failures at the same policy category instead of isolated blocked-command attempts. |
| CONTENT a dependency lifecycle variance | Start on recurrence of dependency setup lifecycle failure; first check offline/network/package-registry state before treating it as an Anvil behavior defect. |
| `fits_viewport` responsive guidance | Start on recurrence across scenarios. The `test0707_009` GAME pass records bottom canvas overflow as informational presentation quality, not a gate. Repeated overflow should become responsive guidance and visual QA, not a hidden release blocker. |
| UX track resumption | Start when instruction 80 is resumed with a visual acceptance protocol. Keep it separate from invisible observability contracts so visible design preferences remain reviewable trades. |
| Data-profile/workflow track | Start when a non-web workflow needs first-class contracts beyond `python-cli`, including fixture/data lifecycle and acceptance probes. |
| T2 model-tier expansion | Start only after a probe card plus smoke/full-round evidence defines the target tier criteria in advance. |
| Second web profile / web-family layer | Start when a non-Next.js web stack needs first-class promotion, shared web obligations, or release-gate parity. Until then, unknown manifests remain generic/static by design. |
| Linux run | Start before claiming Linux host parity, adding Linux-specific release support, or treating Darwin UAT results as portable. |
| English-goal run | Start before claiming English prompt-distribution parity or using English-goal quality as release/marketing evidence. |

## Out Of Scope

The declaration does not claim:

- A second web framework. The web-family shared layer has not yet been
  extracted from the Next.js profile boundary.
- Promotion for arbitrary web frameworks. Unknown manifests remain generic and
  must stop at the static tier unless a known profile boundary is added with
  tests and corpus evidence.
- Non-process domains outside web apps and CLI/process tools.
- Linux behavior. Current UAT evidence is macOS/Darwin-only.
- Output depth beyond the observed model pair. Depth and polish remain
  model-bound even when the contract, probe, and terminal-state mechanisms hold.
