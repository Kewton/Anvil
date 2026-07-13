# Next.js Create Capability Band Summary

- Window start: `uat-test0711-bs-003`
- Scanned UAT sets: `12`
- Aggregate.json rows asserted: `77`
- Total run records: `78`
- Record sources: `{'aggregate': 77, 'report': 1}`
- Included denominator after exclusions: `78`
- Excluded infrastructure records: `0`

## Planner Coverage
| Planner | included records |
| --- | --- |
| gemma4:31b-cloud | 4 |
| qwen3.6:27b-coding-nvfp4 | 74 |

## Scenario x Final State
| Scenario | full | partial | incomplete | failed | n | full rate |
| --- | --- | --- | --- | --- | --- | --- |
| Breakout | 5 | 1 | 5 | 6 | 17 | 29% |
| Quiz | 23 | 0 | 2 | 1 | 26 | 88% |
| Space | 3 | 2 | 14 | 16 | 35 | 9% |

## Scenario x Executor
| Scenario | Executor | full | n | full rate |
| --- | --- | --- | --- | --- |
| Breakout | gemma4:31b-cloud | 3 | 6 | 50% n<10 |
| Breakout | qwen3.6:35b-a3b-coding-nvfp4 | 2 | 11 | 18% |
| Quiz | gemma4:31b-cloud | 12 | 14 | 86% |
| Quiz | qwen3.6:35b-a3b-coding-nvfp4 | 11 | 12 | 92% |
| Space | gemma4:31b-cloud | 2 | 8 | 25% n<10 |
| Space | qwen3.6:35b-a3b-coding-nvfp4 | 1 | 27 | 4% |

## Full Run Durations
| Scope | full runs | min | median | max |
| --- | --- | --- | --- | --- |
| all | 31 | 5m02s | 7m02s | 12m53s |
| Breakout | 5 | 5m02s | 7m49s | 12m24s |
| Quiz | 23 | 5m17s | 7m45s | 12m53s |
| Space | 3 | 6m04s | 6m25s | 6m34s |

## Excluded and Unknown Runs
- Excluded infrastructure runs: none
- Unknown scenario records: none

## False-Full Check
- False-full suspects: 0

## Stop-Class Distribution
| Scenario | Stop classes |
| --- | --- |
| Breakout | full=5, no_progress=4, other=1, path_confinement=1, read_only_loop=5, restart_evidence=1 |
| Quiz | full=23, input_state_change=1, no_progress=1, read_only_loop=1 |
| Space | failed=2, full=3, no_progress=3, partial=2, path_confinement=1, read_only_loop=15, restart_evidence=9 |

## Provisional Comparison
| Scenario | Provisional | Measured | Delta | Note |
| --- | --- | --- | --- | --- |
| Quiz | 85% | 88% | +3pp |  |
| Breakout | 30% | 29% | -1pp |  |
| Space | 7% | 9% | +2pp |  |

## Source Sets
- `uat-test0711-bs-003`
- `uat-test0711-bs-004`
- `uat-test0711-bs-005`
- `uat-test0712-bs-001`
- `uat-test0712-bs-001-fix-smoke`
- `uat-test0712-bs-002`
- `uat-test0712-bs-002-rerun-1245`
- `uat-test0712-g-001`
- `uat-test0712-g-002`
- `uat-test0712-gab-001`
- `uat-test0713-28-001`
- `uat-test0713-g-001`
