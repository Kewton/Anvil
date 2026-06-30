# Parity Gate Rollout Plan

作成日: 2026-06-29

## 1. 目的

runtime semantics parity gate を一度に厳格化して開発を止めるのではなく、段階的に `warn -> fail` へ移行する。

## 2. Rollout Stages

| Stage | Gate | Initial behavior | Fail condition |
| --- | --- | --- | --- |
| R1 | document completeness | fail | required files / G-S01〜G-S16 missing |
| R2 | fixture coverage | warn -> fail | each gate lacks positive/negative fixture |
| R3 | failure kind non-empty | fail | process/acceptance failure has blank kind |
| R4 | provider probe metadata | warn | provider/prompt-sensitive change without probe plan |
| R5 | targeted eval comparison | warn -> fail | targeted eval missing for touched lifecycle stage |
| R6 | UAT-equivalent acceptance | warn -> fail | release gate lacks browser/interaction evidence |
| R7 | anvildev parity threshold | warn -> fail | MVP lower than anvildev by >=10pp without intentional evidence |

## 3. Stage Order

1. Enable document/report schema checks.
2. Add fixture coverage checks as warning.
3. Turn blank failure kind check to failure.
4. Require provider probe metadata for provider/prompt changes.
5. Require targeted eval for runtime semantics changes.
6. Require UAT-equivalent evidence for Next.js/TUI/manual UAT fixes.
7. Require anvildev comparison for parity completion.

## 4. Rollback

Each gate addition must have a bounded rollback:

| Gate | Rollback |
| --- | --- |
| report schema | remove CI invocation, keep docs |
| fixture coverage | downgrade to warning |
| failure taxonomy | allow temporary `unclassified_process_failure` only with raw evidence |
| provider probe | disable opt-in probe, keep skip behavior |
| targeted eval | lower to warning while preserving run commands |
| UAT acceptance | require partial instead of pass, never build-only pass |
| anvildev threshold | keep comparison report but remove fail threshold temporarily |

Rollback must not:

- re-allow blank failure kind silently.
- mark build-only/title-only interactive apps as success.
- delete trace evidence.

## 5. Baseline Updates

Baseline update is allowed only when:

- `parity_gate_report.json` exists.
- old and new summaries are both recorded.
- failure kind blank count is explained.
- lower success is classified as regression or correct failure detection.
- acceptance false positive changes are recorded.

## 6. Owner Phases

| Area | Owner phase |
| --- | --- |
| trace schema/report schema | Phase 0 |
| source/MVP trace capture | Phase 1/2 |
| gate matrix | Phase 3 |
| fixture coverage | Phase 4 |
| failure taxonomy | Phase 5 |
| eval protocol | Phase 6 |
| UAT-equivalent acceptance | Phase 7 |
| CI/preflight | Phase 8 |
| rollout | Phase 9 |
| review/freeze | Phase 10 |

## 7. Current Rollout Decision

Current 022 status:

- R1 can be enforced after this document set is reviewed.
- R2 should start as warning until tests are implemented.
- R3 should become failure soon because current known blank count is 15.
- R4 is warning until provider probe metadata is wired to work plans.
- R5/R6/R7 are not yet fail gates because current same-condition `anvildev` trace and release UAT evidence are missing.
