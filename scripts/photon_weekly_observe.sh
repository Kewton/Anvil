#!/usr/bin/env bash
# Phase 2 (DRY_RUN=false) 1-2 週間運用観測スクリプト
#
# Usage:
#   ./scripts/photon_weekly_observe.sh                 # 標準出力にレポート
#   ./scripts/photon_weekly_observe.sh --append <file> # ファイルに追記
#
# cron 例 (毎週月曜 09:00):
#   0 9 * * 1 cd /Users/maenokota/share/work/github_kewton/Anvil-develop && \
#     ./scripts/photon_weekly_observe.sh \
#     --append workspace/orchestration/runs/photon-weekly-observation.md

set -u

PHOTON_DB="${PHOTON_DB:-/private/var/folders/07/jbhw26n10nj_59d4yp7g5lx80000gn/T/photon-action-memory/summaries.sqlite}"
SESSION_DIR="${SESSION_DIR:-$HOME/.anvil/sessions}"

OUTPUT=""
APPEND_FILE=""
if [ "${1:-}" = "--append" ] && [ -n "${2:-}" ]; then
  APPEND_FILE="$2"
fi

append() {
  if [ -n "$APPEND_FILE" ]; then
    OUTPUT="$OUTPUT$1"$'\n'
  else
    printf '%s\n' "$1"
  fi
}

append "## $(date '+%Y-%m-%d %H:%M:%S') — photon weekly observation"
append ""

if [ ! -f "$PHOTON_DB" ]; then
  append "ERROR: photon DB not found at $PHOTON_DB"
  append "       (photon sidecar may not have run yet, or path is wrong)"
  if [ -n "$APPEND_FILE" ]; then
    printf '%s' "$OUTPUT" >> "$APPEND_FILE"
  fi
  exit 1
fi

append "### photon DB seed counts"
append ""
append '```'
append "$(sqlite3 "$PHOTON_DB" <<'EOF'
.mode column
.headers on
SELECT
  'total' AS bucket,
  COUNT(*) AS count
FROM action_summaries
UNION ALL SELECT 'clean', COUNT(*) FROM action_summaries WHERE quality_check_status='clean'
UNION ALL SELECT 'warned', COUNT(*) FROM action_summaries WHERE quality_check_status='warned'
UNION ALL SELECT 'unchecked', COUNT(*) FROM action_summaries WHERE quality_check_status IS NULL OR quality_check_status='unchecked'
UNION ALL SELECT 'anvil_case_*', COUNT(*) FROM action_summaries WHERE summary_id LIKE 'anvil-case-%'
UNION ALL SELECT 'anvil_eval_*', COUNT(*) FROM action_summaries WHERE summary_id LIKE 'anvil-eval-%'
UNION ALL SELECT 'new_last_7d',  COUNT(*) FROM action_summaries
  WHERE created_at > strftime('%Y-%m-%dT%H:%M:%S', 'now', '-7 days');
EOF
)"
append '```'
append ""

append "### auto_promote events (last 7 days)"
append ""
if [ -d "$SESSION_DIR" ]; then
  TOTAL=$(find "$SESSION_DIR" -name "llm-io.jsonl" -mtime -7 -exec grep -h "agent.photon_auto_promote" {} \; 2>/dev/null | wc -l | tr -d ' ')
  SUCCEEDED=$(find "$SESSION_DIR" -name "llm-io.jsonl" -mtime -7 -exec grep -h "agent.photon_auto_promote.succeeded" {} \; 2>/dev/null | wc -l | tr -d ' ')
  SKIPPED=$(find "$SESSION_DIR" -name "llm-io.jsonl" -mtime -7 -exec grep -h "agent.photon_auto_promote.skipped" {} \; 2>/dev/null | wc -l | tr -d ' ')
  SCRUBBED=$(find "$SESSION_DIR" -name "llm-io.jsonl" -mtime -7 -exec grep -h "agent.photon_auto_promote.scrubbed" {} \; 2>/dev/null | wc -l | tr -d ' ')
  FAILED=$(find "$SESSION_DIR" -name "llm-io.jsonl" -mtime -7 -exec grep -h "agent.photon_auto_promote.failed" {} \; 2>/dev/null | wc -l | tr -d ' ')
  REJECTED=$(find "$SESSION_DIR" -name "llm-io.jsonl" -mtime -7 -exec grep -h "agent.photon_auto_promote.rejected_by_photon" {} \; 2>/dev/null | wc -l | tr -d ' ')

  append "| event | count |"
  append "|-------|-------|"
  append "| total       | $TOTAL |"
  append "| succeeded   | $SUCCEEDED |"
  append "| skipped     | $SKIPPED |"
  append "| scrubbed    | $SCRUBBED |"
  append "| failed      | $FAILED |"
  append "| rejected_by_photon | $REJECTED |"

  if [ "$TOTAL" -gt 0 ]; then
    FAILED_PCT=$(awk -v f="$FAILED" -v t="$TOTAL" 'BEGIN{printf "%.1f", (f/t)*100}')
    append ""
    append "failed rate: ${FAILED_PCT}% (rollback threshold: > 5%)"
  fi
else
  append "session dir not found: $SESSION_DIR"
fi
append ""

append "### Rollback triggers (review)"
append ""
append "- photon DB が急増 (+50 seed/day) → DRY_RUN=true に戻す"
append "- warned 率 > 30% → scrub 強化、photon #119 review"
append "- auto_promote.failed > 5% → bug 調査"
append "- A-1 pass rate -10pp 以上低下 → rollback + investigation"
append ""

if [ -n "$APPEND_FILE" ]; then
  mkdir -p "$(dirname "$APPEND_FILE")"
  printf '%s' "$OUTPUT" >> "$APPEND_FILE"
  echo "appended to $APPEND_FILE"
fi
