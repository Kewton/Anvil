# Benchmark Comparison
A: bench-root-a
B: bench-root-b
Generated: 2026-06-02T03:27:36+00:00

## Common Models

### qwen3

| metric | A (n=2) | B (n=2) | diff | diff% |
|-----|-----|-----|-----|-----|
| success_rate | 100% | 100% | - | - |
| postcheck_rate | 100% (2/2) | 100% (2/2) | +0.0pt | - |
| mean elapsed_s | 105.0 | 125.0 | +20.0 | +19.0% |
| mean we_total | 1.0 | 1.0 | +0.0 | +0.0% |
| page_game_rate | N/A | N/A | - | - |
| mean iter_count | 1.0 | 1.0 | +0.0 | +0.0% |
| mean error_500 | N/A | N/A | - | - |
| mean compacts | 0.0 | 0.0 | +0.0 | - |

## A-only Models

- llama3

## B-only Models

(none)

## Tool Call Comparison

| tool | A total | B total | diff |
|-----|-----|-----|-----|
| Write | 4 | 2 | -2 |
