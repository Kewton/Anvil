# Benchmark Report: bench-root
Generated: 2026-06-02T03:27:35+00:00

## Run Summary

| run | model | case | task_kind | pam | agreement | failure_authority | rc | postcheck | elapsed_s | we_total | page_game | iter_count | error_500 | compacts |
|-----|-----|-----|-----|-----|-----|-----|-----|-----|-----|-----|-----|-----|-----|-----|
| 1 | qwen3 | default | coding | default | true_positive | success | 0 | yes | 120 | 2 | null | 2 | null | 1 |
| 2 | qwen3 | default | coding | default | true_positive | success | 0 | yes | 125 | 2 | null | 2 | null | 1 |
| 3 | qwen3 | default | coding | default | true_positive | success | 0 | yes | 130 | 2 | null | 2 | null | 1 |

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

## PAM Summary

| pam_variant | runs | terminal_success | postcheck_success | both_success | true_positive | false_positive | false_negative | true_negative |
|-----|-----|-----|-----|-----|-----|-----|-----|-----|
| default | 3 | 100% (3/3) | 100% (3/3) | 100% (3/3) | 3 | 0 | 0 | 0 |

## Task Kind Summary

| task_kind | runs | terminal_success | postcheck_success | both_success | true_positive | false_positive | false_negative | true_negative |
|-----|-----|-----|-----|-----|-----|-----|-----|-----|
| coding | 3 | 100% (3/3) | 100% (3/3) | 100% (3/3) | 3 | 0 | 0 | 0 |

## PAM By Task Kind

| task_kind | pam_variant | runs | terminal_success | postcheck_success | both_success | true_positive | false_positive | false_negative | true_negative |
|-----|-----|-----|-----|-----|-----|-----|-----|-----|-----|
| coding | default | 3 | 100% (3/3) | 100% (3/3) | 100% (3/3) | 3 | 0 | 0 | 0 |

## Failure Authority Summary

| failure_authority | runs | terminal_success | postcheck_success | both_success | true_positive | false_positive | false_negative | true_negative |
|-----|-----|-----|-----|-----|-----|-----|-----|-----|
| success | 3 | 100% (3/3) | 100% (3/3) | 100% (3/3) | 3 | 0 | 0 | 0 |

## Objective Matrix

| task_kind | deliverable_kind | evidence_kind | generic_terminal_state | recovery_job_kind | runs | terminal_success | postcheck_success | both_success | true_positive | false_positive | false_negative | true_negative |
|-----|-----|-----|-----|-----|-----|-----|-----|-----|-----|-----|-----|-----|
| coding | source_files | test_run | completed | none | 3 | 100% (3/3) | 100% (3/3) | 100% (3/3) | 3 | 0 | 0 | 0 |

## Terminal State Summary

| generic_terminal_state | runs | terminal_success | postcheck_success | both_success | true_positive | false_positive | false_negative | true_negative |
|-----|-----|-----|-----|-----|-----|-----|-----|-----|
| completed | 3 | 100% (3/3) | 100% (3/3) | 100% (3/3) | 3 | 0 | 0 | 0 |

## Recovery Job Summary

| recovery_job_kind | runs | terminal_success | postcheck_success | both_success | true_positive | false_positive | false_negative | true_negative |
|-----|-----|-----|-----|-----|-----|-----|-----|-----|
| none | 3 | 100% (3/3) | 100% (3/3) | 100% (3/3) | 3 | 0 | 0 | 0 |

## Transition Metrics

| metric | value |
|-----|-----|
| runs | 3 |
| missing_evidence | 0 |
| missing_deliverable | 0 |
| evidence_failed | 0 |
| recovery_exhausted | 0 |
| wrong_target_repair | 0 |
| same_diagnostic_repeated | 0 |
| tool_protocol_failure | 0 |
| runner_present_but_failed | 0 |
| repair_should_target_test_or_setup | 0 |
| evidence_runner_executed | 0/3 (0%) |
| deterministic_operator_hit | 0/3 (0%) |

| failure_class | count |
|-----|-----|
| none | 3 |

## Lifecycle Metrics

| metric | value |
|-----|-----|
| runs | 3 |
| first_pass_scaffold_complete | 3/3 (100%) |
| first_evidence_runnable | 3/3 (100%) |
| binding_failure_count | 0 |
| repair_loop_reached | 0 |
| repair_to_pass_conversion | 0/0 (N/A) |
| same_failure_repeated_count | 0 |
| strategy_switch_count | 0 |
| operator_missing_count | 0 |

### Lifecycle Metrics By Task Kind

| task_kind | runs | first_pass_scaffold_complete | first_evidence_runnable | binding_failure_count | repair_loop_reached | repair_to_pass_conversion | same_failure_repeated_count | strategy_switch_count | operator_missing_count |
|-----|-----|-----|-----|-----|-----|-----|-----|-----|-----|
| coding | 3 | 3/3 (100%) | 3/3 (100%) | 0 | 0 | 0/0 (N/A) | 0 | 0 | 0 |
