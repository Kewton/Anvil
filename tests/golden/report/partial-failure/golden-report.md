# Benchmark Report: bench-root
Generated: 2026-06-02T03:27:36+00:00

## Run Summary

| run | model | case | task_kind | pam | agreement | failure_authority | rc | postcheck | elapsed_s | we_total | page_game | iter_count | error_500 | compacts |
|-----|-----|-----|-----|-----|-----|-----|-----|-----|-----|-----|-----|-----|-----|-----|
| 1 | qwen3 | default | coding | default | true_positive | success | 0 | yes | 100 | 1 | null | 1 | null | 0 |
| 2 | qwen3 | default | coding | default | unknown | unknown | 3 | N/A | N/A | N/A | N/A | N/A | N/A | N/A |

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

## PAM Summary

| pam_variant | runs | terminal_success | postcheck_success | both_success | true_positive | false_positive | false_negative | true_negative |
|-----|-----|-----|-----|-----|-----|-----|-----|-----|
| default | 1 | 100% (1/1) | 100% (1/1) | 100% (1/1) | 1 | 0 | 0 | 0 |

## Task Kind Summary

| task_kind | runs | terminal_success | postcheck_success | both_success | true_positive | false_positive | false_negative | true_negative |
|-----|-----|-----|-----|-----|-----|-----|-----|-----|
| coding | 1 | 100% (1/1) | 100% (1/1) | 100% (1/1) | 1 | 0 | 0 | 0 |

## PAM By Task Kind

| task_kind | pam_variant | runs | terminal_success | postcheck_success | both_success | true_positive | false_positive | false_negative | true_negative |
|-----|-----|-----|-----|-----|-----|-----|-----|-----|-----|
| coding | default | 1 | 100% (1/1) | 100% (1/1) | 100% (1/1) | 1 | 0 | 0 | 0 |

## Failure Authority Summary

| failure_authority | runs | terminal_success | postcheck_success | both_success | true_positive | false_positive | false_negative | true_negative |
|-----|-----|-----|-----|-----|-----|-----|-----|-----|
| success | 1 | 100% (1/1) | 100% (1/1) | 100% (1/1) | 1 | 0 | 0 | 0 |

## Objective Matrix

| task_kind | deliverable_kind | evidence_kind | generic_terminal_state | recovery_job_kind | runs | terminal_success | postcheck_success | both_success | true_positive | false_positive | false_negative | true_negative |
|-----|-----|-----|-----|-----|-----|-----|-----|-----|-----|-----|-----|-----|
| coding | source_files | test_run | completed | none | 1 | 100% (1/1) | 100% (1/1) | 100% (1/1) | 1 | 0 | 0 | 0 |

## Terminal State Summary

| generic_terminal_state | runs | terminal_success | postcheck_success | both_success | true_positive | false_positive | false_negative | true_negative |
|-----|-----|-----|-----|-----|-----|-----|-----|-----|
| completed | 1 | 100% (1/1) | 100% (1/1) | 100% (1/1) | 1 | 0 | 0 | 0 |

## Recovery Job Summary

| recovery_job_kind | runs | terminal_success | postcheck_success | both_success | true_positive | false_positive | false_negative | true_negative |
|-----|-----|-----|-----|-----|-----|-----|-----|-----|
| none | 1 | 100% (1/1) | 100% (1/1) | 100% (1/1) | 1 | 0 | 0 | 0 |
