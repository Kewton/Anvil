# Mechanism Ledger

M6 generality status is declared in [generality.md](generality.md). This ledger
keeps runtime mechanism entries separate from that declaration; M6 introduced
documentation and test guardrails only, with no behavior changes.

## M6 Roadmap Cross-Reference

| milestone | status | date | reference |
|---|---|---|---|
| M0 | complete | 2026-07-04 | [generality.md#roadmap-completion](generality.md#roadmap-completion) |
| M1 | complete | 2026-07-04 | [generality.md#roadmap-completion](generality.md#roadmap-completion) |
| M2 | complete | 2026-07-04 | [generality.md#clause-evidence](generality.md#clause-evidence) |
| M3 | complete | 2026-07-04 | [generality.md#clause-evidence](generality.md#clause-evidence) |
| M4 | complete | 2026-07-04 | [generality.md#clause-evidence](generality.md#clause-evidence) |
| M5 | complete | 2026-07-04 | [generality.md#clause-evidence](generality.md#clause-evidence) |
| M6 | complete | 2026-07-04 | [generality.md](generality.md) |

## Generic Assurance Track Cross-Reference

Status: complete on 2026-07-05.

| milestone | status | date | reference |
|---|---|---|---|
| G0 | complete | 2026-07-04 | Generic assurance scope and named guarantees established in [generality.md#scope-s](generality.md#scope-s). |
| G1 | complete | 2026-07-04 | Generic contract binding guarded by `generic_contract_bound` and runner tests. |
| G2 | complete | 2026-07-04 | Known-manifest promotion covered by `generic_ultra_promotes_to_nextjs_after_workspace_manifest` and `generic_ultra_promotes_to_python_cli_after_pyproject_manifest`. |
| G3 | complete | 2026-07-05 | Ambiguous no-profile evidence includes static-tier fallback `test0704-4030444542434647484814950515354_001` and final promotion run `test0704-403044454243464748481495051535455565758_000`. |
| G4 | complete | 2026-07-05 | Default-port policy, AMBIGUOUS runbook hardening, final G3 corpus harvest, and this closure ledger. |

## Quality Track Cross-Reference

Status: Q1 concluded on 2026-07-05.

| milestone | status | date | reference |
|---|---|---|---|
| Q1 model-tier baseline | complete | 2026-07-05 | [generality.md#recommended-model-tier](generality.md#recommended-model-tier) |
| Q1 final round | complete | 2026-07-05 | [generality.md#q1-final-quality-baseline](generality.md#q1-final-quality-baseline) |
| Q1 residual corpus harvest | complete | 2026-07-05 | [generality.md#clause-evidence](generality.md#clause-evidence) |
| Q1 boundedness closure | complete | 2026-07-05 | [boundedness-guarantees](#boundedness-guarantees) |

## Local-Model Compatibility Track Cross-Reference

Status: complete on 2026-07-06.

| milestone | status | date | reference |
|---|---|---|---|
| Instructions 64-72 | complete | 2026-07-06 | Final local-tier verdict in [generality.md#recommended-model-tier](generality.md#recommended-model-tier); final distribution `test0704-999-Q1-62646566676869707172_001`. |
| Corpus closure | complete | 2026-07-06 | Local residual and golden fixtures in [generality.md#clause-evidence](generality.md#clause-evidence). |
| Runbook closure | complete | 2026-07-06 | Local-provider timeout and co-residency notes in [uat/scenarios.md#command-shape](uat/scenarios.md#command-shape). |

## Multi-Model Generalization Track Cross-Reference

Status: complete on 2026-07-08.

| milestone | status | date | reference |
|---|---|---|---|
| Instructions 89-102 | complete | 2026-07-08 | Model-family adoption moved from repeated dialect-specific fixes to permanent assets: model probe, boundary audits, deterministic reconciliation, repair ladders, boundedness, and corpus-backed tier vocabulary. |
| Tier vocabulary | complete | 2026-07-08 | `model_stagnation:read_only_loop` and repair follow-through classes record model limits for tier decisions; they are not system defects when the terminal state is honest and bounded. |

## Local Single-Model GAME Track Cross-Reference

Status: complete on 2026-07-07.

| milestone | status | date | reference |
|---|---|---|---|
| Instructions 81-87 | complete | 2026-07-07 | Single-model GAME verdict in [generality.md#recommended-model-tier](generality.md#recommended-model-tier). |
| Corpus closure | complete | 2026-07-07 | Golden local single-model GAME full-pass fixture in [local-single-qwen36-game-full-pass](../tests/corpus/apps/local-single-qwen36-game-full-pass/expectations.toml). |
| Contract-design closure | complete | 2026-07-07 | Invisible-only observability contract principle in [generality.md#contract-design-principle](generality.md#contract-design-principle). |

## Promotion-Path Incident Chain

| incident | committed evidence | fix | permanent guard |
|---|---|---|---|
| False-full promoted run: final acceptance reported `full_success` / `assurance_level=full` while browser readiness and interaction evidence were `not_applicable`. | `../../../workspace/management/runs/uat-test0704-40304445424346474848149505153545556-000/uat-report.md` | Instruction 57 introduced the earned-assurance invariant: full assurance is derived from executed gate statuses, and disconnected promoted-web gates fail loudly. | `acceptance_gates_disconnected` is added to release-gate reasons; `earned_assurance_for_completion` reduces assurance when gate telemetry and release-gate status disagree. |
| UTF-8 boundary panic during promoted evidence scanning: substring extraction sliced inside a Japanese character and aborted before final acceptance. | `../../../workspace/management/runs/uat-test0704-403044454243464748481495051535455-000/uat-report.md` | Instruction 56 routed truncation through char-boundary helpers, including `floor_char_boundary` / `truncate_at_char_boundary`, and wrapped TUI slash-command execution with a terminal panic guard. | Multibyte truncation tests cover the helper path; `panic_caught` events distinguish a Rust panic from user abort or model failure. |
| Dependency setup authority required after promotion: package state declared `autoprefixer`, but later verify/build lacked authority to reconcile missing installed dependencies. | `../../../workspace/management/runs/uat-test0704-4030444542434647484814950515354555657-000/uat-report.md` | Instruction 58 added run-level setup authority after promotion/manifest repair and boundary reconciliation for declared Node dependencies. | Later verify/contract steps inherit sanctioned setup authority when the run created the dependency need; absent authority still ends as `dependency_setup_authority_required`. |

## Local-Model Compatibility Incident Chain

| instruction | incident | fix | permanent guard |
|---|---|---|---|
| 64 | Local model tool calls emitted absolute workspace paths and corrupted path prefixes that were semantically workspace-relative but failed tool validation. | Tool-argument path normalization and corrupted-prefix salvage admit the mechanical dialect while preserving path confinement. | `tool_args_path_normalized` telemetry plus path-guard tests ensure repaired paths remain workspace-relative. |
| 65 | Local planner verifier commands used shell-control variants and cwd wrappers that obscured a deterministic base check. | Plan-time verifier normalization splits safe shell-control forms and rewrites absolute workspace `cd` wrappers to cwd-relative verifier commands. | Planner sanitizer tests require normalized verifier commands to pass the same deterministic policy as authored commands. |
| 66 | Scaffold/setup completion could strand after the model created only framework support files or omitted deterministic setup artifacts. | Deterministic scaffold completion materializes known setup members and reports remaining missing paths explicitly. | Setup-scaffold unit coverage and loop telemetry distinguish scaffold members from task artifacts. |
| 67 | Runtime verifier execution did not fully match planner normalization, and local models added `2>&1 | tail/head -N` output pipes that masked base-command exit status. | The runtime boundary applies the same verifier normalization and strips output-limiting head/tail pipes while preserving the base command. | Runtime tests assert parity with plan-time normalization and emit `output_pipe_stripped` / normalization telemetry. |
| 68 | Local provider turns and planner-call chokepoints were tuned for remote latency, causing honest local work to hit provider deadlines too early. | Per-provider defaults set local/Ollama provider turns to 600 seconds and remote API turns to 180 seconds, with explicit `--chat-timeout-secs` override. | Provider-call chokepoint telemetry records effective timeout source; conformance keeps bounded provider turns mandatory. |
| 69 | Direct CLI runs could exit before completion finalization, and release/final fields could ignore a failed Python behavior probe. | The direct CLI entry now finalizes before exit, and earned-assurance fields consume `python-cli` behavior-probe results. | Python-CLI conformance rejects failed behavior probes that leave `full_success`, release pass, or runtime pass fields. |
| 70 | Repeated verifier policy errors caused local-model phase death even when an expected-path deterministic substitute existed. | On the second identical verifier policy rejection within a step, the runner substitutes `test -f <expected_path>`, emits `verify_command_substituted`, and marks the oracle tier degraded. | Substitution tests assert continuation without hiding degradation; final acceptance gates remain unchanged. |
| 71 | Iteration exhaustion could be reported as a bare budget label instead of the concrete artifact blocker. | Exhaustion now classifies non-scaffold missing artifacts as `artifact_follow_through_exhausted` with missing paths and stagnation-feedback count. | Conformance rejects terminal `loop_stop` / `tui_command_stop` reasons that strand at `max_iterations` or bare budget labels. |
| 72 | Deep local web runs exposed two probe accounting gaps: `probe_script_error` was app-labeled, and observed state mutation could still leave persistence `not_evaluated:no_mutation_observed`. | Probe script errors use infrastructure taxonomy with setup remediation, and persistence evaluation reconciles observed state dimensions as the pre-reload mutation. | Interaction-probe tests cover infrastructure taxonomy, error propagation, remediation, and observed-mutation persistence evaluation. |

Reviewer-process lessons:

- Gate-status fields outrank labels. Reviews must quote the executed gate
  fields that earned a status; `full_success`, `pass`, or `full` labels alone
  are not evidence.
- Landing conditions must be pre-committed before final rounds. The expected
  terminal distribution, corpus harvest duty, and no-false-full criteria must
  be written down before calling a measurement round final.

## Local Single-Model GAME Incident Chain

Instructions 81-87 closed the local single-model GAME quality track. The
mechanisms below are permanent because each fix maps to an observed failure
mode rather than a single prompt string.

| instruction | incident | fix | permanent guard |
|---|---|---|---|
| 81 | Local GAME runs stalled on broad evidence failures whose repair prompts did not name the root-cause implementation target. | Root-cause-mapped repair guidance now anchors missing gameplay evidence to route-bound implementation files and concrete source obligations. | Repair prompts must name the missing capability/evidence, implementation target, and bounded change surface instead of asking for generic evidence. |
| 82A' | Restart evidence risked becoming a visible-design mandate rather than an honesty boundary. | Reachability-honest classification treats in-play restart/recovery as behavioral evidence, while unreachable or overlay-only restart degrades to unverified/partial. | Observability contracts may require invisible hooks only; visible restart placement remains a preference. |
| 83A | Compile repair for class/member mismatches lacked the receiver definition context needed for local models to repair both sides consistently. | Class-receiver definition context and public API extraction are included in compile repair prompts. | Corpus fixtures assert imported class context and a remedy menu for keeping caller/callee contracts consistent. |
| 84A' | Side effects in expected paths and generated artifacts could pollute dependency/setup lifecycles or verifier targeting. | Two-tier side-effect sanitization separates task artifacts from setup/generated paths before lifecycle setup and repair targeting. | Runner tests keep side-effect paths out of dependency lifecycle setup and verifier false-positive repairs. |
| 85 | Repeated no-edit compile repair attempts consumed wall clock without re-anchoring source context. | Compact repair sessions escalate after zero-edit/no-progress compile repair turns and carry anchored compile frames forward. | Compile repair tests require compact-session prompts to preserve frame excerpts and tool-schema reminders. |
| 86A | Final acceptance composite failures could target the outer readiness label and miss inner build verifier compile errors. | Composite unwrapping targets implementation compile repair when readiness/acceptance fails because build verifier produced parseable compile errors. | `browser_readiness_failed:build_verifier_failed` with compile frames routes to the shared compile ladder. |
| 86C | Probe evidence was labeled missing or unavailable when the probe was provisioned but never exercised because an upstream gate failed. | `not_exercised:<upstream_reason>` vocabulary distinguishes unreached probes from missing probe infrastructure. | Summary/gate-table and evidence-tier tests require `not_exercised` wording for upstream build/readiness failures. |
| 86D' | Styling invariants forced Tailwind into plain-CSS apps and created incoherent stacks. | Presence-conditional styling accepts plain CSS with zero Tailwind artifacts and repairs only partially present Tailwind stacks. | Plain-CSS and partial-Tailwind fixtures guard the coherence boundary. |
| 87A | Lifecycle-captured SWC source frames were present but not parsed, causing build failures to terminate as dependency setup lifecycle failures. | Lifecycle-frame routing parses SWC `Error:` frames without a `Failed to compile` banner and classifies them as compile errors. | Parser and verifier tests require lifecycle source frames to enter implementation compile repair. |
| 87B | Recovery UltraPlan YAML could fail render/parse roundtrip and be discarded. | Recovery roundtrip hardening uses JSON-compatible YAML quoting and writes loadable `needs_review` artifacts on validation failure. | Roundtrip tests cover quotes, backslashes, multiline, Japanese, and the exact recovery-prompt shape. |
| 94 diagnosis datum | Early-death profile projection remained under diagnosis after `test0708_005`. | `test0708_007` records `Effective profile: nextjs` on a post-binding death path (`recovery_prompt_saved.recovery_profile=nextjs`, `tui_command_stop.effective_profile=nextjs`), supporting the hypothesis that the projection defect is limited to earlier death paths rather than runtime profile resolution. | Corpus fixture preserves the post-binding nextjs projection datum while exhaustion honesty is repaired separately. |
| 95 step-1 diagnosis | `test0708_009` exposed a cross-file hook contract mismatch where page code destructured `gameState`/`movePlayer`/`shoot` from `useGameEngine`, but the repair prompt only showed the failing destructuring line. Attempt 2 was appended and attempt 3 compact; rollback was invoked but skipped with `snapshot_missing:src/app/page.tsx` because only setup/config had completed and no passing page snapshot existed. | Compile repair definition context now also follows missing properties in object destructuring to the imported hook/function and, for long exported functions, extracts the top-level returned object API when the bounded definition head would miss it. Compact escalation and rollback behavior were diagnostic, not changed. | Compile guidance tests require destructured hook missing-property failures to include the imported hook API surface and caller/callee consistency remedy. |
| 95B | Four model families had produced zero-edit compile repair turns even after successful generation writes, leaving single-file compile failures to exhaust or rollback. | After compact zero-edit compile repair, a one-shot compact regeneration prompt requires a full-file Write for the single failing source file and accepts it only when rebuild parsed compile errors strictly decrease; rejected regeneration restores the file snapshot and falls through to rollback/classification. | Runner tests cover successful regeneration, rejected regeneration restore, multi-file skip, telemetry, and rollback fallback after rejected regeneration. |
| post-95/96 A | `test0708_010` showed an ESM `postcss.config.js` in a non-module package, but the failure surfaced as a generic PostCSS `plugins` build error. | Tailwind/PostCSS coherence now treats `.js`/extensionless config files with `export default` as incoherent unless `package.json` is `type=module`, leaves `.mjs` alone, and deterministically rewrites owned config to CommonJS. | Profile tests cover the exact PostCSS ESM shape, `.mjs` pass-through, and compile repair guidance for the `plugins` export error. |
| post-95/96 B | `test0708_011` exhausted on repeated `edit_anchor_not_found` even though the model was attempting localized edits. | Edit now reports a deterministic best-match excerpt, salvages only a unique whitespace-normalized anchor region with `edit_anchor_salvaged` telemetry, and escalates same-file repeated anchor failures to full-file Write guidance. | Edit/registry/session tests cover salvage, ambiguity rejection, best-match feedback, telemetry, and second-failure Write escalation. |
| post-95/96 C | `test0708_012` reached final acceptance with browser readiness/probe evidence, then exhausted as `loop_progress_exhausted: no concrete blocker recorded` while restart/input evidence remained pending. | Final-acceptance repair exhaustion now classifies as `capability_evidence_unresolved:<keys>`, emits pending evidence/remedies, and recovery artifacts lead with per-key remedy guidance and restart attachment points. | Runner and conformance tests require final-acceptance exhaustion with pending keys to name `capability_evidence_unresolved` instead of a bare exhaustion/no-blocker reason. |
| post-97 A | `test0708_016` exposed a Bash-tool runtime policy boundary that rejected `cmd1 && cmd2` before the shared verifier normalizer could split it. | Runtime Bash policy now enters the shared normalization pipeline, executes normalized `&&` and `;` segments through bounded commands, preserves `&&` short-circuit behavior, and still rejects semantic pipes with policy feedback. | Runtime tests cover the verbatim Bash shape, `false && echo x` short-circuiting, and the protection-coverage audit now treats runtime Bash policy as a normalizer boundary. |
| post-97 B | `test0708_013` showed root-anchor path salvage did not fire for an absolute `/Users/.../commandagent_mvp/01/test0708_013/package.json` path whose last two workspace components were present and unique. | Path normalization now tries root-anchor salvage for raw absolute paths that canonicalize outside the active workspace, returning a distinct salvage kind and telemetry. | Path-guard and registry tests cover the verbatim absolute path and an ambiguous duplicate-anchor rejection. |
| post-a3b stale path | The a3b five-run campaign still produced stale absolute paths where root-anchor salvage was insufficient or unobservable, and rejection feedback did not name the active workspace root or nearest declared artifact. | Tool path normalization now emits fallback-evaluation telemetry, then accepts only a unique suffix match against step expected paths plus current-plan artifact paths. Rejected stale absolute paths remain unwritten, recover with feedback naming the current root and nearest relative expected path, and do not guess across workspaces. | Registry and loop-run tests cover accepted required-path fallback, rejected stale absolute feedback, model adaptation after feedback, and current-plan artifact candidates; corpus fixtures record probe correlation. |
| post-97 C | The gemma4-cloud round needed instrumentation for suspected provider context truncation without turning it into a gate. | Provider turn telemetry records estimated prompt tokens sent, provider prompt/eval counts, and finish reason, and emits a warning-only `context_truncation_suspected` event after persistent undercut. | Provider and summary tests require token fields, hysteresis warning emission, and a one-line summary/card note when warnings fire. |
| post-106 A | `test -f X && echo "EXISTS" || echo "MISSING"` was still being diagnosed as `shell_control_syntax` in a later run even though the runtime Bash boundary already knew how to strip summary echoes. | The shared runtime verifier normalizer now strips any plain echo-only success/failure branch pair, so the exact `EXISTS`/`MISSING` form reduces to the base `test -f` check and emits normalization telemetry before policy diagnosis. | Runtime tests cover the verbatim `EXISTS`/`MISSING` fixture, and the verify-boundary audit is keyed to command-capability sites rather than a fixed path list. |
| Q1 boundedness A/B | `content_b` repeatedly issued `ls -R && cat package.json`; each command hit the 180s Bash cap because broad recursive listing traversed the whole workspace including generated dependency trees. The old repeated-tool-error key used exact error text, so timeout variants were not aggregated reliably. | Step sessions now have a wall-clock cap, command/provider time sinks are recorded, similar command timeouts share a normalized prefix key, the second timeout gives strategy-change feedback, and the third terminates as `command_timeout_loop`. | Loop-run tests cover similarity grouping, strategy feedback, and step wall-clock expiry; conformance requires terminal timeout repetition to stop as `command_timeout_loop`. |
| Q1 boundedness C | `tool_a` readiness found port 3011 occupied by a stale Next dev server from the same run rather than an unrelated external service. The earlier lifecycle cleanup was per call site, so the child survived into a later readiness cycle and terminal exhaustion could still say no blocker. | Bounded spawn now owns a run-scoped server child registry. Phase transitions, terminal guards, SIGINT/drop paths, and readiness `port_in_use` retry reap registered children with `server_reaped { origin_phase }`; external owners remain honest failures with pid/command. | Bounded-process and runner tests cover registry reaping telemetry and owner parsing; conformance rejects empty-handed no-blocker exhaustion vocabulary. |
| Post-101 Q1 | `content_b` placed dependency installs in verify commands and later hit invalid npm semver, while `game_a` had camelCase/PascalCase collision logic that static evidence missed. | Verify install segments are substituted into runtime-owned dependency reconciliation before remaining verify commands run; invalid semver output names the manifest entry and corrected example; gameplay identifier matching is case-insensitive across source/browser evidence helpers. | Verify, runtime Bash, sanitizer/lint, semver remedy, evidence property, and corpus tests pin the install-substitution, invalid-semver, and camelCase/PascalCase evidence shapes. |
| Final multi-model A | TOOL/GAME fixtures showed `grep X && grep Y` rejected as unsplit shell control at verifier lint/contract entry points even though plan/runtime/Bash boundaries had split machinery. | StepPlan lint and completion contract validation now enter the shared planner verify normalizer before policy diagnosis; unsupported split fragments fail with allowed verifier categories instead of a generic shell-control label. The 92C protection audit now treats StepPlan verify-policy lint as a normalizer boundary. | Lint, completion, and protection-coverage tests pin the multi-grep shape and prevent new direct lint bypasses. |
| Final multi-model B | CLI fixtures showed setup StepPlan lint exhaustion under `python-cli` did not substitute the 94C setup fallback because the fallback was effectively Next.js-only. | Setup fallback now uses the profile scaffold boundary for known profiles; `python-cli` produces `pyproject.toml`, `src/<package>/main.py`, and setup-safe `test -f` verification rather than failing as phase scaffold exhaustion. Python compilation remains a later deterministic gate. | Runner tests cover lint-exhausted python-cli setup fallback; corpus fixture records the CLI scaffold shape. |

## Review Process Rule

Any instruction touching contracts, invariants, or vocabularies requires a
one-paragraph overfitting review at issue time. The review must say what the
change fossilizes, whose design freedom it narrows, and what degradation path
keeps declined preferences honest. This rule is mandatory because 82A' caught
restart reachability drifting toward visible UX control, 84A' caught artifact
and setup side effects that could narrow legitimate implementation shapes, and
86D' caught Tailwind stack injection fossilizing one styling choice over plain
CSS.

## Boundedness Guarantees

Permanent invariant: no pathway may require human interruption.

| mechanism | instruction | guarantee | permanent guard |
|---|---|---|---|
| Provider-turn wall-clock bound | 62 B | Every chat/provider turn emits duration telemetry, is capped by configuration, retries once on `provider_turn_timeout`, then terminates with an honest terminal handoff instead of requiring manual interruption. | `tests/conformance` named assertion `bounded_provider_turns`; unit coverage for `provider_turn_timeout` terminal records and recovery handoff. |
| Verify-command wall-clock bound | 62 C | Deterministic verify commands are capped per command/profile, the process group is killed on expiry, and timeout is classified as `OracleError` / `verify_command_timeout:<command>` with bounded-check repair guidance, never as an implementation-edit target. | `tests/conformance` named assertion `bounded_verify_commands`; verifier tests for process-group kill, OracleError classification, and python-cli timeout substitution. |
| Step-session wall-clock bound | Q1 boundedness | Any single step session has a generous wall-clock cap. Expiry terminates itself as `step_wall_clock_exhausted` and names the dominant command/provider sink instead of waiting for manual SIGINT. | Loop-run test with a short cap and conformance terminal checks. |
| Similar command timeout aggregation | Q1 boundedness | Bash command timeouts count toward repeated-tool-error handling by normalized similarity, not exact error identity. Repeated broad recursive listings get strategy-change feedback and then terminate as `command_timeout_loop`. | Loop-run timeout-similarity test and conformance `command_timeout_loop` assertion. |
| Run-scoped server reaping | Q1 boundedness | Every launched readiness/dev server registered through bounded spawn is reaped at phase and terminal boundaries. Mid-run `port_in_use` from a registered child is deterministically reaped and retried once; external owners are reported with pid/command. | Bounded-process registry test, readiness owner parser test, and `server_reaped` corpus fixture. |

## Required Fields

- mechanism id
- default state / off flag
- target scenarios
- admission benchmark
- before/after result
- final audit date
- rollback condition

## M001

| field | value |
|---|---|
| mechanism id | M001 |
| default state / off flag | off until admitted |
| target scenarios | TBD |
| admission benchmark | minimal-loop-expanded |
| before/after result | TBD |
| final audit date | TBD |
| rollback condition | regression in target or adjacent scenarios |

## M002

| field | value |
|---|---|
| mechanism id | M002 |
| default state / off flag | off until admitted |
| target scenarios | TBD |
| admission benchmark | minimal-loop-expanded |
| before/after result | TBD |
| final audit date | TBD |
| rollback condition | regression in target or adjacent scenarios |

## Task-track backfill (2026-07-10 – 2026-07-11)

The entries below backfill the task-track mechanisms that were implemented and
measured during the 2026-07-10 to 2026-07-11 campaign. Commit hashes were
checked against `git log --oneline`; corpus fixture names were taken from
`git show --stat` under `mvp/anvilminimal/tests/corpus/apps/`. UAT report paths
are included only where the report exists under `workspace/management/runs/`.

| ID | mechanism | commit | motivation (discovery UAT) | corpus fixture | verification (result UAT) | state |
|---|---|---|---|---|---|---|
| T1 | Port-grep semantic rewrite | `923b623` | `test0710_bs_002` combo2-002 ([report](workspace/management/runs/uat-test0710-bs-002/uat-report.md)) | `test0710_stage0_pivot` / `package-port-only-grep-normalization.jsonl` | Non-recurrence in `test0710_bs_003` ([report](workspace/management/runs/uat-test0710-bs-003/uat-report.md)) | admitted |
| T2 | Import/JSX boundary static guard | `5991fba` | `test0710_bs_002` combo1-001/003 ([report](workspace/management/runs/uat-test0710-bs-002/uat-report.md)) | `test0710_stage0_pivot` / `static-import-guards.jsonl` | Non-recurrence from `test0710_bs_003` onward ([report](workspace/management/runs/uat-test0710-bs-003/uat-report.md)) | admitted |
| T3 | `write_required` rung | `8949be9` | `test0710_bs_002` read-only exhaustion group ([report](workspace/management/runs/uat-test0710-bs-002/uat-report.md)) | `test0710_stage0_pivot` / `read-only-write-required.jsonl` | Fired in `test0710_bs_003`; wrong target exposed and handled by T4 ([report](workspace/management/runs/uat-test0710-bs-003/uat-report.md)) | admitted |
| T4 | Evidence-to-target mapping | `19c1d10` | `test0710_bs_003` combo1 wrong `package.json` target ([report](workspace/management/runs/uat-test0710-bs-003/uat-report.md)) | `test0710_stage0_pivot` / `read-only-write-required-evidence-targets.jsonl` | Removed in `test0710_bs_004`; related route later recurred and was unified by T20 ([report](workspace/management/runs/uat-test0710-bs-004/uat-report.md)) | superseded_by_T20 |
| T5 | Hook snapshot and restoration rung | `f1af34e` | `test0710_bs_003` combo3 primary hook loss ([report](workspace/management/runs/uat-test0710-bs-003/uat-report.md)) | `test0710_stage0_pivot` / `hook-snapshot-primary-restore.jsonl` | Class stopped dominating later runs | admitted |
| T6 | Deterministic step template | `3ae842b` | `test0710_bs_003` planner-time dominance ([report](workspace/management/runs/uat-test0710-bs-003/uat-report.md)) | `test0710_stage0_pivot` / `deterministic-step-plan-used.jsonl` | `test0710_bs_004` observed event counts 2/2/1 ([report](workspace/management/runs/uat-test0710-bs-004/uat-report.md)) | admitted |
| T7 | Deterministic restart repair (rung C) | `-` | `test0710_bs_004` ([report](workspace/management/runs/uat-test0710-bs-004/uat-report.md)) | `-` | Not built by design; decomposed into T9/T16/T19 | not_built |
| T8 | Hook grep quote-independent normalization | `fb1689d` | `test0710_bs_004` combo3 quote false negative ([report](workspace/management/runs/uat-test0710-bs-004/uat-report.md)) | `test0710_bs_004_command_normalization` / `combo3-hook-grep-normalized.jsonl` | Same pair full in `test0710_bs_005` ([report](workspace/management/runs/uat-test0710-bs-005/uat-report.md)) | admitted |
| T9 | State-binding diagnosis | `4e6c4c8` | `test0710_bs_004` combo1-family interaction failures ([report](workspace/management/runs/uat-test0710-bs-004/uat-report.md)) | `test0710_stage0_pivot` / `state-binding-diagnosis.jsonl` | `bs_005`/`bs_006` returned undeterminable, then T16 calibrated/prevented the class ([test0710_bs_005](workspace/management/runs/uat-test0710-bs-005/uat-report.md), [test0710_bs_006](workspace/management/runs/uat-test0710-bs-006/uat-report.md)) | superseded_by_T16 |
| T10 | Inspect-command normalization | `fb1689d` | `test0710_bs_004` combo2 timeout ([report](workspace/management/runs/uat-test0710-bs-004/uat-report.md)) | `test0710_bs_004_command_normalization` / `combo2-inspect-command-normalized.jsonl` | Fired in later runs | admitted |
| T11 | Preset UltraPlan | `c268c53` | `test0710_bs_004` plan-isomorphism evidence ([report](workspace/management/runs/uat-test0710-bs-004/uat-report.md)) | `test0710_stage0_pivot` / `preset-ultra-plan-used.jsonl` | `test0710_bs_006` fired 6/6; default later rolled back by T24 while opt-in remained ([report](workspace/management/runs/uat-test0710-bs-006/uat-report.md)) | admitted_optin |
| T12 | Adversary vocabulary, translation, and probe-dimension scan | `0be0f74` | `test0710_bs_006` breakout combo1 arbitration false negative ([report](workspace/management/runs/uat-test0710-bs-006/uat-report.md)) | `test0710_bs_006_breakout_combo1` | `test0711_bs_001` #7 full ([report](workspace/management/runs/uat-test0711-bs-001/uat-report.md)) | admitted |
| T13 | Edit-anchor recovery ladder | `8aa3c2c` | `test0710_bs_005` combo2 anchor x8 ([report](workspace/management/runs/uat-test0710-bs-005/uat-report.md)) | `test0710_bs_005_anchor_recovery_combo2` | Old terminal state disappeared in `test0711_bs_001`; T18 completed the interlock ([report](workspace/management/runs/uat-test0711-bs-001/uat-report.md)) | admitted |
| T14 | Route-unbound wiring guidance | `f1bfb82` | `test0710_bs_005` combo1 ([report](workspace/management/runs/uat-test0710-bs-005/uat-report.md)) | `test0710_bs_005_route_unbound_combo1` | `test0711_bs_002` #7 recovered in live run and reached full ([report](workspace/management/runs/uat-test0711-bs-002/uat-report.md)) | admitted |
| T15 | Probe preflight | `49325f6` | `test0710_bs_006` space combo3 infrastructure failure ([report](workspace/management/runs/uat-test0710-bs-006/uat-report.md)) | `test0710_bs_006_probe_preflight_space_combo3` | Recorded in every later run | admitted |
| T16 | Input-coupled diagnosis and contract wording | `764094b`, `926d3cd` | Calibration from `bs_005`/`bs_006` real artifacts ([test0710_bs_005](workspace/management/runs/uat-test0710-bs-005/uat-report.md), [test0710_bs_006](workspace/management/runs/uat-test0710-bs-006/uat-report.md)) | `test0710_bs_005_006_state_binding_input_coupled` | Contract wording acted preventively; class disappeared, while the diagnosis path remained unexercised | admitted_prevention |
| T17 | Tier-coupled preset default | `b08e92f` | `test0711_bs_001`/`test0711_bs_002` A/B ([test0711_bs_001](workspace/management/runs/uat-test0711-bs-001/uat-report.md), [test0711_bs_002](workspace/management/runs/uat-test0711-bs-002/uat-report.md)) | `test0711_bs_001_plan_preset_tier` | Did not fire in `test0711_bs_003`, fixed by T23, then `test0711_bs_004` measured distribution degradation and T24 rolled it back ([test0711_bs_003](workspace/management/runs/uat-test0711-bs-003/uat-report.md), [test0711_bs_004](workspace/management/runs/uat-test0711-bs-004/uat-report.md)) | rolled_back |
| T18 | Anchor x stagnation interlock | `8cba4ef` | `test0711_bs_002` #2 ([report](workspace/management/runs/uat-test0711-bs-002/uat-report.md)) | `test0711_bs_007_anchor_stagnation_interlock` | `test0711_bs_003` showed carryover firing ([report](workspace/management/runs/uat-test0711-bs-003/uat-report.md)) | admitted |
| T19 | Contract-attribute-missing guidance | `53a967d` | `test0711_bs_001` #6 ([report](workspace/management/runs/uat-test0711-bs-001/uat-report.md)) | `test0711_bs_001_6_contract_attribute_missing` | Fired in `test0711_bs_003` #1 ([report](workspace/management/runs/uat-test0711-bs-003/uat-report.md)) | admitted |
| T20/T20b | Target-resolution unification and escalation carryover | `f8388cf6`, `7e920fb0` | `test0711_bs_002` #1 `package.json` recurrence ([report](workspace/management/runs/uat-test0711-bs-002/uat-report.md)) | `test0710_stage0_pivot` / `final-acceptance-target-resolution.jsonl`; `test0711_bs_008_escalation_carryover` | `test0711_bs_003` showed `page.tsx` target resolution and carryover telemetry ([report](workspace/management/runs/uat-test0711-bs-003/uat-report.md)) | admitted |
| T21 | Compile diagnostic extraction | `99807bc` | `test0711_bs_002` #8 webpack-internal location ([report](workspace/management/runs/uat-test0711-bs-002/uat-report.md)) | `test0711_bs_008_compile_diagnostic_extraction` | `test0711_bs_003`/`test0711_bs_004` recorded real source locations ([test0711_bs_003](workspace/management/runs/uat-test0711-bs-003/uat-report.md), [test0711_bs_004](workspace/management/runs/uat-test0711-bs-004/uat-report.md)) | admitted |
| T22 | Implementation-detail grep demotion | `1188842` | `test0711_bs_002` #3 `addEventListener` grep ([report](workspace/management/runs/uat-test0711-bs-002/uat-report.md)) | `test0711_bs_002_source_detail_grep_advisory` | Not exercised later; zero demotions also confirmed the gate was not broadly relaxed | admitted_unexercised |
| T23 | Tier decision from resolved model | `fe690b1` | `test0711_bs_003` default non-firing ([report](workspace/management/runs/uat-test0711-bs-003/uat-report.md)) | `test0711_bs_003_resolved_planner_tier` | `test0711_bs_004` fired 6/6 ([report](workspace/management/runs/uat-test0711-bs-004/uat-report.md)) | admitted(対象機構はrolled_back) |
| T24 | Satisfied-setup short-circuit, preset-step conversion, and default-none rollback | `042880f` | `test0711_bs_004` #1/#7 ([report](workspace/management/runs/uat-test0711-bs-004/uat-report.md)) | `test0711_bs_004_preset_setup_no_progress` | Verification UAT passed P0/P1 ([report](workspace/management/runs/uat-test0711-bs-005/uat-report.md)) | admitted |

Summary:

- Across 24 tasks: one rollback (T17), one deliberately unbuilt item (T7), and two replacements (T4→T20, T9→T16).
- Every task has a motivating measured failure run; no speculative mechanism was admitted.
- T17 is the concrete example of the "off until admitted" operating rule working end to end: adopt, measure, detect degradation, and roll back, with each step recorded.
