# anvilminimal eval

This directory contains the MVP eval harness for:

- minimal-loop
- step-plan
- plan-run
- ultra-plan-run
- ultra-step-run diagnostic replay

The harness is intentionally outside the Rust runtime. Python scripts under
`scripts/` read YAML suites, expand model matrices, run `anvilminimal`, execute
deterministic postchecks, score plans, and write comparable artifacts.

## Output

Each run creates one run root:

```text
workspace/eval-artifacts/anvilminimal-mvp/<timestamp>/
  preflight.json
  matrix.json
  summary.eval.tsv
  events.jsonl
  warnings.jsonl
  report.md
  runs/<run_id>/
    command.txt
    meta.json
    stdout.log
    stderr.log
    workdir/
    plans/
    snapshots/
    postcheck/
```

`summary.eval.tsv` is the comparison table. `events.jsonl` is the detailed
evidence stream. `report.md` is the human-readable aggregate.

## Preflight

```bash
python3 scripts/eval-preflight.py \
  --suite eval/suites/mvp-smoke.yaml \
  --model-profile speed-cloud
```

Use `--offline-ok` when checking only the local script/suite wiring.

```bash
python3 scripts/eval-preflight.py \
  --suite eval/suites/mvp-smoke.yaml \
  --model-profile speed-cloud \
  --offline-ok
```

## Dry Run

Dry run never calls LLM providers or Ollama.

```bash
python3 scripts/eval-run.py \
  --suite eval/suites/mvp-smoke.yaml \
  --model-profile speed-cloud \
  --modes minimal-loop,step-plan,plan-run,ultra-plan-run \
  --runs 1 \
  --parallel 4 \
  --dry-run
```

## Speed Cloud Eval

This excludes local LLMs and runs cloud-only rows with provider limits.

```bash
export OPENAI_API_KEY=...
export GEMINI_API_KEY=...

python3 scripts/eval-preflight.py \
  --suite eval/suites/mvp-provider-smoke.yaml \
  --model-profile speed-cloud \
  --live-provider-smoke all

python3 scripts/eval-run.py \
  --suite eval/suites/mvp-provider-smoke.yaml \
  --model-profile speed-cloud \
  --modes minimal-loop,plan-run,ultra-plan-run \
  --runs 1 \
  --parallel 4 \
  --timeout-sec 600

python3 scripts/eval-run.py \
  --suite eval/suites/mvp-smoke.yaml \
  --model-profile speed-cloud \
  --modes minimal-loop,step-plan,plan-run,ultra-plan-run \
  --runs 1 \
  --parallel 4 \
  --provider-smoke-summary workspace/eval-artifacts/anvilminimal-mvp/<provider-smoke>/summary.eval.tsv \
  --timeout-sec 1800
```

Use `--allow-provider-smoke-failure` only for diagnostic runs where the provider
smoke failure is the subject of the investigation. Normal acceptance should keep
the provider smoke gate enabled.

## Local Eval

Local LLM rows are serial by default.

```bash
python3 scripts/eval-run.py \
  --suite eval/suites/mvp-smoke.yaml \
  --model-profile local-only \
  --modes minimal-loop,step-plan,plan-run,ultra-plan-run \
  --runs 1 \
  --parallel 1 \
  --timeout-sec 3600
```

## Full Matrix

```bash
python3 scripts/eval-run.py \
  --suite eval/suites/mvp-full.yaml \
  --model-profile full \
  --modes minimal-loop,step-plan,plan-run,ultra-plan-run,ultra-step-run \
  --runs 3 \
  --parallel 4 \
  --timeout-sec 3600
```

`ultra-step-run` is diagnostic replay. If phase snapshots are unavailable, rows
are written as `diagnostic_skipped` and are not mixed into success rate.

## Plan Scoring

```bash
python3 scripts/eval-score-plan.py \
  --plan eval/fixtures/plans/good-step-plan.yaml \
  --scenario-id nextjs-space-invaders-large
```

Re-score a run root:

```bash
python3 scripts/eval-score-plan.py \
  --run-root workspace/eval-artifacts/anvilminimal-mvp/<timestamp> \
  --rules eval/scoring_rules.yaml
```

## Postcheck

```bash
python3 scripts/eval-postcheck.py \
  --scenario eval/fixtures/postcheck/nextjs-dev-server.yaml \
  --workdir /path/to/workdir \
  --out /tmp/anvilminimal-postcheck
```

Long-running dev servers are started as foreground child processes, checked for
HTTP readiness, then stopped with a signal.

## Report

```bash
python3 scripts/eval-report.py \
  --run-root workspace/eval-artifacts/anvilminimal-mvp/<timestamp>
```

## Compare

```bash
python3 scripts/eval-compare.py \
  --baseline workspace/eval-artifacts/anvilminimal-mvp/<baseline>/summary.eval.tsv \
  --experiment workspace/eval-artifacts/anvilminimal-mvp/<experiment>/summary.eval.tsv \
  --out workspace/eval-artifacts/anvilminimal-mvp/<experiment>/compare.md
```

## Tests

```bash
cargo test
python3 -m unittest discover -s tests/eval -p 'test_*.py'
```

Live provider/network checks are not part of unit tests. Run
`eval-preflight.py --live-provider-smoke all` before cloud eval to verify the
current model/endpoint/tool-declaration contract.
