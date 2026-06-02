# Benchmark Report: bench-root
Generated: 2026-06-02T03:27:36+00:00

## Run Summary

| run | model | case | task_kind | pam | agreement | failure_authority | rc | postcheck | elapsed_s | we_total | page_game | iter_count | error_500 | compacts |
|-----|-----|-----|-----|-----|-----|-----|-----|-----|-----|-----|-----|-----|-----|-----|
| 1 | llama3 | default | coding | default | true_positive | success | 0 | yes | 200 | 3 | null | 3 | null | 1 |
| 2 | llama3 | default | coding | default | false_negative | implementation_bug | 1 | yes | 220 | 3 | null | 3 | null | 1 |
| 1 | qwen3 | default | coding | default | true_positive | success | 0 | yes | 100 | 1 | null | 2 | null | 0 |
| 2 | qwen3 | default | coding | default | true_positive | success | 0 | yes | 110 | 1 | null | 2 | null | 0 |

## Aggregate Statistics

| metric | n | mean | median | min | max |
|-----|-----|-----|-----|-----|-----|
| success_rate | 4 | 75% | - | - | - |
| postcheck_rate | 4 | 100% | 4/4 | - | - |
| elapsed_s ⚠ | 4 | 157.5 | 155 | 100 | 220 |
| we_total ⚠ | 4 | 2.0 | 2 | 1 | 3 |
| page_game_rate | 0 | N/A | - | - | - |
| iter_count | 4 | 2.5 | 2.5 | 2 | 3 |
| error_500 | 0 | N/A | N/A | N/A | N/A |
| compacts | 4 | 0.5 | 0.5 | 0 | 1 |

> ⚠ CV > 0.3 detected for: elapsed_s, we_total

## PAM Summary

| pam_variant | runs | terminal_success | postcheck_success | both_success | true_positive | false_positive | false_negative | true_negative |
|-----|-----|-----|-----|-----|-----|-----|-----|-----|
| default | 4 | 75% (3/4) | 100% (4/4) | 75% (3/4) | 3 | 0 | 1 | 0 |

## Task Kind Summary

| task_kind | runs | terminal_success | postcheck_success | both_success | true_positive | false_positive | false_negative | true_negative |
|-----|-----|-----|-----|-----|-----|-----|-----|-----|
| coding | 4 | 75% (3/4) | 100% (4/4) | 75% (3/4) | 3 | 0 | 1 | 0 |

## PAM By Task Kind

| task_kind | pam_variant | runs | terminal_success | postcheck_success | both_success | true_positive | false_positive | false_negative | true_negative |
|-----|-----|-----|-----|-----|-----|-----|-----|-----|-----|
| coding | default | 4 | 75% (3/4) | 100% (4/4) | 75% (3/4) | 3 | 0 | 1 | 0 |

## Failure Authority Summary

| failure_authority | runs | terminal_success | postcheck_success | both_success | true_positive | false_positive | false_negative | true_negative |
|-----|-----|-----|-----|-----|-----|-----|-----|-----|
| implementation_bug | 1 | 0% (0/1) | 100% (1/1) | 0% (0/1) | 0 | 0 | 1 | 0 |
| success | 3 | 100% (3/3) | 100% (3/3) | 100% (3/3) | 3 | 0 | 0 | 0 |
