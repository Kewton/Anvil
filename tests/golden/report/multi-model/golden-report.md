# Benchmark Report: bench-root
Generated: 2026-04-20T07:06:09+00:00

## Run Summary

| run | model | rc | elapsed_s | we_total | page_game | iter_count | error_500 | compacts |
|-----|-----|-----|-----|-----|-----|-----|-----|-----|
| 1 | llama3 | 0 | 200 | 3 | null | 3 | null | 1 |
| 2 | llama3 | 1 | 220 | 3 | null | 3 | null | 1 |
| 1 | qwen3 | 0 | 100 | 1 | null | 2 | null | 0 |
| 2 | qwen3 | 0 | 110 | 1 | null | 2 | null | 0 |

## Aggregate Statistics

| metric | n | mean | median | min | max |
|-----|-----|-----|-----|-----|-----|
| success_rate | 4 | 75% | - | - | - |
| elapsed_s ⚠ | 4 | 157.5 | 155 | 100 | 220 |
| we_total ⚠ | 4 | 2.0 | 2 | 1 | 3 |
| page_game | 0 | N/A | N/A | N/A | N/A |
| iter_count | 4 | 2.5 | 2.5 | 2 | 3 |
| error_500 | 0 | N/A | N/A | N/A | N/A |
| compacts | 4 | 0.5 | 0.5 | 0 | 1 |

> ⚠ CV > 0.3 detected for: elapsed_s, we_total
