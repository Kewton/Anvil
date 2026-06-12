#!/usr/bin/env bash
# tests/scripts/test_bench_smoke.sh — regression smoke test for scripts/bench.sh
#
# Runs bench.sh with a fake anvil binary and verifies:
#   * summary.tsv header is unchanged
#   * summary.tsv row count and column order match
#   * --pam-ab expands the same prompt suite into pam_on/pam_off variants
#   * run-dir contains session.json, meta.json, logs/llm-io.jsonl
#   * meta.json "rc" and "elapsed_s" are numeric
#   * meta.json records build provenance and active flags

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
engine="legacy"
prompt=""
prev=""
for arg in "$@"; do
  if [[ "$prev" == "--state-dir" ]]; then
    state_dir="$arg"
  fi
  if [[ "$prev" == "--engine" ]]; then
    engine="$arg"
  fi
  if [[ "$prev" == "--prompt" ]]; then
    prompt="$arg"
  fi
  prev="$arg"
done
if [[ -z "$state_dir" ]]; then
  echo "fake anvil: --state-dir missing" >&2
  exit 1
fi
printf 'ok\n' > result.txt
if [[ "$prompt" == *"exit after artifact without session"* ]]; then
  echo "fake anvil: crash after artifact" >&2
  exit 7
fi
uuid="00000000-0000-4000-8000-000000000001"
session_dir="$state_dir/sessions/$uuid"
mkdir -p "$session_dir/logs"
cat > "$session_dir/session.json" <<JSON
{"id":"$uuid","pam":"${ANVIL_PAM_ADVISORY_ENABLED:-unset}","engine":"$engine","messages":[{"role":"assistant","content":"ok","tool_calls":[]}]}
JSON
echo '{"ts_ms":1,"event":"ollama.generate.start","payload":{}}' \
  > "$session_dir/logs/llm-io.jsonl"
echo '{"schema_version":1,"session_id":"fake","final_outcome":"done","pam_eval":{"advisory_only":true}}' \
  > "$session_dir/logs/eval.jsonl"
exit 0
EOF
chmod +x "$fake_anvil"
fake_anvil_real=$(realpath "$fake_anvil" 2>/dev/null || printf '%s' "$fake_anvil")

# --- fake benchmark yaml ---
bench_yaml_dir="$REPO_ROOT/benchmarks"
bench_yaml="$bench_yaml_dir/bench-smoke-fixture.yaml"
mkdir -p "$bench_yaml_dir"
cat > "$bench_yaml" <<'EOF'
args:
  max_iterations: 1
success_check:
  files:
    - path: result.txt
      min_lines: 1
  commands:
    - test -f result.txt
cases:
  - name: docs
    prompt: update README copy
  - name: data
    prompt: transform CSV rows
  - name: crash
    prompt: exit after artifact without session
EOF
trap 'rm -rf "$tmp"; rm -f "$bench_yaml"' EXIT

# --- invoke bench.sh ---
export ANVIL_BIN="$fake_anvil"
export BENCH_DEBUG=1
cd "$REPO_ROOT"
bash scripts/bench.sh bench-smoke-fixture --model "smoke-model" --runs 1 --pam-ab --engines legacy,minimal \
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
if [[ "$rows" -ne 12 ]]; then
  echo "FAIL: expected 12 data rows, got $rows" >&2
  exit 1
fi

