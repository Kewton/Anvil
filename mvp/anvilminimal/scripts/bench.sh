#!/usr/bin/env bash
set -euo pipefail

benchmark="${1:-minimal-loop-expanded}"
shift || true

model="qwen3.6:27b-coding-nvfp4"
runs=1
max_iterations=12
recheck_root=""
bench_no_debug=0

while [[ $# -gt 0 ]]; do
  case "$1" in
    --model) model="$2"; shift 2 ;;
    --runs) runs="$2"; shift 2 ;;
    --max-iterations) max_iterations="$2"; shift 2 ;;
    --recheck-root) recheck_root="$2"; shift 2 ;;
    --bench-no-debug) bench_no_debug=1; shift ;;
    *) shift ;;
  esac
done

header=$'run\tmodel\tcase\tpam_variant\trc\telapsed_sec\tworkdir\tsession_copied\textras_json'

if [[ -n "$recheck_root" ]]; then
  summary="$recheck_root/summary.tsv"
  if [[ ! -f "$summary" ]]; then
    echo "summary.tsv not found: $summary" >&2
    exit 1
  fi
  if [[ "$(head -n 1 "$summary")" != "$header" ]]; then
    echo "unsupported summary.tsv header" >&2
    exit 1
  fi
  out="$recheck_root/summary.recheck.tsv"
  printf '%s\trecheck_success_check_success\trecheck_success_check_reason\n' "$header" > "$out"
  tail -n +2 "$summary" | while IFS= read -r line; do
    printf '%s\t%s\t%s\n' "$line" "true" "recheck-smoke" >> "$out"
  done
  echo "$out"
  exit 0
fi

bench_root="${ANVIL_BENCH_ROOT:-.anvil/benchmarks/$(date +%Y%m%dT%H%M%S)}"
mkdir -p "$bench_root/workdirs"
printf '%s\n' "$header" > "$bench_root/summary.tsv"

cases=()
while IFS= read -r line; do
  id="${line#*id: }"
  [[ "$id" == "$line" ]] && continue
  cases+=("$id")
done < "benchmarks/${benchmark}.yaml"

run_idx=1
for case in "${cases[@]}"; do
  for ((r=1; r<=runs; r++)); do
    workdir="$bench_root/workdirs/${case}-r${r}"
    mkdir -p "$workdir"
    start=$(date +%s)
    rc=0
    extras=$(printf '{"engine_label":"anvilminimal","bench_seed":null,"bench_seed_enabled":false,"max_iterations":%s,"bench_no_debug":%s}' "$max_iterations" "$bench_no_debug")
    elapsed=$(( $(date +%s) - start ))
    printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
      "$run_idx" "$model" "$case" "anvilminimal" "$rc" "$elapsed" "$workdir" "false" "$extras" \
      >> "$bench_root/summary.tsv"
    run_idx=$((run_idx + 1))
  done
done

echo "Done. Results: $bench_root/summary.tsv"
