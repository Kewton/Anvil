#!/usr/bin/env bash
# scripts/bench.sh - Run anvil benchmarks defined in benchmarks/*.yaml
#
# Usage: scripts/bench.sh <benchmark-name> [options]
#   benchmark-name    benchmarks/ 配下の yaml ファイル名（拡張子なし）
#   --model <name>    使用モデル
#   --models <list>   カンマ区切りで複数モデル（matrix 実行、逐次）
#   --runs <n>        実行回数（デフォルト: 5）
#   --dry-run         anvil 呼び出しを echo で代替
#   --bench-no-debug  anvil に --debug を付けない（BENCH_DEBUG=0 と同義）
#   --help            この用例を表示して終了
#
# Environment:
#   BENCH_DEBUG=1 (default) anvil を --debug 付きで起動し、llm-io.jsonl を収集する
#   BENCH_DEBUG=0           --debug を付けない。--bench-no-debug と等価。
#
# Examples:
#   scripts/bench.sh heavy --model qwen3.5:122b --runs 5
#   scripts/bench.sh heavy --models '35b-a3b,122b' --runs 5

set -uo pipefail
shopt -s nullglob

usage() {
  cat <<'EOF'
Usage: scripts/bench.sh <benchmark-name> [options]
  benchmark-name    benchmarks/ 配下の yaml ファイル名（拡張子なし）
                    例: heavy-space-invaders
                    ※ first-write: planned for future, not yet supported
  --model <name>    使用モデル
  --models <list>   カンマ区切りで複数モデル（matrix 実行、逐次）
  --runs <n>        実行回数（デフォルト: 5）
  --dry-run         anvil 呼び出しを echo で代替
  --bench-no-debug  anvil に --debug を付けない（BENCH_DEBUG=0 と同義）
  --help            この用例を表示して終了

Examples:
  scripts/bench.sh heavy-space-invaders --model qwen3.5:122b --runs 5
  scripts/bench.sh heavy-space-invaders --models '35b-a3b,122b' --runs 5
EOF
}

# -------- arg parse --------
benchmark_name=""
model_arg=""
models_arg=""
runs=5
DRY_RUN=0
# BENCH_DEBUG toggles `--debug` on the anvil invocation. Allowed values: "0" or "1".
BENCH_DEBUG="${BENCH_DEBUG:-1}"
case "$BENCH_DEBUG" in
  0|1) ;;
  *)
    echo "Error: BENCH_DEBUG must be 0 or 1 (got: $BENCH_DEBUG)" >&2
    exit 1
    ;;
esac

