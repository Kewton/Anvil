Issue #857 Design Note

Scope:
- Keep PAM advisory-only. Do not feed PAM into completion gates or terminal outcome authority.
- Preserve the existing structured memory report as the context adoption log, adding only bounded and additive fields where needed.
- Add an eval-log projection of the turn-level PAM decision impact so A/B runs can compare outcomes without joining against raw LLM event logs.
- Extend the benchmark runner with a small PAM A/B mode that runs the same benchmark prompt suite twice: PAM enabled and PAM disabled.
- Add a mixed prompt suite with coding, docs, data, research, and ops cases.

Design:
- `PamAdvisoryDecision` remains the agent-layer SSOT for context adoption, suppression, and shadow/live impact.
- `EvalRecord` gets an optional `pam_eval` field with bounded categorical values: mode, decision type(s), counts, and explicit advisory/completion-override booleans.
- `scripts/bench.sh --pam-ab` expands each selected model into two variants using the same benchmark YAML and run count. The only intentional environment difference is `ANVIL_PAM_ADVISORY_ENABLED=true|false`.
- Benchmark suite support stays compatible with the current single-`.prompt` YAML format by allowing optional `.cases[]`; each case becomes part of the same prompt suite and is run for each variant.

Risks:
- Full cargo checks may be slow; run focused tests first, then required full checks.
- Existing benchmark summary consumers expect the current TSV header, so add new columns only if tests are updated and values are stable.
