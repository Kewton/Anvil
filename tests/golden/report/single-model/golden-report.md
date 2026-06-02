# Benchmark Report: bench-root
Generated: 2026-06-02T03:27:35+00:00

## Run Summary

| run | model | case | task_kind | pam | rc | postcheck | elapsed_s | we_total | page_game | iter_count | error_500 | compacts |
|-----|-----|-----|-----|-----|-----|-----|-----|-----|-----|-----|-----|-----|
| 1 | qwen3 | default | coding | default | 0 | yes | 120 | 2 | null | 2 | null | 1 |
| 2 | qwen3 | default | coding | default | 0 | yes | 125 | 2 | null | 2 | null | 1 |
| 3 | qwen3 | default | coding | default | 0 | yes | 130 | 2 | null | 2 | null | 1 |

## Aggregate Statistics

| metric | n | mean | median | min | max |
|-----|-----|-----|-----|-----|-----|
| success_rate | 3 | 100% | - | - | - |
| postcheck_rate | 3 | 100% | 3/3 | - | - |
| elapsed_s | 3 | 125.0 | 125 | 120 | 130 |
| we_total | 3 | 2.0 | 2 | 2 | 2 |
| page_game_rate | 0 | N/A | - | - | - |
| iter_count | 3 | 2.0 | 2 | 2 | 2 |
| error_500 | 0 | N/A | N/A | N/A | N/A |
| compacts | 3 | 1.0 | 1 | 1 | 1 |

## Task Kind Summary

| task_kind | runs | terminal_success | postcheck_success | both_success |
|-----|-----|-----|-----|-----|
| coding | 3 | 100% (3/3) | 100% (3/3) | 100% (3/3) |

## PAM By Task Kind

| task_kind | pam_variant | runs | terminal_success | postcheck_success | both_success |
|-----|-----|-----|-----|-----|-----|
| coding | default | 3 | 100% (3/3) | 100% (3/3) | 100% (3/3) |
