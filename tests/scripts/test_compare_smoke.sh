#!/usr/bin/env bash
# tests/scripts/test_compare_smoke.sh — E2E smoke test for scripts/compare.py
#
# Builds a fake BENCH_ROOT pair on tempdir (skipping bench.sh to keep the
# smoke test fast and hermetic) and verifies:
#   * markdown format -> exit 0, contains the required table header
#   * json format -> exit 0, valid JSON with schema_version==1

set -euo pipefail

REPO_ROOT=$(cd "$(dirname "$0")/../.." && pwd)
SCRIPT="$REPO_ROOT/scripts/compare.py"

if ! command -v python3 >/dev/null 2>&1; then
  echo "SKIP: python3 not installed" >&2
  exit 0
fi

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

SLUG="smoke-model"

build_run() {
  # $1=run-dir, $2=rc, $3=elapsed_s, $4=run_id, $5=iter_count, $6=engine
  local dir="$1" rc="$2" elapsed="$3" rid="$4" iters="$5"
  local engine="${6:-legacy}"
  mkdir -p "$dir"
  cat > "$dir/meta.json" <<EOF
{"rc": $rc, "elapsed_s": $elapsed, "model": "$SLUG", "engine": "$engine", "start_ts": "2026-01-01T00:00:00Z"}
EOF
  python3 - "$dir/session.json" "$rid" "$iters" <<'PY'
import json, sys
out, rid, iters = sys.argv[1], sys.argv[2], int(sys.argv[3])
msgs = []
for i in range(iters):
    msgs.append({"role": "assistant", "content": f"iter {i}", "tool_calls": []})
with open(out, "w") as f:
    json.dump({"id": rid, "messages": msgs}, f)
PY
}

baseline="$tmp/baseline"
experiment="$tmp/experiment"

# baseline: rc=[0,0,1], elapsed=[120,130,125], iter=[10,11,9]
build_run "$baseline/$SLUG/run-1" 0 120 "b1" 10
build_run "$baseline/$SLUG/run-2" 0 130 "b2" 11
build_run "$baseline/$SLUG/run-3" 1 125 "b3" 9

# experiment: rc=[0,0,0], elapsed=[100,110,105], iter=[8,9,7]
build_run "$experiment/$SLUG/run-1" 0 100 "e1" 8
build_run "$experiment/$SLUG/run-2" 0 110 "e2" 9
build_run "$experiment/$SLUG/run-3" 0 105 "e3" 7

# --- markdown ---
md_out="$tmp/out.md"
if ! COMPARE_NOW=2026-01-01T00:00:00Z \
  python3 "$SCRIPT" --format markdown "$baseline" "$experiment" > "$md_out" 2> "$tmp/md.err"; then
  echo "FAIL: compare.py (markdown) exit non-zero" >&2
  cat "$tmp/md.err" >&2
  exit 1
fi

if ! grep -q '^| metric | baseline (mean \[CI\]) | experiment (mean \[CI\]) | delta | delta_pct | verdict |' "$md_out"; then
  echo "FAIL: markdown output missing table header" >&2
  cat "$md_out" >&2
  exit 1
fi

if ! grep -q "model_slug: $SLUG" "$md_out"; then
  echo "FAIL: markdown output missing model_slug" >&2
  exit 1
fi

# --- json ---
json_out="$tmp/out.json"
if ! COMPARE_NOW=2026-01-01T00:00:00Z \
  python3 "$SCRIPT" --format json "$baseline" "$experiment" > "$json_out" 2> "$tmp/json.err"; then
  echo "FAIL: compare.py (json) exit non-zero" >&2
  cat "$tmp/json.err" >&2
  exit 1
fi

python3 - "$json_out" "$SLUG" <<'PY'
import json, sys
path, slug = sys.argv[1], sys.argv[2]
with open(path) as f:
    data = json.load(f)
assert data["schema_version"] == 1, data
assert data["model_slug"] == slug, data
assert "metrics" in data and isinstance(data["metrics"], dict), data
assert "rc" in data["metrics"], data
assert "failure_categories" in data, data
print("json validated")
PY

# --- engine mode ---
engine_root="$tmp/engine-root"
build_run "$engine_root/$SLUG/legacy/docs/default/run-1" 0 120 "el1" 10 legacy
build_run "$engine_root/$SLUG/legacy/docs/default/run-2" 0 130 "el2" 11 legacy
build_run "$engine_root/$SLUG/legacy/docs/default/run-3" 1 125 "el3" 9 legacy
build_run "$engine_root/$SLUG/minimal/docs/default/run-1" 0 100 "em1" 8 minimal
build_run "$engine_root/$SLUG/minimal/docs/default/run-2" 0 110 "em2" 9 minimal
build_run "$engine_root/$SLUG/minimal/docs/default/run-3" 0 105 "em3" 7 minimal

engine_json="$tmp/engine.json"
if ! COMPARE_NOW=2026-01-01T00:00:00Z \
  python3 "$SCRIPT" --format json --engines legacy,minimal "$engine_root" > "$engine_json" 2> "$tmp/engine.err"; then
  echo "FAIL: compare.py engine mode exit non-zero" >&2
  cat "$tmp/engine.err" >&2
  exit 1
fi
python3 - "$engine_json" <<'PY'
import json, sys
with open(sys.argv[1]) as f:
    data = json.load(f)
assert data["model_slug"].endswith(":legacy->minimal"), data
assert "rc" in data["metrics"], data
assert "failure_categories" in data, data
print("engine json validated")
PY

echo "PASS: compare.py smoke test"
