from __future__ import annotations

import json
import re
from collections import Counter
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

EXECUTABLE_WEIGHTS = {
    "actionability": 25,
    "path_instruction_alignment": 20,
    "read_before_create_risk": 20,
    "verify_executability": 20,
    "step_budget_fit": 15,
}

EXECUTION_SHAPE_WEIGHTS = {
    "first_artifact_owner": 25,
    "wrapper_step_minimality": 20,
    "empty_expected_path_minimality": 15,
    "verify_owner_coupling": 15,
    "write_first_bias": 15,
    "terminal_finalization_risk": 10,
}

CONSTRAINT_COVERAGE_WEIGHTS = {
    "expected_artifacts": 45,
    "required_verify": 25,
    "profile_contract": 30,
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
    return {
        "kind": "step",
        "score": 0,
        "executable_score": 0,
        "constraint_coverage_score": 0,
        "verify_strength_score": 0,
        "artifact_ownership_score": 0,
        "execution_shape_readiness_score": 0,
        "details": details,
    }


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
    executable = score_executable_step_plan(plan, scenario)
    constraint = score_constraint_coverage(plan, scenario)
    verify_strength = score_verify_strength(steps, scenario)
    artifact_ownership = score_artifact_ownership(steps, scenario)
    execution_shape = score_execution_shape_readiness(plan, scenario)
    return {
        "kind": "step",
        "score": total,
        "executable_score": executable["score"],
        "constraint_coverage_score": constraint["score"],
        "verify_strength_score": verify_strength["score"],
        "artifact_ownership_score": artifact_ownership["score"],
        "execution_shape_readiness_score": execution_shape["score"],
        "details": details,
        "executable_details": executable["details"],
        "constraint_coverage_details": constraint["details"],
        "verify_strength_details": verify_strength["details"],
        "artifact_ownership_details": artifact_ownership["details"],
        "execution_shape_details": execution_shape["details"],
    }


def score_executable_step_plan(plan: dict[str, Any], scenario: dict[str, Any] | None = None) -> dict[str, Any]:
    scenario = scenario or {}
    steps = plan.get("steps") or []
    expected_artifacts = [str(path) for path in scenario.get("expected_artifacts", []) or []]
    penalties: list[dict[str, Any]] = []
    details: dict[str, Any] = {}
    if not isinstance(steps, list) or not steps:
        return {
            "score": 0,
            "details": {**{key: 0 for key in EXECUTABLE_WEIGHTS}, "penalties": [{"kind": "no_steps"}]},
        }

    details["actionability"] = score_actionability(steps, penalties)
    details["path_instruction_alignment"] = score_path_instruction_alignment(steps, expected_artifacts, penalties)
    details["read_before_create_risk"] = score_read_before_create_risk(steps, expected_artifacts, scenario, penalties)
    details["verify_executability"] = score_verify_executability(steps, penalties)
    details["step_budget_fit"] = score_step_budget_fit(steps, scenario, penalties)
    total = sum(int(details[key]) for key in EXECUTABLE_WEIGHTS)
    total = max(0, min(100, total - executable_penalty_points(penalties)))
    details["penalties"] = penalties
    return {"score": total, "details": details}


def score_constraint_coverage(plan: dict[str, Any], scenario: dict[str, Any] | None = None) -> dict[str, Any]:
    scenario = scenario or {}
    text = plan_text(plan)
    expected = [str(path) for path in scenario.get("expected_artifacts", []) or []]
    required_verify = [str(keyword) for keyword in scenario.get("plan_constraints", {}).get("required_verify_keywords", []) or []]
    profile = str(scenario.get("profile", "generic")).lower()
    prompt = str(scenario.get("prompt", "")).lower()
    details: dict[str, Any] = {}

    artifact_hits = [path for path in expected if path.lower() in text]
    details["expected_artifacts"] = ratio_score(len(artifact_hits), len(expected), CONSTRAINT_COVERAGE_WEIGHTS["expected_artifacts"])
    details["missing_expected_artifacts"] = sorted(set(expected).difference(artifact_hits))

    verify_hits = [keyword for keyword in required_verify if keyword.lower() in text]
    details["required_verify"] = ratio_score(len(verify_hits), len(required_verify), CONSTRAINT_COVERAGE_WEIGHTS["required_verify"])
    details["missing_required_verify"] = sorted(set(required_verify).difference(verify_hits))

    profile_checks = profile_contract_checks(profile, prompt)
    profile_hits = [name for name, patterns in profile_checks.items() if any(pattern in text for pattern in patterns)]
    details["profile_contract"] = ratio_score(
        len(profile_hits),
        len(profile_checks),
        CONSTRAINT_COVERAGE_WEIGHTS["profile_contract"],
    )
    details["missing_profile_contract"] = sorted(set(profile_checks).difference(profile_hits))

    score = int(details["expected_artifacts"] + details["required_verify"] + details["profile_contract"])
    return {"score": max(0, min(100, score)), "details": details}


def score_verify_strength(steps: list[dict[str, Any]], scenario: dict[str, Any] | None = None) -> dict[str, Any]:
    scenario = scenario or {}
    commands = collect_verify_commands(steps)
    required_verify = [str(keyword).lower() for keyword in scenario.get("plan_constraints", {}).get("required_verify_keywords", []) or []]
    if not commands:
        return {
            "score": 0,
            "details": {"commands": [], "required_verify_coverage": 0, "penalties": [{"kind": "no_verify_commands"}]},
        }
    strengths = [{"command": command, "strength": command_strength(command)} for command in commands]
    average_strength = int(sum(item["strength"] for item in strengths) / len(strengths))
    text = "\n".join(commands).lower()
    required_hits = sum(1 for keyword in required_verify if keyword in text)
    required_coverage = ratio_score(required_hits, len(required_verify), 100)
    score = int(0.75 * average_strength + 0.25 * required_coverage)
    penalties = []
    if any(item["strength"] <= 25 for item in strengths):
        penalties.append({"kind": "weak_verify_command"})
    if required_verify and required_hits < len(required_verify):
        penalties.append({"kind": "missing_required_verify_keyword"})
    return {
        "score": max(0, min(100, score)),
        "details": {
            "commands": strengths,
            "average_command_strength": average_strength,
            "required_verify_coverage": required_coverage,
            "penalties": penalties,
        },
    }


def score_artifact_ownership(steps: list[dict[str, Any]], scenario: dict[str, Any] | None = None) -> dict[str, Any]:
    scenario = scenario or {}
    expected = [str(path) for path in scenario.get("expected_artifacts", []) or []]
    expected_set = set(expected)
    all_paths = [str(path) for step in steps for path in step.get("expected_paths", []) or []]
    counts = Counter(all_paths)
    exactly_once = [path for path in expected if counts[path] == 1]
    missing = [path for path in expected if counts[path] == 0]
    duplicates = [path for path in expected if counts[path] > 1]
    extras = [path for path in all_paths if path not in expected_set and not allowed_extra_artifact(path, scenario)]

    ownership_score = ratio_score(len(exactly_once), len(expected), 70)
    no_extra_score = max(0, 15 - len(set(extras)) * 12)
    nested_expected = [path for path in expected if "/" in path]
    nested_ok = [path for path in nested_expected if nested_path_is_naturally_owned(path, steps)]
    nested_score = ratio_score(len(nested_ok), len(nested_expected), 15)
    score = int(ownership_score + no_extra_score + nested_score)
    penalties = []
    if missing:
        penalties.append({"kind": "missing_required_artifact_ownership", "paths": missing})
    if duplicates:
        penalties.append({"kind": "duplicate_required_artifact_ownership", "paths": duplicates})
    if extras:
        penalties.append({"kind": "extra_artifact_ownership", "paths": sorted(set(extras))})
    return {
        "score": max(0, min(100, score)),
        "details": {
            "exactly_once": exactly_once,
            "missing": missing,
            "duplicates": duplicates,
            "extras": sorted(set(extras)),
            "nested_owned": nested_ok,
            "penalties": penalties,
        },
    }


def score_execution_shape_readiness(plan: dict[str, Any], scenario: dict[str, Any] | None = None) -> dict[str, Any]:
    scenario = scenario or {}
    steps = plan.get("steps") or []
    if not isinstance(steps, list) or not steps:
        return {
            "score": 0,
            "details": {
                **{key: 0 for key in EXECUTION_SHAPE_WEIGHTS},
                "penalties": [{"kind": "no_steps"}],
            },
        }

    expected = [str(path) for path in scenario.get("expected_artifacts", []) or []]
    fresh_workspace = not scenario_has_seed_inputs(scenario)
    penalties: list[dict[str, Any]] = []
    details: dict[str, Any] = {}

    owner_indexes = [index for index, step in enumerate(steps) if step.get("expected_paths")]
    first_owner = owner_indexes[0] if owner_indexes else None
    if first_owner is None:
        details["first_artifact_owner"] = 0 if expected else 15
        if expected:
            penalties.append({"kind": "missing_artifact_owner"})
    elif first_owner == 0:
        details["first_artifact_owner"] = EXECUTION_SHAPE_WEIGHTS["first_artifact_owner"]
    elif first_owner == 1:
        details["first_artifact_owner"] = 20
        penalties.append({"kind": "artifact_owner_delayed", "first_owner_index": first_owner})
    elif first_owner == 2:
        details["first_artifact_owner"] = 12
        penalties.append({"kind": "artifact_owner_delayed", "first_owner_index": first_owner})
    else:
        details["first_artifact_owner"] = 5
        penalties.append({"kind": "artifact_owner_delayed", "first_owner_index": first_owner})

    wrapper_kinds = {"inspect", "analyze", "review", "report", "summarize"}
    wrapper_steps = [
        str(step.get("id", ""))
        for step in steps
        if not step.get("expected_paths") and str(step.get("kind", "")).lower() in wrapper_kinds
    ]
    wrapper_score = EXECUTION_SHAPE_WEIGHTS["wrapper_step_minimality"] - len(wrapper_steps) * 7
    if wrapper_steps:
        penalties.append({"kind": "wrapper_steps_without_artifacts", "steps": wrapper_steps})
    details["wrapper_step_minimality"] = max(0, wrapper_score)

    empty_expected_path_steps = [
        str(step.get("id", ""))
        for step in steps
        if not step.get("expected_paths") and not step.get("verify")
    ]
    empty_score = EXECUTION_SHAPE_WEIGHTS["empty_expected_path_minimality"] - len(empty_expected_path_steps) * 5
    if empty_expected_path_steps:
        penalties.append({"kind": "empty_expected_path_steps", "steps": empty_expected_path_steps})
    details["empty_expected_path_minimality"] = max(0, empty_score)

    verify_steps = [step for step in steps if step.get("verify")]
    owner_verify_steps = [step for step in verify_steps if step.get("expected_paths")]
    verify_only_steps = [step for step in verify_steps if not step.get("expected_paths")]
    if not verify_steps:
        details["verify_owner_coupling"] = 6
    elif owner_verify_steps:
        details["verify_owner_coupling"] = EXECUTION_SHAPE_WEIGHTS["verify_owner_coupling"]
    elif len(verify_only_steps) == 1 and owner_indexes:
        details["verify_owner_coupling"] = 10
    else:
        details["verify_owner_coupling"] = 5
        penalties.append({"kind": "verify_detached_from_artifact_owner"})

    read_before_write_risks = []
    created: set[str] = set()
    expected_set = set(expected)
    for index, step in enumerate(steps):
        instruction = str(step.get("instruction", "")).lower()
        kind = str(step.get("kind", "")).lower()
        if (
            fresh_workspace
            and expected_set
            and not created
            and kind in {"inspect", "analyze", "review"}
            and looks_like_existing_workspace_assumption(instruction)
        ):
            read_before_write_risks.append(str(step.get("id", index)))
        created.update(str(path) for path in step.get("expected_paths", []) or [])
    write_bias_score = EXECUTION_SHAPE_WEIGHTS["write_first_bias"] - len(read_before_write_risks) * 8
    if read_before_write_risks:
        penalties.append({"kind": "read_before_write_shape_risk", "steps": read_before_write_risks})
    details["write_first_bias"] = max(0, write_bias_score)

    last = steps[-1]
    last_kind = str(last.get("kind", "")).lower()
    if last_kind in {"report", "summarize"} and not last.get("expected_paths") and not last.get("verify"):
        details["terminal_finalization_risk"] = 2
        penalties.append({"kind": "terminal_report_step"})
    else:
        details["terminal_finalization_risk"] = EXECUTION_SHAPE_WEIGHTS["terminal_finalization_risk"]

    score = sum(int(details[key]) for key in EXECUTION_SHAPE_WEIGHTS)
    details["penalties"] = penalties
    return {"score": max(0, min(100, score)), "details": details}


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


def score_actionability(steps: list[dict[str, Any]], penalties: list[dict[str, Any]]) -> int:
    actionable = 0
    required = 0
    for step in steps:
        paths = [str(path) for path in step.get("expected_paths", []) or []]
        if not paths:
            continue
        required += 1
        instruction = str(step.get("instruction", "")).lower()
        if has_write_action(instruction):
            actionable += 1
        else:
            penalties.append({"kind": "expected_path_without_write_action", "step": str(step.get("id", ""))})
    if required == 0:
        return 0
    return int(EXECUTABLE_WEIGHTS["actionability"] * actionable / required)


def score_path_instruction_alignment(
    steps: list[dict[str, Any]],
    expected_artifacts: list[str],
    penalties: list[dict[str, Any]],
) -> int:
    expected = set(expected_artifacts)
    owned = []
    aligned = 0
    for step in steps:
        instruction = str(step.get("instruction", ""))
        for path in [str(path) for path in step.get("expected_paths", []) or []]:
            owned.append(path)
            if path_mentions_instruction(path, instruction):
                aligned += 1
            else:
                penalties.append({"kind": "expected_path_not_named_in_instruction", "path": path})
    if not owned:
        return 0
    base = int(EXECUTABLE_WEIGHTS["path_instruction_alignment"] * aligned / len(owned))
    if expected:
        missing = sorted(expected.difference(owned))
        if missing:
            penalties.append({"kind": "expected_artifact_not_owned", "paths": missing})
    return base


def score_read_before_create_risk(
    steps: list[dict[str, Any]],
    expected_artifacts: list[str],
    scenario: dict[str, Any],
    penalties: list[dict[str, Any]],
) -> int:
    created: set[str] = set()
    risk = 0
    expected = set(expected_artifacts)
    fresh_workspace = not scenario_has_seed_inputs(scenario)
    for step in steps:
        instruction = str(step.get("instruction", ""))
        lower = instruction.lower()
        kind = str(step.get("kind", "")).lower()
        if fresh_workspace and expected and not created and kind in {"inspect", "analyze", "review"}:
            if looks_like_existing_workspace_assumption(lower):
                risk += 1
                penalties.append({"kind": "workspace_assumption_before_creation", "step": str(step.get("id", ""))})
        mentions = [path for path in expected if path_mentions_instruction(path, instruction)]
        if mentions and looks_like_pre_creation_read(lower):
            blocked = [path for path in mentions if path not in created]
            if blocked:
                risk += len(blocked)
                penalties.append(
                    {"kind": "read_before_create_risk", "paths": blocked, "step": str(step.get("id", ""))}
                )
        created.update(str(path) for path in step.get("expected_paths", []) or [])
    if risk == 0:
        return EXECUTABLE_WEIGHTS["read_before_create_risk"]
    return max(0, EXECUTABLE_WEIGHTS["read_before_create_risk"] - risk * 10)


def score_verify_executability(steps: list[dict[str, Any]], penalties: list[dict[str, Any]]) -> int:
    commands: list[str] = []
    bad = 0
    missing_verify = 0
    for step in steps:
        kind = str(step.get("kind", "")).lower()
        verify = [str(cmd) for cmd in step.get("verify", []) or []]
        if kind == "verify" and not verify:
            missing_verify += 1
            penalties.append({"kind": "verify_step_without_command", "step": str(step.get("id", ""))})
        commands.extend(verify)
    if missing_verify:
        bad += missing_verify
    for command in commands:
        if has_shell_control(command):
            bad += 1
            penalties.append({"kind": "verify_command_policy_error", "command": command})
        if looks_like_setup_or_server(command):
            bad += 1
            penalties.append({"kind": "verify_command_not_deterministic", "command": command})
    if not commands:
        return 5 if not any(str(step.get("kind", "")).lower() == "verify" for step in steps) else 0
    return max(0, EXECUTABLE_WEIGHTS["verify_executability"] - bad * 10)


def score_step_budget_fit(
    steps: list[dict[str, Any]],
    scenario: dict[str, Any],
    penalties: list[dict[str, Any]],
) -> int:
    size = str(scenario.get("size", "")).lower()
    score = EXECUTABLE_WEIGHTS["step_budget_fit"]
    for step in steps:
        paths = [str(path) for path in step.get("expected_paths", []) or []]
        kind = str(step.get("kind", "")).lower()
        if kind in {"implement", "work", "create", "edit", "repair", "scaffold"}:
            limit = 3 if size == "large" else 2
            if len(paths) > limit:
                score -= (len(paths) - limit) * 4
                penalties.append(
                    {"kind": "step_too_broad_for_budget", "step": str(step.get("id", "")), "paths": paths}
                )
    return max(0, score)


def has_write_action(text: str) -> bool:
    return any(
        word in text
        for word in ["create", "write", "edit", "update", "implement", "add", "fix", "generate", "document", "build"]
    )


def path_mentions_instruction(path: str, instruction: str) -> bool:
    lower = instruction.lower()
    candidate = path.lower()
    name = Path(path).name.lower()
    stem = Path(path).stem.lower()
    parent = "/".join(Path(path).parts[:-1]).lower()
    return candidate in lower or name in lower or (stem and stem in lower) or (parent and parent in lower)


def looks_like_pre_creation_read(instruction: str) -> bool:
    read_words = [
        "inspect",
        "check",
        "read",
        "review",
        "look for",
        "determine whether",
        "whether",
        "exists",
        "already",
    ]
    create_words = ["create", "write", "add", "implement", "generate"]
    return any(word in instruction for word in read_words) and not any(word in instruction for word in create_words)


def looks_like_existing_workspace_assumption(instruction: str) -> bool:
    existing_words = [
        "already",
        "current",
        "existing",
        "exists",
        "layout",
        "nearby",
        "present",
        "repository",
        "whether",
        "workspace",
    ]
    discovery_words = ["identify", "inspect", "review", "understand", "confirm", "determine"]
    return any(word in instruction for word in existing_words) and any(word in instruction for word in discovery_words)


def scenario_has_seed_inputs(scenario: dict[str, Any]) -> bool:
    return any(scenario.get(key) for key in ["seed_files", "fixtures", "input_files", "workspace_files"])


def has_shell_control(command: str) -> bool:
    return any(token in command for token in ["&&", "||", "|", ";", "`", "$("])


def looks_like_setup_or_server(command: str) -> bool:
    lower = command.lower()
    return any(
        token in lower
        for token in ["npm install", "pnpm install", "yarn install", "cargo install", "next dev", "vite --host"]
    )


def plan_text(plan: dict[str, Any]) -> str:
    return json.dumps(plan, ensure_ascii=False, sort_keys=True).lower()


def ratio_score(hit_count: int, total_count: int, weight: int) -> int:
    if total_count <= 0:
        return weight
    return int(weight * hit_count / total_count)


def profile_contract_checks(profile: str, prompt: str) -> dict[str, list[str]]:
    checks: dict[str, list[str]] = {}
    if "3011" in prompt:
        checks["port_3011"] = ["3011"]
    if profile == "nextjs":
        checks.update(
            {
                "package_json": ["package.json"],
                "next_dependency": ["next"],
                "react_dependency": ["react"],
                "react_dom_dependency": ["react-dom"],
                "dev_port_3011": ["next dev -p 3011", "next dev --port 3011", "3011"],
                "build_verify": ["npm run build", "next build"],
                "page_entry": ["src/app/page.tsx"],
                "layout_entry": ["src/app/layout.tsx"],
                "global_types": ["src/app/global.d.ts"],
            }
        )
    return checks


def collect_verify_commands(steps: list[dict[str, Any]]) -> list[str]:
    commands = []
    for step in steps:
        commands.extend(str(command) for command in step.get("verify", []) or [])
    return commands


def command_strength(command: str) -> int:
    lower = command.lower()
    score = 20
    if "curl" in lower and "http" in lower:
        score = 85
    elif any(token in lower for token in ["npm run build", "next build", "cargo build", "tsc", "next lint"]):
        score = 85
    elif any(token in lower for token in ["cargo test", "python3 -m unittest", "python -m unittest", "pytest", "npm test"]):
        score = 90
    elif lower.startswith("node ") and "--check" not in lower:
        score = 70
    elif any(token in lower for token in ["py_compile", "node --check", "cargo check"]):
        score = 45
    elif lower.startswith("grep "):
        score = 35
    elif lower.startswith("test -f ") or lower.startswith("test -s "):
        score = 25
    elif lower.startswith("cat ") or lower.startswith("ls "):
        score = 10
    if has_shell_control(command):
        score = max(0, score - 20)
    if looks_like_setup_or_server(command):
        score = min(score, 25)
    return score


def allowed_extra_artifact(path: str, scenario: dict[str, Any]) -> bool:
    profile = str(scenario.get("profile", "generic")).lower()
    allowed = set()
    if profile == "nextjs":
        allowed.update({"tsconfig.json", "src/app/globals.css", "next-env.d.ts"})
    return path in allowed


def nested_path_is_naturally_owned(path: str, steps: list[dict[str, Any]]) -> bool:
    parent = str(Path(path).parent).lower()
    filename = Path(path).name.lower()
    for step in steps:
        paths = [str(item) for item in step.get("expected_paths", []) or []]
        if path not in paths:
            continue
        instruction = str(step.get("instruction", "")).lower()
        if path.lower() in instruction or parent in instruction or filename in instruction:
            return True
    return False


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


def executable_penalty_points(penalties: list[dict[str, Any]]) -> int:
    points = 0
    for penalty in penalties:
        kind = penalty.get("kind")
        if kind == "read_before_create_risk":
            points += 12
        elif kind == "workspace_assumption_before_creation":
            points += 8
        elif kind == "verify_command_policy_error":
            points += 15
        elif kind == "verify_step_without_command":
            points += 15
        elif kind == "expected_path_without_write_action":
            points += 8
        elif kind == "expected_path_not_named_in_instruction":
            points += 4
        elif kind == "expected_artifact_not_owned":
            points += 12
        elif kind == "verify_command_not_deterministic":
            points += 10
        elif kind == "step_too_broad_for_budget":
            points += 6
    return points