for engine in legacy minimal; do
  for case_name in docs data; do
    for pam_variant in pam_on pam_off; do
      run_dir="$BENCH_ROOT/smoke-model/$engine/$case_name/$pam_variant/run-1"
      for f in session.json meta.json logs/llm-io.jsonl logs/eval.jsonl workdir workdir/result.txt; do
        if [[ ! -e "$run_dir/$f" ]]; then
          echo "FAIL: missing $run_dir/$f" >&2
          exit 1
        fi
      done
      expected_pam="true"
      if [[ "$pam_variant" == "pam_off" ]]; then
        expected_pam="false"
      fi
      jq -e --arg expected "$expected_pam" --arg engine "$engine" \
        '.pam == $expected and .engine == $engine' "$run_dir/session.json" >/dev/null || {
        echo "FAIL: PAM/env mismatch in $run_dir/session.json" >&2
        cat "$run_dir/session.json" >&2
        exit 1
      }
      jq -e --arg engine "$engine" \
        '.engine == $engine and .success_check_success == true and .success_check_reason == "ok"' \
        "$run_dir/meta.json" >/dev/null || {
        echo "FAIL: meta engine/success_check mismatch in $run_dir/meta.json" >&2
        cat "$run_dir/meta.json" >&2
        exit 1
      }
      jq -e --arg bin "$fake_anvil_real" --arg engine "$engine" --arg pam "ANVIL_PAM_ADVISORY_ENABLED=$expected_pam" \
        '(.build.git_dirty | type == "boolean")
         and (.build.git_revision == null or (.build.git_revision | type == "string"))
         and .build.binary_path == $bin
         and (.build.build_time == null or (.build.build_time | test("^[0-9]{4}-[0-9]{2}-[0-9]{2}T")))
         and (.active_flags | index("BENCH_DEBUG=1"))
         and (.active_flags | index($pam))
         and (.active_flags | index("ENGINE=" + $engine))' \
        "$run_dir/meta.json" >/dev/null || {
        echo "FAIL: meta build/active_flags mismatch in $run_dir/meta.json" >&2
        cat "$run_dir/meta.json" >&2
        exit 1
      }
    done
  done
done

run_dir="$BENCH_ROOT/smoke-model/legacy/docs/pam_on/run-1"

pam_on_count=$(awk -F'\t' 'NR > 1 && $4 == "pam_on" { n++ } END { print n + 0 }' "$summary")
pam_off_count=$(awk -F'\t' 'NR > 1 && $4 == "pam_off" { n++ } END { print n + 0 }' "$summary")
if [[ "$pam_on_count" -ne 6 || "$pam_off_count" -ne 6 ]]; then
  echo "FAIL: expected 6 pam_on and 6 pam_off rows" >&2
  cat "$summary" >&2
  exit 1
fi

for engine in legacy minimal; do
  for pam_variant in pam_on pam_off; do
    run_dir="$BENCH_ROOT/smoke-model/$engine/crash/$pam_variant/run-1"
    if [[ -e "$run_dir/session.json" ]]; then
      echo "FAIL: crash run unexpectedly copied session.json: $run_dir/session.json" >&2
      exit 1
    fi
    for f in meta.json workdir workdir/result.txt; do
      if [[ ! -e "$run_dir/$f" ]]; then
        echo "FAIL: missing crash artifact $run_dir/$f" >&2
        exit 1
      fi
    done
    jq -e --arg engine "$engine" \
      '.rc == 7 and .engine == $engine and .success_check_success == true and .success_check_reason == "ok"' \
      "$run_dir/meta.json" >/dev/null || {
      echo "FAIL: crash meta mismatch in $run_dir/meta.json" >&2
      cat "$run_dir/meta.json" >&2
      exit 1
    }
    crash_row=$(awk -F'\t' -v engine="$engine" -v pam="$pam_variant" \
      '$3 == "crash" && $4 == pam && $0 ~ ("/" engine "/") { print $0 }' "$summary")
    crash_session_copied=$(printf '%s\n' "$crash_row" | awk -F'\t' '{ print $8 }')
    crash_extras=$(printf '%s\n' "$crash_row" | awk -F'\t' '{ print $9 }')
    if [[ "$crash_session_copied" != "0" ]]; then
      echo "FAIL: crash summary session_copied should be 0" >&2
      echo "$crash_row" >&2
      exit 1
    fi
    printf '%s' "$crash_extras" | jq -e --arg engine "$engine" \
      '.engine == $engine and .success_check_success == true and .success_check_reason == "ok" and .tool_call_count == null' \
      >/dev/null || {
      echo "FAIL: crash summary extras did not fall back to meta.json" >&2
      echo "$crash_row" >&2
      exit 1
    }
  done
done

run_dir="$BENCH_ROOT/smoke-model/legacy/docs/pam_on/run-1"
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
