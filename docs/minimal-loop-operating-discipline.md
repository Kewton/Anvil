# Minimal Loop Operating Discipline

This document records the operating discipline used for the minimal-loop
strangler work. It complements `docs/minimal-loop-plan.md`; the plan defines
the phases and principles, while this document defines the repeatable review
skills that keep Phase 3 and Phase 4 from drifting.

## Scope

These rules apply to minimal-loop evaluation, mechanism admission, ablation
audits, and Phase 4 default-switch decisions.

- Do not add a new mechanism directly from an aggregate failure table.
- Triage first, classify failures with engine-neutral vocabulary, then decide
  whether a deterministic fix, benchmark check fix, environment fix, or actual
  mechanism is justified.
- A mechanism is admitted only when its target failures are reproducible, the
  trigger is deterministic, the feedback is neutral, and an off flag exists.
- Every admitted mechanism must be recorded in
  `docs/eval/mechanism-ledger.md`.

## Phase 4 Philosophy

Minimal becoming stronger than legacy-lite in one matrix is not by itself a
default-switch decision. Phase 4 requires stability evidence, not just a high
headline score.

Before switching the default engine to minimal, record:

1. A full seeded matrix on the primary model.
2. A second-model confirmation matrix.
3. An ablation result for each admitted mechanism whose contribution is still
   material to the decision.
4. A statement that the headline delta exceeds the measured run-to-run noise
   band.
5. A check that no severe interactive-use regression is known.

The default-switch decision remains a human decision. Evaluation reports may
state whether the conditions are satisfied, but should not perform the switch.

## Skill: Failure Triage

Use this before proposing a mechanism.

Inputs:

- A concrete benchmark root or pair of roots.
- Scenario IDs and run IDs.
- `summary.tsv`, `meta.json`, copied `session.json`, and `logs/llm-io.jsonl`.

Procedure:

1. Compare failing minimal runs with successful legacy or previous-minimal
   runs when available.
2. Identify the first behavioral divergence, not just the final check failure.
3. Classify with engine-neutral terms:
   `check_failed`, `no_edit_loop`, `no_edit_loop_after_feedback`,
   `command_failed`, `missing_file`, `semantic_failure`,
   `parser_failure`, `environment_trap`, `run_variance`, or `unknown`.
4. Separate benchmark/check bugs and environment affordance traps from model
   capability limits.
5. Stop with a report if a deterministic or check/environment fix is more
   direct than a mechanism.

Outputs:

- A `docs/eval/triage/*.md` report with per-run evidence, aggregate counts,
  and an explicit recommendation: mechanism candidate, deterministic fix,
  check fix, environment fix, variance, or no candidate.

## Skill: Mechanism Admission

Use this only after failure triage has identified a focused mechanism
candidate.

Required PR evidence:

- Target scenario IDs and links to the triage report.
- Deterministic trigger definition.
- Feedback or intervention text, with token count.
- Individual off flag name.
- Minimal-loop core line-count before and after.
- Unit tests for the state transition.
- Human benchmark request or result covering target improvement and non-target
  non-regression.

The mechanism must avoid task-intent heuristics unless the triage proves that a
deterministic fact is insufficient. Prefer removing an affordance trap over
adding feedback that teaches around the trap.

## Skill: Ablation Audit

Use this whenever an admitted mechanism may be confounded with a later
environment, prompt, benchmark, or policy change.

Procedure:

1. Record the mechanism, off flag, model, date, benchmark root, and active
   flags in `docs/eval/mechanism-ledger.md`.
2. Compare on/off runs with the same binary, same seed policy, same model, and
   same scenario matrix.
3. Report both aggregate and target-scenario deltas.
4. If the measured contribution shrinks after an environment fix, record that
   fact without rewriting the original admission result.

The ledger is intentionally conservative: it is both a record of admitted
mechanisms and an early-warning system for minimal-loop growth.

## Skill: Eval Report

Every formal evaluation report under `docs/eval/` should include:

- Exact model name and quantization.
- Engine and mechanism flags.
- Benchmark root path and run count.
- Git revision, dirty state, binary path, and build time when available.
- Seed policy.
- Headline success rate and elapsed time.
- Scenario table.
- Failure-class table.
- Noise/variance caveats.
- Links to triage reports and ledger entries.
- A clear statement of what the data does and does not prove.

Reports should avoid turning a single `n=5` scenario delta into a mechanism
claim. Use the currently measured noise band, and treat small scenario movement
as variance unless the run-level logs show a mechanism-causal path.
