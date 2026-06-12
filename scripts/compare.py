#!/usr/bin/env python3
"""compare.py — A/B comparison of two bench BENCH_ROOTs.

Reads two BENCH_ROOT directories (produced by scripts/bench.sh) and emits a
diff Markdown / JSON summary of metrics computed by scripts/analyze_run.py.

Usage:
    scripts/compare.py [--metric METRIC[,METRIC...]] [--format markdown|json]
                       [--threshold FLOAT] baseline_dir experiment_dir

Exit codes:
    0  success
    1  argument / security / shape error
    2  missing directory / zero runs
    3  insufficient valid runs (< MIN_VALID_RUNS)

See docs/compare.md for full specification.
"""

from __future__ import annotations

import argparse
import json
import math
import os
import stat
import statistics
import subprocess
import sys
from dataclasses import dataclass, field
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Callable, Literal

# ---------------------------------------------------------------------------
# constants
# ---------------------------------------------------------------------------

SCHEMA_VERSION = 1
DEFAULT_THRESHOLD_PCT = 0.05  # continuous metrics: relative-delta threshold (5%)
DEFAULT_THRESHOLD_ABS = 0.05  # bool_rate metrics: absolute-delta threshold (5pt)
MAX_RUNS = 500  # DoS guard; supports 20-30 scenarios x 5 runs x engine matrices.
MIN_VALID_RUNS = 3  # below this -> exit 3
ALPHA = 0.05  # 95% CI
UMASK = 0o077

# metric-name aliases -> canonical metric key
ALIASES: dict[str, str] = {
    "postcheck": "postcheck_success",
    "rc0": "rc",
    "page_game": "page_tsx_has_game_keywords",
}

# canonical key -> aggregation spec
METRIC_TYPE_SPEC: dict[str, dict[str, Any]] = {
    "rc": {"type": "bool_rate", "direction": "up"},
    "postcheck_success": {"type": "bool_rate", "direction": "up"},
    "page_tsx_has_game_keywords": {"type": "bool_rate", "direction": "up"},
    "we_total": {"type": "informational", "direction": None},
    "elapsed_s": {"type": "continuous", "direction": "down"},
    "iter_count": {"type": "continuous", "direction": "down"},
    "error_500_count": {"type": "continuous", "direction": "down"},
}

VERDICT_EMOJI: dict[str, str] = {
    "improved": "\u2705",
    "regressed": "\u274c",
    "unchanged": "\u2796",
    "informational": "\u2139\ufe0f",
}

MetricType = Literal["continuous", "bool_rate", "informational"]
Verdict = Literal["improved", "regressed", "unchanged", "informational"]


# ---------------------------------------------------------------------------
# data model
# ---------------------------------------------------------------------------


@dataclass
class AggResult:
    mean: float | None
    ci_low: float | None
    ci_high: float | None
    n: int


@dataclass
class MetricResult:
    key: str
    metric_type: MetricType
    baseline: AggResult
    experiment: AggResult
    delta: float | None
    delta_pct: float | None
    verdict: Verdict


@dataclass
class CompareResult:
    schema_version: int
    baseline_dir: Path
    experiment_dir: Path
    model_slug: str
    generated_at: str
    threshold: float
    metrics: dict[str, MetricResult] = field(default_factory=dict)
    failure_categories: dict[str, dict[str, int]] = field(default_factory=dict)


# ---------------------------------------------------------------------------
# helpers
# ---------------------------------------------------------------------------


def _warn(msg: str) -> None:
    print(f"warning: {msg}", file=sys.stderr)


def _die(msg: str, code: int) -> None:
    print(f"error: {msg}", file=sys.stderr)
    sys.exit(code)


def _is_regular_file(p: Path) -> bool:
    try:
        st = p.lstat()
    except OSError:
        return False
    if stat.S_ISLNK(st.st_mode):
        return False
    if not stat.S_ISREG(st.st_mode):
        return False
    return True


