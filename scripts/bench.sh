#!/usr/bin/env bash
# scripts/bench.sh - Run anvil benchmarks defined in benchmarks/*.yaml
#
# Usage: scripts/bench.sh <benchmark-name> [options]
#   benchmark-name    benchmarks/ 配下の yaml ファイル名（拡張子なし）
#   --model <name>    使用モデル
#   --models <list>   カンマ区切りで複数モデル（matrix 実行、逐次）
#   --engine <name>   legacy|minimal（デフォルト: legacy）
#   --engines <list>  カンマ区切りで複数 engine（例: legacy,minimal）
#   --runs <n>        実行回数（デフォルト: 5）
#   --max-iterations <n>
#                     YAML の args.max_iterations を全 case で上書き
#   --dry-run         anvil 呼び出しを echo で代替
#   --pam-ab          Same prompt suite with PAM enabled and disabled
#   --bench-no-debug  anvil に --trace を付けない（BENCH_DEBUG=0 と同義）
#   --help            この用例を表示して終了
#
# Environment:
#   BENCH_DEBUG=1 (default) anvil を --trace 付きで起動し、詳細ログを stderr に流す
#                           （llm-io.jsonl は log level によらず常時保存される）
#   BENCH_DEBUG=0           --trace を付けない。--bench-no-debug と等価。
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
  --engine <name>   legacy|minimal（デフォルト: legacy）
  --engines <list>  カンマ区切りで複数 engine（例: legacy,minimal）
  --runs <n>        実行回数（デフォルト: 5）
  --max-iterations <n>
                    YAML の args.max_iterations を全 case で上書き
  --no-precautions  Reminder Sidecar を無効化（ANVIL_NO_REMINDER=1）
  --no-case-memory  Case memory を無効化（ANVIL_NO_CASE_RETRIEVAL=1 ANVIL_NO_CASE_RECORD=1）
  --no-auto-test    Auto test を無効化（ANVIL_NO_AUTO_TEST=1）
  --pam-ab          Same prompt suite with PAM enabled and disabled
  --dry-run         anvil 呼び出しを echo で代替
  --bench-no-debug  anvil に --trace を付けない（BENCH_DEBUG=0 と同義）
  --help            この用例を表示して終了

Examples:
  scripts/bench.sh heavy-space-invaders --model qwen3.5:122b --runs 5
  scripts/bench.sh heavy-space-invaders --models '35b-a3b,122b' --runs 5
  scripts/bench.sh heavy-space-invaders --model qwen3.5:122b --no-precautions --no-auto-test
EOF
}

