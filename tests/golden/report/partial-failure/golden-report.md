# Benchmark Report: bench-root
Generated: 2026-06-02T03:27:36+00:00

## Run Summary

| run | model | case | task_kind | pam | rc | postcheck | elapsed_s | we_total | page_game | iter_count | error_500 | compacts |
|-----|-----|-----|-----|-----|-----|-----|-----|-----|-----|-----|-----|-----|
| 1 | qwen3 | default | coding | default | 0 | yes | 100 | 1 | null | 1 | null | 0 |
| 2 | qwen3 | default | coding | default | 3 | N/A | N/A | N/A | N/A | N/A | N/A | N/A |

## Aggregate Statistics

| metric | n | mean | median | min | max |
|-----|-----|-----|-----|-----|-----|
| success_rate | 1 | 100% | - | - | - |
| postcheck_rate | 1 | 100% | 1/1 | - | - |
| elapsed_s | 1 | 100.0 | 100 | 100 | 100 |
| we_total | 1 | 1.0 | 1 | 1 | 1 |
| page_game_rate | 0 | N/A | - | - | - |
| iter_count | 1 | 1.0 | 1 | 1 | 1 |
| error_500 | 0 | N/A | N/A | N/A | N/A |
| compacts | 1 | 0.0 | 0 | 0 | 0 |

## Task Kind Summary

| task_kind | runs | terminal_success | postcheck_success | both_success |
|-----|-----|-----|-----|-----|
| coding | 1 | 100% (1/1) | 100% (1/1) | 100% (1/1) |

## PAM By Task Kind

| task_kind | pam_variant | runs | terminal_success | postcheck_success | both_success |
|-----|-----|-----|-----|-----|-----|
| coding | default | 1 | 100% (1/1) | 100% (1/1) | 100% (1/1) |
