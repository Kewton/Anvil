from __future__ import annotations

import json
from pathlib import Path
from typing import Any

from .acceptance_contract import contract_from_scenario
from .plan_capability_contract import score_plan_capability_contract
from .plan_output_adherence import evaluate_plan_output_adherence
from .plan_verify_coverage import score_plan_verify_coverage
from .simple_yaml import load_yaml
from .source_semantic_oracle import evaluate_source_semantics


CALIBRATION_ORACLE_VERSION = "oracle-calibration-v1"


def evaluate_calibration_case(fixture_dir: Path) -> dict[str, Any]:
    scenario_path = fixture_dir / "scenario.yaml"
    expected_path = fixture_dir / "expected.json"
    if not scenario_path.exists() or not expected_path.exists():
        raise FileNotFoundError(f"calibration fixture must contain scenario.yaml and expected.json: {fixture_dir}")
    scenario = load_yaml(scenario_path)
    expected = json.loads(expected_path.read_text(encoding="utf-8"))
    if not isinstance(scenario, dict):
        raise ValueError(f"scenario fixture must be an object: {scenario_path}")
    workdir = fixture_dir / "workdir"
    plan_paths = [path for path in [fixture_dir / "plan.yaml", fixture_dir / "plan.json"] if path.exists()]
    contract = contract_from_scenario(scenario)
    source = evaluate_source_semantics(scenario, workdir, contract)
    plan_output = evaluate_plan_output_adherence(
        plan_paths=plan_paths,
        workdir=workdir,
        scenario=scenario,
    )
    plan_capability = score_plan_capability_contract(
        scenario=scenario,
        plan_paths=plan_paths,
    )
    plan_verify = score_plan_verify_coverage(
        scenario=scenario,
        mode=str(expected.get("mode", "plan-run")),
        plan_paths=plan_paths,
        workdir=workdir,
        plan_capability_result=plan_capability,
    )
    return {
        "fixture": fixture_dir.name,
        "oracle_version": CALIBRATION_ORACLE_VERSION,
        "category": contract.category,
        "expected": expected,
        "source_semantic_success": source.get("source_semantic_success", ""),
        "source_semantic_score": source.get("source_semantic_score", ""),
        "source_semantic_failure_kind": source.get("source_semantic_failure_kind", ""),
        "plan_output_adherence_success": plan_output.get("plan_output_adherence_success", ""),
        "plan_output_adherence_score": plan_output.get("plan_output_adherence_score", ""),
        "plan_verify_coverage_score": plan_verify.get("plan_verify_coverage_score", ""),
        "plan_verify_gap_kind": plan_verify.get("plan_verify_gap_kind", ""),
        "gate_success": gate_success_from_result(expected, source),
        "predictor_scores": {
            "plan_output_adherence_score": plan_output.get("plan_output_adherence_score", ""),
            "plan_verify_coverage_score": plan_verify.get("plan_verify_coverage_score", ""),
        },
    }


def gate_success_from_result(expected: dict[str, Any], source: dict[str, Any]) -> bool | str:
    if "acceptance_success" not in expected:
        return "out_of_scope"
    return bool(source.get("source_semantic_success", ""))


def evaluate_calibration_root(root: Path) -> list[dict[str, Any]]:
    cases: list[dict[str, Any]] = []
    for fixture_dir in sorted(path for path in root.iterdir() if path.is_dir()):
        if (fixture_dir / "scenario.yaml").exists() and (fixture_dir / "expected.json").exists():
            cases.append(evaluate_calibration_case(fixture_dir))
    return cases


def summarize_calibration(cases: list[dict[str, Any]]) -> dict[str, Any]:
    summary = {
        "oracle_version": CALIBRATION_ORACLE_VERSION,
        "total": len(cases),
        "positive": 0,
        "negative": 0,
        "out_of_scope": 0,
        "passed": 0,
        "false_positive": 0,
        "false_negative": 0,
    }
    for case in cases:
        expected = case.get("expected", {})
        if "acceptance_success" not in expected:
            summary["out_of_scope"] += 1
            summary["passed"] += 1
            continue
        expected_success = bool(expected["acceptance_success"])
        actual_success = bool(case.get("gate_success"))
        if expected_success:
            summary["positive"] += 1
        else:
            summary["negative"] += 1
        if actual_success == expected_success:
            summary["passed"] += 1
        elif actual_success:
            summary["false_positive"] += 1
        else:
            summary["false_negative"] += 1
    return summary