def _resolve_analyze_run_path() -> Path:
    """Resolve the path to scripts/analyze_run.py next to this compare.py.

    Rejects symlinks / non-regular files / paths outside the compare.py repo.
    Exits 1 on violation.
    """
    compare_path = Path(__file__).resolve(strict=True)
    scripts_dir = compare_path.parent
    repo_root = scripts_dir.parent
    candidate = scripts_dir / "analyze_run.py"
    if candidate.is_symlink():
        _die(f"analyze_run.py must not be a symlink: {candidate}", 1)
    if not candidate.exists():
        _die(f"analyze_run.py not found: {candidate}", 1)
    try:
        resolved = candidate.resolve(strict=True)
    except (OSError, RuntimeError):
        _die(f"analyze_run.py cannot be resolved: {candidate}", 1)
    try:
        repo_resolved = repo_root.resolve(strict=True)
    except (OSError, RuntimeError):
        _die(f"repo root cannot be resolved: {repo_root}", 1)
    if not resolved.is_relative_to(repo_resolved):
        _die(f"analyze_run.py resolves outside repo: {resolved}", 1)
    if not _is_regular_file(resolved):
        _die(f"analyze_run.py is not a regular file: {resolved}", 1)
    return resolved


def _validate_input(path: Path) -> Path:
    """Validate a bench-root positional argument.

    - reject if the top-level path is a symlink (exit 1)
    - resolve strictly; missing path -> exit 2
    - must be a directory (exit 2)
    """
    if path.is_symlink():
        _die(f"input must not be a symlink: {path}", 1)
    try:
        resolved = path.resolve(strict=True)
    except (OSError, RuntimeError):
        _die(f"input directory not found: {path}", 2)
    if not resolved.is_dir():
        _die(f"input is not a directory: {path}", 2)
    return resolved


def _find_model_slug(baseline: Path, experiment: Path) -> str:
    """Return the shared single model_slug of baseline/experiment BENCH_ROOTs.

    Exits 1 if either side has 0 or >1 slugs, or if the two do not match.
    """

    def _one(root: Path, label: str) -> str:
        try:
            entries = [
                p for p in sorted(root.iterdir()) if p.is_dir() and not p.is_symlink()
            ]
        except OSError as e:
            _die(f"{label} listing failed: {e}", 1)
            raise  # unreachable (satisfy type checker)
        slugs = [p.name for p in entries]
        if len(slugs) == 0:
            _die(f"{label} contains no model_slug directory: {root}", 1)
        if len(slugs) > 1:
            _die(
                f"{label} contains multiple model_slug directories "
                f"({', '.join(slugs)}); v1 supports a single slug only",
                1,
            )
        return slugs[0]

    b_slug = _one(baseline, "baseline_dir")
    e_slug = _one(experiment, "experiment_dir")
    if b_slug != e_slug:
        _die(
            f"model_slug mismatch: baseline={b_slug!r}, experiment={e_slug!r}",
            1,
        )
    return b_slug


def _find_single_model_slug(root: Path) -> str:
    try:
        entries = [p for p in sorted(root.iterdir()) if p.is_dir() and not p.is_symlink()]
    except OSError as e:
        _die(f"bench root listing failed: {e}", 1)
        raise
    slugs = [p.name for p in entries]
    if len(slugs) == 0:
        _die(f"BENCH_ROOT contains no model_slug directory: {root}", 1)
    if len(slugs) > 1:
        _die(
            f"BENCH_ROOT contains multiple model_slug directories "
            f"({', '.join(slugs)}); engine comparison supports a single slug only",
            1,
        )
    return slugs[0]


def _collect_run_dirs(root: Path, slug: str) -> list[Path]:
    """Collect run-* directories under root/<slug>/.

    - symlinks are skipped (with a warning)
    - if resulting count > MAX_RUNS -> exit 1 (security)
    - supports both flat model/run-N and nested model/case/pam/run-N layouts
    """
    slug_dir = root / slug
    runs: list[Path] = []

    def _append_if_run(p: Path) -> None:
        if not p.name.startswith("run-"):
            return
        suffix = p.name[len("run-") :]
        if not suffix.isdigit():
            _warn(f"skipping non-numeric run-dir: {p}")
            return
        if p.is_symlink():
            _warn(f"skipping symlink run-dir: {p}")
            return
        if not p.is_dir():
            return
        runs.append(p)

    def _walk(p: Path, depth: int) -> None:
        if depth > 5:
            return
        try:
            entries = sorted(p.iterdir())
        except OSError as e:
            _warn(f"cannot list {p}: {e}")
            return
        for child in entries:
            if child.name.startswith("run-"):
                _append_if_run(child)
                continue
            if child.is_symlink() or not child.is_dir():
                continue
            _walk(child, depth + 1)

    _walk(slug_dir, 0)
    if len(runs) > MAX_RUNS:
        _die(
            f"run-dir count {len(runs)} exceeds MAX_RUNS={MAX_RUNS} for {slug_dir}",
            1,
        )
    return runs


