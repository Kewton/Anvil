#!/usr/bin/env python3
"""report.py — aggregate bench run metrics to markdown.

Usage:
    scripts/report.py BENCH_ROOT > report.md
    scripts/report.py --compare BENCH_ROOT_A BENCH_ROOT_B > compare.md

Calls scripts/analyze_run.py via subprocess for each
{model_slug}/run-{N}/ directory under BENCH_ROOT and renders a Markdown
summary to stdout. Warnings and errors go to stderr so that stdout can be
safely redirected.

Exit codes:
    0  success
    1  argument or BENCH_ROOT validation error
"""

from __future__ import annotations

import argparse
import datetime as _dt
import json
import os
import re
import statistics
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path

# ---------------------------------------------------------------------------
# constants
# ---------------------------------------------------------------------------

MODEL_SLUG_RE = re.compile(r"^[A-Za-z0-9._-]+$")
RUN_DIR_RE = re.compile(r"^run-(\d+)$")
ANALYZE_RUN = Path(__file__).resolve().parent / "analyze_run.py"
MAX_RUNS = 1000
SUBPROCESS_TIMEOUT = 60  # seconds per run-dir
STDERR_LIMIT = 2048
CV_WARN_THRESHOLD = 0.3

# Restrict child env: avoid PYTHONPATH / user-site / sitecustomize injection.
_MIN_ENV = {
    k: v
    for k, v in os.environ.items()
    if k in ("PATH", "HOME", "TMPDIR", "TEMP", "TMP")
}


@dataclass
class Column:
    key: str
    label: str
    cv_warn: bool
    type: str  # 'int' | 'float' | 'bool'


COLUMNS: list[Column] = [
    Column("rc", "rc", False, "int"),
    Column("postcheck_success", "postcheck", False, "bool"),
    Column("elapsed_s", "elapsed_s", True, "int"),
    Column("we_total", "we_total", True, "int"),
    Column("page_tsx_has_game_keywords", "page_game", False, "bool"),
    Column("iter_count", "iter_count", True, "int"),
    Column("error_500_count", "error_500", False, "int"),
    Column("compact_events", "compacts", False, "int"),
]


# ---------------------------------------------------------------------------
# helpers
# ---------------------------------------------------------------------------


def _warn(msg: str) -> None:
    print(f"warning: {msg}", file=sys.stderr)


def _sanitize_stderr(text: str, limit: int = STDERR_LIMIT) -> str:
    """Strip non-printable control chars (keep \\n / \\t) and bound length."""
    if not text:
        return ""
    cleaned = "".join(
        ch for ch in text if ch == "\n" or ch == "\t" or ord(ch) >= 0x20
    )
    return cleaned[:limit]


def _fmt_bool(v: object) -> str:
    if v is None:
        return "null"
    if v is True:
        return "yes"
    if v is False:
        return "no"
    return str(v)


def _fmt_cell(v: object) -> str:
    if v is None:
        return "null"
    if isinstance(v, bool):
        return _fmt_bool(v)
    return str(v)


def _fmt_num(v: object) -> str:
    if v is None or v == "N/A":
        return "N/A"
    if isinstance(v, float):
        return f"{v:.1f}"
    return str(v)


def _md_escape_cell(s: str) -> str:
    """Escape a string for safe embedding in a Markdown table cell.

    - Replace newlines / carriage returns with a single space so the cell
      cannot break the table row.
    - Escape pipe characters so they do not introduce new columns.
    - Escape backticks so they cannot open a code span that swallows the
      remaining cells.
    - Escape backslashes first to preserve any literal backslashes the
      author may have intended.
    """
    if not isinstance(s, str):
        s = str(s)
    s = s.replace("\\", "\\\\")
    s = s.replace("\r\n", " ").replace("\n", " ").replace("\r", " ")
    s = s.replace("`", "\\`")
    s = s.replace("|", "\\|")
    return s


# ---------------------------------------------------------------------------
# validation
# ---------------------------------------------------------------------------


