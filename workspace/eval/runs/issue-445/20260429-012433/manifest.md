# Issue 445 Evaluation Manifest

Run ID: `20260429-012433`
Issue: `445` / Epic B - Verification & Temporary Testing
Commit: `28825fa14fed07c68a349223a0bed878b5d4b055`

## Inputs

- Baseline comparison: `workspace/eval/runs/issue-444/20260428-105334/report.md`
- Static logs:
  - `raw/cargo-fmt-check.log`
  - `raw/cargo-clippy.log`
  - `raw/cargo-test.log`
- Practical E2E logs:
  - `raw/e2e-python-no-anvil-qwen36.log`
  - `raw/e2e-python-anvil-qwen36.log`
  - `raw/e2e-python-no-tests-tester-qwen36.log`
  - `raw/e2e-python-anvil-qwen35.log`

## Scenarios

| ID | Purpose | Model |
| --- | --- | --- |
| P1-aggregate | Epic B unit/integration coverage: AnvilScore, AutoTest wiring, temporary tests, Tester Skill, slash commands, sandbox | local cargo tests |
| P0-445-01 | Existing Python bug fix without ANVIL.md, compare against Issue 444 false-negative | `qwen3.6:27b-coding-nvfp4` |
| P0-445-02 | Existing Python bug fix with ANVIL.md preferred `python -m pytest -q` | `qwen3.6:27b-coding-nvfp4` |
| P0-445-03 | No-test Python edit to observe Tester/AnvilScore behavior on missing implementation | `qwen3.6:27b-coding-nvfp4` |
| P0-445-04 | Same ANVIL.md Python bug fix on heavier XML-tool-call model | `qwen3.5:122b` |
