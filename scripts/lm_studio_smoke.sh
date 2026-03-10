#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"

export ANVIL_LM_STUDIO_ENDPOINT="${ANVIL_LM_STUDIO_ENDPOINT:-http://127.0.0.1:1234}"
export ANVIL_LM_STUDIO_MODEL="${ANVIL_LM_STUDIO_MODEL:-lmstudio/qwen3.5-35b-a3b}"

cd "$ROOT_DIR"

echo "LM Studio endpoint: $ANVIL_LM_STUDIO_ENDPOINT"
echo "LM Studio model: $ANVIL_LM_STUDIO_MODEL"

cargo test --test pm_and_models -- --ignored lm_studio_live_smoke_test