def _validate_bench_root(raw: str) -> Path:
    """Return resolved BENCH_ROOT path or exit(1) on any rejection."""
    p = Path(raw)
    if p.is_symlink():
        print(f"error: BENCH_ROOT must not be a symlink: {raw}", file=sys.stderr)
        sys.exit(1)
    try:
        resolved = p.resolve(strict=True)
    except (OSError, RuntimeError):
        print(f"error: BENCH_ROOT not found: {raw}", file=sys.stderr)
        sys.exit(1)
    if not resolved.is_dir():
        print(
            f"error: BENCH_ROOT is not a directory: {raw}", file=sys.stderr
        )
        sys.exit(1)
    return resolved


def _resolve_in(p: Path, root: Path) -> Path | None:
    """Return resolved path only if it stays within root."""
    try:
        resolved = p.resolve(strict=True)
        root_resolved = root.resolve(strict=True)
    except (OSError, RuntimeError):
        return None
    return resolved if resolved.is_relative_to(root_resolved) else None


# ---------------------------------------------------------------------------
# discovery
# ---------------------------------------------------------------------------


def _run_sort_key(p: Path) -> int:
    m = RUN_DIR_RE.match(p.name)
    return int(m.group(1)) if m else -1


def _valid_suite_dir(p: Path, bench_root: Path) -> bool:
    if p.is_symlink() or not p.is_dir():
        return False
    if not MODEL_SLUG_RE.match(p.name):
        return False
    return _resolve_in(p, bench_root) is not None


def _append_run(
    runs: list[tuple[str, int, Path]],
    bench_root: Path,
    model_slug: str,
    run_dir: Path,
) -> bool:
    if not RUN_DIR_RE.match(run_dir.name):
        return False
    if run_dir.is_symlink():
        _warn(f"skipping symlink run-dir: {run_dir}")
        return False
    if not run_dir.is_dir():
        return False
    if _resolve_in(run_dir, bench_root) is None:
        _warn(f"skipping run-dir outside BENCH_ROOT: {run_dir}")
        return False
    runs.append((model_slug, int(run_dir.name[4:]), run_dir))
    return len(runs) >= MAX_RUNS


def _discover_runs(bench_root: Path) -> list[tuple[str, int, Path]]:
    """Walk bench_root and return sorted [(model_slug, run_n, run_dir), ...].

    Supports both legacy flat layout:
      <root>/<model>/run-N

    and suite/PAM layout:
      <root>/<model>/<case>/<pam_variant>/run-N
    """
    runs: list[tuple[str, int, Path]] = []
    try:
        children = sorted(bench_root.iterdir())
    except OSError as e:
        _warn(f"cannot list BENCH_ROOT: {e}")
        return runs
    for model_dir in children:
        if model_dir.is_symlink():
            _warn(f"skipping symlink model dir: {model_dir.name}")
            continue
        if not model_dir.is_dir():
            continue
        if not MODEL_SLUG_RE.match(model_dir.name):
            continue
        if _resolve_in(model_dir, bench_root) is None:
            _warn(f"skipping model dir outside BENCH_ROOT: {model_dir.name}")
            continue
        try:
            run_children = sorted(model_dir.iterdir(), key=_run_sort_key)
        except OSError as e:
            _warn(f"cannot list {model_dir.name}: {e}")
            continue
        for child in run_children:
            if RUN_DIR_RE.match(child.name):
                if _append_run(runs, bench_root, model_dir.name, child):
                    _warn(f"reached MAX_RUNS={MAX_RUNS}, truncating discovery")
                    return runs
                continue
            if not _valid_suite_dir(child, bench_root):
                continue
            try:
                case_children = sorted(child.iterdir(), key=_run_sort_key)
            except OSError as e:
                _warn(f"cannot list {child}: {e}")
                continue
            for nested in case_children:
                if RUN_DIR_RE.match(nested.name):
                    if _append_run(runs, bench_root, model_dir.name, nested):
                        _warn(f"reached MAX_RUNS={MAX_RUNS}, truncating discovery")
                        return runs
                    continue
                if not _valid_suite_dir(nested, bench_root):
                    continue
                try:
                    variant_children = sorted(nested.iterdir(), key=_run_sort_key)
                except OSError as e:
                    _warn(f"cannot list {nested}: {e}")
                    continue
                for run_dir in variant_children:
                    if _append_run(runs, bench_root, model_dir.name, run_dir):
                        _warn(f"reached MAX_RUNS={MAX_RUNS}, truncating discovery")
                        return runs
    return runs


