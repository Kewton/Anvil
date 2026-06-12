# Benchmark Seed Smoke

Date: 2026-06-13 JST

Purpose: Task24 gate check for benchmark seed wiring before Cycle 3 reruns.

## Preconditions

Revision:

- `97d5c08de16bf29049cd07a3e6307acce2e1b255`

Required merge commits present:

- `#1042` seed support: `b60760f8`
- `#1044` failed-run log preservation: `9446731`
- `#1045` Write parent-directory catalog text: `490e35b`
- `#1046` offline Bash Write hint: `af12a65`
- `#1048` minimal-loop stack integration: `97d5c08`

Release binary:

- Path: `target/release/anvil`
- Build mtime: `2026-06-13 00:23:18 JST`
- Size: `13680656` bytes

Ollama:

- `/api/tags` reachable from the bench environment with elevated local-network
  permission.
- `qwen3.6:27b-coding-nvfp4` present.
- `qwen3:8b` present.

## Smoke Setup

Temporary one-case benchmark, not committed:

- Benchmark name: `seed-smoke-single`
- Case: `seed-smoke-note`
- Engine: `minimal`
- Model: `qwen3.6:27b-coding-nvfp4`
- Runs: `1`
- Flags: `--max-iterations 2 --bench-no-debug --no-precautions
  --no-case-memory --no-auto-test`

The benchmark was run twice with the same benchmark name, case name, PAM variant,
and run index, so the derived seed was identical.

## Results

| root | rc | success_check | elapsed_s | bench_seed | llm events | artifact sha256 |
|---|---:|---|---:|---:|---:|---|
| `20260613T002447-42971` | 1 | `true` / `ok` | 9 | `3325482096` | 7 | `7df81c564c9b790f48531a65e8b3439eddb956f55517f2ddff647aef4e208503` |
| `20260613T002507-48872` | 1 | `true` / `ok` | 8 | `3325482096` | 7 | `d1082d8b038cf6b7cad7a24dddbae9c238e6f13cfbacaf0ac76d0ab26284bde0` |

Both runs recorded:

- `bench_seed: 3325482096`
- `bench_seed_enabled: true`
- `ANVIL_BENCH_SEED=3325482096` in `active_flags`

Both runs produced the same tool sequence:

1. `Write`
2. `Read`

Both runs failed by `rc=1` because `--max-iterations 2` ended after the `Read`
tool call, but the deterministic `success_check` passed because the requested
file existed.

## Difference

The response was not byte-for-byte deterministic.

The generated file shared the title and first bullet, but the last two bullets
differed:

Run 1:

```markdown
- A fixed seed eliminates randomness in model outputs for consistent evaluation.
- Always document the seed value used so others can replicate the experiment.
```

Run 2:

```markdown
- Each benchmark run should log the seed value for traceability.
- Smoke tests validate that seeding does not introduce runtime errors.
```

The first tool-call IDs also differed (`call_ifow86ls` vs `call_txhbit5s`).

## Assessment

The seed is correctly derived, injected, and recorded, but this Ollama/backend
combination did not provide strict byte-for-byte determinism for the same seed.
The observed difference is limited to generated text content; the high-level
control path, tool names, success_check result, and artifact shape matched.

This is acceptable for Task24. The goal of Task19/Task24 is variance reduction,
not a hard guarantee of perfect deterministic replay. Task25 can proceed.
