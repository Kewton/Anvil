# Benchmark Comparison
A: bench-root-a
B: bench-root-b
Generated: 2026-06-02T03:27:37+00:00

## Common Models

### qwen3

> note: nA=3 != nB=2

| metric | A (n=3) | B (n=2) | diff | diff% |
|-----|-----|-----|-----|-----|
| success_rate | 100% | 100% | - | - |
| postcheck_rate | 100% (3/3) | 100% (2/2) | +0.0pt | - |
| mean elapsed_s | 110.0 | 145.0 | +35.0 | +31.8% |
| mean we_total | 1.0 | 1.0 | +0.0 | +0.0% |
| page_game_rate | N/A | N/A | - | - |
| mean iter_count | 1.0 | 1.0 | +0.0 | +0.0% |
| mean error_500 | N/A | N/A | - | - |
| mean compacts | 0.0 | 0.0 | +0.0 | - |

## A-only Models

(none)

## B-only Models

(none)

## Tool Call Comparison

| tool | A total | B total | diff |
|-----|-----|-----|-----|
| Write | 3 | 2 | -1 |
