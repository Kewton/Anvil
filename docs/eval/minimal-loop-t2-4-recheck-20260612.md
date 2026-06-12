# Minimal Loop T2-4 Recheck After Task14 Check Fixes

- Date: 2026-06-12 JST
- Benchmark: `minimal-loop-expanded`
- Recheck implementation: #1026
- Check fix PRs: #1027, #1028, #1029, #1030, #1031, #1032, #1033, #1034
- Recheck branch head: `a300027010aabd6fd207a07855ef145f6640af80`
- Legacy source root: `.anvil/benchmarks/20260612T024532-70925`
- Minimal source root: `.anvil/benchmarks/20260612T145227-40162`
- Output files: `summary.recheck.tsv` under each source root

The legacy source root contains 147 rows because an interrupted follow-up wrote 22
non-legacy rows into the same root. The recheck comparison below filters legacy
to the 125 rows whose workdir contains `/legacy/`, matching the fixed-binary
T2-4 report.

## Commands

```sh
bash scripts/bench.sh minimal-loop-expanded \
  --recheck-root /Users/maenokota/share/work/github_kewton/Anvil-develop/workspace/minimal-loop-plan/.anvil/benchmarks/20260612T024532-70925

bash scripts/bench.sh minimal-loop-expanded \
  --recheck-root /Users/maenokota/share/work/github_kewton/Anvil-develop/workspace/minimal-loop-plan/.anvil/benchmarks/20260612T145227-40162
```

These commands do not require GPU or Ollama. They reuse existing workdirs and
write `summary.recheck.tsv`; the original `summary.tsv` files were not
overwritten.

## Aggregate Result

| engine | previous success | recheck success | delta | elapsed mean sec |
|---|---:|---:|---:|---:|
| legacy-lite | 46/125 | 66/125 | +20 | 66.6 |
| minimal | 45/125 | 69/125 | +24 | 28.9 |

After check-only re-evaluation, minimal is ahead on success_check by 3 runs
while retaining the previous speed advantage. This is a new check-quality
baseline, not a new model run.

## Scenario Result

| scenario | legacy recheck | minimal recheck | delta |
|---|---:|---:|---:|
| fix-css-token-doc | 5/5 | 3/5 | -2 |
| fix-js-date-helper | 5/5 | 1/5 | -4 |
| fix-json-normalizer | 5/5 | 5/5 | +0 |
| fix-python-retry-policy | 0/5 | 5/5 | +5 |
| fix-python-slugify | 4/5 | 2/5 | -2 |
| fix-readme-command | 5/5 | 2/5 | -3 |
| fix-rust-parser-error | 5/5 | 1/5 | -4 |
| fix-shell-safe-clean | 2/5 | 5/5 | +3 |
| long-session-data-report | 0/5 | 2/5 | +2 |
| long-session-large-component | 0/5 | 1/5 | +1 |
| long-session-read-edit | 0/5 | 1/5 | +1 |
| multi-file-docs-and-examples | 0/5 | 2/5 | +2 |
| multi-file-node-package | 0/5 | 5/5 | +5 |
| multi-file-python-package | 0/5 | 2/5 | +2 |
| multi-file-rust-library | 5/5 | 5/5 | +0 |
| new-large-react-kanban | 0/5 | 2/5 | +2 |
| new-markdown-release-notes | 5/5 | 5/5 | +0 |
| new-python-csv-small | 5/5 | 2/5 | -3 |
| new-rust-cli-small | 5/5 | 0/5 | -5 |
| new-typescript-formatter | 5/5 | 4/5 | -1 |
| non-coding-research-brief | 5/5 | 5/5 | +0 |
| non-coding-runbook | 5/5 | 3/5 | -2 |
| scaffold-fastapi-service | 0/5 | 5/5 | +5 |
| scaffold-next-dashboard | 0/5 | 1/5 | +1 |
| scaffold-rust-cli | 0/5 | 0/5 | +0 |

## Task14 Impact

Check-only fixes removed most of the dead-scenario noise:

| scenario | previous shape | recheck outcome |
|---|---|---|
| fix-json-normalizer | 0/5 vs 0/5 | 5/5 vs 5/5 |
| multi-file-rust-library | 0/5 vs 0/5 | 5/5 vs 5/5 |
| new-rust-cli-small | 0/5 vs 0/5 | legacy 5/5, minimal 0/5 |
| new-typescript-formatter | 0/5 vs 0/5 | legacy 5/5, minimal 4/5 |
| long-session-data-report | 0/5 vs 0/5 | legacy 0/5, minimal 2/5 |
| scaffold-fastapi-service | 0/5 vs 0/5 | legacy 0/5, minimal 5/5 |
| scaffold-next-dashboard | 0/5 vs 0/5 | legacy 0/5, minimal 1/5 |
| scaffold-rust-cli | 0/5 vs 0/5 | unchanged 0/5 vs 0/5 |
| new-python-csv-small | minimal false negatives | legacy 5/5, minimal 2/5 |

Only `scaffold-rust-cli` remains a true 0/5 vs 0/5 dead scenario under the
current checks. The other formerly dead cases now have discriminatory signal.

## Next Baseline For Task15

Use these recheck numbers as the baseline before adding
completion-without-write feedback:

- legacy-lite reference: 66/125
- minimal before Task15: 69/125
- Task15 target scenarios: `new-python-csv-small`,
  `fix-rust-parser-error`, `fix-readme-command`, `fix-python-slugify`
- Non-regression watchlist: `multi-file-node-package`,
  `multi-file-rust-library`, `scaffold-fastapi-service`,
  `new-typescript-formatter`, `new-markdown-release-notes`,
  `non-coding-research-brief`

Because this was a post-hoc recheck, Task15 still requires a fresh minimal-only
125-run execution after the new mechanism is merged. Legacy artifacts can reuse
the recheck result above.
