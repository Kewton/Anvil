from __future__ import annotations

import re
from dataclasses import dataclass, field
from typing import Any


INTERACTIVE_GAME_CAPABILITIES = [
    "stateful_interaction",
    "start_or_restart_flow",
    "player_control",
    "adversary_or_challenge",
    "progression_or_score",
    "failure_or_collision_rule",
]

INTERACTIVE_WEB_APP_CAPABILITIES = [
    "stateful_interaction",
    "user_input_or_action",
    "visible_state_change",
]

CLI_TOOL_CAPABILITIES = [
    "entrypoint",
    "deterministic_check",
]

LIBRARY_WITH_TESTS_CAPABILITIES = [
    "implementation",
    "deterministic_test",
]

DOCS_CONTENT_CAPABILITIES = [
    "requested_content",
]

DATA_TRANSFORM_CAPABILITIES = [
    "input_output_contract",
    "deterministic_check",
]


@dataclass
class AcceptanceContract:
    category: str = "generic"
    required_capabilities: list[str] = field(default_factory=list)
    optional_capabilities: list[str] = field(default_factory=list)
    required_obligations: list[str] = field(default_factory=list)
    forbidden_minimal_outputs: list[str] = field(default_factory=list)
    interaction: dict[str, Any] = field(default_factory=dict)
    runtime: dict[str, Any] = field(default_factory=dict)
    oracle_contract: dict[str, Any] = field(default_factory=dict)
    explicit: bool = False
    warnings: list[str] = field(default_factory=list)

    def to_dict(self) -> dict[str, Any]:
        return {
            "category": self.category,
            "required_capabilities": self.required_capabilities,
            "optional_capabilities": self.optional_capabilities,
            "required_obligations": self.required_obligations,
            "forbidden_minimal_outputs": self.forbidden_minimal_outputs,
            "interaction": self.interaction,
            "runtime": self.runtime,
            "oracle_contract": self.oracle_contract,
            "explicit": self.explicit,
            "warnings": self.warnings,
        }


def contract_from_scenario(scenario: dict[str, Any]) -> AcceptanceContract:
    explicit_contract = scenario.get("functional_contract") or {}
    quality_contract = scenario.get("quality_contract") or {}
    interaction_contract = scenario.get("interaction_contract") or {}
    oracle_contract = scenario.get("oracle_contract") or {}
    prompt = str(scenario.get("prompt", ""))
    profile = str(scenario.get("profile", "generic"))
    category = str(explicit_contract.get("category") or infer_contract_category(scenario))
    required = list(explicit_contract.get("required_capabilities") or default_capabilities(category))
    optional = list(explicit_contract.get("optional_capabilities") or [])
    required_obligations = list(
        explicit_contract.get("required_obligations") or default_obligations(category)
    )
    forbidden = list(
        explicit_contract.get("forbidden_minimal_outputs")
        or quality_contract.get("forbidden_minimal_outputs")
        or default_forbidden_outputs(category)
    )
    interaction = {
        **default_interaction(category, prompt),
        **as_dict(interaction_contract),
        **as_dict(explicit_contract.get("interaction")),
    }
    runtime = infer_runtime_contract(prompt)
    if profile:
        runtime.setdefault("profile", profile)
    minimum_acceptance = quality_contract.get("minimum_acceptance")
    if minimum_acceptance:
        runtime["minimum_acceptance"] = minimum_acceptance
    oracles = {
        "deterministic_oracles": default_deterministic_oracles(category),
        **as_dict(oracle_contract),
    }
    warnings: list[str] = []
    for capability in required:
        if not isinstance(capability, str) or not capability.strip():
            warnings.append("empty_required_capability")
    return AcceptanceContract(
        category=category,
        required_capabilities=[str(item) for item in required if str(item).strip()],
        optional_capabilities=[str(item) for item in optional if str(item).strip()],
        required_obligations=[str(item) for item in required_obligations if str(item).strip()],
        forbidden_minimal_outputs=[str(item) for item in forbidden if str(item).strip()],
        interaction=interaction,
        runtime=runtime,
        oracle_contract=oracles,
        explicit=bool(explicit_contract),
        warnings=warnings,
    )


