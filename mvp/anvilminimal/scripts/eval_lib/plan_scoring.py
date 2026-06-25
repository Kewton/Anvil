from __future__ import annotations

import json
import re
from pathlib import Path
from typing import Any

from .simple_yaml import load_yaml


STEP_WEIGHTS = {
    "yaml_validity": 10,
    "step_count_fit": 10,
    "atomicity": 15,
    "responsibility_boundary": 15,
    "instruction_clarity": 15,
    "expected_paths": 10,
    "verify_commands": 10,
    "dependency_order": 10,
    "repairability": 5,
}

ULTRA_WEIGHTS = {
    "phase_count_fit": 15,
    "phase_boundary": 20,
    "contract_carryover": 15,
    "phase_prompt_clarity": 15,
    "risk_isolation": 10,
    "step_plan_quality_average": 15,
    "final_verification_strength": 10,
}


def score_plan_file(path: str | Path, scenario: dict[str, Any] | None = None) -> dict[str, Any]:
    try:
        data = load_yaml(path)
    except Exception as err:
        return parse_failure(str(err))
    if "phases" in data:
        return score_ultra_plan(data, scenario)
    return score_step_plan(data, scenario)


def parse_failure(error: str) -> dict[str, Any]:
    details = {key: 0 for key in STEP_WEIGHTS}
    details["penalties"] = [{"kind": "parse_failure", "message": error}]
    return {"kind": "step", "score": 0, "details": details}


def score_step_plan(plan: dict[str, Any], scenario: dict[str, Any] | None = None) -> dict[str, Any]:
    scenario = scenario or {}
    steps = plan.get("steps") or []
    if not plan.get("goal") or not isinstance(steps, list) or not steps:
        return parse_failure("StepPlan missing goal or steps")
    constraints = scenario.get("plan_constraints", {})
    min_steps = int(constraints.get("min_steps", 1))
    max_steps = int(constraints.get("max_steps", 8))
    required_verify = constraints.get("required_verify_keywords", []) or []
    expected_artifacts = set(scenario.get("expected_artifacts", []) or [])
    penalties: list[dict[str, Any]] = []
    details: dict[str, Any] = {"yaml_validity": STEP_WEIGHTS["yaml_validity"]}
    count = len(steps)
    if min_steps <= count <= max_steps:
        details["step_count_fit"] = STEP_WEIGHTS["step_count_fit"]
    else:
        distance = min(abs(count - min_steps), abs(count - max_steps))
        details["step_count_fit"] = max(0, STEP_WEIGHTS["step_count_fit"] - distance * 4)
        penalties.append({"kind": "step_count_out_of_range", "count": count, "min": min_steps, "max": max_steps})
    instructions = [str(step.get("instruction", "")) for step in steps]
    details["atomicity"] = score_atomicity(instructions)
    kinds = [str(step.get("kind", "work")).lower() for step in steps]
    details["responsibility_boundary"] = score_boundaries(kinds, instructions)
    details["instruction_clarity"] = score_instruction_clarity(steps)
    path_score, path_penalties = score_expected_paths(steps, expected_artifacts)
    details["expected_paths"] = path_score
    penalties.extend(path_penalties)
    verify_score, verify_penalties = score_verify(steps, required_verify)
    details["verify_commands"] = verify_score
    penalties.extend(verify_penalties)
    details["dependency_order"] = score_dependency_order(steps)
    details["repairability"] = score_repairability(steps)
    total = sum(int(details[key]) for key in STEP_WEIGHTS)
    details["penalties"] = penalties
    total = max(0, min(100, total - penalty_points(penalties)))
    return {"kind": "step", "score": total, "details": details}


def score_ultra_plan(plan: dict[str, Any], scenario: dict[str, Any] | None = None) -> dict[str, Any]:
    phases = plan.get("phases") or []
    if not plan.get("goal") or not isinstance(phases, list) or not phases:
        return {"kind": "ultra", "score": 0, "details": {"penalties": [{"kind": "parse_failure"}]}}
    prompts = [str(phase.get("prompt", "")) for phase in phases]
    ids = [str(phase.get("id", "")) for phase in phases]
    details: dict[str, Any] = {}
    details["phase_count_fit"] = 15 if 2 <= len(phases) <= 8 else max(0, 15 - abs(len(phases) - 4) * 4)
    details["phase_boundary"] = min(20, len(set(ids)) * 6 + keyword_hits(" ".join(ids), ["scaffold", "implement", "verify", "repair"]) * 2)
    contract_text = " ".join(prompts + [str(plan.get("profile", "")), str(plan.get("goal", ""))]).lower()
    details["contract_carryover"] = min(15, keyword_hits(contract_text, ["profile", "port", "3011", "verify", "expected", "build"]) * 3)
    details["phase_prompt_clarity"] = min(15, sum(1 for p in prompts if len(p) >= 24) * 5)
    details["risk_isolation"] = 10 if any("verify" in p.lower() or "repair" in p.lower() for p in prompts) else 4
    details["step_plan_quality_average"] = 0
    details["final_verification_strength"] = 10 if any(word in prompts[-1].lower() for word in ["verify", "build", "repair", "test"]) else 2
    details["penalties"] = []
    score = sum(int(details[key]) for key in ULTRA_WEIGHTS)
    return {"kind": "ultra", "score": max(0, min(100, score)), "details": details}


