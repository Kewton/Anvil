#!/usr/bin/env python3
"""Aggregate local-tier nextjs/create UAT capability bands.

The source of truth is workspace/management/runs/uat-* aggregate.json files.
For the one post-window report-only smoke set, this script falls back to the
report markdown. The generated summary is written to stdout and to
workspace/management/runs/band_summary.md.
"""

from __future__ import annotations

import json
import re
import statistics
from collections import Counter, defaultdict
from dataclasses import dataclass
from pathlib import Path
from typing import Any


ROOT = Path(__file__).resolve().parents[3]
RUNS_DIR = ROOT / "workspace" / "management" / "runs"
OUTPUT = RUNS_DIR / "band_summary.md"
WINDOW_START = "uat-test0711-bs-003"

FINAL_STATES = ("full_success", "partial", "incomplete", "failed")
PROVISIONAL = {"Quiz": 85, "Breakout": 30, "Space": 7}


@dataclass
class RunRecord:
    set_id: str
    run_id: str
    scenario: str
    planner: str
    executor: str
    plan_preset: str
    final_state: str
    stop_class: str
    elapsed_seconds: int | None
    source: str
    excluded_reason: str = ""
    false_full_reason: str = ""
    prompt: str = ""


def normalize_scenario(*parts: Any) -> str:
    text = " ".join(str(p or "") for p in parts).lower()
    if "space" in text or "invader" in text or "インベーダ" in text:
        return "Space"
    if "breakout" in text or "ブロック" in text:
        return "Breakout"
    if "quiz" in text or "クイズ" in text:
        return "Quiz"
    return "unknown"


def normalize_final(raw: Any, status: Any = "", release_gate: Any = "") -> str:
    text = str(raw or "").strip().lower()
    status_text = str(status or "").strip().lower()
    gate_text = str(release_gate or "").strip().lower()
    if text in {"full_success", "full", "completed"}:
        return "full_success"
    if text == "partial" or status_text == "partial":
        return "partial"
    if text == "incomplete":
        return "incomplete"
    if text in {"failed", "failure"} or status_text == "failed":
        return "failed"
    if text in {"not_checked", "not_applicable", ""}:
        if gate_text == "failed":
            return "incomplete"
        return "failed"
    return "failed"


def classify_stop(row: dict[str, Any], stop_reason: str, final_state: str) -> str:
    explicit = row.get("class") or row.get("failure_class")
    if explicit:
        return str(explicit)
    if final_state == "full_success":
        return "full"
    text = stop_reason.lower()
    if "probe_infrastructure_failed" in text or "probe infrastructure" in text:
        return "probe_infrastructure_failed"
    if "path_confinement" in text:
        return "path_confinement"
    if "no_progress" in text:
        return "no_progress"
    if "read_only_loop" in text or "write_required exhausted" in text:
        return "read_only_loop"
    if "restart_or_recoverable_state" in text or "restart" in text:
        return "restart_evidence"
    if "input_state_change" in text:
        return "input_state_change"
    if "compile" in text or "type error" in text:
        return "compile"
    if "dependency" in text:
        return "dependency"
    if "panic" in text or "char boundary" in text:
        return "panic"
    return final_state


def should_exclude(stop_class: str, stop_reason: str) -> str:
    text = f"{stop_class} {stop_reason}".lower()
    patterns = [
        "probe_infrastructure_failed",
        "probe infrastructure",
        "probe_preflight_failed",
        "playwright unavailable",
        "managed_interaction_probe unavailable",
    ]
    if any(p in text for p in patterns):
        return "probe_infrastructure_failed"
    return ""


def nested_value(row: dict[str, Any], key: str, nested_key: str) -> str:
    value = row.get(key)
    if isinstance(value, dict):
        return str(value.get(nested_key) or "")
    return ""


def artifact_dirs(set_dir: Path, row: dict[str, Any], run_id: str) -> list[Path]:
    candidates: list[Path] = []
    artifacts = row.get("artifacts")
    if isinstance(artifacts, list):
        for item in artifacts:
            path = set_dir / str(item)
            if path.exists():
                candidates.append(path.parent)
    direct = set_dir / "artifacts" / run_id
    if direct.exists():
        candidates.append(direct)
    # Some reports flatten or rename attempts.
    for path in (set_dir / "artifacts").glob(f"*{run_id}*"):
        if path.is_dir():
            candidates.append(path)
    seen: set[Path] = set()
    unique: list[Path] = []
    for path in candidates:
        if path not in seen:
            seen.add(path)
            unique.append(path)
    return unique


def json_file_has_interaction_pass(path: Path) -> bool:
    try:
        data = json.loads(path.read_text())
    except Exception:
        return False
    if not isinstance(data, dict):
        return False
    ok = data.get("ok") is True
    success = data.get("interaction_success") is True
    performed = data.get("interaction_performed") is True
    failure = str(data.get("failure_category") or "")
    return ok and performed and (success or failure == "")


