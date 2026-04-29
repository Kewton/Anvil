#!/usr/bin/env bash
# Issue #465: Command::new allow-list 検証.
#
# AgentSkill / SkillRegistry 経由で skill が tool safety policy を迂回しないことを保証する。
# 新規 skill が `Command::new` / `process::Command::spawn` を直接呼ぶ場合は
# `crate::tools::bash::run_with_outcome` 経由に置き換えること。
#
# allow-list は production source (`src/`) の以下 5 ファイル限定:
#   - src/tools/bash.rs            (SSOT: check_blocked_command)
#   - src/model_registry.rs        (sysctl detect)
#   - src/agent/loop_run/auto_test.rs (auto-test runner)
#   - src/session/case_record.rs   (hardened git remote, 1s timeout + GIT_* scrub)
#   - src/agent/loop_run/tester.rs (transient harness, DR1-009)
#
# tests/ は scan 対象外 (E2E では一時的な Command 利用が許容される)。

set -euo pipefail

ALLOWLIST_PATTERN='(tools/bash|model_registry|loop_run/auto_test|session/case_record|loop_run/tester)\.rs$'

HITS=$(grep -rln 'Command::new\|process::Command' src/ 2>/dev/null \
  | grep -vE "$ALLOWLIST_PATTERN" \
  || true)

if [ -n "$HITS" ]; then
  echo "ERROR: Command::new / process::Command outside allow-list:"
  echo "$HITS"
  echo ""
  echo "AgentSkill 実装は crate::tools::bash::run_with_outcome 経由で shell を起動してください。"
  echo "allow-list を更新する場合は scripts/check_command_new.sh の ALLOWLIST_PATTERN を編集。"
  exit 1
fi

echo "OK: Command::new allow-list (5 files) is respected."