def score_atomicity(instructions: list[str]) -> int:
    if not instructions:
        return 0
    score = 0
    for text in instructions:
        lowered = text.lower()
        if len(text) < 1200:
            score += 4
        if sum(1 for word in [" and ", " then ", " plus ", " including " ] if word in lowered) <= 2:
            score += 3
    return min(15, score)


def score_boundaries(kinds: list[str], instructions: list[str]) -> int:
    text = " ".join(kinds + instructions).lower()
    hits = keyword_hits(text, ["scaffold", "implement", "verify", "test", "docs", "repair"])
    return min(15, hits * 3)


def score_instruction_clarity(steps: list[dict[str, Any]]) -> int:
    score = 0
    for step in steps:
        text = str(step.get("instruction", "")).lower()
        if len(text) >= 24:
            score += 3
        if any(word in text for word in ["create", "write", "fix", "implement", "verify", "test", "document"]):
            score += 2
        if any(word in text for word in ["expected", "path", "workspace", "port", "constraint"]):
            score += 1
    return min(15, score)


def score_expected_paths(steps: list[dict[str, Any]], expected: set[str]) -> tuple[int, list[dict[str, Any]]]:
    paths: list[str] = []
    penalties = []
    for step in steps:
        paths.extend(str(path) for path in step.get("expected_paths", []) or [])
    for path in paths:
        if invalid_path(path):
            penalties.append({"kind": "path_escape", "path": path})
    duplicates = len(paths) - len(set(paths))
    if duplicates:
        penalties.append({"kind": "duplicate_expected_path_ownership", "count": duplicates})
    if not paths:
        return (0, penalties)
    if expected:
        matched = len(expected.intersection(paths))
        score = int(10 * matched / max(1, len(expected)))
    else:
        score = min(10, len(paths) * 2)
    return (score, penalties)


def score_verify(steps: list[dict[str, Any]], required: list[str]) -> tuple[int, list[dict[str, Any]]]:
    commands: list[str] = []
    penalties = []
    for step in steps:
        commands.extend(str(cmd) for cmd in step.get("verify", []) or [])
    if not commands:
        return (0, penalties)
    duplicates = len(commands) - len(set(commands))
    if duplicates:
        penalties.append({"kind": "duplicate_verify", "count": duplicates})
    text = "\n".join(commands).lower()
    if any(("&&" in cmd or "||" in cmd or "|" in cmd or ";" in cmd) for cmd in commands):
        penalties.append({"kind": "verify_command_policy_error"})
    if required:
        hits = sum(1 for keyword in required if str(keyword).lower() in text)
        return (int(10 * hits / max(1, len(required))), penalties)
    allowed = ["cargo test", "npm run build", "python3 -m unittest", "node", "curl", "pytest"]
    return (min(10, sum(2 for keyword in allowed if keyword in text)), penalties)


def score_dependency_order(steps: list[dict[str, Any]]) -> int:
    first_verify = None
    first_write = None
    for index, step in enumerate(steps):
        if step.get("verify"):
            first_verify = index if first_verify is None else first_verify
        if step.get("expected_paths"):
            first_write = index if first_write is None else first_write
    if first_verify is None:
        return 3
    if first_write is None:
        return 5
    return 10 if first_write <= first_verify else 2


def score_repairability(steps: list[dict[str, Any]]) -> int:
    text = json.dumps(steps).lower()
    if any(word in text for word in ["repair", "retry", "fix failure", "failure"]):
        return 5
    if any(step.get("verify") for step in steps):
        return 3
    return 1


def keyword_hits(text: str, keywords: list[str]) -> int:
    lowered = text.lower()
    return sum(1 for keyword in keywords if keyword.lower() in lowered)


def invalid_path(path: str) -> bool:
    return path.startswith("/") or path.startswith("~") or ".." in Path(path).parts


def penalty_points(penalties: list[dict[str, Any]]) -> int:
    points = 0
    for penalty in penalties:
        if penalty["kind"] == "path_escape":
            points += 20
        elif penalty["kind"] == "duplicate_verify":
            points += min(18, int(penalty.get("count", 1)) * 3)
        elif penalty["kind"] == "duplicate_expected_path_ownership":
            points += min(20, int(penalty.get("count", 1)) * 5)
        elif penalty["kind"] == "verify_command_policy_error":
            points += 15
        elif penalty["kind"] == "step_count_out_of_range":
            points += 5
    return points
