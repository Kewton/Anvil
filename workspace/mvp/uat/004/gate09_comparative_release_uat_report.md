# UAT004-GATE-09 Comparative / Release / UAT Report

作成日: 2026-07-02

## Result

GATE-09 は release pass ではない。比較・release・manual UAT は実施済みで、判定は `open` / release blocked。

Primary evidence:

- `gate09_release/parity_gate_report.json`: schema validation errors `[]`
- `gate09_release/mvp-provider-smoke-live.summary.eval.tsv`: MVP targeted eval failed
- `gate09_release/anvildev-provider-smoke-live.summary.eval.tsv`: same-condition anvildev eval passed
- `gate09_release/source-mvp-runtime-trace-diff.json`: normalized trace compared, same-condition signature matched
- `gate09_release/browser-readiness.json`: browser route evidence passed
- `gate09_release/interaction-evidence.json`: basic interaction evidence passed
- `gate09_release/test0701_005.original-events.jsonl`: original manual TUI failure evidence remains release-blocking
- `gate09_release/recovery-run.events.jsonl`: saved recovery YAML was executed and failed with a concrete failure

## Verification

| check | result |
| --- | --- |
| `cargo build --release --manifest-path mvp/anvilminimal/Cargo.toml` | passed |
| `cargo test --manifest-path mvp/anvilminimal/Cargo.toml` | passed, 446 lib tests plus integration/doc tests |
| `pytest mvp/anvilminimal/tests/eval` | passed, 227 passed / 1 skipped |
| MVP targeted eval | failed with `verify_repair_no_change` |
| anvildev same-condition eval | passed |
| parity gate report validation | passed, `validation_errors=[]` |

## Comparative Eval

Targeted scenario:

- suite: `eval/suites/mvp-provider-smoke.yaml`
- scenario: `write-one-file-small`
- mode: `minimal-loop`
- model profile: `openai-only`
- provider/model: OpenAI `gpt-5.4-mini`
- provider limit / parallel: 1 / 1

| side | binary | result | release status | artifact |
| --- | --- | --- | --- | --- |
| MVP | `mvp/anvilminimal/target/release/anvilminimal` | `success=false`, `failure_kind=verify_repair_no_change` | `failed` | `hello.txt` contained `provider smoke ok.` |
| source/anvildev | `anvildev --engine minimal` | `success=true` | `pass` | `hello.txt` contained exactly `provider smoke ok` |

Classification:

- MVP did not produce a false full success; this preserves correct failure detection.
- The lower success rate is not pure correct failure detection because source/anvildev produced the accepted artifact.
- `parity_gate_report.json` classifies the delta as `release_quality_blocker_detected`, with `correct_failure_detection=false`.

## Trace Gate Outcome

`source-mvp-runtime-trace-diff.json` has `same_condition.status=match`.

| status | gates |
| --- | --- |
| pass | G-S02, G-S12, G-S14 |
| fail / reopened for release | G-S01, G-S03, G-S04, G-S05, G-S06, G-S07, G-S08, G-S09, G-S10, G-S11, G-S13, G-S15, G-S16 |

Important nuance:

- G-S05/G-S06 remain passed for the GATE-06 scoped trace, but the GATE-09 minimal-loop release comparison lacked prompt/phase trace observations. Trace absence is not accepted as pass, so release trace coverage remains open.
- G-S12 has positive browser/interaction evidence, but release pass is still blocked by comparative/TUI/recovery gaps.

## Manual UAT

The original `test0701_005` workspace was copied to `/private/tmp/anvil-uat004-gate09/test0701_005_browser_uat` and served with `env -u NODE_ENV npm run dev` on port 3011.

| probe | result |
| --- | --- |
| route readiness | HTTP 200, route rendered, DOM ready |
| interaction | Playwright clicked `DIFF 1`; canvas count changed 0 -> 1 |
| dev server lifecycle | start, wait, probe, interaction probe, cleanup recorded in `dev-server-events.jsonl` |
| TUI/manual run evidence | original `test0701_005` events remain `tui_command_stop ok=false`; parity gate marks TUI evidence `fail` |

## Recovery Run

The saved recovery YAML from the MVP provider-smoke failure was executed with the release binary and dotenv-loaded OpenAI credentials.

Result:

- exit code: 1
- failure: `phase_scaffold_error`
- concrete cause: generated StepPlan verify command violated shell-control policy
- artifacts saved: follow-up recovery prompt/YAML plus run events/summary

This satisfies the GATE-09 requirement that recovery handoff is not only saved; it is executable evidence with a concrete success/failure result.

## Release Decision

Release gate remains blocked.

Reasons:

- MVP same-condition provider-smoke is below anvildev in accepted artifact quality.
- Runtime trace comparison has reopened release gaps for most gates.
- Original manual TUI failure evidence is still release-blocking.
- Recovery run executes but fails concretely; handoff persistence is not success.

Rollback guard:

- Do not re-allow blank failure kinds.
- Do not re-allow build-only, title-only, static-only, path-only, or recovery-handoff-only success.
- Do not let browser route success alone imply full release pass without interaction, TUI, diagnostics, and comparative gate evidence.
