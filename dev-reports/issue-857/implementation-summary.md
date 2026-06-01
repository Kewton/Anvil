Issue #857 Implementation Summary

Implemented:
- Added `EvalRecord.pam_eval` as an optional additive field.
- Added `PamEvalSummary` with PAM mode, decision type(s), injected/suppressed/counterfactual counts, and explicit advisory-only/completion-override audit flags.
- Projected `PamAdvisoryDecision` into `pam_eval` at eval-log write time without changing completion judgement or terminal outcome logic.
- Kept per-context PAM adoption and suppression reasons in the existing structured `agent.memory.report.pam_decision` payload.
- Extended `scripts/bench.sh` with:
  - `.cases[]` benchmark suite support.
  - `--pam-ab` expansion into `pam_on` and `pam_off` variants.
  - `case` and `pam_variant` columns in `summary.tsv`.
  - copying `logs/eval.jsonl` into each run directory.
- Added `benchmarks/pam-ab-general.yaml` with coding, docs, data, research, and ops cases.
- Updated benchmark docs and the shell smoke test for the A/B suite path.

Completion authority:
- PAM remains advisory. The new eval field records `advisory_only=true` and `completion_judgement_override=false`; no completion gate reads PAM.