def infer_contract_category(scenario: dict[str, Any]) -> str:
    prompt = str(scenario.get("prompt", "")).lower()
    category = str(scenario.get("category", "")).lower()
    profile = str(scenario.get("profile", "")).lower()
    if has_any(prompt, ["game", "ゲーム", "space invaders", "invader", "シューティング"]):
        return "interactive-game"
    if has_any(prompt, ["cli", "command line", "コマンド", "terminal"]) or category in {"cli", "cli-tool"}:
        return "cli-tool"
    if has_any(prompt, ["readme", "docs", "documentation", "ドキュメント", "説明"]):
        return "docs-content"
    if has_any(prompt, ["library", "unit test", "unittest", "pytest", "cargo test", "ライブラリ"]):
        return "library-with-tests"
    if has_any(prompt, ["transform", "convert", "parse", "schema", "変換"]):
        return "data-transform"
    if profile in {"nextjs", "web", "vite"} or has_any(prompt, ["web app", "next.js", "nextjs", "form", "button"]):
        return "interactive-web-app" if has_any(prompt, ["interactive", "input", "click", "keyboard", "操作"]) else "web-app"
    return "generic"


def default_capabilities(category: str) -> list[str]:
    mapping = {
        "interactive-game": INTERACTIVE_GAME_CAPABILITIES,
        "interactive-web-app": INTERACTIVE_WEB_APP_CAPABILITIES,
        "cli-tool": CLI_TOOL_CAPABILITIES,
        "library-with-tests": LIBRARY_WITH_TESTS_CAPABILITIES,
        "docs-content": DOCS_CONTENT_CAPABILITIES,
        "data-transform": DATA_TRANSFORM_CAPABILITIES,
    }
    return list(mapping.get(category, []))


def default_obligations(category: str) -> list[str]:
    mapping = {
        "interactive-game": [
            "setup",
            "scaffold",
            "implementation",
            "verification",
            "acceptance_evidence",
        ],
        "interactive-web-app": ["setup", "scaffold", "implementation", "verification"],
        "web-app": ["setup", "scaffold", "implementation", "verification"],
        "cli-tool": ["implementation", "verification"],
        "library-with-tests": ["implementation", "verification"],
        "docs-content": ["acceptance_evidence"],
        "data-transform": ["implementation", "verification", "acceptance_evidence"],
    }
    return list(mapping.get(category, []))


def default_forbidden_outputs(category: str) -> list[str]:
    if category in {"interactive-game", "interactive-web-app"}:
        return ["static_title_only"]
    return []


def default_interaction(category: str, prompt: str) -> dict[str, Any]:
    if category == "interactive-game":
        return {
            "keyboard": has_any(prompt.lower(), ["keyboard", "key", "キー", "操作"]) or True,
            "pointer": "optional",
        }
    if category == "interactive-web-app":
        return {"keyboard": "optional", "pointer": "optional"}
    return {}


def default_deterministic_oracles(category: str) -> list[str]:
    if category in {"interactive-game", "interactive-web-app"}:
        return ["source_semantic", "browser_interaction"]
    if category in {"cli-tool", "library-with-tests", "data-transform"}:
        return ["source_semantic", "postcheck"]
    if category == "docs-content":
        return ["source_semantic"]
    return []


def infer_runtime_contract(prompt: str) -> dict[str, Any]:
    runtime: dict[str, Any] = {}
    match = re.search(r"(?:port|ポート)\s*([0-9]{2,5})|([0-9]{2,5})\s*(?:port|ポート)", prompt, re.IGNORECASE)
    if match:
        runtime["port"] = int(match.group(1) or match.group(2))
    return runtime


def has_any(text: str, needles: list[str]) -> bool:
    return any(needle.lower() in text for needle in needles)


def as_dict(value: Any) -> dict[str, Any]:
    return value if isinstance(value, dict) else {}
