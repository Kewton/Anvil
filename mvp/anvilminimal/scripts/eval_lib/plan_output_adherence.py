from __future__ import annotations

from pathlib import Path
from typing import Any

from .plan_capability_contract import (
    PLAN_CAPABILITY_ORACLE_VERSION,
    collect_plan_contract,
    required_capabilities_from_plan_text,
    source_capability_detected,
)
from .acceptance_contract import contract_from_scenario
from .source_semantic_oracle import collect_source_corpus, evaluate_source_semantics


PLAN_OUTPUT_ORACLE_VERSION = "plan-output-v2-connected-output"
INTERACTIVE_PLAN_CAPABILITIES = {
    "render_loop_or_canvas",
    "keyboard_or_player_control",
    "player_entity",
    "adversary_entity",
    "projectile_or_shooting",
    "collision_or_failure_rule",
    "score_or_progression",
    "audio_feedback",
    "visual_effects",
}


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
    contract = contract_from_scenario(scenario)
    source_semantic = evaluate_source_semantics(scenario, workdir, contract)
    connection_missing = (
        bool(set(required).intersection(INTERACTIVE_PLAN_CAPABILITIES))
        and source_semantic.get("source_semantic_success") is False
        and contract.category in {"interactive-game", "interactive-web-app"}
    )
    if connection_missing:
        capability_results["implemented_capability_connection"] = False
        if "implemented_capability_connection" not in missing:
            missing.append("implemented_capability_connection")
    score = round(100.0 * (len(required) - len(missing)) / max(1, len(required)), 1)
    if connection_missing:
        score = min(score, 55.0)
    failure_kind = ""
    if missing:
        failure_kind = "plan_output_missing_required_capabilities"
    elif connection_missing:
        failure_kind = "plan_output_capability_not_connected"
    return {
        "plan_output_adherence_success": not missing,
        "plan_output_adherence_score": score,
        "plan_output_failure_kind": failure_kind,
        "plan_output_oracle_version": PLAN_OUTPUT_ORACLE_VERSION,
        "plan_output_details": {
            "applicable": True,
            "gate_evidence": ["source_semantic_success"],
            "predictor_evidence": ["plan_required_capabilities", "source_capability_tokens"],
            "plan_paths": [str(path) for path in plan_paths],
            "files_scanned": [path for path, _ in corpus],
            "plan_capability_oracle_version": PLAN_CAPABILITY_ORACLE_VERSION,
            "source_semantic_category": source_semantic.get("source_semantic_details", {}).get("contract", {}).get("category", ""),
            "parse_errors": parse_errors,
            "required_capabilities": required,
            "capabilities": capability_results,
            "missing_capabilities": missing,
            "connection_missing": connection_missing,
            "source_semantic_failure_kind": source_semantic.get("source_semantic_failure_kind", ""),
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