# -------- arg parse --------
benchmark_name=""
model_arg=""
models_arg=""
engine_arg=""
engines_arg=""
runs=5
max_iterations_override=""
DRY_RUN=0
no_precautions=0
no_case_memory=0
no_auto_test=0
pam_ab=0
# BENCH_DEBUG toggles `--trace` on the anvil invocation. Allowed values: "0" or "1".
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
    --engine)
      [[ $# -ge 2 ]] || { echo "Error: --engine requires a value" >&2; exit 1; }
      engine_arg="$2"
      shift 2
      ;;
    --engines)
      [[ $# -ge 2 ]] || { echo "Error: --engines requires a value" >&2; exit 1; }
      engines_arg="$2"
      shift 2
      ;;
    --runs)
      [[ $# -ge 2 ]] || { echo "Error: --runs requires a value" >&2; exit 1; }
      runs="$2"
      shift 2
      ;;
    --max-iterations)
      [[ $# -ge 2 ]] || { echo "Error: --max-iterations requires a value" >&2; exit 1; }
      max_iterations_override="$2"
      shift 2
      ;;
    --no-precautions)
      no_precautions=1
      shift
      ;;
    --no-case-memory)
      no_case_memory=1
      shift
      ;;
    --no-auto-test)
      no_auto_test=1
      shift
      ;;
    --pam-ab)
      pam_ab=1
      shift
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

if [[ -n "$engine_arg" && -n "$engines_arg" ]]; then
  echo "Error: --engine and --engines are mutually exclusive" >&2
  exit 1
fi

if ! [[ "$runs" =~ ^[1-9][0-9]*$ ]]; then
  echo "Error: --runs must be a positive integer, got: $runs" >&2
  exit 1
fi

if ! { [[ -z "$max_iterations_override" ]] || [[ "$max_iterations_override" =~ ^[1-9][0-9]*$ ]]; }; then
  echo "Error: --max-iterations must be a positive integer, got: $max_iterations_override" >&2
  exit 1
fi

# -------- yq check --------
if ! yq --version 2>&1 | grep -qi 'mikefarah'; then
  echo "Error: yq (mikefarah/yq v4.x) is required." >&2
  echo "  macOS: brew install yq" >&2
  echo "  Linux: https://github.com/mikefarah/yq/releases" >&2
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

# -------- build metadata --------
git_commit() {
  git -C "$REPO_ROOT" rev-parse --verify HEAD 2>/dev/null || true
}

git_dirty_flag() {
  if ! git -C "$REPO_ROOT" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
    printf 'false'
    return 0
  fi
  if [[ -n "$(git -C "$REPO_ROOT" status --porcelain 2>/dev/null || true)" ]]; then
    printf 'true'
  else
    printf 'false'
  fi
}

file_mtime_epoch() {
  local path="$1"
  stat -f '%m' "$path" 2>/dev/null || stat -c '%Y' "$path" 2>/dev/null || true
}

iso_from_epoch() {
  local epoch="$1"
  if [[ -z "$epoch" ]]; then
    return 0
  fi
  date -u -r "$epoch" '+%Y-%m-%dT%H:%M:%SZ' 2>/dev/null \
    || date -u -d "@$epoch" '+%Y-%m-%dT%H:%M:%SZ' 2>/dev/null \
    || true
}

BENCH_GIT_COMMIT=$(git_commit)
BENCH_GIT_DIRTY=$(git_dirty_flag)
BENCH_GIT_REVISION="$BENCH_GIT_COMMIT"
if [[ -n "$BENCH_GIT_REVISION" && "$BENCH_GIT_DIRTY" == "true" ]]; then
  BENCH_GIT_REVISION="$BENCH_GIT_REVISION-dirty"
fi

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

ANVIL_BIN_REAL=""
ANVIL_BIN_MTIME=""
if [[ -n "$ANVIL_BIN" ]]; then
  ANVIL_BIN_REAL=$(realpath "$ANVIL_BIN" 2>/dev/null || printf '%s' "$ANVIL_BIN")
  ANVIL_BIN_MTIME=$(iso_from_epoch "$(file_mtime_epoch "$ANVIL_BIN")")
fi

umask 077

# -------- BENCH_ROOT --------
BENCH_ROOT="$REPO_ROOT/.anvil/benchmarks/$(date +%Y%m%dT%H%M%S)-$$"
mkdir -p "$BENCH_ROOT"

# -------- yaml validation --------
case_count=$(yq -r '(.cases // []) | length' "$BENCH_YAML")
if ! [[ "$case_count" =~ ^[0-9]+$ ]]; then
  echo "Error: invalid .cases length in $BENCH_YAML" >&2
  exit 1
fi
if [[ "$case_count" -eq 0 ]]; then
  prompt=$(yq -r '.prompt // ""' "$BENCH_YAML")
  if [[ -z "$prompt" || "$prompt" == "null" ]]; then
    echo "Error: .prompt or .cases[].prompt is required in $BENCH_YAML" >&2
    exit 1
  fi
else
  for (( case_idx=0; case_idx<case_count; case_idx++ )); do
    prompt=$(yq -r ".cases[$case_idx].prompt // \"\"" "$BENCH_YAML")
    if [[ -z "$prompt" || "$prompt" == "null" ]]; then
      echo "Error: .cases[$case_idx].prompt is required in $BENCH_YAML" >&2
      exit 1
    fi
  done
fi

# -------- summary.tsv header --------
printf 'run\tmodel\tcase\tpam_variant\trc\telapsed_sec\tworkdir\tsession_copied\textras_json\n' > "$BENCH_ROOT/summary.tsv"

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

# -------- write_meta_json --------
write_meta_json() {
  # $1=rc, $2=elapsed_s, $3=model, $4=start_ts, $5=run_dir, $6=case, $7=task_kind, $8=pam_variant, $9=engine, $10=success_check_success, $11=success_check_reason, $12=active_flags_json
  local _rc="$1" _elapsed="$2" _model="$3" _start_ts="$4" _run_dir="$5"
  local _case="${6:-default}" _task_kind="${7:-coding}" _pam_variant="${8:-default}"
  local _engine="${9:-legacy}" _success_check_success="${10:-null}" _success_check_reason="${11:-}"
  local _active_flags_json="${12:-[]}"
  if [[ -z "$_run_dir" ]]; then
    return 0
  fi
  mkdir -p "$_run_dir"
  jq -n \
    --argjson rc "$_rc" \
    --argjson elapsed_s "$_elapsed" \
    --arg model "$_model" \
    --arg start_ts "$_start_ts" \
    --arg case "$_case" \
    --arg task_kind "$_task_kind" \
    --arg pam_variant "$_pam_variant" \
    --arg engine "$_engine" \
    --argjson success_check_success "$_success_check_success" \
    --arg success_check_reason "$_success_check_reason" \
    --arg build_git_commit "$BENCH_GIT_COMMIT" \
    --argjson build_git_dirty "$BENCH_GIT_DIRTY" \
    --arg build_git_revision "$BENCH_GIT_REVISION" \
    --arg build_binary_path "$ANVIL_BIN_REAL" \
    --arg build_time "$ANVIL_BIN_MTIME" \
    --argjson active_flags "$_active_flags_json" \
    '{
      rc: $rc,
      elapsed_s: $elapsed_s,
      model: $model,
      start_ts: $start_ts,
      case: $case,
      task_kind: $task_kind,
      pam_variant: $pam_variant,
      engine: $engine,
      success_check_success: $success_check_success,
      success_check_reason: (if $success_check_reason == "" then null else $success_check_reason end),
      build: {
        git_commit: (if $build_git_commit == "" then null else $build_git_commit end),
        git_dirty: $build_git_dirty,
        git_revision: (if $build_git_revision == "" then null else $build_git_revision end),
        binary_path: (if $build_binary_path == "" then null else $build_binary_path end),
        build_time: (if $build_time == "" then null else $build_time end)
      },
      active_flags: $active_flags
    }' \
    > "$_run_dir/meta.json"
}