def _failure_category_counts(runs: list[dict[str, Any]]) -> dict[str, int]:
    counts: dict[str, int] = {}
    for run in runs:
        category = run.get("generic_terminal_state")
        if not isinstance(category, str) or not category:
            observation = run.get("failure_observation")
            if isinstance(observation, dict):
                raw = observation.get("failure_class")
                if isinstance(raw, str) and raw:
                    category = raw
        if not isinstance(category, str) or not category:
            category = "unknown"
        counts[category] = counts.get(category, 0) + 1
    return dict(sorted(counts.items()))


def _now() -> str:
    """Return ISO 8601 UTC now, overridable via COMPARE_NOW env var."""
    env = os.environ.get("COMPARE_NOW")
    if env:
        return env
    return datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def _build_subprocess_env() -> dict[str, str]:
    """Build a subprocess env without Python-behavior-altering variables."""
    strip_keys = {
        "PYTHONPATH",
        "PYTHONHOME",
        "PYTHONSTARTUP",
        "PYTHONINSPECT",
        "PYTHONDONTWRITEBYTECODE",
        "PYTHONUNBUFFERED",
    }
    env = {k: v for k, v in os.environ.items() if k not in strip_keys}
    return env


def _filter_runs(runs: list[dict[str, Any]]) -> list[dict[str, Any]]:
    """Remove runs with rc==130 (user-interrupted runs)."""
    out: list[dict[str, Any]] = []
    for r in runs:
        if r.get("rc") == 130:
            continue
        out.append(r)
    return out


def _safe_number(v: Any) -> float | None:
    """Convert v to float; return None for non-numeric / NaN / inf / bool."""
    if v is None:
        return None
    if isinstance(v, bool):
        # treat booleans as 0/1 for aggregation convenience
        return 1.0 if v else 0.0
    if isinstance(v, (int, float)):
        f = float(v)
        if math.isnan(f) or math.isinf(f):
            return None
        return f
    return None


# ---------------------------------------------------------------------------
# confidence intervals
# ---------------------------------------------------------------------------

# two-sided t critical values at alpha=0.05 for df=1..30. For df>30, we fall
# back to the z approximation (1.96).
_T_TABLE_95: dict[int, float] = {
    1: 12.7062,
    2: 4.3027,
    3: 3.1824,
    4: 2.7764,
    5: 2.5706,
    6: 2.4469,
    7: 2.3646,
    8: 2.3060,
    9: 2.2622,
    10: 2.2281,
    11: 2.2010,
    12: 2.1788,
    13: 2.1604,
    14: 2.1448,
    15: 2.1314,
    16: 2.1199,
    17: 2.1098,
    18: 2.1009,
    19: 2.0930,
    20: 2.0860,
    21: 2.0796,
    22: 2.0739,
    23: 2.0687,
    24: 2.0639,
    25: 2.0595,
    26: 2.0555,
    27: 2.0518,
    28: 2.0484,
    29: 2.0452,
    30: 2.0423,
}


def _t_critical_95(df: int) -> float:
    if df <= 0:
        return float("nan")
    if df in _T_TABLE_95:
        return _T_TABLE_95[df]
    # large-df fallback -> normal z
    return 1.96


