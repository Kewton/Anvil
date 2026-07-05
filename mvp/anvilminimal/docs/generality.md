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
| Guardrails are permanent and cheap enough to run in CI. | M6 docs and tests | `cargo test --test corpus_regression`, `cargo test --test generality_guardrails`, plus the scenario contract golden tests named below. |

## Required Gates

These gates are required for M6 branch protection or equivalent release
approval:

- `cargo test --test corpus_regression`
- `cargo test --test generality_guardrails`
- `cargo test generic_ultra_promotes_to_nextjs_after_workspace_manifest --lib`
- `cargo test generic_ultra_promotes_to_python_cli_after_pyproject_manifest --lib`
- `cargo test generic_ultra_without_manifest_keeps_static_tier --lib`
- `python3 -m unittest tests/eval/test_acceptance_contract.py`
- `python3 -m unittest tests/eval/test_completion_contract_snapshots.py`
- `python3 -m unittest tests/eval/test_false_positive_regression.py`

The corpus harness guards harvested probe/evidence/profile behavior. The
scenario contract tests are the golden suite for contract inference. The
false-positive regression protects the "static screen is not a game success"
boundary. The generality guardrails protect static- and reduced-assurance
rendering and the Next.js boundary-erosion tripwire.

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