def has_interaction_pass(set_dir: Path, row: dict[str, Any], run_id: str) -> bool:
    if row.get("full_has_interaction_pass") is True:
        return True
    for art_dir in artifact_dirs(set_dir, row, run_id):
        for path in art_dir.glob("*browser-interaction.json"):
            if json_file_has_interaction_pass(path):
                return True
        for path in art_dir.glob("summary.md"):
            text = path.read_text(errors="ignore").lower()
            if "interaction evidence: passed" in text:
                return True
        for path in art_dir.glob("events.jsonl"):
            text = path.read_text(errors="ignore").lower()
            if '"interaction_evidence_status":"passed"' in text:
                return True
    run_dir = row.get("run_dir")
    if run_dir:
        evidence_dir = Path(str(run_dir)).parents[1] / "evidence"
        path = evidence_dir / "browser-interaction.json"
        if path.exists() and json_file_has_interaction_pass(path):
            return True
    return False


def elapsed_from_summary(path: Path) -> int | None:
    if not path.exists():
        return None
    text = path.read_text(errors="ignore")
    match = re.search(r"total ([0-9]+)m([0-9]+)s", text)
    if match:
        return int(match.group(1)) * 60 + int(match.group(2))
    match = re.search(r"total ([0-9]+)s", text)
    if match:
        return int(match.group(1))
    return None


def record_from_row(set_dir: Path, row: dict[str, Any]) -> RunRecord:
    set_id = set_dir.name
    run_id = str(row.get("name") or row.get("run") or row.get("id") or "unknown")
    prompt = str(row.get("prompt") or row.get("goal") or "")
    scenario = normalize_scenario(row.get("scenario"), run_id, prompt)
    planner = str(row.get("planner") or row.get("planner_model") or "")
    executor = str(row.get("executor") or row.get("model") or "")
    plan_preset = (
        str(row.get("plan_preset") or row.get("plan_preset_arg") or row.get("preset") or "")
        or nested_value(row, "plan_preset_resolved", "value")
        or nested_value(row, "preset_resolved", "value")
        or "unknown"
    )
    final_raw = row.get("final_acceptance") or row.get("summary_final_acceptance")
    status = row.get("status") or row.get("summary_status")
    gate = row.get("release_gate") or row.get("summary_release_gate")
    final_state = normalize_final(final_raw, status, gate)
    stop_reason = str(row.get("stop_reason") or row.get("summary_stop_reason") or "")
    stop_class = classify_stop(row, stop_reason, final_state)
    elapsed = row.get("elapsed_seconds")
    elapsed_seconds = int(elapsed) if isinstance(elapsed, int) else None
    if elapsed_seconds is None:
        for art_dir in artifact_dirs(set_dir, row, run_id):
            elapsed_seconds = elapsed_from_summary(art_dir / "summary.md")
            if elapsed_seconds is not None:
                break
    excluded_reason = should_exclude(stop_class, stop_reason)
    false_full_reason = ""
    if final_state == "full_success" and not has_interaction_pass(set_dir, row, run_id):
        false_full_reason = "missing_browser_interaction_pass_evidence"
    return RunRecord(
        set_id=set_id,
        run_id=run_id,
        scenario=scenario,
        planner=planner,
        executor=executor,
        plan_preset=plan_preset,
        final_state=final_state,
        stop_class=stop_class,
        elapsed_seconds=elapsed_seconds,
        source="aggregate",
        excluded_reason=excluded_reason,
        false_full_reason=false_full_reason,
        prompt=prompt,
    )


def aggregate_rows(path: Path) -> list[dict[str, Any]]:
    data = json.loads(path.read_text())
    if isinstance(data, list):
        return [row for row in data if isinstance(row, dict)]
    if isinstance(data, dict):
        rows = data.get("results")
        if isinstance(rows, list):
            return [row for row in rows if isinstance(row, dict)]
    return []