def _t_ci(
    values: list[float], alpha: float = ALPHA
) -> tuple[float | None, float | None]:
    """t-distribution two-sided CI for the mean at 1-alpha (default 95%).

    Returns (None, None) if fewer than 2 finite values are supplied.
    alpha is kept as a parameter for API symmetry with _wilson_ci; only
    alpha=0.05 is backed by the lookup table.
    """
    n = len(values)
    if n < 2:
        return (None, None)
    mean = statistics.fmean(values)
    try:
        stdev = statistics.stdev(values)
    except statistics.StatisticsError:
        return (None, None)
    # degrees of freedom = n - 1
    tcrit = _t_critical_95(n - 1)
    se = stdev / math.sqrt(n)
    margin = tcrit * se
    return (mean - margin, mean + margin)


def _wilson_ci(
    k: int, n: int, alpha: float = ALPHA
) -> tuple[float | None, float | None]:
    """Wilson score two-sided CI for a proportion k/n at 1-alpha (default 95%).

    Only alpha=0.05 is supported (z=1.96). Returns (None, None) if n<=0.
    """
    if n <= 0:
        return (None, None)
    z = 1.96  # two-sided 95%
    p = k / n
    denom = 1.0 + (z * z) / n
    center = p + (z * z) / (2.0 * n)
    margin = z * math.sqrt((p * (1.0 - p) / n) + (z * z) / (4.0 * n * n))
    low = (center - margin) / denom
    high = (center + margin) / denom
    # Clamp to [0, 1] to avoid -0.0 or fp leak past the bounds.
    low = max(0.0, min(1.0, low))
    high = max(0.0, min(1.0, high))
    return (low, high)


# ---------------------------------------------------------------------------
# analyze_run.py invocation
# ---------------------------------------------------------------------------


def _analyze_one(
    run_dir: Path,
    analyze_path: Path,
    *,
    runner: Callable[..., subprocess.CompletedProcess[str]] = subprocess.run,
) -> dict[str, Any] | None:
    """Invoke analyze_run.py for a single run-dir.

    Returns a dict on success, None on any failure (with a stderr warning).
    """
    try:
        result = runner(
            [sys.executable, "-I", str(analyze_path), str(run_dir)],
            capture_output=True,
            text=True,
            check=False,
            shell=False,
            env=_build_subprocess_env(),
            cwd=str(analyze_path.parent.parent),
            timeout=60,
        )
    except subprocess.TimeoutExpired:
        _warn(f"analyze_run.py timed out for {run_dir}")
        return None
    except OSError as e:
        _warn(f"analyze_run.py spawn failed for {run_dir}: {e}")
        return None
    if result.returncode != 0:
        fallback = _analyze_meta_only(run_dir)
        if fallback is not None:
            _warn(
                f"analyze_run.py rc={result.returncode} for {run_dir}; "
                "using meta.json fallback"
            )
            return fallback
        _warn(
            f"analyze_run.py rc={result.returncode} for {run_dir}: "
            f"{(result.stderr or '').strip()}"
        )
        return None
    try:
        data = json.loads(result.stdout)
    except json.JSONDecodeError as e:
        _warn(f"analyze_run.py non-JSON for {run_dir}: {e}")
        return None
    if not isinstance(data, dict):
        _warn(f"analyze_run.py output not a dict for {run_dir}")
        return None
    sv = data.get("schema_version")
    if sv != 1:
        _warn(f"analyze_run.py schema_version={sv} for {run_dir}, skipping")
        return None
    return data


def _analyze_meta_only(run_dir: Path) -> dict[str, Any] | None:
    """Build minimal comparable metrics from run-dir/meta.json.

    Some failed Anvil runs exit before a session.json is copied. Those runs are
    still valid benchmark failures and must count toward rc / elapsed /
    postcheck rates.
    """
    meta_path = run_dir / "meta.json"
    if not _is_regular_file(meta_path):
        return None
    try:
        meta = json.loads(meta_path.read_text())
    except (OSError, json.JSONDecodeError):
        return None
    if not isinstance(meta, dict):
        return None

    rc = meta.get("rc")
    elapsed_s = meta.get("elapsed_s")
    success = meta.get("success_check_success")
    failure_kind = meta.get("failure_kind")
    failure_class = failure_kind if isinstance(failure_kind, str) and failure_kind else "run_error"
    terminal_state = "completed" if rc == 0 else "failed"

    return {
        "schema_version": 1,
        "run_id": run_dir.name,
        "rc": rc,
        "elapsed_s": elapsed_s,
        "iter_count": None,
        "error_500_count": 0,
        "we_total": 0,
        "postcheck_success": success,
        "postcheck_reason": meta.get("success_check_reason"),
        "success_check_success": success,
        "success_check_reason": meta.get("success_check_reason"),
        "engine": meta.get("engine"),
        "task_kind": meta.get("task_kind"),
        "pam_variant": meta.get("pam_variant"),
        "failure_kind": failure_kind,
        "generic_terminal_state": terminal_state,
        "failure_observation": {
            "failure_class": failure_class,
            "terminal_state": terminal_state,
            "postcheck_success": success,
        },
        "page_tsx_has_game_keywords": None,
    }