# ---------------------------------------------------------------------------
# analyze_run.py spawn
# ---------------------------------------------------------------------------


def _analyze(run_dir: Path) -> tuple[int, dict | None, str]:
    """Returns (returncode, metrics_or_None, sanitized_stderr)."""
    try:
        result = subprocess.run(
            [sys.executable, "-I", str(ANALYZE_RUN), str(run_dir)],
            capture_output=True,
            text=True,
            timeout=SUBPROCESS_TIMEOUT,
            cwd=str(ANALYZE_RUN.parent),
            env=_MIN_ENV,
            shell=False,
            check=False,
        )
    except subprocess.TimeoutExpired:
        return -1, None, f"timeout after {SUBPROCESS_TIMEOUT}s"
    stderr_text = _sanitize_stderr(result.stderr)
    if result.returncode != 0:
        return result.returncode, None, stderr_text
    try:
        metrics = json.loads(result.stdout)
    except json.JSONDecodeError as e:
        return -1, None, _sanitize_stderr(str(e))
    if not isinstance(metrics, dict):
        return -1, None, "analyze_run.py output is not a JSON object"
    if metrics.get("schema_version") != 1:
        _warn(
            f"unexpected schema_version={metrics.get('schema_version')}, "
            "using known keys only"
        )
    return 0, metrics, stderr_text


# ---------------------------------------------------------------------------
# aggregation
# ---------------------------------------------------------------------------


def _success_rate(rows: list[dict]) -> tuple[str, int, int]:
    """Return (formatted_pct, n_success, n_with_rc)."""
    rcs = [r["rc"] for r in rows if r.get("rc") is not None]
    if not rcs:
        return "N/A", 0, 0
    success = sum(1 for rc in rcs if rc == 0)
    pct = 100 * success // len(rcs)
    return f"{pct}%", success, len(rcs)


def _bool_success_rate(rows: list[dict], key: str) -> tuple[str, int, int]:
    vals = [r.get(key) for r in rows if isinstance(r.get(key), bool)]
    if not vals:
        return "N/A", 0, 0
    success = sum(1 for v in vals if v is True)
    pct = 100 * success // len(vals)
    return f"{pct}%", success, len(vals)


def _aggregate(rows: list[dict]) -> dict:
    """Compute per-column statistics from list of analysis dicts."""
    result: dict = {}
    for col in COLUMNS:
        if col.key == "rc":
            continue
        raw_vals = [r.get(col.key) for r in rows]
        vals: list[float] = []
        for v in raw_vals:
            if v is None or isinstance(v, bool):
                continue
            if isinstance(v, (int, float)):
                vals.append(float(v))
        n = len(vals)
        if n == 0:
            result[col.key] = {
                "n": 0,
                "mean": "N/A",
                "median": "N/A",
                "min": "N/A",
                "max": "N/A",
                "cv_warn": False,
            }
            continue
        m = statistics.mean(vals)
        med = statistics.median(vals)
        mn = min(vals)
        mx = max(vals)
        cv_warn = False
        if col.cv_warn and n >= 2 and m != 0:
            sd = statistics.stdev(vals)
            cv_warn = (sd / abs(m)) > CV_WARN_THRESHOLD
        # Format ints as int, floats kept rounded.
        def _norm(x: float) -> object:
            if x.is_integer():
                return int(x)
            return round(x, 1)

        result[col.key] = {
            "n": n,
            "mean": round(m, 1),
            "median": _norm(med),
            "min": _norm(mn),
            "max": _norm(mx),
            "cv_warn": cv_warn,
        }
    return result