def parse_report_only(set_dir: Path) -> list[RunRecord]:
    report = set_dir / "uat-report.md"
    if not report.exists():
        return []
    text = report.read_text(errors="ignore")
    if "## Smoke Result" not in text:
        return []
    run_id = "smoke_result"
    scenario = normalize_scenario(text)
    planner = find_markdown_value(text, "Planner")
    executor = find_markdown_value(text, "Executor")
    preset_match = re.search(r"Resolved preset:\s*`?([A-Za-z0-9_-]+)`?", text)
    plan_preset = preset_match.group(1) if preset_match else "unknown"
    final = find_markdown_value(text, "Final acceptance")
    release_gate = find_markdown_value(text, "Release gate")
    final_state = normalize_final(final, "", release_gate)
    stop_class = "full" if final_state == "full_success" else final_state
    elapsed = elapsed_from_summary(set_dir / "artifacts" / "attempt-2-pass" / "summary.md")
    full_has_pass = "Interaction evidence: passed" in text
    false_full = ""
    if final_state == "full_success" and not full_has_pass:
        summary = set_dir / "artifacts" / "attempt-2-pass" / "summary.md"
        if summary.exists() and "Interaction evidence: passed" in summary.read_text(errors="ignore"):
            full_has_pass = True
    if final_state == "full_success" and not full_has_pass:
        false_full = "missing_browser_interaction_pass_evidence"
    return [
        RunRecord(
            set_id=set_dir.name,
            run_id=run_id,
            scenario=scenario,
            planner=planner,
            executor=executor,
            plan_preset=plan_preset,
            final_state=final_state,
            stop_class=stop_class,
            elapsed_seconds=elapsed,
            source="report",
            false_full_reason=false_full,
        )
    ]


def find_markdown_value(text: str, label: str) -> str:
    match = re.search(rf"- {re.escape(label)}:\s*`?([^`\n]+)`?", text)
    return match.group(1).strip() if match else ""


def discover_records() -> tuple[list[RunRecord], int, int, list[str]]:
    records: list[RunRecord] = []
    aggregate_row_total = 0
    aggregate_record_total = 0
    scanned_sets: list[str] = []
    for set_dir in sorted(RUNS_DIR.glob("uat-*")):
        if set_dir.name < WINDOW_START:
            continue
        if not set_dir.is_dir():
            continue
        scanned_sets.append(set_dir.name)
        aggregate = set_dir / "aggregate.json"
        if aggregate.exists():
            rows = aggregate_rows(aggregate)
            aggregate_row_total += len(rows)
            for row in rows:
                records.append(record_from_row(set_dir, row))
                aggregate_record_total += 1
        else:
            records.extend(parse_report_only(set_dir))
    assert aggregate_record_total == aggregate_row_total, (
        f"aggregate-derived record count {aggregate_record_total} != "
        f"aggregate.json row count {aggregate_row_total}"
    )
    return records, aggregate_row_total, aggregate_record_total, scanned_sets


def pct(num: int, den: int) -> str:
    if den == 0:
        return "0%"
    return f"{round(num * 100 / den)}%"


def state_counts(records: list[RunRecord]) -> dict[str, Counter[str]]:
    counts: dict[str, Counter[str]] = defaultdict(Counter)
    for rec in records:
        if rec.excluded_reason:
            continue
        counts[rec.scenario][rec.final_state] += 1
    return counts


def executor_counts(records: list[RunRecord]) -> dict[tuple[str, str], Counter[str]]:
    counts: dict[tuple[str, str], Counter[str]] = defaultdict(Counter)
    for rec in records:
        if rec.excluded_reason:
            continue
        counts[(rec.scenario, rec.executor or "unknown")] [rec.final_state] += 1
    return counts


def median(values: list[int]) -> int:
    return int(statistics.median(values))


def time_label(seconds: int | None) -> str:
    if seconds is None:
        return "unknown"
    mins, secs = divmod(seconds, 60)
    return f"{mins}m{secs:02d}s"


def table(headers: list[str], rows: list[list[str]]) -> list[str]:
    lines = ["| " + " | ".join(headers) + " |"]
    lines.append("| " + " | ".join("---" for _ in headers) + " |")
    lines.extend("| " + " | ".join(row) + " |" for row in rows)
    return lines