# ---------------------------------------------------------------------------
# aggregation
# ---------------------------------------------------------------------------


def _aggregate(values: list[Any], metric_key: str) -> AggResult:
    """Aggregate a list of per-run values for a single metric key."""
    spec = METRIC_TYPE_SPEC.get(metric_key, {"type": "continuous", "direction": None})
    mtype = spec["type"]

    if metric_key == "rc":
        # bool_rate: success = (rc == 0); skip non-int / None / bool
        bools: list[bool] = []
        for v in values:
            if v is None:
                continue
            if isinstance(v, bool):
                continue
            if isinstance(v, int):
                bools.append(v == 0)
        n = len(bools)
        if n == 0:
            return AggResult(mean=None, ci_low=None, ci_high=None, n=0)
        k = sum(1 for b in bools if b)
        mean = k / n
        low, high = _wilson_ci(k, n)
        return AggResult(mean=mean, ci_low=low, ci_high=high, n=n)

    if mtype == "bool_rate":
        bools2: list[bool] = []
        for v in values:
            if v is None:
                continue
            if isinstance(v, bool):
                bools2.append(v)
        n2 = len(bools2)
        if n2 == 0:
            return AggResult(mean=None, ci_low=None, ci_high=None, n=0)
        k2 = sum(1 for b in bools2 if b)
        mean2 = k2 / n2
        low2, high2 = _wilson_ci(k2, n2)
        return AggResult(mean=mean2, ci_low=low2, ci_high=high2, n=n2)

    # continuous / informational: numeric aggregate
    nums: list[float] = []
    for v in values:
        f = _safe_number(v)
        if f is None:
            continue
        nums.append(f)
    n3 = len(nums)
    if n3 == 0:
        return AggResult(mean=None, ci_low=None, ci_high=None, n=0)
    mean3 = statistics.fmean(nums)
    if n3 >= 2:
        low3, high3 = _t_ci(nums)
    else:
        low3, high3 = (None, None)
    return AggResult(mean=mean3, ci_low=low3, ci_high=high3, n=n3)


# ---------------------------------------------------------------------------
# verdict
# ---------------------------------------------------------------------------


def _verdict(mr: MetricResult, threshold: float) -> Verdict:
    mtype = mr.metric_type
    spec = METRIC_TYPE_SPEC.get(mr.key, {"type": mtype, "direction": None})
    direction = spec.get("direction")

    if mtype == "informational":
        if mr.delta is None or mr.delta == 0:
            return "unchanged"
        return "informational"

    if mr.delta is None:
        return "unchanged"

    if mtype == "bool_rate":
        # absolute-delta against threshold (default 5pt = 0.05)
        thr = threshold
        if direction == "up":
            if mr.delta > thr:
                return "improved"
            if mr.delta < -thr:
                return "regressed"
            return "unchanged"
        if direction == "down":
            if mr.delta < -thr:
                return "improved"
            if mr.delta > thr:
                return "regressed"
            return "unchanged"
        return "unchanged"

    # continuous
    # Prefer delta_pct if available; otherwise fall back to sign of delta.
    if mr.delta_pct is None:
        # baseline==0 case -> evaluate by sign of delta only
        if direction == "up":
            return (
                "improved"
                if mr.delta > 0
                else "regressed"
                if mr.delta < 0
                else "unchanged"
            )
        if direction == "down":
            return (
                "improved"
                if mr.delta < 0
                else "regressed"
                if mr.delta > 0
                else "unchanged"
            )
        return "unchanged"

    thr = threshold
    if direction == "up":
        if mr.delta_pct > thr:
            return "improved"
        if mr.delta_pct < -thr:
            return "regressed"
        return "unchanged"
    if direction == "down":
        if mr.delta_pct < -thr:
            return "improved"
        if mr.delta_pct > thr:
            return "regressed"
        return "unchanged"
    return "unchanged"


