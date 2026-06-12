#!/usr/bin/env bash
# tests/scripts/test_bench_smoke.sh — regression smoke test for scripts/bench.sh
#
# Runs bench.sh with a fake anvil binary and verifies:
#   * summary.tsv header is unchanged
#   * summary.tsv row count and column order match
#   * --pam-ab expands the same prompt suite into pam_on/pam_off variants
#   * run-dir contains session.json, meta.json, logs/llm-io.jsonl
#   * meta.json "rc", "elapsed_s", and bench seed fields are valid

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
prompt=""
prev=""
for arg in "$@"; do
  if [[ "$prev" == "--state-dir" ]]; then
    state_dir="$arg"
  elif [[ "$prev" == "--prompt" ]]; then
    prompt="$arg"
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
if [[ "$prompt" == "fail logs only" ]]; then
  echo '{"ts_ms":1,"event":"ollama.generate.start","payload":{"failure_fixture":true}}' \
    > "$session_dir/logs/llm-io.jsonl"
  echo '{"schema_version":1,"session_id":"fake","final_outcome":"error"}' \
    > "$session_dir/logs/eval.jsonl"
  echo "fake anvil: failed before session flush" >&2
  exit 1
fi
cat > "$session_dir/session.json" <<JSON
{"id":"$uuid","pam":"${ANVIL_PAM_ADVISORY_ENABLED:-unset}","seed":"${ANVIL_BENCH_SEED:-unset}","messages":[{"role":"assistant","content":"ok","tool_calls":[]}]}
JSON
echo '{"ts_ms":1,"event":"ollama.generate.start","payload":{}}' \
  > "$session_dir/logs/llm-io.jsonl"
echo '{"schema_version":1,"session_id":"fake","final_outcome":"done","pam_eval":{"advisory_only":true}}' \
  > "$session_dir/logs/eval.jsonl"
exit 0
EOF
chmod +x "$fake_anvil"

# --- fake benchmark yaml ---
bench_yaml_dir="$REPO_ROOT/benchmarks"
bench_yaml="$bench_yaml_dir/bench-smoke-fixture.yaml"
bench_fail_yaml="$bench_yaml_dir/bench-smoke-fail-fixture.yaml"
mkdir -p "$bench_yaml_dir"
cat > "$bench_yaml" <<'EOF'
args:
  max_iterations: 1
cases:
  - name: docs
    prompt: update README copy
  - name: data
    prompt: transform CSV rows
EOF
cat > "$bench_fail_yaml" <<'EOF'
args:
  max_iterations: 1
cases:
  - name: fail-logs-only
    prompt: fail logs only
EOF
trap 'rm -rf "$tmp"; rm -f "$bench_yaml" "$bench_fail_yaml"' EXIT

# --- invoke bench.sh ---
export ANVIL_BIN="$fake_anvil"
export BENCH_DEBUG=1
cd "$REPO_ROOT"
bash scripts/bench.sh bench-smoke-fixture --model "smoke-model" --runs 1 --pam-ab \
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
expected_header=$'run\tmodel\tcase\tpam_variant\trc\telapsed_sec\tworkdir\tsession_copied\textras_json'
if [[ "$header" != "$expected_header" ]]; then
  echo "FAIL: summary.tsv header mismatch" >&2
  echo "  got:      $header" >&2
  echo "  expected: $expected_header" >&2
  exit 1
fi

rows=$(($(wc -l < "$summary") - 1))
if [[ "$rows" -ne 4 ]]; then
  echo "FAIL: expected 4 data rows, got $rows" >&2
  exit 1
fi

for case_name in docs data; do
  for pam_variant in pam_on pam_off; do
    run_dir="$BENCH_ROOT/smoke-model/$case_name/$pam_variant/run-1"
    for f in session.json meta.json logs/llm-io.jsonl logs/eval.jsonl workdir; do
      if [[ ! -e "$run_dir/$f" ]]; then
        echo "FAIL: missing $run_dir/$f" >&2
        exit 1
      fi
    done
    expected_pam="true"
    if [[ "$pam_variant" == "pam_off" ]]; then
      expected_pam="false"
    fi
    jq -e --arg expected "$expected_pam" '.pam == $expected' "$run_dir/session.json" >/dev/null || {
      echo "FAIL: PAM env mismatch in $run_dir/session.json" >&2
      cat "$run_dir/session.json" >&2
      exit 1
    }
    seed_val=$(jq -r '.bench_seed' "$run_dir/meta.json")
    seed_enabled=$(jq -r '.bench_seed_enabled' "$run_dir/meta.json")
    if ! [[ "$seed_val" =~ ^[0-9]+$ && "$seed_enabled" == "true" ]]; then
      echo "FAIL: meta.json bench_seed invalid in $run_dir/meta.json" >&2
      cat "$run_dir/meta.json" >&2
      exit 1
    fi
    jq -e --arg seed "$seed_val" '.seed == $seed' "$run_dir/session.json" >/dev/null || {
      echo "FAIL: ANVIL_BENCH_SEED env mismatch in $run_dir/session.json" >&2
      cat "$run_dir/session.json" >&2
      cat "$run_dir/meta.json" >&2
      exit 1
    }
  done
