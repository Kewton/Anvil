#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RUNS="${1:-5}"

if ! [[ "$RUNS" =~ ^[0-9]+$ ]] || [ "$RUNS" -lt 1 ]; then
  echo "usage: $0 [runs>=1]" >&2
  exit 2
fi

cd "$ROOT_DIR"

COMMANDS=(
  "cargo test -q local_bootstrap --lib"
  "cargo test -q local_bootstrap --test provider_integration"
  "cargo test -q local_mode_failed_bootstrap --test provider_integration"
)

echo "local bootstrap regression harness"
echo "runs: $RUNS"
echo "commands: ${#COMMANDS[@]}"

total_passes=0
total_steps=$((RUNS * ${#COMMANDS[@]}))
started_at="$(date +%s)"

for run in $(seq 1 "$RUNS"); do
  echo
  echo "== run $run/$RUNS =="
  for command in "${COMMANDS[@]}"; do
    echo "-- $command"
    step_started="$(date +%s)"
    bash -lc "$command"
    step_elapsed="$(( $(date +%s) - step_started ))"
    total_passes=$((total_passes + 1))
    echo "ok (${step_elapsed}s)"
  done
done

elapsed="$(( $(date +%s) - started_at ))"
echo
echo "completed: $total_passes/$total_steps command runs passed in ${elapsed}s"
