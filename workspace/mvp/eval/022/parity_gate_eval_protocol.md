# Parity Gate Eval Protocol

作成日: 2026-06-29

## 1. 目的

runtime semantics parity の修正を、MVP 単体成功率だけで判断しない。同一条件の `anvildev --engine minimal` 比較、provider probe、targeted eval、failure taxonomy、UAT-equivalent acceptance を gate level に応じて実行する。

## 2. Gate Levels

| Level | When | Required eval |
| --- | --- | --- |
| local | docs/schema/classifier/fixture変更 | fixture pytest, report schema, failure taxonomy dry |
| network | prompt/provider/tool-call変更 | local + provider smoke/probe |
| comparative | runtime semantics変更 | network + MVP/anvildev same-condition comparison |
| release | TUI/UAT/Next.js品質変更 | comparative + browser/interaction acceptance |

## 3. MVP Smoke

```bash
python3 mvp/anvilminimal/scripts/eval-run.py \
  --suite mvp/anvilminimal/eval/suites/mvp-smoke.yaml \
  --model-profile speed-cloud-5x \
  --modes minimal-loop,step-plan,plan-run,ultra-plan-run \
  --runs 1 \
  --parallel 5 \
  --provider-limit 5 \
  --binary mvp/anvilminimal/target/release/anvilminimal \
  --binary-kind anvilminimal
```

Acceptance:

- failure kind blank count is 0 for failures.
- mode success and lifecycle failure counts are written to `parity_gate_report.json`.
- current run root is added to `source_mvp_trace_manifest.md`.

## 4. Source Comparison

```bash
python3 mvp/anvilminimal/scripts/eval-run.py \
  --suite mvp/anvilminimal/eval/suites/mvp-smoke.yaml \
  --model-profile speed-cloud-5x \
  --modes minimal-loop,step-plan,plan-run,ultra-plan-run \
  --runs 1 \
  --parallel 5 \
  --provider-limit 5 \
  --binary anvildev \
  --binary-kind anvildev \
  --engine minimal
```

Notes:

- If the eval runner injects `--engine minimal` for `binary-kind anvildev`, command duplication must be avoided.
- Same suite/model profile/modes/run count is required.
- Different provider availability must be recorded as trace gap.

Threshold:

| Condition | Result |
| --- | --- |
| MVP success rate lower than anvildev by >= 5pp | warning |
| MVP success rate lower than anvildev by >= 10pp | failure |
| same lifecycle stage failure grows | warning |
| false positive reduction lowers success but improves acceptance correctness | can be `intentionally_different` with evidence |

## 5. Provider Smoke / Probe

Provider smoke:

```bash
python3 mvp/anvilminimal/scripts/eval-run.py \
  --suite mvp/anvilminimal/eval/suites/mvp-provider-smoke.yaml \
  --model-profile speed-cloud-5x \
  --modes minimal-loop,step-plan,plan-run,ultra-plan-run \
  --runs 1 \
  --parallel 5 \
  --provider-limit 5 \
  --binary mvp/anvilminimal/target/release/anvilminimal \
  --binary-kind anvilminimal
```

Live provider probe:

```bash
ANVIL_PROVIDER_PROBE=1 \
ANVIL_PROVIDER_PROBE_OUT=/private/tmp/anvil-provider-probe-gate.jsonl \
cargo test --manifest-path mvp/anvilminimal/Cargo.toml --test live_provider provider_probe -- --nocapture
```

Acceptance:

- API key missing is skip, not failure.
- Probe records provider, model, tool args shape, schema result, error kind.
- Provider-specific failure must not become blank `failure_kind`.

## 6. Targeted Eval Set

| Target | Purpose |
| --- | --- |
| `targeted-plan-run-bridge` | good StepPlan -> step runtime -> acceptance connection |
| `targeted-ultra-nextjs` | phase generation/context/recovery/final acceptance |
| `targeted-minimal-repair` | repair target/no-progress/finalization |
| `targeted-dependency-lifecycle` | manifest/setup/build rerun |
| `targeted-failure-taxonomy` | known stderr/event -> failure kind |
| `targeted-uat-equivalent` | build-only/title-only/capability evidence |

## 7. Report Requirements

After each comparative or release run:

- update `source_mvp_trace_manifest.md` or generate a run-specific manifest.
- update `parity_gate_report.json`.
- attach summary paths.
- include source/MVP success delta and lifecycle stage delta.
- record whether lower success is regression or correct failure detection.

## 8. Speed Rules

- Local LLM unused cloud eval should use `--parallel 5 --provider-limit 5`.
- Local LLM eval should not run in high parallel by default.
- Provider probe is opt-in and should stay small.
