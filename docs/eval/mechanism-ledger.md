# Mechanism Ledger

This ledger records every mechanism admitted into `minimal_loop`. It is intended
to make growth visible: each new feedback, guard, recovery path, or prompt
addition must have a measured reason, an off flag, and a current audit status.

## Entries

| id | mechanism | status | admitted by | final audit date |
|---|---|---|---|---|
| M001 | completion-without-write feedback | admitted | #1036 | 2026-06-13 |

## M001: Completion-Without-Write Feedback

| field | value |
|---|---|
| mechanism | completion-without-write feedback |
| PR | #1036 |
| status | admitted |
| trigger | Act-mode no-tool completion while session has observed zero `Write`/`Edit` calls |
| action | Inject one user-role ephemeral feedback message; accept the second no-tool response |
| target scenarios | `new-python-csv-small`, `fix-rust-parser-error`, `fix-readme-command`, `fix-python-slugify` |
| injection size | 196 chars, 35 whitespace words, approximately 61 minimal-loop tokens |
| off flag | `ANVIL_NO_MINIMAL_COMPLETION_WITHOUT_WRITE_FEEDBACK=1` |
| bench option | `--no-minimal-completion-without-write-feedback` |
| admission model | `qwen3.6:27b-coding-nvfp4` |
| admission benchmark | `minimal-loop-expanded`, 25 scenarios x 5 runs |
| admission report | [minimal-loop-t2-4-task15-feedback-rerun-20260612.md](minimal-loop-t2-4-task15-feedback-rerun-20260612.md) |
| baseline report | [minimal-loop-t2-4-recheck-20260612.md](minimal-loop-t2-4-recheck-20260612.md) |
| final audit date | 2026-06-13 |

Admission result:

| metric | before | after | delta |
|---|---:|---:|---:|
| overall minimal success | 69/125 | 80/125 | +11 |
| target scenarios total | 7/20 | 17/20 | +10 |
| legacy-lite reference | 66/125 | 66/125 | n/a |
| minimal elapsed mean | 28.9 sec | 38.5 sec | +9.6 sec |

Final Task18 recheck baseline:

| metric | value |
|---|---:|
| minimal+M001 success | 83/125 |
| legacy-lite reference | 71/125 |
| minimal elapsed mean | 38.5 sec |
| legacy-lite elapsed mean | 66.6 sec |

Audit notes:

- The mechanism passed the primary admission criterion: all four target
  no-edit-loop scenarios improved.
- `scaffold-fastapi-service` regressed from 5/5 to 1/5, but the mechanism did
  not fire in that scenario's runs, so current evidence does not support a
  direct causal link.
- `multi-file-rust-library` regressed from 5/5 to 4/5 and feedback fired in all
  five runs; this remains the only watchlist item with plausible mechanism
  involvement, but the observed delta is one run at n=5.
- The admission measurement includes possible confounding from the
  [blocked-mkdir trap](triage/blocked-mkdir-trap.md). At admission time, the
  environment allowed a failure path where M001 fired, the model tried
  `Bash mkdir`, offline policy blocked it, and the session did not recover to
  `Write`.
- After #1045 and #1046, M001's marginal contribution was expected to change,
  so Task26 re-ran an on/off ablation and recorded the pure contribution.
- Task26 fulfilled that reservation after the blocked-`mkdir` trap fixes:
  [Cycle 3 full matrix](cycle3-full-matrix-20260613.md) measured the same
  binary with M001 on/off under seeded 25x5 runs. M001 on scored 88/125 versus
  M001 off at 64/125, a +24 run contribution. Pure no-edit failures dropped
  from 50 to 16. Mean elapsed time increased from 28.5s to 39.7s.
- Future reviews should compare against this ledger before admitting another
  feedback or recovery mechanism.