# ---------------------------------------------------------------------------
# compare
# ---------------------------------------------------------------------------


def _resolve_metric_keys(requested: list[str] | None) -> list[str]:
    """Resolve --metric arg list with alias expansion.

    Unknown keys -> exit 1.
    """
    if not requested:
        return list(METRIC_TYPE_SPEC.keys())
    resolved: list[str] = []
    seen: set[str] = set()
    for raw in requested:
        name = raw.strip()
        if not name:
            continue
        canonical = ALIASES.get(name, name)
        if canonical not in METRIC_TYPE_SPEC:
            _die(f"unknown metric key: {raw!r}", 1)
        if canonical in seen:
            continue
        seen.add(canonical)
        resolved.append(canonical)
    if not resolved:
        _die("--metric parsed to empty list", 1)
    return resolved


def _compare(
    baseline_runs: list[dict[str, Any]],
    experiment_runs: list[dict[str, Any]],
    metrics: list[str],
    threshold: float,
) -> dict[str, MetricResult]:
    """Build a MetricResult dict for the given metric keys.

    Pre-conditions: caller has already checked both sides' MIN_VALID_RUNS.
    """
    out: dict[str, MetricResult] = {}
    for key in metrics:
        spec = METRIC_TYPE_SPEC[key]
        mtype: MetricType = spec["type"]
        b_vals = [r.get(key) for r in baseline_runs]
        e_vals = [r.get(key) for r in experiment_runs]
        b_agg = _aggregate(b_vals, key)
        e_agg = _aggregate(e_vals, key)

        delta: float | None
        delta_pct: float | None
        if b_agg.mean is None or e_agg.mean is None:
            delta = None
            delta_pct = None
        else:
            delta = e_agg.mean - b_agg.mean
            if mtype == "continuous" and b_agg.mean != 0:
                delta_pct = delta / b_agg.mean
            else:
                delta_pct = None

        mr = MetricResult(
            key=key,
            metric_type=mtype,
            baseline=b_agg,
            experiment=e_agg,
            delta=delta,
            delta_pct=delta_pct,
            verdict="unchanged",
        )
        mr.verdict = _verdict(mr, threshold)
        out[key] = mr
    return out


# ---------------------------------------------------------------------------
# rendering
# ---------------------------------------------------------------------------


def _fmt_num(v: float | None, digits: int = 3) -> str:
    if v is None:
        return "null"
    if isinstance(v, float) and (math.isnan(v) or math.isinf(v)):
        return "null"
    return f"{v:.{digits}f}"


def _fmt_cell(agg: AggResult) -> str:
    if agg.mean is None:
        return "n/a"
    lo = _fmt_num(agg.ci_low)
    hi = _fmt_num(agg.ci_high)
    mean = _fmt_num(agg.mean)
    return f"{mean} [{lo}, {hi}] (n={agg.n})"


