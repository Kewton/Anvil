from __future__ import annotations

import re
from pathlib import Path
from typing import Any

from .acceptance_contract import AcceptanceContract, contract_from_scenario


SOURCE_EXTENSIONS = {
    ".js",
    ".jsx",
    ".ts",
    ".tsx",
    ".py",
    ".rs",
    ".go",
    ".java",
    ".css",
    ".html",
    ".md",
}

SKIP_DIRS = {".git", ".anvil", ".next", "node_modules", "target", "dist", "build"}

CAPABILITY_PATTERNS = {
    "stateful_interaction": [
        "usestate",
        "usereducer",
        "setstate",
        "useeffect",
        "addeventlistener",
        "onkeydown",
        "onkeyup",
        "onclick",
        "requestanimationframe",
        "setinterval",
        "canvas",
        "getcontext(",
    ],
    "start_or_restart_flow": [
        "start",
        "restart",
        "reset",
        "gamestate",
        "setgamestate",
        "gameover",
        "keydown",
        "onclick",
        "press",
        "スタート",
        "開始",
    ],
    "player_control": [
        "player",
        "keydown",
        "keyup",
        "arrowleft",
        "arrowright",
        "arrowup",
        "arrowdown",
        "wasd",
        "moveplayer",
        "movement",
        "自機",
    ],
    "adversary_or_challenge": [
        "enemy",
        "enemies",
        "invader",
        "alien",
        "obstacle",
        "wave",
        "spawn",
        "challenge",
        "敵",
    ],
    "progression_or_score": [
        "score",
        "points",
        "level",
        "stage",
        "wave",
        "combo",
        "lives",
        "life",
        "スコア",
    ],
    "failure_or_collision_rule": [
        "collision",
        "collide",
        "hit",
        "intersect",
        "overlap",
        "damage",
        "gameover",
        "lives",
        "life",
        "衝突",
        "当たり",
    ],
    "user_input_or_action": [
        "onchange",
        "onclick",
        "onsubmit",
        "onkeydown",
        "addeventlistener",
        "input",
        "button",
        "form",
    ],
    "visible_state_change": [
        "usestate",
        "usereducer",
        "setstate",
        "setinterval",
        "requestanimationframe",
        "classlist",
        "innertext",
        "textcontent",
    ],
    "entrypoint": ["if __name__", "fn main", "main()", "process.argv", "argparse", "clap"],
    "deterministic_check": ["unittest", "pytest", "cargo test", "#[test]", "assert", "npm test"],
    "implementation": ["def ", "fn ", "function ", "export function", "class "],
    "deterministic_test": ["unittest", "pytest", "#[test]", "assert", "describe(", "it("],
    "requested_content": ["usage", "example", "command", "readme", "使い方", "例"],
    "input_output_contract": ["input", "output", "parse", "transform", "convert", "schema"],
}


def evaluate_source_semantics(
    scenario: dict[str, Any],
    workdir: Path,
    contract: AcceptanceContract | None = None,
) -> dict[str, Any]:
    contract = contract or contract_from_scenario(scenario)
    required = list(contract.required_capabilities)
    if not required:
        return {
            "source_semantic_success": "",
            "source_semantic_score": "",
            "source_semantic_failure_kind": "",
            "source_semantic_details": {
                "applicable": False,
                "contract": contract.to_dict(),
                "reason": "no_required_capabilities",
            },
        }

    corpus = collect_source_corpus(workdir, scenario)
    text = "\n".join(source for _, source in corpus).lower()
    capability_results = {
        capability: capability_detected(capability, text)
        for capability in required
    }
    missing = [capability for capability, ok in capability_results.items() if not ok]
    static_title = detects_static_title_only(text, contract)
    placeholder_tokens = detects_placeholder_tokens(text, contract)
    hit_count = len(required) - len(missing)
    score = round(100.0 * hit_count / max(1, len(required)), 1)
    failure_kind = ""
    if placeholder_tokens:
        score = min(score, 40.0)
        failure_kind = "placeholder_tokens"
    elif static_title:
        score = min(score, 35.0)
        failure_kind = "static_title_only"
    elif missing:
        failure_kind = "missing_required_capabilities"
    success = not missing and not static_title and not placeholder_tokens
    return {
        "source_semantic_success": success,
        "source_semantic_score": score,
        "source_semantic_failure_kind": failure_kind,
        "source_semantic_details": {
            "applicable": True,
            "contract": contract.to_dict(),
            "files_scanned": [path for path, _ in corpus],
            "capabilities": capability_results,
            "missing_capabilities": missing,
            "static_title_only": static_title,
            "placeholder_tokens": placeholder_tokens,
        },
    }


def collect_source_corpus(workdir: Path, scenario: dict[str, Any]) -> list[tuple[str, str]]:
    paths: list[Path] = []
    for artifact in scenario.get("expected_artifacts", []) or []:
        candidate = workdir / str(artifact)
        if candidate.is_file():
            paths.append(candidate)
    if workdir.exists():
        for path in workdir.rglob("*"):
            if not path.is_file():
                continue
            if any(part in SKIP_DIRS for part in path.parts):
                continue
            if path.suffix.lower() in SOURCE_EXTENSIONS:
                paths.append(path)
    out: list[tuple[str, str]] = []
    seen: set[Path] = set()
    for path in paths:
        resolved = path.resolve()
        if resolved in seen:
            continue
        seen.add(resolved)
        try:
            rel = str(path.relative_to(workdir))
        except ValueError:
            rel = str(path)
        try:
            out.append((rel, path.read_text(encoding="utf-8", errors="replace")[:200_000]))
        except OSError:
            continue
    return out


def capability_detected(capability: str, text: str) -> bool:
    patterns = CAPABILITY_PATTERNS.get(capability, [])
    return any(pattern in text for pattern in patterns)


def detects_static_title_only(text: str, contract: AcceptanceContract) -> bool:
    if "static_title_only" not in contract.forbidden_minimal_outputs:
        return False
    has_title_like_output = bool(
        re.search(r"space\s*invaders|press\s+any\s+key|title|hero|landing", text, re.IGNORECASE)
    )
    has_interaction = any(
        pattern in text
        for pattern in [
            "usestate",
            "usereducer",
            "useeffect",
            "addeventlistener",
            "onkeydown",
            "onclick",
            "requestanimationframe",
            "setinterval",
            "canvas",
            "getcontext(",
        ]
    )
    return has_title_like_output and not has_interaction


def detects_placeholder_tokens(text: str, contract: AcceptanceContract) -> bool:
    if not contract.required_capabilities:
        return False
    if not any(marker in text for marker in ["todo", "placeholder", "stub", "not implemented"]):
        return False
    if contract.category in {"interactive-game", "interactive-web-app"}:
        has_interactive_structure = any(
            pattern in text
            for pattern in [
                "usestate",
                "useeffect",
                "addeventlistener",
                "requestanimationframe",
                "setinterval",
                "onkeydown",
                "onclick",
                "canvas",
                "getcontext(",
            ]
        )
        return not has_interactive_structure
    has_code_structure = any(
        pattern in text
        for pattern in [
            "usestate",
            "useeffect",
            "addeventlistener",
            "requestanimationframe",
            "fn main",
            "def ",
            "function ",
            "class ",
        ]
    )
    return not has_code_structure