def _aggregate_tool_calls(rows: list[dict]) -> dict[str, int]:
    """Sum tool_calls across runs."""
    totals: dict[str, int] = {}
    for r in rows:
        tc = r.get("tool_calls")
        if not isinstance(tc, dict):
            continue
        for name, count in tc.items():
            if not isinstance(name, str) or not isinstance(count, int):
                continue
            totals[name] = totals.get(name, 0) + count
    return dict(sorted(totals.items()))


# ---------------------------------------------------------------------------
# rendering: per-bench-root report
# ---------------------------------------------------------------------------


def _now_iso() -> str:
    return _dt.datetime.now(_dt.timezone.utc).isoformat(timespec="seconds")


def _gather(bench_root: Path) -> tuple[list[tuple[str, int, Path]], list[dict]]:
    """Return (run_keys, parallel rows). Failed runs become a stub dict."""
    runs = _discover_runs(bench_root)
    rows: list[dict] = []
    for model_slug, run_n, run_dir in runs:
        rc, metrics, stderr_text = _analyze(run_dir)
        if metrics is None:
            if stderr_text:
                _warn(
                    f"{model_slug}/run-{run_n}: analyze_run.py rc={rc}: "
                    f"{stderr_text.strip()[:512]}"
                )
            else:
                _warn(f"{model_slug}/run-{run_n}: analyze_run.py rc={rc}")
            rows.append(
                {
                    "_model": model_slug,
                    "_run_n": run_n,
                    "_failed": True,
                    "_analyze_rc": rc,
                }
            )
        else:
            metrics["_model"] = model_slug
            metrics["_run_n"] = run_n
            metrics["_failed"] = False
            rows.append(metrics)
    return runs, rows


def _render_run_summary(rows: list[dict]) -> list[str]:
    headers = ["run", "model", "case", "task_kind", "pam"] + [c.label for c in COLUMNS]
    lines: list[str] = []
    lines.append("| " + " | ".join(headers) + " |")
    lines.append("|" + "|".join("-----" for _ in headers) + "|")
    for r in rows:
        cells: list[str] = [
            str(r["_run_n"]),
            _md_escape_cell(r["_model"]),
            _md_escape_cell(r.get("case", "default")),
            _md_escape_cell(r.get("task_kind", "coding")),
            _md_escape_cell(r.get("pam_variant", "default")),
        ]
        if r.get("_failed"):
            # rc column shows the analyze_run.py rc; rest are N/A.
            cells.append(str(r.get("_analyze_rc", "")))
            for _ in COLUMNS[1:]:
                cells.append("N/A")
        else:
            for col in COLUMNS:
                v = r.get(col.key)
                if col.type == "bool":
                    cells.append(_fmt_bool(v))
                else:
                    cells.append(_fmt_cell(v))
        lines.append("| " + " | ".join(cells) + " |")
    return lines


def _render_aggregate(rows: list[dict]) -> tuple[list[str], list[str]]:
    """Return (table_lines, warning_lines)."""
    success_rate, _ok, _tot = _success_rate(rows)
    only_ok = [r for r in rows if not r.get("_failed")]
    agg = _aggregate(only_ok)

    headers = ["metric", "n", "mean", "median", "min", "max"]
    lines: list[str] = []
    lines.append("| " + " | ".join(headers) + " |")
    lines.append("|" + "|".join("-----" for _ in headers) + "|")
    warnings: list[str] = []
    for col in COLUMNS:
        if col.key == "rc":
            lines.append(
                "| "
                + " | ".join(
                    [
                        "success_rate",
                        str(len([r for r in rows if r.get("rc") is not None])),
                        success_rate,
                        "-",
                        "-",
                        "-",
                    ]
                )
                + " |"
            )
            continue
        if col.type == "bool":
            rate, ok, total = _bool_success_rate(only_ok, col.key)
            lines.append(
                "| "
                + " | ".join(
                    [
                        f"{col.label}_rate",
                        str(total),
                        rate,
                        f"{ok}/{total}" if total else "-",
                        "-",
                        "-",
                    ]
                )
                + " |"
            )
            continue
        stats = agg.get(col.key, {})
        label = col.label
        if stats.get("cv_warn"):
            label = f"{col.label} ⚠"
            warnings.append(col.label)
        cells = [
            label,
            str(stats.get("n", 0)),
            _fmt_num(stats.get("mean")),
            _fmt_num(stats.get("median")),
            _fmt_num(stats.get("min")),
            _fmt_num(stats.get("max")),
        ]
        lines.append("| " + " | ".join(cells) + " |")
    return lines, warnings