def _render_markdown(result: CompareResult) -> str:
    lines: list[str] = []
    lines.append("# compare report")
    lines.append("")
    lines.append(f"- schema_version: {result.schema_version}")
    lines.append(f"- model_slug: {result.model_slug}")
    lines.append(f"- baseline_dir: {result.baseline_dir}")
    lines.append(f"- experiment_dir: {result.experiment_dir}")
    lines.append(f"- generated_at: {result.generated_at}")
    lines.append(f"- threshold: {result.threshold}")
    lines.append("")
    lines.append(
        "| metric | baseline (mean [CI]) | experiment (mean [CI]) | delta | delta_pct | verdict |"
    )
    lines.append("|---|---|---|---|---|---|")
    for key in sorted(result.metrics.keys()):
        mr = result.metrics[key]
        b_cell = _fmt_cell(mr.baseline)
        e_cell = _fmt_cell(mr.experiment)
        delta_s = _fmt_num(mr.delta)
        dp_s = "null" if mr.delta_pct is None else f"{mr.delta_pct * 100:.2f}%"
        emoji = VERDICT_EMOJI.get(mr.verdict, "")
        verdict_s = f"{emoji} {mr.verdict}" if emoji else mr.verdict
        lines.append(
            f"| {key} | {b_cell} | {e_cell} | {delta_s} | {dp_s} | {verdict_s} |"
        )
    if result.failure_categories:
        lines.append("")
        lines.append("## failure categories")
        lines.append("")
        lines.append("| category | baseline | experiment |")
        lines.append("|---|---:|---:|")
        baseline_counts = result.failure_categories.get("baseline", {})
        experiment_counts = result.failure_categories.get("experiment", {})
        for category in sorted(set(baseline_counts) | set(experiment_counts)):
            lines.append(
                f"| {category} | {baseline_counts.get(category, 0)} | {experiment_counts.get(category, 0)} |"
            )
    lines.append("")
    return "\n".join(lines)


def _agg_to_json(a: AggResult) -> dict[str, Any]:
    return {
        "mean": a.mean,
        "ci_low": a.ci_low,
        "ci_high": a.ci_high,
        "n": a.n,
    }


def _mr_to_json(mr: MetricResult) -> dict[str, Any]:
    return {
        "baseline": _agg_to_json(mr.baseline),
        "experiment": _agg_to_json(mr.experiment),
        "delta": mr.delta,
        "delta_pct": mr.delta_pct,
        "verdict": mr.verdict,
    }


def _render_json(result: CompareResult) -> str:
    out: dict[str, Any] = {
        "schema_version": result.schema_version,
        "baseline_dir": str(result.baseline_dir),
        "experiment_dir": str(result.experiment_dir),
        "model_slug": result.model_slug,
        "generated_at": result.generated_at,
        "threshold": result.threshold,
        "metrics": {k: _mr_to_json(v) for k, v in sorted(result.metrics.items())},
        "failure_categories": result.failure_categories,
    }
    return json.dumps(out, sort_keys=True, ensure_ascii=False, indent=2) + "\n"


# ---------------------------------------------------------------------------
# argparse / main
# ---------------------------------------------------------------------------


def _parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        prog="compare.py",
        description="A/B comparison of two bench BENCH_ROOTs.",
    )
    parser.add_argument("baseline_dir", help="baseline BENCH_ROOT, or BENCH_ROOT with --engines")
    parser.add_argument("experiment_dir", nargs="?", help="experiment BENCH_ROOT")
    parser.add_argument(
        "--metric",
        default=None,
        help="comma-separated metric keys (default: all known)",
    )
    parser.add_argument(
        "--format",
        choices=["markdown", "json"],
        default="markdown",
        help="output format (default: markdown)",
    )
    parser.add_argument(
        "--threshold",
        type=float,
        default=DEFAULT_THRESHOLD_PCT,
        help="delta threshold (default: 0.05)",
    )
    parser.add_argument(
        "--engines",
        default=None,
        help="compare two engines within one BENCH_ROOT, e.g. legacy,minimal",
    )
    return parser.parse_args(argv)


