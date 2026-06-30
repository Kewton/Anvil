from __future__ import annotations

from typing import Any

from .failure_classification import blank_failure_kind_gate_violations


REQUIRED_GATE_IDS = {f"G-S{index:02d}" for index in range(1, 17)}
GATE_PARTITION_FIELDS = (
    "passed_gate_ids",
    "partial_gate_ids",
    "failed_gate_ids",
    "intentionally_different_gate_ids",
)


def validate_parity_gate_report(report: dict[str, Any]) -> list[str]:
    errors: list[str] = []
    if str(report.get("schema_version", "")) != "1":
        errors.append("schema_version must be 1")
    required = set(str(item) for item in report.get("required_gate_ids", []) or [])
    if required != REQUIRED_GATE_IDS:
        errors.append("required_gate_ids must be exactly G-S01..G-S16")

    seen: set[str] = set()
    for field in GATE_PARTITION_FIELDS:
        values = report.get(field, []) or []
        if not isinstance(values, list):
            errors.append(f"{field} must be a list")
            continue
        ids = set(str(item) for item in values)
        overlap = seen.intersection(ids)
        if overlap:
            errors.append(f"gate ids appear in multiple status lists: {sorted(overlap)}")
        seen.update(ids)
    if required and seen != required:
        missing = sorted(required.difference(seen))
        extra = sorted(seen.difference(required))
        if missing:
            errors.append(f"gate ids missing from status partition: {missing}")
        if extra:
            errors.append(f"unknown gate ids in status partition: {extra}")

    blank_count = int_or_none(report.get("failure_kind_blank_count"))
    if blank_count is None:
        errors.append("failure_kind_blank_count must be an integer")
    elif blank_count > 0:
        report_errors = report.get("errors", []) or []
        joined = "\n".join(str(item) for item in report_errors)
        if "failure_kind" not in joined:
            errors.append("blank failure kind count must be listed in report errors")

    gate_level = str(report.get("gate_level", ""))
    comparison = report.get("anvildev_comparison", {}) or {}
    if gate_level in {"comparative", "release"}:
        if comparison.get("status") == "missing_current_same_condition_trace":
            report_errors = report.get("errors", []) or []
            joined = "\n".join(str(item) for item in report_errors)
            if "anvildev" not in joined:
                errors.append("missing anvildev comparison must be listed in report errors")
    uat = report.get("uat_equivalent", {}) or {}
    if gate_level == "release" and uat.get("status") == "pass":
        evidence = uat.get("evidence_paths", []) or []
        if not evidence:
            errors.append("release gate cannot pass without UAT evidence paths")
    return errors


def blank_failure_kind_count(rows: list[dict[str, Any]]) -> int:
    return len(blank_failure_kind_gate_violations(rows))


def int_or_none(value: Any) -> int | None:
    try:
        return int(value)
    except (TypeError, ValueError):
        return None
