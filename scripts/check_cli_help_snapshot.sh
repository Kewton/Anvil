#!/usr/bin/env bash
# Guard the public CLI help text against accidental drift.

set -euo pipefail

REPO_ROOT=$(cd "$(dirname "$0")/.." && pwd)
SNAPSHOT="$REPO_ROOT/docs/cli-help.snapshot.txt"

if [[ ! -f "$SNAPSHOT" ]]; then
  echo "error: missing CLI help snapshot: $SNAPSHOT" >&2
  exit 1
fi

tmp=$(mktemp)
trap 'rm -f "$tmp"' EXIT

if [[ -n "${ANVIL_BIN:-}" ]]; then
  "$ANVIL_BIN" --help > "$tmp"
else
  (cd "$REPO_ROOT" && cargo run --quiet -- --help > "$tmp")
fi

if ! diff -u "$SNAPSHOT" "$tmp"; then
  cat >&2 <<'MSG'
error: CLI help output changed.

If the change is intentional, update docs/cli-help.snapshot.txt with:
  cargo run --quiet -- --help > docs/cli-help.snapshot.txt
MSG
  exit 1
fi

echo "PASS: CLI help snapshot matches"
