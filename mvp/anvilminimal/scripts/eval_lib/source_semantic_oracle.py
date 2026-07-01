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
        "hazard",
        "target",
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
    forbidden_minimal_output = detects_forbidden_minimal_output(corpus, text, contract)
    connection = interactive_connection_evidence(text, contract)
    hit_count = len(required) - len(missing)
    score = round(100.0 * hit_count / max(1, len(required)), 1)
    failure_kind = ""
    if placeholder_tokens:
        score = min(score, 40.0)
        failure_kind = "placeholder_tokens"
    elif static_title:
        score = min(score, 35.0)
        failure_kind = "static_title_only"
    elif forbidden_minimal_output:
        score = min(score, 35.0)
        failure_kind = forbidden_minimal_output
    elif connection.get("applicable") and not connection.get("connected"):
        score = min(score, float(connection.get("score_cap", 55.0)))
        failure_kind = str(connection.get("failure_kind", "interactive_connection_missing"))
    elif missing:
        failure_kind = "missing_required_capabilities"
    success = (
        not missing
        and not static_title
        and not placeholder_tokens
        and not forbidden_minimal_output
        and not (connection.get("applicable") and not connection.get("connected"))
    )
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
            "forbidden_minimal_output": forbidden_minimal_output,
            "interactive_connection": connection,
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


def interactive_connection_evidence(text: str, contract: AcceptanceContract) -> dict[str, Any]:
    if contract.category not in {"interactive-game", "interactive-web-app"}:
        return {"applicable": False}
    signals = {
        "input_subscription": has_pattern(
            text,
            [
                r"addeventlistener\s*\(\s*['\"](?:keydown|keyup|click|pointer|mouse)",
                r"\bon(?:key|click|submit|change)",
                r"keyboardevent",
            ],
        ),
        "state_mutation": has_pattern(
            text,
            [
                r"\buse(?:state|reducer)\b",
                r"\bset[A-Z][A-Za-z0-9_]*\s*\(",
                r"\bdispatch\s*\(",
                r"\.current\s*=",
                r"\+=|-=|\+\+|--",
            ],
        ),
        "render_or_tick": has_pattern(
            text,
            [
                r"requestanimationframe",
                r"setinterval\s*\(",
                r"settimeout\s*\(",
                r"\bcanvas\b",
                r"getcontext\s*\(",
                r"\bmap\s*\(",
            ],
        ),
        "domain_entities": has_pattern(
            text,
            [
                r"\bplayer\b",
                r"\benem(?:y|ies)\b",
                r"\binvader",
                r"\bbullet",
                r"\bprojectile",
                r"\bscore\b",
                r"\blives?\b",
            ],
        ),
        "progression_update": has_pattern(
            text,
            [
                r"setscore\s*\(",
                r"setlives\s*\(",
                r"setgamestate\s*\(",
                r"\bscore\s*(?:\+|=)",
                r"\blives?\s*(?:-|=)",
                r"gameover",
            ],
        ),
        "rule_or_collision": has_pattern(
            text,
            [
                r"\bcollid",
                r"\bhit\b",
                r"\bintersect",
                r"\boverlap",
                r"\bdamage",
                r"\.x\s*[<>]=?\s*[^&|;\n]+\.x",
                r"\.y\s*[<>]=?\s*[^&|;\n]+\.y",
            ],
        ),
    }
    required = 4 if contract.category == "interactive-game" else 2
    if contract.category == "interactive-game" and not signals["input_subscription"]:
        return {
            "applicable": True,
            "connected": False,
            "failure_kind": "interactive_input_not_connected",
            "score_cap": 50.0,
            "required_signal_count": required,
            "signals": signals,
        }
    count = sum(1 for value in signals.values() if value)
    connected = count >= required
    return {
        "applicable": True,
        "connected": connected,
        "failure_kind": "" if connected else "interactive_connection_missing",
        "score_cap": 55.0,
        "required_signal_count": required,
        "signal_count": count,
        "signals": signals,
    }


def has_pattern(text: str, patterns: list[str]) -> bool:
    return any(re.search(pattern, text, re.IGNORECASE) for pattern in patterns)


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


def detects_forbidden_minimal_output(
    corpus: list[tuple[str, str]],
    text: str,
    contract: AcceptanceContract,
) -> str:
    if contract.category not in {"interactive-game", "interactive-web-app"}:
        return ""
    forbidden = set(contract.forbidden_minimal_outputs)
    if not corpus:
        return "empty_output" if "empty_output" in forbidden else ""
    paths = [path.lower() for path, _ in corpus]
    suffixes = [Path(path).suffix.lower() for path in paths]
    if "docs_only" in forbidden and suffixes and all(suffix == ".md" for suffix in suffixes):
        return "docs_only"
    if "style_only" in forbidden and suffixes and all(
        suffix in {".css", ".scss", ".sass", ".less"} for suffix in suffixes
    ):
        return "style_only"
    if "manifest_only" in forbidden and paths and all(is_manifest_or_config_path(path) for path in paths):
        return "manifest_only"
    source_paths = [
        path
        for path, suffix in zip(paths, suffixes)
        if suffix in {".js", ".jsx", ".ts", ".tsx", ".html"}
        and not is_manifest_or_config_path(path)
    ]
    if "scaffold_only" in forbidden and source_paths and not has_interactive_structure(text):
        has_scaffold_shape = any(
            marker in text
            for marker in [
                "export default function",
                "children",
                "<html",
                "<body",
                "metadata",
                "return null",
                "coming soon",
            ]
        )
        if has_scaffold_shape:
            return "scaffold_only"
    return ""


def is_manifest_or_config_path(path: str) -> bool:
    name = Path(path).name.lower()
    return name in {
        "package.json",
        "package-lock.json",
        "pnpm-lock.yaml",
        "yarn.lock",
        "bun.lockb",
        "tsconfig.json",
        "next.config.js",
        "next.config.mjs",
        "next.config.ts",
        "postcss.config.js",
        "tailwind.config.js",
        "tailwind.config.ts",
    }


def has_interactive_structure(text: str) -> bool:
    return any(
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