def _terminal_postcheck_summary(rows: list[dict], group_keys: list[str]) -> list[list[str]]:
    groups: dict[tuple[str, ...], list[dict]] = {}
    for r in rows:
        if r.get("_failed"):
            continue
        key = tuple(str(r.get(k, "default")) for k in group_keys)
        groups.setdefault(key, []).append(r)

    out: list[list[str]] = []
    for key in sorted(groups):
        sub = groups[key]
        term_rate, term_ok, term_total = _success_rate(sub)
        post_rate, post_ok, post_total = _bool_success_rate(sub, "postcheck_success")
        both_total = 0
        both_ok = 0
        for r in sub:
            if r.get("rc") is None or not isinstance(r.get("postcheck_success"), bool):
                continue
            both_total += 1
            if r.get("rc") == 0 and r.get("postcheck_success") is True:
                both_ok += 1
        both_rate = "N/A" if both_total == 0 else f"{100 * both_ok // both_total}%"
        out.append(
            [
                *[_md_escape_cell(v) for v in key],
                str(len(sub)),
                f"{term_rate} ({term_ok}/{term_total})",
                f"{post_rate} ({post_ok}/{post_total})",
                f"{both_rate} ({both_ok}/{both_total})" if both_total else "N/A",
            ]
        )
    return out


def _render_task_kind_summary(rows: list[dict]) -> list[str]:
    table_rows = _terminal_postcheck_summary(rows, ["task_kind"])
    lines = ["## Task Kind Summary", ""]
    if not table_rows:
        lines.append("(no completed analyses)")
        return lines
    headers = ["task_kind", "runs", "terminal_success", "postcheck_success", "both_success"]
    lines.append("| " + " | ".join(headers) + " |")
    lines.append("|" + "|".join("-----" for _ in headers) + "|")
    for row in table_rows:
        lines.append("| " + " | ".join(row) + " |")
    return lines


def _render_pam_task_kind_summary(rows: list[dict]) -> list[str]:
    table_rows = _terminal_postcheck_summary(rows, ["task_kind", "pam_variant"])
    lines = ["## PAM By Task Kind", ""]
    if not table_rows:
        lines.append("(no completed analyses)")
        return lines
    headers = [
        "task_kind",
        "pam_variant",
        "runs",
        "terminal_success",
        "postcheck_success",
        "both_success",
    ]
    lines.append("| " + " | ".join(headers) + " |")
    lines.append("|" + "|".join("-----" for _ in headers) + "|")
    for row in table_rows:
        lines.append("| " + " | ".join(row) + " |")
    return lines


def _render_report(bench_root: Path, rows: list[dict]) -> str:
    parts: list[str] = []
    parts.append(f"# Benchmark Report: {bench_root.name}")
    parts.append(f"Generated: {_now_iso()}")
    parts.append("")
    parts.append("## Run Summary")
    parts.append("")
    if rows:
        parts.extend(_render_run_summary(rows))
    else:
        parts.append("(no runs discovered)")
    parts.append("")
    parts.append("## Aggregate Statistics")
    parts.append("")
    if rows:
        agg_lines, warnings = _render_aggregate(rows)
        parts.extend(agg_lines)
        if warnings:
            parts.append("")
            parts.append(
                "> ⚠ CV > 0.3 detected for: " + ", ".join(warnings)
            )
    else:
        parts.append("(no runs discovered)")
    parts.append("")
    if rows:
        parts.extend(_render_task_kind_summary(rows))
        parts.append("")
        parts.extend(_render_pam_task_kind_summary(rows))
        parts.append("")
    return "\n".join(parts)


# ---------------------------------------------------------------------------
# rendering: A/B compare
# ---------------------------------------------------------------------------