done

run_dir="$BENCH_ROOT/smoke-model/docs/pam_on/run-1"

pam_on_count=$(awk -F'\t' 'NR > 1 && $4 == "pam_on" { n++ } END { print n + 0 }' "$summary")
pam_off_count=$(awk -F'\t' 'NR > 1 && $4 == "pam_off" { n++ } END { print n + 0 }' "$summary")
if [[ "$pam_on_count" -ne 2 || "$pam_off_count" -ne 2 ]]; then
  echo "FAIL: expected 2 pam_on and 2 pam_off rows" >&2
  cat "$summary" >&2
  exit 1
fi

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

# Seed injection can be disabled for ablation.
bash scripts/bench.sh bench-smoke-fixture --model "smoke-model" --runs 1 --no-bench-seed \
  > "$tmp/bench-no-seed.stdout" 2> "$tmp/bench-no-seed.stderr" || {
  echo "FAIL: bench.sh --no-bench-seed non-zero exit" >&2
  cat "$tmp/bench-no-seed.stderr" >&2
  exit 1
}
BENCH_ROOT_NO_SEED=$(awk '/^Done\. Results: / { sub(/^Done\. Results: /, ""); sub(/\/summary\.tsv$/, ""); print }' \
  "$tmp/bench-no-seed.stdout")
no_seed_run_dir="$BENCH_ROOT_NO_SEED/smoke-model/docs/default/run-1"
jq -e '.bench_seed == null and .bench_seed_enabled == false' "$no_seed_run_dir/meta.json" >/dev/null || {
  echo "FAIL: --no-bench-seed meta.json mismatch" >&2
  cat "$no_seed_run_dir/meta.json" >&2
  exit 1
}
jq -e '.seed == "unset"' "$no_seed_run_dir/session.json" >/dev/null || {
  echo "FAIL: --no-bench-seed env leaked into fake anvil" >&2
  cat "$no_seed_run_dir/session.json" >&2
  exit 1
}
rm -rf "$BENCH_ROOT_NO_SEED"

# Failed runs can have structured logs before session.json is flushed. Preserve
# those logs for triage even when session.json cannot be copied.
bash scripts/bench.sh bench-smoke-fail-fixture --model "smoke-model" --runs 1 \
  > "$tmp/bench-fail.stdout" 2> "$tmp/bench-fail.stderr" || {
  echo "FAIL: bench.sh fail-fixture wrapper should still complete" >&2
  cat "$tmp/bench-fail.stderr" >&2
  exit 1
}
BENCH_ROOT_FAIL=$(awk '/^Done\. Results: / { sub(/^Done\. Results: /, ""); sub(/\/summary\.tsv$/, ""); print }' \
  "$tmp/bench-fail.stdout")
fail_run_dir="$BENCH_ROOT_FAIL/smoke-model/fail-logs-only/default/run-1"
if [[ ! -f "$fail_run_dir/logs/llm-io.jsonl" || ! -f "$fail_run_dir/logs/eval.jsonl" ]]; then
  echo "FAIL: failed run logs were not copied" >&2
  find "$fail_run_dir" -maxdepth 4 -type f >&2 || true
  exit 1
fi
if [[ -e "$fail_run_dir/session.json" ]]; then
  echo "FAIL: fail fixture should not create copied session.json" >&2
  cat "$fail_run_dir/session.json" >&2
  exit 1
fi
fail_rc=$(jq -r '.rc' "$fail_run_dir/meta.json")
if [[ "$fail_rc" != "1" ]]; then
  echo "FAIL: fail fixture meta rc mismatch: $fail_rc" >&2
  cat "$fail_run_dir/meta.json" >&2
  exit 1
fi
fail_session_copied=$(awk -F'\t' 'NR == 2 { print $8 }' "$BENCH_ROOT_FAIL/summary.tsv")
if [[ "$fail_session_copied" != "0" ]]; then
  echo "FAIL: fail fixture should report session_copied=0" >&2
  cat "$BENCH_ROOT_FAIL/summary.tsv" >&2
  exit 1
fi
rm -rf "$BENCH_ROOT_FAIL"

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
