from __future__ import annotations

import csv
import json
from pathlib import Path
from typing import Any


SUMMARY_HEADER = [
    "run_id",
    "suite",
    "scenario",
    "size",
    "category",
    "mode",
    "main_provider",
    "main_model",
    "planner_provider",
    "planner_model",
    "local_llm_used",
    "rc",
    "success",
    "queue_wait_sec",
    "process_elapsed_sec",
    "exec_elapsed_sec",
    "model_elapsed_sec",
    "tool_elapsed_sec",
    "postcheck_elapsed_sec",
    "dependency_elapsed_sec",
    "iterations",
    "tool_calls",
    "files_changed",
    "stop_reason",
    "last_blocking_reason",
    "missing_artifacts",
    "verify_attempts",
    "last_provider_error_kind",
    "last_provider_http_status",
    "provider_attempts",
    "fallback_decision",
    "planner_stage",
    "planner_error_kind",
    "planner_error_count",
    "planner_repair_attempts",
    "planner_schema_repaired",
    "planner_raw_schema_violation",
    "planner_parser_limitation",
    "planner_prompt_issue",
    "planner_quality_issue_count",
    "planner_retryable_quality_count",
    "planner_advisory_quality_count",
    "planner_quality_retry_count",
    "planner_quality_retry_degraded_count",
    "plan_quality_score",
    "executable_plan_score",
    "constraint_coverage_score",
    "verify_strength_score",
    "artifact_ownership_score",
    "lint_repair_score",
    "stability_score",
    "ultra_phase_quality_score",
    "execution_score",
    "time_score",
    "overall_score",
    "workdir",
    "plan_artifacts",
    "extras_json",
]


def empty_summary_row(spec: dict[str, Any]) -> dict[str, Any]:
    scenario = spec["scenario"]
    return {
        "run_id": spec["run_id"],
        "suite": spec["suite"],
        "scenario": scenario["id"],
        "size": scenario["size"],
        "category": scenario["category"],
        "mode": spec["mode"],
        "main_provider": spec["main"]["provider"],
        "main_model": spec["main"]["model"],
        "planner_provider": spec["planner"]["provider"],
        "planner_model": spec["planner"]["model"],
        "local_llm_used": str(bool(spec["local_llm_used"])).lower(),
        "rc": "",
        "success": "",
        "queue_wait_sec": "",
        "process_elapsed_sec": "",
        "exec_elapsed_sec": "",
        "model_elapsed_sec": "",
        "tool_elapsed_sec": "",
        "postcheck_elapsed_sec": "",
        "dependency_elapsed_sec": "",
        "iterations": "",
        "tool_calls": "",
        "files_changed": "",
        "stop_reason": "",
        "last_blocking_reason": "",
        "missing_artifacts": "",
        "verify_attempts": "",
        "last_provider_error_kind": "",
        "last_provider_http_status": "",
        "provider_attempts": "",
        "fallback_decision": "",
        "planner_stage": "",
        "planner_error_kind": "",
        "planner_error_count": "",
        "planner_repair_attempts": "",
        "planner_schema_repaired": "",
        "planner_raw_schema_violation": "",
        "planner_parser_limitation": "",
        "planner_prompt_issue": "",
        "planner_quality_issue_count": "",
        "planner_retryable_quality_count": "",
        "planner_advisory_quality_count": "",
        "planner_quality_retry_count": "",
        "planner_quality_retry_degraded_count": "",
        "plan_quality_score": "",
        "executable_plan_score": "",
        "constraint_coverage_score": "",
        "verify_strength_score": "",
        "artifact_ownership_score": "",
        "lint_repair_score": "",
        "stability_score": "",
        "ultra_phase_quality_score": "",
        "execution_score": "",
        "time_score": "",
        "overall_score": "",
        "workdir": "",
        "plan_artifacts": "",
        "extras_json": "",
    }


def write_summary(path: Path, rows: list[dict[str, Any]]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w", encoding="utf-8", newline="") as f:
        writer = csv.DictWriter(f, fieldnames=SUMMARY_HEADER, delimiter="\t", extrasaction="ignore")
        writer.writeheader()
        for row in rows:
            writer.writerow({key: serialize_cell(row.get(key, "")) for key in SUMMARY_HEADER})


def read_summary(path: Path) -> list[dict[str, str]]:
    with path.open(encoding="utf-8", newline="") as f:
        reader = csv.DictReader(f, delimiter="\t")
        fieldnames = reader.fieldnames or []
        projected_header = [key for key in SUMMARY_HEADER if key in fieldnames]
        if fieldnames != projected_header:
            raise ValueError(f"unsupported summary header in {path}: {reader.fieldnames}")
        rows = list(reader)
        for row in rows:
            for key in SUMMARY_HEADER:
                row.setdefault(key, "")
        return rows


def serialize_cell(value: Any) -> str:
    if value is None:
        return ""
    if isinstance(value, bool):
        return str(value).lower()
    if isinstance(value, (dict, list)):
        return json.dumps(value, ensure_ascii=False, sort_keys=True)
    return str(value)


def calculate_overall(
    mode: str,
    plan_score: float | None,
    ultra_score: float | None,
    execution_score: float,
    time_score: float,
    executable_score: float | None = None,
    constraint_score: float | None = None,
    verify_strength_score: float | None = None,
    artifact_ownership_score: float | None = None,
    lint_repair_score: float | None = None,
) -> float:
    if mode == "minimal-loop":
        return round(0.80 * execution_score + 0.20 * time_score, 1)
    if mode == "step-plan":
        return round(
            0.35 * (plan_score or 0)
            + 0.20 * (executable_score or 0)
            + 0.15 * (constraint_score or 0)
            + 0.10 * (verify_strength_score or 0)
            + 0.10 * (artifact_ownership_score or 0)
            + 0.10 * (lint_repair_score or 0),
            1,
        )
    if mode == "plan-run":
        return round(
            0.18 * (plan_score or 0)
            + 0.12 * (executable_score or 0)
            + 0.08 * (constraint_score or 0)
            + 0.05 * (verify_strength_score or 0)
            + 0.05 * (artifact_ownership_score or 0)
            + 0.05 * (lint_repair_score or 0)
            + 0.37 * execution_score
            + 0.10 * time_score,
            1,
        )
    if mode == "ultra-plan-run":
        return round(0.30 * (ultra_score or 0) + 0.35 * execution_score + 0.10 * time_score, 1)
    if mode == "ultra-step-run":
        return round(
            0.25 * (plan_score or 0)
            + 0.10 * (executable_score or 0)
            + 0.08 * (constraint_score or 0)
            + 0.05 * (verify_strength_score or 0)
            + 0.05 * (artifact_ownership_score or 0)
            + 0.05 * (lint_repair_score or 0)
            + 0.32 * execution_score
            + 0.10 * time_score,
            1,
        )
    return execution_score
