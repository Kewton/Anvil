from __future__ import annotations

import re
from typing import Any

from .acceptance_contract import AcceptanceContract, contract_from_scenario


BEHAVIOR_ORACLE_MARKERS = [
    "playwright",
    "cypress",
    "puppeteer",
    "testing-library",
    "vitest",
    "jest",
    "npm test",
    "python3 -m unittest",
    "python -m unittest",
    "pytest",
    "cargo test",
    "go test",
]


def score_verify_adequacy_for_plan(
    plan: dict[str, Any],
    scenario: dict[str, Any] | None = None,
) -> dict[str, Any]:
    return score_verify_adequacy(plan.get("steps") or [], scenario or {})


def score_verify_adequacy(
    steps: list[dict[str, Any]],
    scenario: dict[str, Any] | None = None,
    contract: AcceptanceContract | None = None,
) -> dict[str, Any]:
    scenario = scenario or {}
    contract = contract or contract_from_scenario(scenario)
    commands = collect_verify_commands(steps)
    text = "\n".join(commands).lower()
    required = list(contract.required_capabilities)
    semantic_score, missing_semantic = semantic_verify_coverage(text, required)
    behavior_score = behavior_oracle_declared_score(text, contract)
    contentless = [command for command in commands if contentless_verify_command(command)]
    contentless_penalty = min(70.0, len(contentless) * 18.0)
    if required and commands and all(build_or_file_only_command(command) for command in commands):
        contentless_penalty = max(contentless_penalty, 45.0)
    no_verify_penalty = 35.0 if not commands and required else 0.0
    score = round(
        0.45 * semantic_score
        + 0.35 * behavior_score
        + 0.20 * max(0.0, 100.0 - contentless_penalty)
        - no_verify_penalty,
        1,
    )
    if required and behavior_score < 40.0:
        score = min(score, 65.0)
    if required and semantic_score < 50.0:
        score = min(score, 55.0)
    return {
        "verify_adequacy_score": max(0.0, min(100.0, score)),
        "semantic_verify_coverage_score": round(semantic_score, 1),
        "behavior_oracle_declared_score": round(behavior_score, 1),
        "contentless_verify_penalty": round(contentless_penalty + no_verify_penalty, 1),
        "verify_adequacy_details": {
            "commands": commands,
            "required_capabilities": required,
            "missing_semantic_verify_capabilities": missing_semantic,
            "contentless_commands": contentless,
            "contract_category": contract.category,
        },
    }


def semantic_verify_coverage(text: str, required: list[str]) -> tuple[float, list[str]]:
    if not required:
        return 100.0, []
    missing = []
    hits = 0
    for capability in required:
        keywords = capability_keywords(capability)
        if any(keyword in text for keyword in keywords):
            hits += 1
        else:
            missing.append(capability)
    return 100.0 * hits / max(1, len(required)), missing


def behavior_oracle_declared_score(text: str, contract: AcceptanceContract) -> float:
    if not contract.required_capabilities:
        return 100.0
    if any(marker in text for marker in BEHAVIOR_ORACLE_MARKERS):
        return 100.0
    deterministic = contract.oracle_contract.get("deterministic_oracles") or []
    if "browser_interaction" in deterministic and any(marker in text for marker in ["browser", "interaction", "keydown", "click"]):
        return 75.0
    if "source_semantic" in deterministic and any(marker in text for marker in ["grep", "assert", "semantic", "capability"]):
        return 55.0
    if any(marker in text for marker in ["npm run build", "next build", "cargo build", "tsc"]):
        return 35.0
    return 20.0 if text else 0.0


def collect_verify_commands(steps: list[dict[str, Any]]) -> list[str]:
    commands: list[str] = []
    for step in steps:
        commands.extend(str(command) for command in step.get("verify", []) or [])
    return [command.strip() for command in commands if command.strip()]


def contentless_verify_command(command: str) -> bool:
    lower = " ".join(command.lower().split())
    if lower.startswith(("cat ", "ls ", "test -f ", "test -s ")):
        return True
    if lower.startswith("grep ") and not re.search(r"grep\s+-q\s+['\"]?.{8,}", lower):
        return True
    if lower.startswith("node ") and "-e" in lower and any(token in lower for token in ["readfilesync", "existssync"]):
        return True
    return False


def build_or_file_only_command(command: str) -> bool:
    lower = command.lower()
    return (
        contentless_verify_command(command)
        or "npm run build" in lower
        or "next build" in lower
        or lower.startswith("tsc")
        or lower.startswith("cargo build")
    )


def capability_keywords(capability: str) -> list[str]:
    mapping = {
        "stateful_interaction": ["state", "usestate", "interaction", "event", "canvas", "keyboard"],
        "start_or_restart_flow": ["start", "restart", "reset", "game state"],
        "player_control": ["player", "keyboard", "keydown", "control", "move"],
        "adversary_or_challenge": ["enemy", "invader", "challenge", "wave", "obstacle"],
        "progression_or_score": ["score", "progress", "level", "stage", "points"],
        "failure_or_collision_rule": ["collision", "hit", "failure", "game over", "lives"],
        "user_input_or_action": ["input", "click", "submit", "keyboard", "interaction"],
        "visible_state_change": ["state", "render", "update", "change"],
        "entrypoint": ["entrypoint", "main", "cli"],
        "deterministic_check": ["test", "assert", "check"],
        "implementation": ["implementation", "function", "module"],
        "deterministic_test": ["test", "assert"],
        "requested_content": ["content", "heading", "example", "usage"],
        "input_output_contract": ["input", "output", "transform", "parse"],
    }
    return mapping.get(capability, [capability.replace("_", " ")])
