# Capability Frontier

This file records comparable minimal-loop benchmark rows by model. It is not a
leaderboard: rows are only comparable when the benchmark, binary, flags, and
success checks match.

## 2026-06-13 Cycle 3 Seeded Matrix

Benchmark: `minimal-loop-expanded`, 25 scenarios x 5 runs, `minimal` engine,
M001 on, seed enabled, `--max-iterations 12 --no-auto-test --no-precautions
--no-case-memory --bench-no-debug`.

Binary: `target/release/anvil` from
`b32e1bbfc08eb33617c85e15b544b725cc5101d1`, dirty `false`.

Hardware: Mac Studio, Apple M3 Ultra, 28 CPU cores, 256 GB memory, Apple M3
Ultra GPU with 60 cores. Hardware was captured manually; it is not yet recorded
in benchmark metadata.

| model | quantization / tag | root | success | rc=0 | mean elapsed_s | notes |
|---|---|---|---:|---:|---:|---|
| `qwen3.6:27b-coding-nvfp4` | 27B coding, `nvfp4` tag | `.anvil/benchmarks/20260613T014538-362` | 88/125 | 125/125 | 39.7 | Cycle 3 baseline candidate |
| `qwen3:8b` | 8B tag; exact quantization not recorded in bench metadata | `.anvil/benchmarks/20260613T040938-58258` | 16/125 | 124/125 | 32.3 | First 8B frontier row; writes often but fails checks |

## Interpretation

The 27B row confirms that the minimal loop with M001 can exceed the current
legacy-lite reference on this matrix. The 8B row does not establish a usable
default path yet: most failures are not pure no-edit, but artifacts that fail
semantic or completeness checks after a file-changing tool ran.