def _diff_str(a: object, b: object) -> tuple[str, str]:
    """Return (diff, diff_pct) cells for two numeric/null values."""
    if isinstance(a, bool) or isinstance(b, bool):
        return "-", "-"
    if not isinstance(a, (int, float)) or not isinstance(b, (int, float)):
        return "-", "-"
    diff = b - a
    sign = "+" if diff >= 0 else ""
    diff_cell = f"{sign}{diff:.1f}" if isinstance(diff, float) else f"{sign}{diff}"
    if a == 0:
        pct_cell = "-"
    else:
        pct = (b - a) / abs(a) * 100.0
        pct_sign = "+" if pct >= 0 else ""
        pct_cell = f"{pct_sign}{pct:.1f}%"
    return diff_cell, pct_cell


def _render_compare_section(model: str, rows_a: list[dict], rows_b: list[dict]) -> list[str]:
    only_a = [r for r in rows_a if not r.get("_failed")]
    only_b = [r for r in rows_b if not r.get("_failed")]
    agg_a = _aggregate(only_a)
    agg_b = _aggregate(only_b)
    sr_a, _, _ = _success_rate(rows_a)
    sr_b, _, _ = _success_rate(rows_b)
    n_a = len(rows_a)
    n_b = len(rows_b)

    lines: list[str] = []
    lines.append(f"### {model}")
    lines.append("")
    if n_a != n_b:
        lines.append(f"> note: nA={n_a} != nB={n_b}")
        lines.append("")
    headers = ["metric", f"A (n={n_a})", f"B (n={n_b})", "diff", "diff%"]
    lines.append("| " + " | ".join(headers) + " |")
    lines.append("|" + "|".join("-----" for _ in headers) + "|")
    # success rate row
    lines.append("| " + " | ".join(["success_rate", sr_a, sr_b, "-", "-"]) + " |")
    for col in COLUMNS:
        if col.key == "rc":
            continue
        if col.type == "bool":
            rate_a, ok_a, total_a = _bool_success_rate(only_a, col.key)
            rate_b, ok_b, total_b = _bool_success_rate(only_b, col.key)
            if total_a and total_b:
                diff = (ok_b / total_b) - (ok_a / total_a)
                sign = "+" if diff >= 0 else ""
                diff_cell = f"{sign}{diff * 100:.1f}pt"
            else:
                diff_cell = "-"
            cells = [
                f"{col.label}_rate",
                f"{rate_a} ({ok_a}/{total_a})" if total_a else "N/A",
                f"{rate_b} ({ok_b}/{total_b})" if total_b else "N/A",
                diff_cell,
                "-",
            ]
            lines.append("| " + " | ".join(cells) + " |")
            continue
        sa = agg_a.get(col.key, {})
        sb = agg_b.get(col.key, {})
        mean_a = sa.get("mean")
        mean_b = sb.get("mean")
        a_na = mean_a == "N/A"
        b_na = mean_b == "N/A"
        if a_na and b_na:
            diff_cell, pct_cell = "-", "-"
        elif a_na or b_na:
            diff_cell, pct_cell = "-", "-"
        else:
            diff_cell, pct_cell = _diff_str(mean_a, mean_b)
        # When only one side is N/A, annotate the side that DOES have data
        # so a "A=N/A, B=5.3" row is visually distinct from "A=N/A, B=N/A".
        # The diff/diff% cells stay "-" because no comparison is possible,
        # but the annotation makes it clear that one side actually had data.
        a_cell = _fmt_num(mean_a)
        b_cell = _fmt_num(mean_b)
        if a_na and not b_na:
            b_cell = f"{b_cell} (A: N/A)"
        elif b_na and not a_na:
            a_cell = f"{a_cell} (B: N/A)"
        cells = [
            f"mean {col.label}",
            a_cell,
            b_cell,
            diff_cell,
            pct_cell,
        ]
        lines.append("| " + " | ".join(cells) + " |")
    return lines


