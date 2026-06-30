# Parity Gate CI / Preflight Contract

作成日: 2026-06-29

## 1. 目的

runtime semantics parity gate を手作業の文書に留めず、pytest / cargo / eval preflight で検査可能にする。

## 2. Local Gate Tests

通常 PR / local 開発で network なしに実行できる test:

```bash
python3 -m pytest mvp/anvilminimal/tests/eval
cargo test --manifest-path mvp/anvilminimal/Cargo.toml
```

追加予定 test:

| Test | Responsibility |
| --- | --- |
| `test_runtime_semantics_gate_matrix.py` | G-S01〜G-S16 row/column/status completeness |
| `test_parity_gate_fixture_coverage.py` | each gate has positive/negative fixture |
| `test_failure_kind_coverage_gate.py` | blank failure kind rejection |
| `test_source_mvp_trace_manifest.py` | required trace fields and redaction |
| `test_uat_acceptance_contract.py` | build-only/title-only/capability evidence contract |
| `test_parity_gate_report_schema.py` | JSON schema, gate partition |
| `test_parity_gate_comparison_threshold.py` | warn/fail threshold and intentional difference |
| `test_trace_redaction_contract.py` | API keys and secret-like values are not persisted |

## 3. Network / Comparative Gates

These are opt-in and not required for ordinary unit test:

| Gate | Command family |
| --- | --- |
| provider probe | `ANVIL_PROVIDER_PROBE=1 cargo test ... --test live_provider` |
| MVP cloud eval | `eval-run.py --binary ...anvilminimal --parallel 5` |
| anvildev comparison | `eval-run.py --binary anvildev --binary-kind anvildev` |
| release/UAT | browser readiness and interaction checks |

## 4. eval-preflight Integration

`scripts/eval-preflight.py` or equivalent should:

- read `parity_gate_report.json`
- verify report schema
- reject blank failure kind count for required gate levels
- warn when source comparison is missing
- fail when requested gate level is `comparative` and anvildev comparison is missing
- fail release gate when UAT/browser evidence is missing
- print required next command rather than silently passing

## 5. Failure vs Warning Boundary

| Condition | local | network | comparative | release |
| --- | --- | --- | --- | --- |
| missing source trace | warn | warn | fail | fail |
| failure_kind blank | fail | fail | fail | fail |
| provider probe missing | pass | fail if provider change | fail if provider change | fail if provider change |
| anvildev comparison missing | pass | warn | fail | fail |
| browser evidence missing | pass | pass | warn | fail |
| fixture coverage missing | fail | fail | fail | fail |

## 6. CI Rollout

Initial CI should enforce:

- report schema test
- matrix completeness test
- fixture coverage test
- failure taxonomy fixture test

CI should not require:

- API keys
- network
- live provider calls
- browser installation
- `anvildev` availability

Those remain opt-in release/comparative gates until the project has stable infrastructure for them.