if [[ $# -eq 0 ]]; then
  usage
  exit 1
fi

while [[ $# -gt 0 ]]; do
  case "$1" in
    --help|-h)
      usage
      exit 0
      ;;
    --model)
      [[ $# -ge 2 ]] || { echo "Error: --model requires a value" >&2; exit 1; }
      model_arg="$2"
      shift 2
      ;;
    --models)
      [[ $# -ge 2 ]] || { echo "Error: --models requires a value" >&2; exit 1; }
      models_arg="$2"
      shift 2
      ;;
    --runs)
      [[ $# -ge 2 ]] || { echo "Error: --runs requires a value" >&2; exit 1; }
      runs="$2"
      shift 2
      ;;
    --dry-run)
      DRY_RUN=1
      shift
      ;;
    --bench-no-debug)
      BENCH_DEBUG=0
      shift
      ;;
    --)
      shift
      break
      ;;
    -*)
      echo "Error: unknown option: $1" >&2
      usage >&2
      exit 1
      ;;
    *)
      if [[ -z "$benchmark_name" ]]; then
        benchmark_name="$1"
        shift
      else
        echo "Error: unexpected argument: $1" >&2
        usage >&2
        exit 1
      fi
      ;;
  esac
done

if [[ -z "$benchmark_name" ]]; then
  echo "Error: benchmark-name is required" >&2
  usage >&2
  exit 1
fi

if [[ -z "$model_arg" && -z "$models_arg" ]]; then
  echo "Error: --model or --models is required" >&2
  usage >&2
  exit 1
fi

if [[ -n "$model_arg" && -n "$models_arg" ]]; then
  echo "Error: --model and --models are mutually exclusive" >&2
  exit 1
fi

if ! [[ "$runs" =~ ^[1-9][0-9]*$ ]]; then
  echo "Error: --runs must be a positive integer, got: $runs" >&2
  exit 1
fi

# -------- yq check --------
if ! yq --version 2>&1 | grep -qi 'mikefarah'; then
  echo "Error: yq (mikefarah/yq v4.x) is required." >&2
  echo "  macOS: brew install yq" >&2
  echo "  Linux: sudo apt-get install -y yq" >&2
  exit 1
fi

# -------- jq check --------
if ! command -v jq >/dev/null 2>&1; then
  echo "Error: jq is required (used to write run-dir/meta.json)." >&2
  echo "  macOS: brew install jq" >&2
  echo "  Linux: sudo apt-get install -y jq" >&2
  exit 1
fi

# -------- repo root --------
REPO_ROOT=$(cd "$(dirname "$0")/.." && pwd)

# -------- benchmark-name validation --------
if ! [[ "$benchmark_name" =~ ^[A-Za-z0-9][A-Za-z0-9._-]*$ ]]; then
  echo "Error: invalid benchmark-name: $benchmark_name" >&2
  exit 1
fi

BENCH_YAML="$REPO_ROOT/benchmarks/$benchmark_name.yaml"
if [[ ! -f "$BENCH_YAML" ]]; then
  echo "Error: benchmark yaml not found: $BENCH_YAML" >&2
  exit 1
fi

# -------- anvil binary resolve --------
# Preserve ANVIL_BIN from caller environment before any local assignment.
# This avoids the trap of setting ANVIL_BIN="" first and then checking
# "${ANVIL_BIN:-}", which would always see the empty local value.
_anvil_bin_env="${ANVIL_BIN:-}"
ANVIL_BIN=""
if [[ "$DRY_RUN" -eq 0 ]]; then
  if [[ -n "$_anvil_bin_env" ]]; then
    ANVIL_BIN="$_anvil_bin_env"
  elif [[ -x "$REPO_ROOT/target/release/anvil" ]]; then
    ANVIL_BIN="$REPO_ROOT/target/release/anvil"
  elif command -v anvil &>/dev/null; then
    ANVIL_BIN=$(command -v anvil)
  else
    echo "Error: anvil binary not found." >&2
    echo "  Run 'cargo build --release' or set ANVIL_BIN" >&2
    exit 1
  fi
fi

umask 077

# -------- BENCH_ROOT --------
BENCH_ROOT="$REPO_ROOT/.anvil/benchmarks/$(date +%Y%m%dT%H%M%S)-$$"
mkdir -p "$BENCH_ROOT"

# -------- yaml load (whitelist) --------
prompt=$(yq -r '.prompt' "$BENCH_YAML")
max_iterations=$(yq -r '.args.max_iterations // ""' "$BENCH_YAML")
chat_retries=$(yq -r '.args.chat_retries // ""' "$BENCH_YAML")
sidecar_model=$(yq -r '.args.sidecar_model // ""' "$BENCH_YAML")

if [[ -z "$prompt" || "$prompt" == "null" ]]; then
  echo "Error: .prompt is required in $BENCH_YAML" >&2
  exit 1
fi

if ! { [[ -z "$max_iterations" ]] || [[ "$max_iterations" =~ ^[0-9]+$ ]]; }; then
  echo "Error: invalid max_iterations: $max_iterations" >&2
  exit 1
fi
if ! { [[ -z "$chat_retries" ]] || [[ "$chat_retries" =~ ^[0-9]+$ ]]; }; then
  echo "Error: invalid chat_retries: $chat_retries" >&2
  exit 1
fi

# -------- summary.tsv header --------
printf 'run\tmodel\trc\telapsed_sec\tworkdir\tsession_copied\n' > "$BENCH_ROOT/summary.tsv"

# -------- trap --------
CURRENT_RUN=""
CURRENT_MODEL=""
CURRENT_RUN_DIR=""
CURRENT_START_TS=""
RUN_LOGGED=0
META_WRITTEN=0

write_meta_json() {
  # $1=rc, $2=elapsed_s, $3=model, $4=start_ts, $5=run_dir
  local _rc="$1" _elapsed="$2" _model="$3" _start_ts="$4" _run_dir="$5"
  if [[ -z "$_run_dir" ]]; then
    return 0
  fi
  mkdir -p "$_run_dir"
  jq -n \
    --argjson rc "$_rc" \
    --argjson elapsed_s "$_elapsed" \
    --arg model "$_model" \
    --arg start_ts "$_start_ts" \
    '{rc: $rc, elapsed_s: $elapsed_s, model: $model, start_ts: $start_ts}' \
    > "$_run_dir/meta.json"
}

on_interrupt() {
  if [[ "$RUN_LOGGED" -eq 0 && -n "$CURRENT_RUN" ]]; then
    printf '%s\t%s\t%s\t%s\t%s\t%s\n' \
      "$CURRENT_RUN" "${CURRENT_MODEL:-unknown}" "130" "N/A" "N/A" "0" \
      >> "$BENCH_ROOT/summary.tsv"
  fi
  if [[ "$META_WRITTEN" -eq 0 && -n "$CURRENT_RUN_DIR" ]]; then
    write_meta_json 130 0 "${CURRENT_MODEL:-unknown}" "${CURRENT_START_TS:-}" "$CURRENT_RUN_DIR" \
      || true
  fi
  exit 130
}
trap 'on_interrupt' SIGINT SIGTERM

# -------- validate_model --------
validate_model() {
  local m="$1"
  if [[ "$m" =~ [^A-Za-z0-9._:/-] ]]; then
    echo "Error: model name contains invalid characters: $m" >&2
    exit 1
  fi
}

# -------- slugify --------
slugify() {
  printf '%s' "$1" | sed 's/[^A-Za-z0-9._-]/-/g'
}

# -------- model list parse --------
models_array=()
if [[ -n "$models_arg" ]]; then
  IFS=',' read -ra models_array <<< "$models_arg"
else
  models_array=("$model_arg")
fi

# trim whitespace & drop empties
cleaned_models=()
for m in "${models_array[@]}"; do
  # remove leading/trailing whitespace
  m_trim="${m#"${m%%[![:space:]]*}"}"
  m_trim="${m_trim%"${m_trim##*[![:space:]]}"}"
  if [[ -n "$m_trim" ]]; then
    cleaned_models+=("$m_trim")
  fi
done

if [[ ${#cleaned_models[@]} -eq 0 ]]; then
  echo "Error: no valid models specified" >&2
  exit 1
fi

# -------- main loop --------
for model in "${cleaned_models[@]}"; do
  for (( run=1; run<=runs; run++ )); do
    CURRENT_RUN="$run"
    CURRENT_MODEL="$model"
    RUN_LOGGED=0
    META_WRITTEN=0

    validate_model "$model"
    model_slug=$(slugify "$model")
    if [[ -z "$model_slug" || "$model_slug" == "." || "$model_slug" == ".." ]]; then
      echo "invalid model slug: $model_slug" >&2
      exit 1
    fi

    RUN_DIR="$BENCH_ROOT/$model_slug/run-$run"
    WORKDIR="$RUN_DIR/workdir"
    STATE_DIR="$RUN_DIR/state"
    CURRENT_RUN_DIR="$RUN_DIR"
    CURRENT_START_TS=$(date -u +%Y-%m-%dT%H:%M:%SZ)

    # STATE_DIR assert: absolute path & no ..
    if [[ "$STATE_DIR" != /* ]]; then
      echo "STATE_DIR must be absolute" >&2
      exit 1
    fi
    case "$STATE_DIR" in
      *..*)
        echo "STATE_DIR must not contain '..'" >&2
        exit 1
        ;;
    esac

    mkdir -p "$WORKDIR" "$STATE_DIR" "$RUN_DIR/logs"
    cd "$WORKDIR" || { echo "Error: cd $WORKDIR failed" >&2; exit 1; }

    start=$SECONDS
    if [[ "$DRY_RUN" -eq 1 ]]; then
      echo "(dry-run) anvil --oneshot --prompt ... --state-dir $STATE_DIR --model $model" \
        > ../stdout.log
      rc=0
    else
      anvil_args=(--oneshot --prompt "$prompt" --state-dir "$STATE_DIR" --model "$model")
      if [[ -n "$max_iterations" ]]; then
        anvil_args+=(--max-iterations "$max_iterations")
      fi
      if [[ -n "$chat_retries" ]]; then
        anvil_args+=(--chat-retries "$chat_retries")
      fi
      if [[ -n "$sidecar_model" ]]; then
        anvil_args+=(--sidecar-model "$sidecar_model")
      fi
      if [[ "$BENCH_DEBUG" -eq 1 ]]; then
        anvil_args+=(--debug)
      fi
      # set -e is not enabled; capture rc directly
      "$ANVIL_BIN" "${anvil_args[@]}" > ../stdout.log 2>&1
      rc=$?
    fi
    elapsed=$(( SECONDS - start ))

    # session.json copy (nullglob → empty array if no match)
    # bash 3.2 + set -u needs guard for empty array expansion
    session_copied=0
    latest_session=""
    session_json_list=("$STATE_DIR/sessions/"*/session.json)
    if [[ ${#session_json_list[@]} -gt 0 ]]; then
      for f in "${session_json_list[@]}"; do
        latest_session="$f"
      done
    fi
    if [[ -n "$latest_session" && -f "$latest_session" && ! -L "$latest_session" ]]; then
      # CB-001: reject symlinks before copy to prevent information leakage
      real_session=$(realpath "$latest_session" 2>/dev/null || true)
      real_state=$(realpath "$STATE_DIR" 2>/dev/null || true)
      if [[ -n "$real_session" && -n "$real_state" && "$real_session" == "$real_state"/* ]]; then
        # CB-002: check copy success before setting session_copied=1
        if cp "$latest_session" ../session.json; then
          session_copied=1
        else
          echo "warning: session.json copy failed" >&2
        fi

        # Copy llm-io.jsonl from the state-dir session directory (if present)
        session_id=$(basename "$(dirname "$latest_session")")
        # Validate session id (UUID-ish: alnum + dash). Skip copy on odd values.
        if [[ "$session_id" =~ ^[A-Za-z0-9._-]+$ ]]; then
          log_src="$STATE_DIR/sessions/$session_id/logs/llm-io.jsonl"
          # CB-001: reject symlinks for llm-io.jsonl too
          if [[ -f "$log_src" && ! -L "$log_src" ]]; then
            real_log=$(realpath "$log_src" 2>/dev/null || true)
            if [[ -n "$real_log" && "$real_log" == "$real_state"/* ]]; then
              cp "$log_src" "$RUN_DIR/logs/llm-io.jsonl" || echo "warning: llm-io.jsonl copy failed" >&2
            fi
          fi
        fi
      else
        echo "warning: session.json path outside STATE_DIR, skipping copy" >&2
      fi
    fi

    # Write run-dir/meta.json so analyze_run.py can read rc/elapsed_s
    # CB-002: check write success
    if write_meta_json "$rc" "$elapsed" "$model" "$CURRENT_START_TS" "$RUN_DIR"; then
      META_WRITTEN=1
    else
      echo "warning: meta.json write failed" >&2
    fi

    workdir_rel="$model_slug/run-$run/workdir"
    printf '%s\t%s\t%s\t%s\t%s\t%s\n' \
      "$run" "$model" "$rc" "$elapsed" "$workdir_rel" "$session_copied" \
      >> "$BENCH_ROOT/summary.tsv"
    RUN_LOGGED=1

    CURRENT_RUN_DIR=""
    CURRENT_START_TS=""
    cd "$REPO_ROOT" || { echo "Error: cd $REPO_ROOT failed" >&2; exit 1; }
  done
done

echo "Done. Results: $BENCH_ROOT/summary.tsv"
