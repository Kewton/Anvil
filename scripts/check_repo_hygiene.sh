#!/usr/bin/env bash
# Ensure transient local artifacts are not tracked in git.

set -euo pipefail

blocked='^(\.anvil/|\.commandmate/|workspace/|dev-reports/|sandbox/)'

tracked=$(git ls-files | grep -E "$blocked" || true)
if [[ -n "$tracked" ]]; then
  cat >&2 <<'MSG'
error: transient local artifacts are tracked.

Move durable public material into docs/, tests/fixtures/, or tests/golden/.
Keep local eval runs, command attachments, runtime state, and scratch work ignored.

Tracked transient paths:
MSG
  printf '%s\n' "$tracked" >&2
  exit 1
fi

echo "PASS: repository hygiene policy"
