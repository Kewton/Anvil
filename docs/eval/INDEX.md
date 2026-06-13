# Evaluation Index

## Current Baselines

- [Task15 feedback rerun](minimal-loop-t2-4-task15-feedback-rerun-20260612.md):
  first admitted Phase 3 mechanism, minimal 80/125.
- [Task15 non-fired run variance](t2-4-run-variance.md):
  GPU-free estimate of scenario-level run-to-run variance.
- [Task18 recheck](minimal-loop-t2-4-task18-recheck-20260612.md):
  updated baseline after the remaining check false-negative fixes.
- [Phase 3 cycle 1 evaluation](minimal-loop-phase3-cycle1-evaluation-20260612.md):
  admission decision and current baseline for the first admitted mechanism.
- [Task14 recheck baseline](minimal-loop-t2-4-recheck-20260612.md):
  check-only baseline before Task15, minimal 69/125 and legacy-lite 66/125.
- [Mechanism ledger](mechanism-ledger.md):
  admitted minimal-loop mechanisms and audit status.
- [Blocked mkdir trap triage](triage/blocked-mkdir-trap.md):
  tool-affordance mismatch behind many offline Bash `mkdir` blocks.
- [Blocked script-run triage](triage/blocked-script-run.md):
  local `cd ... && python/node ...` validation blocked by offline Bash policy.
- [Benchmark seed smoke](seed-smoke.md):
  Task24 seeded rerun gate check before Cycle 3.
- [Cycle 3 narrow seeded rerun](cycle3-narrow-seeded-rerun.md):
  Task25 blocked-mkdir trap verification before the full matrix.
- [Cycle 3 full matrix](cycle3-full-matrix-20260613.md):
  Task26 seeded 27B M001 on/off ablation and 8B frontier run.
- [Cycle 3 Task29 rebaseline and vibe-local comparison](cycle3-task29-vibe-local-comparison-20260613.md):
  Task29-inclusive full matrix, same-model vibe-local comparison, and focused 4-scenario rerun.
- [Capability frontier](frontier.md):
  model-by-model minimal-loop benchmark rows.

## T2-4 Reports

- [llm-io schema](llm-io-schema.md)
- [Initial T2-4 comparison](minimal-loop-t2-4-20260612.md)
- [Parser-fix rerun](minimal-loop-t2-4-rerun-20260612.md)
- [Task14 recheck baseline](minimal-loop-t2-4-recheck-20260612.md)
- [Task15 feedback rerun](minimal-loop-t2-4-task15-feedback-rerun-20260612.md)
- [Task15 non-fired run variance](t2-4-run-variance.md)
- [Task18 recheck](minimal-loop-t2-4-task18-recheck-20260612.md)
- [Phase 3 cycle 1 evaluation](minimal-loop-phase3-cycle1-evaluation-20260612.md)
- [Cycle 3 narrow seeded rerun](cycle3-narrow-seeded-rerun.md)
- [Cycle 3 full matrix](cycle3-full-matrix-20260613.md)
- [Cycle 3 Task29 rebaseline and vibe-local comparison](cycle3-task29-vibe-local-comparison-20260613.md)
- [Capability frontier](frontier.md)

## Triage

- [Parser contamination triage](triage/t2-4-parser-contamination.md)
- [Parser feedback loop deepdive](triage/t2-4-parser-feedback-loop-deepdive-20260612.md)
- [Cycle 2 loss triage](triage/cycle2-loss-triage.md)
- [Blocked mkdir trap triage](triage/blocked-mkdir-trap.md)
- [Blocked script-run triage](triage/blocked-script-run.md)