validate_engine() {
  local e="$1"
  case "$e" in
    legacy|minimal) ;;
    *)
      echo "Error: engine must be legacy or minimal, got: $e" >&2
      exit 1
      ;;
  esac
}

SUCCESS_CHECK_SUCCESS="null"
SUCCESS_CHECK_REASON="no_success_check"
evaluate_success_check() {
  # $1=case_idx
  local case_idx="$1"
  local selector
  if [[ "$case_count" -eq 0 ]]; then
    selector='(.success_check // {})'
  else
    selector="(.cases[$case_idx].success_check // .success_check // {})"
  fi

  local has_check
  has_check=$(yq -r "$selector | has(\"files\") or has(\"commands\")" "$BENCH_YAML")
  if [[ "$has_check" != "true" ]]; then
    SUCCESS_CHECK_SUCCESS="null"
    SUCCESS_CHECK_REASON="no_success_check"
    return 0
  fi

  local ok=1
  local reasons=()
  local file_count command_count
  file_count=$(yq -r "$selector | (.files // []) | length" "$BENCH_YAML")
  command_count=$(yq -r "$selector | (.commands // []) | length" "$BENCH_YAML")

  for (( check_idx=0; check_idx<file_count; check_idx++ )); do
    local rel min_lines path line_count
    rel=$(yq -r "$selector | .files[$check_idx].path // .files[$check_idx] // \"\"" "$BENCH_YAML")
    min_lines=$(yq -r "$selector | .files[$check_idx].min_lines // \"\"" "$BENCH_YAML")
    if [[ -z "$rel" || "$rel" == "null" || "$rel" == /* || "$rel" == *..* ]]; then
      ok=0
      reasons+=("invalid_file_check")
      continue
    fi
    path="$WORKDIR/$rel"
    if [[ ! -f "$path" ]]; then
      ok=0
      reasons+=("missing_file:$rel")
      continue
    fi
    if [[ -n "$min_lines" && "$min_lines" != "null" ]]; then
      if ! [[ "$min_lines" =~ ^[0-9]+$ ]]; then
        ok=0
        reasons+=("invalid_min_lines:$rel")
        continue
      fi
      line_count=$(wc -l < "$path" | tr -d '[:space:]')
      if [[ "$line_count" -lt "$min_lines" ]]; then
        ok=0
        reasons+=("min_lines:$rel:$line_count<$min_lines")
      fi
    fi
  done

  for (( check_idx=0; check_idx<command_count; check_idx++ )); do
    local command
    command=$(yq -r "$selector | .commands[$check_idx].command // .commands[$check_idx] // \"\"" "$BENCH_YAML")
    if [[ -z "$command" || "$command" == "null" ]]; then
      ok=0
      reasons+=("invalid_command_check")
      continue
    fi
    if ! (cd "$WORKDIR" && bash -lc "$command" >/dev/null 2>&1); then
      ok=0
      reasons+=("command_failed:$check_idx")
    fi
  done

  if [[ "$ok" -eq 1 ]]; then
    SUCCESS_CHECK_SUCCESS="true"
    SUCCESS_CHECK_REASON="ok"
  else
    SUCCESS_CHECK_SUCCESS="false"
    local joined
    joined=$(IFS=','; echo "${reasons[*]}")
    SUCCESS_CHECK_REASON="$joined"
  fi
}

# -------- validate_models_array --------
validate_models_array() {
  local seen_models=() seen_slugs=()
  for m in "${cleaned_models[@]}"; do
    validate_model "$m"
    for s in "${seen_models[@]+"${seen_models[@]}"}"; do
      [[ "$s" == "$m" ]] && { echo "Error: duplicate model: $m" >&2; exit 1; }
    done
    seen_models+=("$m")
    local slug
    slug=$(slugify "$m")
    if [[ -z "$slug" || "$slug" == "." || "$slug" == ".." ]]; then
      echo "Error: invalid slug for '$m'" >&2; exit 1
    fi
    for s in "${seen_slugs[@]+"${seen_slugs[@]}"}"; do
      [[ "$s" == "$slug" ]] && { echo "Error: slug collision for '$m' (slug='$slug')" >&2; exit 1; }
    done
    seen_slugs+=("$slug")
  done
}

# -------- generate_matrix_report --------
# generate_matrix_report <bench_root> <benchmark_name> <runs> <model...>
generate_matrix_report() {
  local bench_root="$1"
  local bench_name="$2"
  local total_runs="$3"
  shift 3
  local models_list=("$@")

  local tmp ts models_joined
  tmp=$(mktemp "$bench_root/.matrix-report.md.XXXXXX") || return 1
  ts=$(date -u '+%Y-%m-%dT%H:%M:%SZ')
  # Join with comma (commas are invalid in model names per validate_model)
  models_joined=$(printf '%s,' "${models_list[@]}")
  models_joined="${models_joined%,}"

  awk -F'\t' \
    -v models_arg="$models_joined" \
    -v bench_name="$bench_name" \
    -v total_runs="$total_runs" \
    -v ts="$ts" \
    '
    BEGIN {
      order_count = 0
      n = split(models_arg, models_order, ",")
      for (i = 1; i <= n; i++) {
        if (models_order[i] != "") {
          order[++order_count] = models_order[i]
        }
      }
    }
    NR == 1 {
      for (i = 1; i <= NF; i++) col[$i] = i
      col_model   = col["model"]
      col_rc      = col["rc"]
      col_elapsed = col["elapsed_sec"]
      if (!col_model || !col_rc || !col_elapsed) exit 2
      next
    }
    NR > 1 {
      m = $col_model
      rc = $col_rc
      el = $col_elapsed
      cnt[m]++
      if      (rc == "0")   succ[m]++
      else if (rc == "130") intr[m]++
      else                  fail[m]++
      if (el ~ /^[0-9]+$/) {
        idx = ++cnt_el[m]
        elvals[m, idx] = el + 0
      }
    }
    function median(m,    n, i, j, tmp, ta) {
      n = cnt_el[m]
      if (n == 0) return -1
      for (i = 1; i <= n; i++) ta[i] = elvals[m, i]
      for (i = 1; i < n; i++) {
        for (j = i + 1; j <= n; j++) {
          if (ta[i] > ta[j]) { tmp = ta[i]; ta[i] = ta[j]; ta[j] = tmp }
        }
      }
      if (n % 2 == 1) return ta[int((n + 1) / 2)]
      return (ta[int(n / 2)] + ta[int(n / 2) + 1]) / 2.0
    }
    function mean(m,    i, s) {
      if (cnt_el[m] == 0) return -1
      s = 0
      for (i = 1; i <= cnt_el[m]; i++) s += elvals[m, i]
      return s / cnt_el[m]
    }
    function min_val(m,    i, v) {
      if (cnt_el[m] == 0) return -1
      v = elvals[m, 1]
      for (i = 2; i <= cnt_el[m]; i++) if (elvals[m, i] < v) v = elvals[m, i]
      return v
    }
    function max_val(m,    i, v) {
      if (cnt_el[m] == 0) return -1
      v = elvals[m, 1]
      for (i = 2; i <= cnt_el[m]; i++) if (elvals[m, i] > v) v = elvals[m, i]
      return v
    }
    END {
      printf "# Benchmark Matrix: %s\n", bench_name
      printf "Generated: %s  Models: %d  Runs: %s\n\n", ts, order_count, total_runs
      printf "| model | runs | success | fail | interrupted | median_sec | mean_sec | min_sec | max_sec |\n"
      printf "|-------|------|---------|------|-------------|------------|----------|---------|----------|\n"
      for (i = 1; i <= order_count; i++) {
        m = order[i]
        if (cnt[m] == 0) {
          printf "| %s | - | - | - | - | - | - | - | - |\n", m
        } else if (succ[m]+0 == 0 && fail[m]+0 == 0 && intr[m]+0 > 0) {
          printf "| %s | %d | 0 | 0 | %d | - | - | - | - |\n", m, cnt[m], intr[m]
        } else {
          printf "| %s | %d | %d | %d | %d | %.1f | %.1f | %.1f | %.1f |\n", \
            m, cnt[m], succ[m]+0, fail[m]+0, intr[m]+0, \
            median(m), mean(m), min_val(m), max_val(m)
        }
      }
    }
    ' "$bench_root/summary.tsv" > "$tmp" || { rm -f "$tmp"; return 1; }

  mv "$tmp" "$bench_root/matrix-report.md" || { rm -f "$tmp"; return 1; }
}

# -------- model list parse --------
models_array=()
if [[ -n "$models_arg" ]]; then
  IFS=',' read -ra models_array <<< "$models_arg"
else
  models_array=("$model_arg")
fi

# -------- engine list parse --------
engines_array=()
engine_path_segment=0
engine_cli_explicit=0
if [[ -n "$engines_arg" ]]; then
  IFS=',' read -ra engines_array <<< "$engines_arg"
  engine_path_segment=1
  engine_cli_explicit=1
elif [[ -n "$engine_arg" ]]; then
  engines_array=("$engine_arg")
  engine_path_segment=1
  engine_cli_explicit=1
else
  engines_array=("legacy")
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

cleaned_engines=()
for e in "${engines_array[@]}"; do
  e_trim="${e#"${e%%[![:space:]]*}"}"
  e_trim="${e_trim%"${e_trim##*[![:space:]]}"}"
  if [[ -n "$e_trim" ]]; then
    validate_engine "$e_trim"
    for seen in "${cleaned_engines[@]+"${cleaned_engines[@]}"}"; do
      [[ "$seen" == "$e_trim" ]] && { echo "Error: duplicate engine: $e_trim" >&2; exit 1; }
    done
    cleaned_engines+=("$e_trim")
  fi
done
if [[ ${#cleaned_engines[@]} -eq 0 ]]; then
  echo "Error: no valid engines specified" >&2
  exit 1
fi

# validate models (character set, duplicate, slug collision)
validate_models_array

# -------- trap (registered after cleaned_models is populated) --------
CURRENT_RUN=""
CURRENT_MODEL=""
CURRENT_ENGINE="legacy"
CURRENT_CASE="default"
CURRENT_TASK_KIND="coding"
CURRENT_PAM_VARIANT="default"
CURRENT_RUN_DIR=""
CURRENT_START_TS=""
CURRENT_ACTIVE_FLAGS_JSON="[]"
RUN_LOGGED=0
META_WRITTEN=0
on_interrupt() {
  if [[ "$RUN_LOGGED" -eq 0 && -n "$CURRENT_RUN" ]]; then
    printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
      "$CURRENT_RUN" "${CURRENT_MODEL:-unknown}" "${CURRENT_CASE:-default}" \
      "${CURRENT_PAM_VARIANT:-default}" "130" "N/A" "N/A" "0" "null" \
      >> "$BENCH_ROOT/summary.tsv"
  fi
  if [[ "$META_WRITTEN" -eq 0 && -n "$CURRENT_RUN_DIR" ]]; then
    write_meta_json 130 0 "${CURRENT_MODEL:-unknown}" "${CURRENT_START_TS:-}" "$CURRENT_RUN_DIR" \
      "${CURRENT_CASE:-default}" "${CURRENT_TASK_KIND:-coding}" "${CURRENT_PAM_VARIANT:-default}" \
      "${CURRENT_ENGINE:-legacy}" "null" "interrupted" "${CURRENT_ACTIVE_FLAGS_JSON:-[]}" \
      || true
  fi
  if [[ -n "$models_arg" ]]; then
    generate_matrix_report "$BENCH_ROOT" "$benchmark_name" "$runs" "${cleaned_models[@]}" || true
  fi
  exit 130
}
trap 'on_interrupt' SIGINT SIGTERM

# -------- main loop --------
pam_variants=("default")
if [[ "$pam_ab" -eq 1 ]]; then
  pam_variants=("pam_on" "pam_off")
fi

case_loop_count="$case_count"
if [[ "$case_loop_count" -eq 0 ]]; then
  case_loop_count=1
fi

model_idx=0
for model in "${cleaned_models[@]}"; do
  model_idx=$((model_idx + 1))
  model_slug=$(slugify "$model")

  engine_idx=0
  for engine in "${cleaned_engines[@]}"; do
    engine_idx=$((engine_idx + 1))

    for (( case_idx=0; case_idx<case_loop_count; case_idx++ )); do
    if [[ "$case_count" -eq 0 ]]; then
      case_name="default"
      task_kind="coding"
      prompt=$(yq -r '.prompt // ""' "$BENCH_YAML")
      max_iterations=$(yq -r '.args.max_iterations // ""' "$BENCH_YAML")
      chat_retries=$(yq -r '.args.chat_retries // ""' "$BENCH_YAML")
      sidecar_model=$(yq -r '.args.sidecar_model // ""' "$BENCH_YAML")
    else
      case_name=$(yq -r ".cases[$case_idx].name // \"case-$((case_idx + 1))\"" "$BENCH_YAML")
      task_kind=$(yq -r ".cases[$case_idx].task_kind // .cases[$case_idx].category // \"coding\"" "$BENCH_YAML")
      prompt=$(yq -r ".cases[$case_idx].prompt // \"\"" "$BENCH_YAML")
      max_iterations=$(yq -r ".cases[$case_idx].args.max_iterations // .args.max_iterations // \"\"" "$BENCH_YAML")
      chat_retries=$(yq -r ".cases[$case_idx].args.chat_retries // .args.chat_retries // \"\"" "$BENCH_YAML")
      sidecar_model=$(yq -r ".cases[$case_idx].args.sidecar_model // .args.sidecar_model // \"\"" "$BENCH_YAML")
    fi
    if [[ -n "$max_iterations_override" ]]; then
      max_iterations="$max_iterations_override"
    fi

    if [[ -z "$case_name" || "$case_name" == "null" ]]; then
      echo "Error: empty benchmark case name" >&2
      exit 1
    fi
    if ! [[ "$task_kind" =~ ^(coding|docs|data|research|ops|authoring)$ ]]; then
      echo "Error: invalid task_kind/category for case $case_name: $task_kind" >&2
      exit 1
    fi
    case_slug=$(slugify "$case_name")
    if [[ -z "$case_slug" || "$case_slug" == "." || "$case_slug" == ".." ]]; then
      echo "Error: invalid benchmark case name: $case_name" >&2
      exit 1
    fi
    if ! { [[ -z "$max_iterations" ]] || [[ "$max_iterations" =~ ^[0-9]+$ ]]; }; then
      echo "Error: invalid max_iterations for case $case_name: $max_iterations" >&2
      exit 1
    fi
    if ! { [[ -z "$chat_retries" ]] || [[ "$chat_retries" =~ ^[0-9]+$ ]]; }; then
      echo "Error: invalid chat_retries for case $case_name: $chat_retries" >&2
      exit 1
    fi

    for pam_variant in "${pam_variants[@]}"; do
      for (( run=1; run<=runs; run++ )); do
        printf '[model %d/%d | engine %d/%d | case %d/%d | %s | run %d/%d] %s / %s\n' \
          "$model_idx" "${#cleaned_models[@]}" "$engine_idx" "${#cleaned_engines[@]}" \
          "$((case_idx + 1))" "$case_loop_count" "$pam_variant" "$run" "$runs" "$model" "$engine" >&2

        CURRENT_RUN="$run"
        CURRENT_MODEL="$model"
        CURRENT_ENGINE="$engine"
        CURRENT_CASE="$case_name"
        CURRENT_TASK_KIND="$task_kind"
        CURRENT_PAM_VARIANT="$pam_variant"
        RUN_LOGGED=0
        META_WRITTEN=0

        if [[ "$engine_path_segment" -eq 1 ]]; then
          RUN_DIR="$BENCH_ROOT/$model_slug/$engine/$case_slug/$pam_variant/run-$run"
          workdir_rel="$model_slug/$engine/$case_slug/$pam_variant/run-$run/workdir"
        elif [[ "$case_slug" == "default" && "$pam_variant" == "default" ]]; then
          RUN_DIR="$BENCH_ROOT/$model_slug/run-$run"
          workdir_rel="$model_slug/run-$run/workdir"
        else
          RUN_DIR="$BENCH_ROOT/$model_slug/$case_slug/$pam_variant/run-$run"
          workdir_rel="$model_slug/$case_slug/$pam_variant/run-$run/workdir"
        fi
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

        # -------- caller env isolation --------
        # Unset ANVIL_NO_* from caller env to prevent baseline cell contamination.
        unset ANVIL_NO_REMINDER ANVIL_NO_CASE_RETRIEVAL ANVIL_NO_CASE_RECORD \
              ANVIL_NO_AUTO_TEST ANVIL_NO_TESTER ANVIL_NO_REPO_GRAPH \
              ANVIL_CASE_RECORD_DRY_RUN ANVIL_CASE_RETRIEVAL_DRY_RUN \
              ANVIL_PAM_ADVISORY_ENABLED

        # Build env_kv array from feature flags (bash array, no eval)
        declare -a env_kv=()
        if [[ "$no_precautions" -eq 1 ]]; then
          env_kv+=("ANVIL_NO_REMINDER=1")
        fi
        if [[ "$no_case_memory" -eq 1 ]]; then
          env_kv+=("ANVIL_NO_CASE_RETRIEVAL=1" "ANVIL_NO_CASE_RECORD=1")
        fi
        if [[ "$no_auto_test" -eq 1 ]]; then
          env_kv+=("ANVIL_NO_AUTO_TEST=1")
        fi
        if [[ "$pam_variant" == "pam_on" ]]; then
          env_kv+=("ANVIL_PAM_ADVISORY_ENABLED=true")
        elif [[ "$pam_variant" == "pam_off" ]]; then
          env_kv+=("ANVIL_PAM_ADVISORY_ENABLED=false")
        fi
        active_flags_json=$(
          {
            printf 'BENCH_DEBUG=%s\n' "$BENCH_DEBUG"
            printf 'ANVIL_BIN=%s\n' "$ANVIL_BIN_REAL"
            printf 'ENGINE=%s\n' "$engine"
            [[ -n "$max_iterations" ]] && printf 'MAX_ITERATIONS=%s\n' "$max_iterations"
            [[ -n "$chat_retries" ]] && printf 'CHAT_RETRIES=%s\n' "$chat_retries"
            [[ -n "$sidecar_model" ]] && printf 'SIDECAR_MODEL=%s\n' "$sidecar_model"
            printf '%s\n' "${env_kv[@]+"${env_kv[@]}"}"
          } | jq -R -s 'split("\n") | map(select(length > 0))'
        )
        CURRENT_ACTIVE_FLAGS_JSON="$active_flags_json"

        start=$SECONDS
        if [[ "$DRY_RUN" -eq 1 ]]; then
          echo "(dry-run) anvil --oneshot --offline --yes --prompt ... --state-dir $STATE_DIR --model $model --engine $engine --case $case_name --pam-variant $pam_variant" \
            > ../stdout.log
          rc=0
        else
          anvil_args=(--oneshot --offline --yes --prompt "$prompt" --state-dir "$STATE_DIR" --model "$model")
          if [[ "$engine_cli_explicit" -eq 1 ]]; then
            anvil_args+=(--engine "$engine")
          fi
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
            anvil_args+=(--trace)
          fi
          # Launch anvil with:
          #   - env_kv feature flags (ANVIL_NO_* vars)
          #   - stripped parent credentials (security: DR4-002)
          #   - allowlisted env vars only
          env -i \
            HOME="$HOME" \
            PATH="$PATH" \
            TERM="${TERM:-xterm}" \
            TMPDIR="${TMPDIR:-/tmp}" \
            "${env_kv[@]+"${env_kv[@]}"}" \
            "$ANVIL_BIN" "${anvil_args[@]}" > ../stdout.log 2>&1
          rc=$?
        fi
        elapsed=$(( SECONDS - start ))
        evaluate_success_check "$case_idx"

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
          # reject symlinks before copy to prevent information leakage
          real_session=$(realpath "$latest_session" 2>/dev/null || true)
          real_state=$(realpath "$STATE_DIR" 2>/dev/null || true)
          if [[ -n "$real_session" && -n "$real_state" && "$real_session" == "$real_state"/* ]]; then
            if cp "$latest_session" ../session.json; then
              session_copied=1
            else
              echo "warning: session.json copy failed" >&2
            fi

            # Copy structured logs from the state-dir session directory (if present).
            session_id=$(basename "$(dirname "$latest_session")")
            if [[ "$session_id" =~ ^[A-Za-z0-9._-]+$ ]]; then
              for log_name in llm-io.jsonl eval.jsonl; do
                log_src="$STATE_DIR/sessions/$session_id/logs/$log_name"
                if [[ -f "$log_src" && ! -L "$log_src" ]]; then
                  real_log=$(realpath "$log_src" 2>/dev/null || true)
                  if [[ -n "$real_log" && "$real_log" == "$real_state"/* ]]; then
                    cp "$log_src" "$RUN_DIR/logs/$log_name" || echo "warning: $log_name copy failed" >&2
                  fi
                fi
              done
            fi
          else
            echo "warning: session.json path outside STATE_DIR, skipping copy" >&2
          fi
        fi

        # Write run-dir/meta.json so analyze_run.py can read rc/elapsed_s
        if write_meta_json "$rc" "$elapsed" "$model" "$CURRENT_START_TS" "$RUN_DIR" \
          "$case_name" "$task_kind" "$pam_variant" "$engine" \
          "$SUCCESS_CHECK_SUCCESS" "$SUCCESS_CHECK_REASON" "$active_flags_json"; then
          META_WRITTEN=1
        else
          echo "warning: meta.json write failed" >&2
        fi

        # -------- analyze_run.py per-cell invocation (DR2-002, DR4-001) --------
        # Absolute path + symlink check (DR4-001: prevent hijack from benchmark workdir)
        ANALYZE_RUN="$REPO_ROOT/scripts/analyze_run.py"
        extras_json="null"
        if [[ -f "$ANALYZE_RUN" && ! -L "$ANALYZE_RUN" ]] && command -v python3 &>/dev/null; then
          # python3 -I: isolated mode (no PYTHONPATH/sitecustomize from environment)
          raw_json=$(python3 -I "$ANALYZE_RUN" "$RUN_DIR" 2>/dev/null || echo "null")
          # jq -c: compact JSON + validate + whitelist fields (DR4-003: TSV/Markdown injection)
          if command -v jq &>/dev/null; then
            extras_json=$(printf '%s' "$raw_json" | jq -c '{
              anvil_score: .anvil_score,
              task_kind: .task_kind,
              pam_variant: .pam_variant,
              postcheck_success: .postcheck_success,
              postcheck_reason: .postcheck_reason,
              success_check_success: .success_check_success,
              success_check_reason: .success_check_reason,
              engine: .engine,
              tool_call_count: .tool_call_total,
              failure_kind: .failure_kind,
              token_prompt: .token_prompt,
              token_completion: .token_completion
            }' 2>/dev/null || echo "null")
          else
            extras_json="null"
          fi
        fi

        printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
          "$run" "$model" "$case_name" "$pam_variant" "$rc" "$elapsed" \
          "$workdir_rel" "$session_copied" "$extras_json" \
          >> "$BENCH_ROOT/summary.tsv"
        RUN_LOGGED=1

        CURRENT_RUN_DIR=""
        CURRENT_START_TS=""
        CURRENT_ACTIVE_FLAGS_JSON="[]"
        cd "$REPO_ROOT" || { echo "Error: cd $REPO_ROOT failed" >&2; exit 1; }
      done
    done
  done
  done

  if [[ -n "$models_arg" ]]; then
    generate_matrix_report "$BENCH_ROOT" "$benchmark_name" "$runs" "${cleaned_models[@]}"
  fi
done

echo "Done. Results: $BENCH_ROOT/summary.tsv"
