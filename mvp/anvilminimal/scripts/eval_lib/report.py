from __future__ import annotations

import statistics
from collections import Counter, defaultdict
from pathlib import Path
from typing import Iterable

from .run_summary import read_summary


def generate_report(run_root: Path) -> str:
    rows = read_summary(run_root / "summary.eval.tsv")
    lines = ["# anvilminimal Eval Report", ""]
    lines.extend(section_table("Mode Summary", aggregate(rows, "mode")))
    lines.extend(section_table("Size Summary", aggregate(rows, "size")))
    lines.extend(section_table("Model Profile Summary", aggregate(rows, "main_provider")))
    lines.extend(plan_rankings(rows))
    lines.extend(failure_summary(rows))
    return "\n".join(lines) + "\n"


def aggregate(rows: list[dict[str, str]], key: str) -> list[dict[str, str]]:
    groups: dict[str, list[dict[str, str]]] = defaultdict(list)
    for row in rows:
        groups[row.get(key, "")].append(row)
    out = []
    for name, group in sorted(groups.items()):
        elapsed = [to_float(row.get("exec_elapsed_sec")) for row in group if to_float(row.get("exec_elapsed_sec")) is not None]
        success_count = sum(1 for row in group if row.get("success") == "true")
        out.append(
            {
                "group": name,
                "success": f"{success_count}/{len(group)}",
                "p50_exec_sec": fmt(percentile(elapsed, 50)),
                "p90_exec_sec": fmt(percentile(elapsed, 90)),
                "avg_score": fmt(mean([to_float(row.get("overall_score")) for row in group])),
            }
        )
    return out


def section_table(title: str, rows: list[dict[str, str]]) -> list[str]:
    lines = [f"## {title}", ""]
    if not rows:
        return lines + ["No rows.", ""]
    headers = list(rows[0].keys())
    lines.append("| " + " | ".join(headers) + " |")
    lines.append("|" + "|".join("---" for _ in headers) + "|")
    for row in rows:
        lines.append("| " + " | ".join(str(row.get(h, "")) for h in headers) + " |")
    lines.append("")
    return lines


def plan_rankings(rows: list[dict[str, str]]) -> list[str]:
    scored = [(to_float(row.get("plan_quality_score")), row) for row in rows]
    scored = [(score, row) for score, row in scored if score is not None]
    lines = ["## Plan Quality", ""]
    if not scored:
        return lines + ["No plan scores.", ""]
    top = sorted(scored, key=lambda item: item[0], reverse=True)[:5]
    bottom = sorted(scored, key=lambda item: item[0])[:5]
    lines.append("| rank | scenario | mode | score |")
    lines.append("|---|---|---|---|")
    for label, items in [("top", top), ("bottom", bottom)]:
        for score, row in items:
            lines.append(f"| {label} | {row['scenario']} | {row['mode']} | {fmt(score)} |")
    lines.append("")
    return lines


def failure_summary(rows: list[dict[str, str]]) -> list[str]:
    counter = Counter()
    for row in rows:
        if row.get("success") == "true":
            continue
        if row.get("success") == "diagnostic_skipped":
            counter["diagnostic_skipped"] += 1
        elif row.get("rc") == "124":
            counter["timeout"] += 1
        elif row.get("rc") not in {"", "0"}:
            counter["process_failure"] += 1
        else:
            counter["postcheck_failure"] += 1
    lines = ["## Failures", "", "| kind | count |", "|---|---:|"]
    if not counter:
        lines.append("| none | 0 |")
    else:
        for key, count in sorted(counter.items()):
            lines.append(f"| {key} | {count} |")
    lines.append("")
    return lines


def compare_summaries(baseline: Path, experiment: Path) -> str:
    base = read_summary(baseline)
    exp = read_summary(experiment)
    lines = ["# Eval Compare", "", "| metric | baseline | experiment | delta |", "|---|---:|---:|---:|"]
    metrics = [
        ("success_rate", success_rate(base), success_rate(exp)),
        ("p50_exec_sec", percentile([to_float(r.get("exec_elapsed_sec")) for r in base], 50), percentile([to_float(r.get("exec_elapsed_sec")) for r in exp], 50)),
        ("overall_score_avg", mean([to_float(r.get("overall_score")) for r in base]), mean([to_float(r.get("overall_score")) for r in exp])),
    ]
    for name, b, e in metrics:
        lines.append(f"| {name} | {fmt(b)} | {fmt(e)} | {fmt((e or 0) - (b or 0))} |")
    lines.append("")
    return "\n".join(lines)


def success_rate(rows: list[dict[str, str]]) -> float:
    if not rows:
        return 0.0
    return 100.0 * sum(1 for row in rows if row.get("success") == "true") / len(rows)


def to_float(value: str | None) -> float | None:
    if value is None or value == "":
        return None
    try:
        return float(value)
    except ValueError:
        return None


def mean(values: Iterable[float | None]) -> float | None:
    clean = [value for value in values if value is not None]
    return statistics.fmean(clean) if clean else None


def percentile(values: Iterable[float | None], pct: int) -> float | None:
    clean = sorted(value for value in values if value is not None)
    if not clean:
        return None
    if len(clean) == 1:
        return clean[0]
    index = round((pct / 100) * (len(clean) - 1))
    return clean[index]


def fmt(value: float | None) -> str:
    if value is None:
        return ""
    return f"{value:.1f}"

