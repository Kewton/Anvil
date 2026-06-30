# Parity Gate Review

作成日: 2026-06-29

## 1. Review Scope

対象:

- `runtime_semantics_parity_gate_plan.md`
- `runtime_semantics_gate_matrix.md`
- `source_mvp_trace_manifest.md`
- `parity_gate_fixture_plan.md`
- `parity_gate_failure_taxonomy.md`
- `parity_gate_eval_protocol.md`
- `parity_gate_uat_acceptance.md`
- `parity_gate_ci_contract.md`
- `parity_gate_rollout_plan.md`
- `parity_gate_trace_schema.md`
- `parity_gate_report_schema.md`
- `parity_gate_report.json`

## 2. Review Findings

| 観点 | 判定 | 内容 | 対応 |
| --- | --- | --- | --- |
| 実行意味論ベースか | pass | G-S01〜G-S16 を task contract -> plan -> prompt -> tool -> verify -> repair -> acceptance -> diagnostics で整理 | matrix に lifecycle stage と source/MVP refs を入れた |
| source trace なしで完了扱いしていないか | pass | 最新 same-condition anvildev trace は missing と明記 | 全体を pass にせず partial/fail とした |
| MVP の小さい API 境界を壊していないか | pass | full TaskContract/RepairJob 移植を前提にしていない | CompletionContract / RuntimeAcceptanceReport / RepairTarget への写像を前提化 |
| eval 過適応になっていないか | pass | Space Invaders 固有ではなく interactive game capability contract とした | fixture naming rule に scenario id 禁止を追加 |
| UAT-equivalent acceptance が成果物品質を見ているか | pass | build-only/title-only を failure、browser absence を partial とした | `parity_gate_uat_acceptance.md` に明記 |
| provider 固有挙動を fixture だけで済ませていないか | pass | provider/prompt変更は network gate 以上 | provider probe metadata gate を追加 |
| failure kind 空欄を許していないか | pass | blank failure kind count を gate failure とした | current known blank count 15 を report に記録 |
| anvildev 比較が必須か | partial | protocol/threshold は定義済みだが最新 source trace は未取得 | `source-current-same-condition` を missing として report errors に残す |
| CI 実装まで進んでいるか | partial | test 名と contract は定義済み、コード実装は未実施 | next action として pytest 実装が必要 |
| manual TUI trace が gate 化されているか | partial | UAT trace 要求は定義済みだが最新証跡未取得 | G-S16 を partial に固定 |

## 3. Remaining Gate Failures

| Gate | Reason |
| --- | --- |
| G-S08 | verify command policy / deterministic verify failure taxonomy is not fully clean |
| G-S09 | dependency/setup/build lifecycle still fails in Next.js path |
| G-S10 | repair targeting still shows no-change / target-not-followed risk |
| G-S12 | final acceptance does not yet have release-grade browser/interaction evidence |
| G-S14 | improved to partial by 022-1: latest known summaries normalize to blank failure kind count 0, but a fresh v3 summary and normalized trace diff are still needed for full pass |

## 4. Defer Decisions

| Item | Defer reason | Required next action |
| --- | --- | --- |
| latest `anvildev` same-condition run | user asked Phase execution under docs/gate; running full cloud comparison is separate cost | run comparative eval and update manifest/report |
| pytest implementation | this Phase creates the gate contract; implementation should be a follow-up code change | add tests listed in CI contract |
| browser oracle | requires runtime/browser infrastructure | implement release gate runner or browser-backed acceptance |
| normalized event sequence generator | not yet implemented | add event normalization script and attach to report |

## 5. Final Review Judgment

022 の成果物は、過去の「機能一覧を見て移植したつもりになる」問題を防ぐ方向に整理されている。ただし、この時点では gate 実装そのものは未完であり、`parity_gate_report.json` も現状を `partial/fail` として示す。したがって、次の runtime semantics 修正はこの gate を参照し、対象 stage の source trace / MVP trace / fixture / eval / UAT 証跡を揃えない限り完了扱いにしない。