def main(argv: list[str]) -> int:
    os.umask(UMASK)
    ns = _parse_args(argv)

    if math.isnan(ns.threshold) or ns.threshold < 0:
        _die(f"--threshold must be a non-negative number, got {ns.threshold!r}", 1)

    analyze_path = _resolve_analyze_run_path()

    baseline_dir = _validate_input(Path(ns.baseline_dir))

    if ns.engines:
        if ns.experiment_dir is not None:
            _die("--engines mode accepts exactly one BENCH_ROOT positional argument", 1)
        raw_engines = [e.strip() for e in ns.engines.split(",") if e.strip()]
        if len(raw_engines) != 2:
            _die("--engines must contain exactly two engine names", 1)
        baseline_engine, experiment_engine = raw_engines
        slug = _find_single_model_slug(baseline_dir)
        run_dirs = _collect_run_dirs(baseline_dir, slug)
        if len(run_dirs) == 0:
            _die(f"zero run-dirs for slug={slug!r}", 2)

        grouped: dict[str, list[dict[str, Any]]] = {
            baseline_engine: [],
            experiment_engine: [],
        }
        for rd in run_dirs:
            analyzed = _analyze_one(rd, analyze_path)
            if analyzed is None:
                continue
            engine = analyzed.get("engine")
            if engine in grouped:
                grouped[engine].append(analyzed)

        b_filtered = _filter_runs(grouped[baseline_engine])
        e_filtered = _filter_runs(grouped[experiment_engine])
        if len(b_filtered) < MIN_VALID_RUNS or len(e_filtered) < MIN_VALID_RUNS:
            _die(
                f"insufficient valid runs "
                f"({baseline_engine}={len(b_filtered)}, {experiment_engine}={len(e_filtered)}; "
                f"MIN_VALID_RUNS={MIN_VALID_RUNS})",
                3,
            )

        metric_list = _resolve_metric_keys(
            [s for s in (ns.metric or "").split(",") if s.strip()] if ns.metric else None
        )
        metrics = _compare(b_filtered, e_filtered, metric_list, ns.threshold)
        result = CompareResult(
            schema_version=SCHEMA_VERSION,
            baseline_dir=baseline_dir,
            experiment_dir=baseline_dir,
            model_slug=f"{slug}:{baseline_engine}->{experiment_engine}",
            generated_at=_now(),
            threshold=ns.threshold,
            metrics=metrics,
            failure_categories={
                "baseline": _failure_category_counts(b_filtered),
                "experiment": _failure_category_counts(e_filtered),
            },
        )
        if ns.format == "json":
            sys.stdout.write(_render_json(result))
        else:
            sys.stdout.write(_render_markdown(result))
        return 0

    if ns.experiment_dir is None:
        _die("experiment_dir is required unless --engines is used", 1)

    experiment_dir = _validate_input(Path(ns.experiment_dir))

    slug = _find_model_slug(baseline_dir, experiment_dir)

    b_runs = _collect_run_dirs(baseline_dir, slug)
    e_runs = _collect_run_dirs(experiment_dir, slug)

    if len(b_runs) == 0 or len(e_runs) == 0:
        _die(
            f"zero run-dirs for slug={slug!r}: "
            f"baseline={len(b_runs)}, experiment={len(e_runs)}",
            2,
        )

    metric_list = _resolve_metric_keys(
        [s for s in (ns.metric or "").split(",") if s.strip()] if ns.metric else None
    )

    b_results: list[dict[str, Any]] = []
    for rd in b_runs:
        m = _analyze_one(rd, analyze_path)
        if m is not None:
            b_results.append(m)
    e_results: list[dict[str, Any]] = []
    for rd in e_runs:
        m = _analyze_one(rd, analyze_path)
        if m is not None:
            e_results.append(m)

    b_filtered = _filter_runs(b_results)
    e_filtered = _filter_runs(e_results)

    if len(b_filtered) < MIN_VALID_RUNS or len(e_filtered) < MIN_VALID_RUNS:
        _die(
            f"insufficient valid runs "
            f"(baseline={len(b_filtered)}, experiment={len(e_filtered)}; "
            f"MIN_VALID_RUNS={MIN_VALID_RUNS})",
            3,
        )

    metrics = _compare(b_filtered, e_filtered, metric_list, ns.threshold)

    result = CompareResult(
        schema_version=SCHEMA_VERSION,
        baseline_dir=baseline_dir,
        experiment_dir=experiment_dir,
        model_slug=slug,
        generated_at=_now(),
        threshold=ns.threshold,
        metrics=metrics,
        failure_categories={
            "baseline": _failure_category_counts(b_filtered),
            "experiment": _failure_category_counts(e_filtered),
        },
    )

    if ns.format == "json":
        sys.stdout.write(_render_json(result))
    else:
        md = _render_markdown(result)
        if not md.endswith("\n"):
            md += "\n"
        sys.stdout.write(md)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
