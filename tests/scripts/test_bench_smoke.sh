#!/usr/bin/env bash
# tests/scripts/test_bench_smoke.sh — regression smoke test for scripts/bench.sh
#
# Runs bench.sh with a fake anvil binary and verifies:
#   * summary.tsv header is unchanged
#   * summary.tsv row count and column order match
#   * run-dir contains session.json, meta.json, logs/llm-io.jsonl
#   * meta.json "rc" and "elapsed_s" are numeric

set -euo pipefail

REPO_ROOT=$(cd "$(dirname "$0")/../.." && pwd)

if ! command -v jq >/dev/null 2>&1; then
  echo "SKIP: jq not installed" >&2
  exit 0
fi

if ! yq --version 2>&1 | grep -qi 'mikefarah'; then
  echo "SKIP: mikefarah/yq not installed" >&2
  exit 0
fi

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

# --- fake anvil ---
fake_anvil="$tmp/anvil"
cat > "$fake_anvil" <<'EOF'
#!/usr/bin/env bash
# Parse --state-dir and write a session directory.
set -euo pipefail
state_dir=""
prev=""
for arg in "$@"; do
  if [[ "$prev" == "--state-dir" ]]; then
    state_dir="$arg"
  fi
  prev="$arg"
done
if [[ -z "$state_dir" ]]; then
  echo "fake anvil: --state-dir missing" >&2
  exit 1
fi
uuid="00000000-0000-4000-8000-000000000001"
session_dir="$state_dir/sessions/$uuid"
mkdir -p "$session_dir/logs"
cat > "$session_dir/session.json" <<JSON
{"id":"$uuid","messages":[{"role":"assistant","content":"ok","tool_calls":[]}]}
JSON
echo '{"ts_ms":1,"event":"ollama.generate.start","payload":{}}' \
  > "$session_dir/logs/llm-io.jsonl"
exit 0
EOF
chmod +x "$fake_anvil"

# --- fake benchmark yaml ---
bench_yaml_dir="$REPO_ROOT/benchmarks"
bench_yaml="$bench_yaml_dir/bench-smoke-fixture.yaml"
mkdir -p "$bench_yaml_dir"
cat > "$bench_yaml" <<'EOF'
prompt: smoke test
args:
  max_iterations: 1
EOF
trap 'rm -rf "$tmp"; rm -f "$bench_yaml"' EXIT

# --- invoke bench.sh ---
export ANVIL_BIN="$fake_anvil"
export BENCH_DEBUG=1
cd "$REPO_ROOT"
bash scripts/bench.sh bench-smoke-fixture --model "smoke-model" --runs 1 \
  > "$tmp/bench.stdout" 2> "$tmp/bench.stderr" || {
  echo "FAIL: bench.sh non-zero exit" >&2
  cat "$tmp/bench.stderr" >&2
  exit 1
}

# --- locate BENCH_ROOT ---
BENCH_ROOT=$(awk '/^Done\. Results: / { sub(/^Done\. Results: /, ""); sub(/\/summary\.tsv$/, ""); print }' \
  "$tmp/bench.stdout")
if [[ -z "$BENCH_ROOT" || ! -d "$BENCH_ROOT" ]]; then
  echo "FAIL: BENCH_ROOT not found" >&2
  cat "$tmp/bench.stdout" >&2
  exit 1
fi

summary="$BENCH_ROOT/summary.tsv"
if [[ ! -f "$summary" ]]; then
  echo "FAIL: summary.tsv missing: $summary" >&2
  exit 1
fi

header=$(head -n 1 "$summary")
expected_header=$'run\tmodel\trc\telapsed_sec\tworkdir\tsession_copied\textras_json'
if [[ "$header" != "$expected_header" ]]; then
  echo "FAIL: summary.tsv header mismatch" >&2
  echo "  got:      $header" >&2
  echo "  expected: $expected_header" >&2
  exit 1
fi

rows=$(($(wc -l < "$summary") - 1))
if [[ "$rows" -ne 1 ]]; then
  echo "FAIL: expected 1 data row, got $rows" >&2
  exit 1
fi

run_dir="$BENCH_ROOT/smoke-model/run-1"
for f in session.json meta.json logs/llm-io.jsonl workdir; do
  if [[ ! -e "$run_dir/$f" ]]; then
    echo "FAIL: missing $run_dir/$f" >&2
    exit 1
  fi
done

rc_val=$(jq -r '.rc' "$run_dir/meta.json")
elapsed_val=$(jq -r '.elapsed_s' "$run_dir/meta.json")
if ! [[ "$rc_val" =~ ^[0-9]+$ ]]; then
  echo "FAIL: meta.json rc is not numeric: $rc_val" >&2
  exit 1
fi
if ! [[ "$elapsed_val" =~ ^[0-9]+$ ]]; then
  echo "FAIL: meta.json elapsed_s is not numeric: $elapsed_val" >&2
  exit 1
fi

# Optional: analyze_run.py should succeed on the produced run-dir
if command -v python3 >/dev/null 2>&1; then
  python3 scripts/analyze_run.py "$run_dir" > "$tmp/analyze.json"
  jq -e '.rc == 0 and .run_id != null' "$tmp/analyze.json" >/dev/null || {
    echo "FAIL: analyze_run.py output invalid" >&2
    cat "$tmp/analyze.json" >&2
    exit 1
  }
fi

# Cleanup the generated BENCH_ROOT (under repo .anvil)
rm -rf "$BENCH_ROOT"

echo "PASS: bench.sh smoke test"
