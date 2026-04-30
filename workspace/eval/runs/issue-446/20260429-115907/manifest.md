# Issue 446 Evaluation Manifest

Run ID: `20260429-115907`
Issue: `446` / Epic C - Case Memory & CBR
Commit: `9b38a14dcd98d9ff430327b2709e4afd8e19fb0e`

## Inputs

- Evaluation plan: `workspace/eval/issue_444_449_eval_plan.md`
- Epic C summary: `workspace/orchestration/runs/2026-04-29/summary.md`
- Static logs:
  - `raw/cargo-fmt-check.log`
  - `raw/cargo-clippy.log`
  - `raw/cargo-test.log`
- Practical E2E logs:
  - `raw/e2e-case-doc-qwen36-run1.log`
  - `raw/e2e-case-doc-qwen36-run2.log`
  - `raw/e2e-case-doc-qwen36-run3-exact.log`
  - `raw/e2e-case-doc-qwen35-retrieval.log`
  - `raw/e2e-secret-case-qwen36.log`
  - `raw/e2e-secret-case-qwen36-run2.log`
  - `raw/e2e-anti-pattern-qwen36-run1.log`
  - `raw/e2e-anti-pattern-qwen36-run2.log`
  - `raw/e2e-anti-pattern-qwen36-run3-retrieval.log`

## Scenarios

| ID | Purpose | Model |
| --- | --- | --- |
| P1-aggregate | Epic C unit/integration coverage: CaseRecord, CaseRetrieval, AntiPattern | local cargo tests |
| P3-01a | Successful doc edit creates CaseRecord | `qwen3.6:27b-coding-nvfp4` |
| P3-01b | Similar doc edit sees candidate but below threshold | `qwen3.6:27b-coding-nvfp4` |
| P3-01c | Near-identical doc edit retrieves and injects Relevant Local Cases | `qwen3.6:27b-coding-nvfp4` |
| P3-01d | qwen3.5 receives Relevant Local Cases and completes faster | `qwen3.5:122b` |
| P3-02 | Repeated unsafe Bash failure creates and retrieves AntiPattern | `qwen3.6:27b-coding-nvfp4` |
| P3-03 | Secret-bearing task checks CaseRecord/raw-session behavior | `qwen3.6:27b-coding-nvfp4` |