def build_summary(records: list[RunRecord], aggregate_row_total: int, scanned_sets: list[str]) -> str:
    included = [rec for rec in records if not rec.excluded_reason]
    excluded = [rec for rec in records if rec.excluded_reason]
    unknowns = [rec for rec in included if rec.scenario == "unknown"]
    false_full = [rec for rec in included if rec.false_full_reason]
    source_counts = Counter(rec.source for rec in records)
    planner_counts = Counter(rec.planner or "unknown" for rec in included)
    lines: list[str] = []
    lines.append("# Next.js Create Capability Band Summary")
    lines.append("")
    lines.append(f"- Window start: `{WINDOW_START}`")
    lines.append(f"- Scanned UAT sets: `{len(scanned_sets)}`")
    lines.append(f"- Aggregate.json rows asserted: `{aggregate_row_total}`")
    lines.append(f"- Total run records: `{len(records)}`")
    lines.append(f"- Record sources: `{dict(sorted(source_counts.items()))}`")
    lines.append(f"- Included denominator after exclusions: `{len(included)}`")
    lines.append(f"- Excluded infrastructure records: `{len(excluded)}`")
    lines.append("")
    lines.append("## Planner Coverage")
    rows = [[planner, str(count)] for planner, count in sorted(planner_counts.items())]
    lines.extend(table(["Planner", "included records"], rows))
    lines.append("")
    lines.append("## Scenario x Final State")
    rows: list[list[str]] = []
    counts = state_counts(included)
    for scenario in sorted(counts):
        counter = counts[scenario]
        den = sum(counter.values())
        full = counter["full_success"]
        note = " n<10" if den < 10 else ""
        rows.append([
            scenario,
            str(counter["full_success"]),
            str(counter["partial"]),
            str(counter["incomplete"]),
            str(counter["failed"]),
            str(den),
            f"{pct(full, den)}{note}",
        ])
    lines.extend(table(["Scenario", "full", "partial", "incomplete", "failed", "n", "full rate"], rows))
    lines.append("")
    lines.append("## Scenario x Executor")
    rows = []
    for (scenario, executor), counter in sorted(executor_counts(included).items()):
        den = sum(counter.values())
        full = counter["full_success"]
        note = " n<10" if den < 10 else ""
        rows.append([scenario, executor, str(full), str(den), f"{pct(full, den)}{note}"])
    lines.extend(table(["Scenario", "Executor", "full", "n", "full rate"], rows))
    lines.append("")
    lines.append("## Full Run Durations")
    rows = []
    full_by_scenario: dict[str, list[int]] = defaultdict(list)
    all_full: list[int] = []
    for rec in included:
        if rec.final_state == "full_success" and rec.elapsed_seconds is not None:
            full_by_scenario[rec.scenario].append(rec.elapsed_seconds)
            all_full.append(rec.elapsed_seconds)
    if all_full:
        rows.append([
            "all",
            str(len(all_full)),
            time_label(min(all_full)),
            time_label(median(all_full)),
            time_label(max(all_full)),
        ])
    for scenario in sorted(full_by_scenario):
        values = full_by_scenario[scenario]
        rows.append([
            scenario,
            str(len(values)),
            time_label(min(values)),
            time_label(median(values)),
            time_label(max(values)),
        ])
    lines.extend(table(["Scope", "full runs", "min", "median", "max"], rows))
    lines.append("")
    lines.append("## Excluded and Unknown Runs")
    if excluded:
        rows = [[rec.set_id, rec.run_id, rec.scenario, rec.excluded_reason] for rec in excluded]
        lines.extend(table(["Set", "Run", "Scenario", "Reason"], rows))
    else:
        lines.append("- Excluded infrastructure runs: none")
    if unknowns:
        rows = [[rec.set_id, rec.run_id, rec.stop_class] for rec in unknowns]
        lines.append("")
        lines.append("Unknown scenario records:")
        lines.extend(table(["Set", "Run", "Stop class"], rows))
    else:
        lines.append("- Unknown scenario records: none")
    lines.append("")
    lines.append("## False-Full Check")
    if false_full:
        rows = [[rec.set_id, rec.run_id, rec.scenario, rec.false_full_reason] for rec in false_full]
        lines.extend(table(["Set", "Run", "Scenario", "Reason"], rows))
    else:
        lines.append("- False-full suspects: 0")
    lines.append("")
    lines.append("## Stop-Class Distribution")
    stop_counts: dict[str, Counter[str]] = defaultdict(Counter)
    for rec in included:
        stop_counts[rec.scenario][rec.stop_class] += 1
    rows = []
    for scenario in sorted(stop_counts):
        parts = ", ".join(f"{k}={v}" for k, v in sorted(stop_counts[scenario].items()))
        rows.append([scenario, parts])
    lines.extend(table(["Scenario", "Stop classes"], rows))
    lines.append("")
    lines.append("## Provisional Comparison")
    rows = []
    for scenario, expected in PROVISIONAL.items():
        counter = counts.get(scenario, Counter())
        den = sum(counter.values())
        actual = round(counter["full_success"] * 100 / den) if den else 0
        delta = actual - expected
        note = ""
        if abs(delta) > 15:
            note = "diff >15pp; target window includes post-0711 gate/A-B/task28 sets and counts every rerun in time order"
        rows.append([scenario, f"{expected}%", f"{actual}%", f"{delta:+d}pp", note])
    lines.extend(table(["Scenario", "Provisional", "Measured", "Delta", "Note"], rows))
    lines.append("")
    lines.append("## Source Sets")
    for set_id in scanned_sets:
        lines.append(f"- `{set_id}`")
    lines.append("")
    return "\n".join(lines)


def main() -> int:
    records, aggregate_row_total, _aggregate_record_total, scanned_sets = discover_records()
    summary = build_summary(records, aggregate_row_total, scanned_sets)
    OUTPUT.write_text(summary, encoding="utf-8")
    print(summary)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
