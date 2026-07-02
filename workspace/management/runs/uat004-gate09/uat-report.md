# UAT Report: uat004-gate09

Date: 2026-07-02

## Scope

UAT004-GATE-09 comparative / release / manual UAT gate for MVP runtime semantics. This run compares MVP release binary behavior with `anvildev --engine minimal`, validates release evidence, and checks test0701_005-equivalent browser/manual evidence.

## Result

Status: failed / open for release.

This is not a release pass. The gate completed and produced evidence, but MVP is below anvildev for the same provider-smoke condition and the release parity report keeps multiple gates open.

## Automated Checks

| check | result |
| --- | --- |
| release build | passed |
| cargo test | passed, 446 lib tests plus integration/doc tests |
| eval pytest | passed, 227 passed / 1 skipped |
| MVP targeted eval | failed with `verify_repair_no_change` |
| anvildev targeted eval | passed |
| parity gate validation | passed, no schema validation errors |

## Manual / Release Evidence

| evidence | result |
| --- | --- |
| browser route | passed, HTTP 200 and route rendered |
| interaction | passed, clicking `DIFF 1` changed visible state to canvas |
| TUI/manual run | failed, original `test0701_005` events contain `tui_command_stop ok=false` |
| recovery run | failed concretely with `phase_scaffold_error`; recovery handoff was executed, not merely saved |

## Key Artifacts

- `workspace/mvp/uat/004/gate09_comparative_release_uat_report.md`
- `workspace/mvp/uat/004/gate09_release/parity_gate_report.json`
- `workspace/mvp/uat/004/gate09_release/source-mvp-runtime-trace-diff.json`
- `workspace/mvp/uat/004/gate09_release/browser-readiness.json`
- `workspace/mvp/uat/004/gate09_release/interaction-evidence.json`
- `workspace/mvp/uat/004/gate09_release/recovery-run.summary.md`
- `workspace/mvp/uat/004/source_first_gate_status.md`
- `workspace/mvp/uat/004/source_mvp_trace_manifest.md`

## Decision

Do not promote to release. Fix or intentionally document the MVP/source differences before re-running GATE-09:

- MVP provider-smoke artifact quality gap: MVP wrote `provider smoke ok.` while anvildev wrote exact accepted content.
- G-S05/G-S06 release trace coverage is missing for the GATE-09 scenario even though GATE-06 scoped trace passed.
- G-S16 remains blocked by manual TUI failure evidence.
- Recovery run must end in success or a planned concrete fix, not only saved handoff.
