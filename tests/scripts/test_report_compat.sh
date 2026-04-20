#!/usr/bin/env bash
# tests/scripts/test_report_compat.sh — E2E compatibility for scripts/report.py
#
# Builds a synthetic BENCH_ROOT from the existing analyze_run.py "full" fixture
# and verifies that report.py exits 0 and produces a Markdown report header.

set -euo pipefail

REPO_ROOT=$(cd "$(dirname "$0")/../.." && pwd)
FIXTURE_RUN_DIR="$REPO_ROOT/tests/golden/full/run-dir"

if [[ ! -d "$FIXTURE_RUN_DIR" ]]; then
  echo "FAIL: missing fixture: $FIXTURE_RUN_DIR" >&2
  exit 1
fi

BENCH_ROOT=$(mktemp -d)
trap 'rm -rf "$BENCH_ROOT"' EXIT

MODEL_DIR="$BENCH_ROOT/test-model"
RUN_DIR="$MODEL_DIR/run-1"
mkdir -p "$RUN_DIR"

cp -R "$FIXTURE_RUN_DIR/." "$RUN_DIR/"

OUT=$(python3 "$REPO_ROOT/scripts/report.py" "$BENCH_ROOT")
if ! printf '%s\n' "$OUT" | grep -q '^# Benchmark Report'; then
  echo "FAIL: report.py did not produce expected header" >&2
  printf '%s\n' "$OUT" >&2
  exit 1
fi

if ! printf '%s\n' "$OUT" | grep -q 'test-model'; then
  echo "FAIL: report.py did not list the synthetic model" >&2
  printf '%s\n' "$OUT" >&2
  exit 1
fi

echo "PASS: report.py e2e compatibility"
