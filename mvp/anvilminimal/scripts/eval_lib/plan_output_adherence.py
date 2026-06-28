from __future__ import annotations

from pathlib import Path
from typing import Any

from .plan_capability_contract import (
    PLAN_CAPABILITY_ORACLE_VERSION,
    collect_plan_contract,
    required_capabilities_from_plan_text,
    source_capability_detected,
)
from .source_semantic_oracle import collect_source_corpus


PLAN_OUTPUT_ORACLE_VERSION = "plan-output-v1"


def evaluate_plan_output_adherence(
    *,
    plan_paths: list[Path],
    workdir: Path,
    scenario: dict[str, Any],
) -> dict[str, Any]:
    plan_text, _, parse_errors = collect_plan_contract(plan_paths)
    if not plan_text:
        return not_applicable("no_plan_contract")

    required = required_capabilities_from_plan_text(plan_text)
    if not required:
        return not_applicable("no_plan_output_capabilities")

    corpus = collect_source_corpus(workdir, scenario)
    source_text = "\n".join(source for _, source in corpus).lower()
    capability_results = {
        name: source_capability_detected(name, source_text)
        for name in required
    }
    missing = [name for name, ok in capability_results.items() if not ok]
    score = round(100.0 * (len(required) - len(missing)) / max(1, len(required)), 1)
    return {
        "plan_output_adherence_success": not missing,
        "plan_output_adherence_score": score,
        "plan_output_failure_kind": "plan_output_missing_required_capabilities" if missing else "",
        "plan_output_oracle_version": PLAN_OUTPUT_ORACLE_VERSION,
        "plan_output_details": {
            "applicable": True,
            "plan_paths": [str(path) for path in plan_paths],
            "files_scanned": [path for path, _ in corpus],
            "plan_capability_oracle_version": PLAN_CAPABILITY_ORACLE_VERSION,
            "parse_errors": parse_errors,
            "required_capabilities": required,
            "capabilities": capability_results,
            "missing_capabilities": missing,
        },
    }


def not_applicable(reason: str) -> dict[str, Any]:
    return {
        "plan_output_adherence_success": "",
        "plan_output_adherence_score": "",
        "plan_output_failure_kind": "",
        "plan_output_oracle_version": PLAN_OUTPUT_ORACLE_VERSION,
        "plan_output_details": {"applicable": False, "reason": reason},
    }
