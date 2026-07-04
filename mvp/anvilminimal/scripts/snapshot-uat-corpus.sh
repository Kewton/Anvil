#!/usr/bin/env bash
set -euo pipefail

usage() {
  echo "usage: $0 <uat-workspace> <case-id>" >&2
  exit 2
}

if [ "$#" -ne 2 ]; then
  usage
fi

workspace=$1
case_id=$2

if [ ! -d "$workspace" ]; then
  echo "workspace not found: $workspace" >&2
  exit 1
fi

case "$case_id" in
  *[!A-Za-z0-9._-]*|"")
    echo "invalid case id: $case_id" >&2
    exit 1
    ;;
esac

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
crate_root=$(CDPATH= cd -- "$script_dir/.." && pwd)
dest="$crate_root/tests/corpus/apps/$case_id"

mkdir -p "$dest"

copy_if_present() {
  local rel=$1
  if [ -e "$workspace/$rel" ]; then
    mkdir -p "$dest/$(dirname "$rel")"
    cp -R "$workspace/$rel" "$dest/$rel"
  fi
}

copy_if_present "src"
for rel in \
  package.json \
  tsconfig.json \
  jsconfig.json \
  next-env.d.ts \
  next.config.js \
  next.config.mjs \
  next.config.ts \
  postcss.config.js \
  postcss.config.mjs \
  postcss.config.cjs \
  tailwind.config.js \
  tailwind.config.ts \
  tailwind.config.mjs \
  tailwind.config.cjs \
  eslint.config.js \
  .eslintrc.json
do
  copy_if_present "$rel"
done

if [ ! -f "$dest/expectations.toml" ]; then
  cat > "$dest/expectations.toml" <<EOF
case_id = "$case_id"
source = "$workspace"
required_paths = []
verify_commands = []
required_capabilities = []
required_evidence = []
required_obligations = []
deferred_verify_requirements = []
evidence_hint_tokens = []

[route_closure]
include = []
exclude = []

[evidence]

[weak_evidence]
contains = []

[diagnostics]
contains = []

[compile]
expect = "not_checked"
EOF
fi

echo "$dest"
