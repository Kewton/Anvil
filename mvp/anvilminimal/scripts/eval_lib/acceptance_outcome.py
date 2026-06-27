from __future__ import annotations

from pathlib import Path
from typing import Any

from .acceptance_contract import AcceptanceContract, contract_from_scenario
from .plan_output_adherence import evaluate_plan_output_adherence
from .postcheck import load_postcheck_events
from .source_semantic_oracle import evaluate_source_semantics


ACCEPTANCE_ORACLE_VERSION = "acceptance-v2-plan-output"


def evaluate_acceptance_outcome(
    *,
    scenario: dict[str, Any],
    workdir: Path,
    run_dir: Path,
    mode: str,
    process_success: bool,
    legacy_success: bool,
    postcheck: dict[str, Any],
    plan_paths: list[Path] | None = None,
    browser_result: dict[str, Any] | None = None,
) -> dict[str, Any]:
    if mode == "step-plan":
        return not_applicable_outcome("plan_only_mode", legacy_success)

    contract = contract_from_scenario(scenario)
    artifact_success = expected_artifacts_success(scenario, workdir)
    post_events = load_postcheck_events(Path(postcheck["events_path"])) if postcheck.get("events_path") else []
    build_success = build_success_from_events(post_events, scenario)
    launch_success = launch_success_from_events(post_events, scenario)
    semantic = evaluate_source_semantics(scenario, workdir, contract)
    semantic_success = semantic.get("source_semantic_success", "")
    plan_output = evaluate_plan_output_adherence(
        plan_paths=plan_paths or [],
        workdir=workdir,
        scenario=scenario,
    )
    plan_output_success = plan_output.get("plan_output_adherence_success", "")
    browser_result = browser_result or {"browser_success": "", "browser_failure_kind": "", "browser_details": {}}
    behavior_success = combine_behavior_success(semantic_success, browser_result.get("browser_success", ""))
    prompt_contract_success = semantic_success if semantic_success != "" else True

    acceptance_success = (
        bool(process_success)
        and bool(artifact_success)
        and build_success is not False
        and launch_success is not False
        and semantic_success is not False
        and plan_output_success is not False
        and browser_result.get("browser_success", "") is not False
        and bool(postcheck.get("ok", True))
    )
    failure_kind = acceptance_failure_kind(
        process_success=process_success,
        artifact_success=artifact_success,
        build_success=build_success,
        launch_success=launch_success,
        semantic_success=semantic_success,
        plan_output_success=plan_output_success,
        browser_success=browser_result.get("browser_success", ""),
        post_ok=bool(postcheck.get("ok", True)),
        semantic_failure_kind=str(semantic.get("source_semantic_failure_kind", "")),
        plan_output_failure_kind=str(plan_output.get("plan_output_failure_kind", "")),
    )
    false_positive = bool(legacy_success) and not bool(acceptance_success)
    return {
        "legacy_success": legacy_success,
        "process_success": bool(process_success),
        "artifact_success": artifact_success,
        "build_success": build_success,
        "launch_success": launch_success,
        "behavior_success": behavior_success,
        "source_semantic_success": semantic_success,
        "source_semantic_score": semantic.get("source_semantic_score", ""),
        "plan_output_adherence_success": plan_output_success,
        "plan_output_adherence_score": plan_output.get("plan_output_adherence_score", ""),
        "plan_output_failure_kind": plan_output.get("plan_output_failure_kind", ""),
        "prompt_contract_success": prompt_contract_success,
        "acceptance_success": acceptance_success,
        "acceptance_failure_kind": failure_kind,
        "acceptance_false_positive": false_positive,
        "oracle_gap_kind": oracle_gap_kind(false_positive, semantic, plan_output, postcheck),
        "acceptance_oracle_version": ACCEPTANCE_ORACLE_VERSION,
        "acceptance_details": {
            "contract": contract.to_dict(),
            "source_semantic": semantic,
            "plan_output": plan_output,
            "browser": browser_result,
        },
    }


def not_applicable_outcome(reason: str, legacy_success: bool) -> dict[str, Any]:
    return {
        "legacy_success": legacy_success,
        "process_success": "",
        "artifact_success": "",
        "build_success": "",
        "launch_success": "",
        "behavior_success": "",
        "source_semantic_success": "",
        "source_semantic_score": "",
        "plan_output_adherence_success": "",
        "plan_output_adherence_score": "",
        "plan_output_failure_kind": "",
        "prompt_contract_success": "",
        "acceptance_success": "",
        "acceptance_failure_kind": "",
        "acceptance_false_positive": "",
        "oracle_gap_kind": reason,
        "acceptance_oracle_version": ACCEPTANCE_ORACLE_VERSION,
        "acceptance_details": {"reason": reason},
    }


def expected_artifacts_success(scenario: dict[str, Any], workdir: Path) -> bool:
    expected = [str(path) for path in scenario.get("expected_artifacts", []) or []]
    return all((workdir / path).exists() for path in expected)


def build_success_from_events(events: list[dict[str, Any]], scenario: dict[str, Any]) -> bool | str:
    build_commands = {
        str(command)
        for command in scenario.get("postcheck", {}).get("commands", []) or []
        if is_build_command(str(command))
    }
    if not build_commands:
        return ""
    matching = [
        event
        for event in events
        if event.get("event") == "postcheck" and str(event.get("command", "")) in build_commands
    ]
    if not matching:
        return False
    return all(int(event.get("rc") or 0) == 0 for event in matching)


def launch_success_from_events(events: list[dict[str, Any]], scenario: dict[str, Any]) -> bool | str:
    if not scenario.get("postcheck", {}).get("dev_server"):
        return ""
    dev_events = [event for event in events if event.get("event") == "dev_server"]
    if not dev_events:
        return False
    return bool(dev_events[-1].get("ready"))


def combine_behavior_success(source_success: object, browser_success: object) -> bool | str:
    values = [value for value in [source_success, browser_success] if value != ""]
    if not values:
        return ""
    return all(value is True for value in values)


def acceptance_failure_kind(
    *,
    process_success: bool,
    artifact_success: bool,
    build_success: bool | str,
    launch_success: bool | str,
    semantic_success: object,
    plan_output_success: object,
    browser_success: object,
    post_ok: bool,
    semantic_failure_kind: str,
    plan_output_failure_kind: str,
) -> str:
    if not process_success:
        return "process_failure"
    if not artifact_success:
        return "artifact_failure"
    if not post_ok:
        return "postcheck_failure"
    if build_success is False:
        return "build_failure"
    if launch_success is False:
        return "launch_failure"
    if plan_output_success is False:
        return plan_output_failure_kind or "plan_output_contract_failure"
    if semantic_success is False:
        return semantic_failure_kind or "source_semantic_failure"
    if browser_success is False:
        return "browser_behavior_failure"
    return ""


def oracle_gap_kind(
    false_positive: bool,
    semantic: dict[str, Any],
    plan_output: dict[str, Any],
    postcheck: dict[str, Any],
) -> str:
    if not false_positive:
        return ""
    if plan_output.get("plan_output_failure_kind"):
        return "postcheck_too_weak_for_plan_contract"
    if semantic.get("source_semantic_failure_kind"):
        return "postcheck_too_weak_for_semantic_contract"
    if postcheck.get("ok"):
        return "acceptance_oracle_gap"
    return ""


def is_build_command(command: str) -> bool:
    lowered = command.lower().strip()
    return lowered in {"npm run build", "pnpm build", "yarn build", "cargo build"} or "next build" in lowered or lowered.startswith("tsc")
