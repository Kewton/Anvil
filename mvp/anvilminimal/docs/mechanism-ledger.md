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

## Promotion-Path Incident Chain

| incident | committed evidence | fix | permanent guard |
|---|---|---|---|
| False-full promoted run: final acceptance reported `full_success` / `assurance_level=full` while browser readiness and interaction evidence were `not_applicable`. | `../../../workspace/management/runs/uat-test0704-40304445424346474848149505153545556-000/uat-report.md` | Instruction 57 introduced the earned-assurance invariant: full assurance is derived from executed gate statuses, and disconnected promoted-web gates fail loudly. | `acceptance_gates_disconnected` is added to release-gate reasons; `earned_assurance_for_completion` reduces assurance when gate telemetry and release-gate status disagree. |
| UTF-8 boundary panic during promoted evidence scanning: substring extraction sliced inside a Japanese character and aborted before final acceptance. | `../../../workspace/management/runs/uat-test0704-403044454243464748481495051535455-000/uat-report.md` | Instruction 56 routed truncation through char-boundary helpers, including `floor_char_boundary` / `truncate_at_char_boundary`, and wrapped TUI slash-command execution with a terminal panic guard. | Multibyte truncation tests cover the helper path; `panic_caught` events distinguish a Rust panic from user abort or model failure. |
| Dependency setup authority required after promotion: package state declared `autoprefixer`, but later verify/build lacked authority to reconcile missing installed dependencies. | `../../../workspace/management/runs/uat-test0704-4030444542434647484814950515354555657-000/uat-report.md` | Instruction 58 added run-level setup authority after promotion/manifest repair and boundary reconciliation for declared Node dependencies. | Later verify/contract steps inherit sanctioned setup authority when the run created the dependency need; absent authority still ends as `dependency_setup_authority_required`. |

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
