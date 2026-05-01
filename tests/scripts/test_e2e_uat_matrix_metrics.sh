#!/usr/bin/env bash
# Smoke-test the E2E/UAT observability artifact shape without invoking Ollama.

set -euo pipefail

REPO_ROOT=$(cd "$(dirname "$0")/../.." && pwd)
SCRIPT="$REPO_ROOT/scripts/e2e_uat_matrix.py"

if ! command -v python3 >/dev/null 2>&1; then
  echo "SKIP: python3 not installed" >&2
  exit 0
fi

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

python3 "$SCRIPT" \
  --dry-run \
  --models qwen3.6:27b-coding-nvfp4 \
  --reps 1 \
  --scenarios S0-01 \
  --run-id metrics-shape \
  --out-root "$tmp/runs" >/dev/null

results="$tmp/runs/metrics-shape/results.csv"
metrics="$tmp/runs/metrics-shape/metrics.json"
metrics_md="$tmp/runs/metrics-shape/metrics.md"

python3 - "$results" "$metrics" "$metrics_md" <<'PY'
import csv
import importlib.util
import json
import sys

results_path, metrics_path, metrics_md_path = sys.argv[1:]
spec = importlib.util.spec_from_file_location("e2e_uat_matrix", "scripts/e2e_uat_matrix.py")
module = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = module
spec.loader.exec_module(module)

with open(results_path, newline="", encoding="utf-8") as fh:
    header = next(csv.reader(fh))

for key in [
    "fallback_level",
    "mode_confidence",
    "mode_alternative_gap",
    "mode_override_count",
    "verifier_source",
    "verifier_candidate_count",
    "repo_context_seed_source",
    "repo_context_no_candidates",
]:
    assert key in header, key

with open(metrics_path, encoding="utf-8") as fh:
    metrics = json.load(fh)

for key in [
    "schema_version",
    "mode_confidence_distribution",
    "mode_alternative_gap_distribution",
    "fallback_level_used",
    "verifier_source_distribution",
    "repo_context_seed_source",
    "false_negative_verifier_rate",
]:
    assert key in metrics, key

assert metrics["schema_version"] == 1, metrics
assert metrics["fallback_level_used"]["minimal-patch"] == 1, metrics

observed = module.extract_observability([
    {
        "event": "ollama.chat.request",
        "payload": {
            "messages": [
                {
                    "content": "[Mode Policy] Work mode is TypeScript UI. Prefer the existing framework.",
                }
            ]
        },
    },
    {
        "event": "agent.autotest.candidates",
        "payload": {"selected_source": "package_json_scripts", "candidate_count": 2},
    },
    {
        "event": "agent.repo_context.completed",
        "payload": {
            "total_candidates_considered": 3,
            "selected_files": [
                {"reasons": ["lexical_keyword", "node_project"]},
            ],
        },
    },
])
assert observed["mode"] == "TypeScript UI", observed
assert observed["verifier_source"] == "package_json_scripts", observed
assert observed["repo_context_seed_source"] == "node_project", observed

with open(metrics_md_path, encoding="utf-8") as fh:
    text = fh.read()
assert "| metric | value |" in text, text
print("metrics shape validated")
PY

echo "PASS: e2e_uat_matrix metrics smoke test"