def _render_tool_calls_compare(rows_a: list[dict], rows_b: list[dict]) -> list[str]:
    tot_a = _aggregate_tool_calls([r for r in rows_a if not r.get("_failed")])
    tot_b = _aggregate_tool_calls([r for r in rows_b if not r.get("_failed")])
    names = sorted(set(tot_a) | set(tot_b))
    if not names:
        return []
    lines = ["## Tool Call Comparison", ""]
    headers = ["tool", "A total", "B total", "diff"]
    lines.append("| " + " | ".join(headers) + " |")
    lines.append("|" + "|".join("-----" for _ in headers) + "|")
    for name in names:
        a = tot_a.get(name, 0)
        b = tot_b.get(name, 0)
        diff = b - a
        sign = "+" if diff >= 0 else ""
        # tool_calls.name comes from session.json which we treat as
        # untrusted input; escape it so it cannot inject pipe characters,
        # newlines, or backticks that would break the Markdown table.
        safe_name = _md_escape_cell(name)
        lines.append(
            "| " + " | ".join([safe_name, str(a), str(b), f"{sign}{diff}"]) + " |"
        )
    return lines


def _models_in(rows: list[dict]) -> list[str]:
    seen = []
    for r in rows:
        m = r.get("_model")
        if m and m not in seen:
            seen.append(m)
    return seen


def _render_compare(root_a: Path, root_b: Path, rows_a: list[dict], rows_b: list[dict]) -> str:
    parts: list[str] = []
    parts.append("# Benchmark Comparison")
    parts.append(f"A: {root_a.name}")
    parts.append(f"B: {root_b.name}")
    parts.append(f"Generated: {_now_iso()}")
    parts.append("")

    models_a = _models_in(rows_a)
    models_b = _models_in(rows_b)
    set_a = set(models_a)
    set_b = set(models_b)
    common = sorted(set_a & set_b)
    only_a = sorted(set_a - set_b)
    only_b = sorted(set_b - set_a)

    parts.append("## Common Models")
    parts.append("")
    if not common:
        parts.append("(none)")
    else:
        for model in common:
            sub_a = [r for r in rows_a if r.get("_model") == model]
            sub_b = [r for r in rows_b if r.get("_model") == model]
            parts.extend(_render_compare_section(model, sub_a, sub_b))
            parts.append("")

    parts.append("## A-only Models")
    parts.append("")
    if only_a:
        for m in only_a:
            parts.append(f"- {m}")
    else:
        parts.append("(none)")
    parts.append("")

    parts.append("## B-only Models")
    parts.append("")
    if only_b:
        for m in only_b:
            parts.append(f"- {m}")
    else:
        parts.append("(none)")
    parts.append("")

    tc_lines = _render_tool_calls_compare(rows_a, rows_b)
    if tc_lines:
        parts.extend(tc_lines)
        parts.append("")

    return "\n".join(parts)


# ---------------------------------------------------------------------------
# entry points
# ---------------------------------------------------------------------------


def _cmd_report(raw_bench_root: str) -> int:
    bench_root = _validate_bench_root(raw_bench_root)
    _runs, rows = _gather(bench_root)
    sys.stdout.write(_render_report(bench_root, rows))
    return 0


def _cmd_compare(raw_a: str, raw_b: str) -> int:
    root_a = _validate_bench_root(raw_a)
    root_b = _validate_bench_root(raw_b)
    _runs_a, rows_a = _gather(root_a)
    _runs_b, rows_b = _gather(root_b)
    sys.stdout.write(_render_compare(root_a, root_b, rows_a, rows_b))
    return 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description="Aggregate bench run metrics to a Markdown report",
    )
    parser.add_argument(
        "bench_root",
        nargs="?",
        metavar="BENCH_ROOT",
        help="Root directory containing {model_slug}/run-{N}/ subdirectories",
    )
    parser.add_argument(
        "--compare",
        nargs=2,
        metavar=("A", "B"),
        help="Render an A/B comparison report instead of a single-root report",
    )
    args = parser.parse_args(argv)

    if args.compare:
        return _cmd_compare(args.compare[0], args.compare[1])
    if args.bench_root:
        return _cmd_report(args.bench_root)
    parser.print_help(sys.stderr)
    return 1


if __name__ == "__main__":
    os.umask(0o077)
    sys.exit(main())
