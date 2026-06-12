# Minimal Loop Mechanism Ledger

This ledger records admitted minimal-loop mechanisms so Phase 3 additions stay
small, measured, and reversible.

## M001: completion-without-write feedback

Status: admitted (2026-06-12)

Introduced by: `#1036`

Final audit date: 2026-06-12

Admission report:
[Minimal Loop Phase 3 Cycle 1 Evaluation](minimal-loop-phase3-cycle1-evaluation-20260612.md)

Target scenarios:

- `new-python-csv-small`
- `fix-rust-parser-error`
- `fix-readme-command`
- `fix-python-slugify`

Mechanism:

- Trigger once per session when the model returns a no-tool completion before
  any `Write` or `Edit` has executed.
- Inject a neutral user-role ephemeral feedback asking the model to create or
  modify files if the task requires it, or finish if no file change is needed.
- Accept the next no-tool completion to prevent loops.

Admission result:

- Target set improved from 7/20 to 17/20 in the Task15 real rerun.
- Final Task18 recheck baseline: minimal+M001 83/125 vs legacy-lite 71/125.
- Elapsed mean remained faster: 38.5 sec vs 66.6 sec.

Configuration:

- Off flag: `ANVIL_NO_MINIMAL_COMPLETION_WITHOUT_WRITE_FEEDBACK=1`
- Admission model: `qwen3.6:27b-coding-nvfp4`
- Injected text: 210 characters, 38 whitespace-delimited words. The token count
  is tokenizer-dependent; this is treated as a small one-message injection.

Open watchlist:

- `multi-file-rust-library`: one-run regression while feedback fired in all
  runs; currently within n=5 noise.
- `fix-js-date-helper`, `new-python-csv-small`, `non-coding-runbook`: remaining
  losses require separate triage before any M002 admission.
