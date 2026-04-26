# UAT Protocol 001

Date: 2026-04-26

## Scope

Implemented and verified:

- ExecutionProtocol abstraction
- AutoTestRunner
- Bash classifier strengthening
- deterministic fallback boundary module
- JSON-shaped mode classifier
- protocol-specific success checks
- SvelteKit native deterministic framework support as the first unsupported-framework expansion

Requested files `workspace/v0.1.0/uatspace/v3/uatplan_v3.md`, `v4/uatplan_v4.md`, and `v5/uatplan_v5.md` were not present. The available scenario files were used:

- `workspace/v0.1.0/uatspace/v3/uat_scenarios_v3.md`
- `workspace/v0.1.0/uatspace/v4/uat_scenarios_v4.md`
- `workspace/v0.1.0/uatspace/v5/uat_scenarios_v5.md`

## Four-Instruction Stability Set

The stability set used one turn with four instructions:

1. Answer-only README summary and design-issue analysis with no file changes.
2. Python CSV CLI with sample CSV and usage docs.
3. README documentation creation.
4. SvelteKit reservation/form-style app with validation, persistence, progress, accessible status, and port 3012.

## Results

| Model | Pass | AnswerOnly | Python CLI | Docs | SvelteKit App |
| --- | --- | --- | --- | --- | --- |
| qwen3.5:122b | 1 | PASS, iter 2/50, edited 0 | PASS, iter 1/50 | PASS, iter 1/50 | PASS, iter 1/50 |
| qwen3.5:122b | 2 | PASS, iter 2/50, edited 0 | PASS, iter 1/50 | PASS, iter 1/50 | PASS, iter 1/50 |
| qwen3.5:122b | 3 | PASS, iter 2/50, edited 0 | PASS, iter 1/50 | PASS, iter 1/50 | PASS, iter 1/50 |
| qwen3.6:27b-coding-nvfp4 | 1 | PASS, iter 8/50, edited 0 | PASS, iter 1/50 | PASS, iter 1/50 | PASS, iter 1/50 |
| qwen3.6:27b-coding-nvfp4 | 2 | PASS, iter 4/50, edited 0 | PASS, iter 1/50 | PASS, iter 1/50 | PASS, iter 1/50 |
| qwen3.6:27b-coding-nvfp4 | 3 | PASS, iter 4/50, edited 0 | PASS, iter 1/50 | PASS, iter 1/50 | PASS, iter 1/50 |

Verification:

- `python3 analyze_csv.py sample.csv` printed `Grand Total,4950.00` in every Python CLI pass.
- `npm test` printed `smoke ok: src/routes/+page.svelte; quality layers ok: L1 structure, L2 runnable scripts, L3 interaction, L4 functional primitives` in every SvelteKit pass.
- No pass reached 50 iterations.
- AnswerOnly did not write files in any pass.

## Incident During Test Harness

Two harness mistakes were corrected before counting stable passes:

- Initial batch run executed from the repository root instead of each UAT directory.
- A later batch redirected `run.log` inside the tested work directory, making an empty-workspace scenario non-empty and suppressing deterministic fallback.

These were test harness issues, not counted as product pass/fail results.

