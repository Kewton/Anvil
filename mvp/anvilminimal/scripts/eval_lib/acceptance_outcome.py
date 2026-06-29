from __future__ import annotations

from pathlib import Path
from typing import Any

from .acceptance_contract import AcceptanceContract, contract_from_scenario
from .plan_capability_contract import score_plan_capability_contract
from .plan_output_adherence import evaluate_plan_output_adherence
from .plan_verify_coverage import score_plan_verify_coverage
from .postcheck import load_postcheck_events
from .source_semantic_oracle import evaluate_source_semantics


ACCEPTANCE_ORACLE_VERSION = "acceptance-v4-capability-calibrated"


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
    plan_capability: dict[str, Any] | None = None,
    plan_verify_coverage: dict[str, Any] | None = None,
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
    plan_capability = plan_capability or score_plan_capability_contract(
        scenario=scenario,
        plan_paths=plan_paths or [],
    )
    plan_verify_coverage = plan_verify_coverage or score_plan_verify_coverage(
        scenario=scenario,
        mode=mode,
        plan_paths=plan_paths or [],
        workdir=workdir,
        postcheck_events=post_events,
        plan_capability_result=plan_capability,
    )
    plan_output_success = plan_output.get("plan_output_adherence_success", "")
    browser_result = browser_result or {"browser_success": "", "browser_failure_kind": "", "browser_details": {}}
    behavior_success = combine_behavior_success(semantic_success, browser_result.get("browser_success", ""))
    prompt_contract_success = semantic_success if semantic_success != "" else True
    capability_acceptance_success = (
        semantic_success is not False
        and plan_output_success is not False
        and browser_result.get("browser_success", "") is not False
    )

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
    failure_reasons = acceptance_failure_reasons(
        process_success=process_success,
        artifact_success=artifact_success,
        build_success=build_success,
        launch_success=launch_success,
        semantic_success=semantic_success,
        plan_output_success=plan_output_success,
        browser_success=browser_result.get("browser_success", ""),
        post_ok=bool(postcheck.get("ok", True)),
        failure_kind=failure_kind,
    )
    confidence = acceptance_confidence(
        acceptance_success=acceptance_success,
        plan_output_score=plan_output.get("plan_output_adherence_score", ""),
        plan_verify_score=plan_verify_coverage.get("plan_verify_coverage_score", ""),
        verify_adequacy_score="",
        prompt_plan_score=plan_capability.get("prompt_plan_capability_coverage_score", ""),
        build_success=build_success,
        launch_success=launch_success,
        semantic=semantic,
        plan_output=plan_output,
        plan_verify=plan_verify_coverage,
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
        "plan_capability_contract_score": plan_capability.get("plan_capability_contract_score", ""),
        "plan_capability_oracle_version": plan_capability.get("plan_capability_oracle_version", ""),
        "prompt_plan_capability_coverage_score": plan_capability.get("prompt_plan_capability_coverage_score", ""),
        "prompt_plan_missing_capability_count": plan_capability.get("prompt_plan_missing_capability_count", ""),
        "plan_required_capability_count": plan_capability.get("plan_required_capability_count", ""),
        "plan_verify_declared_coverage_score": plan_verify_coverage.get("plan_verify_declared_coverage_score", ""),
        "executed_verify_coverage_score": plan_verify_coverage.get("executed_verify_coverage_score", ""),
        "plan_verify_coverage_score": plan_verify_coverage.get("plan_verify_coverage_score", ""),
        "plan_verified_capability_count": plan_verify_coverage.get("plan_verified_capability_count", ""),
        "plan_unverified_capability_count": plan_verify_coverage.get("plan_unverified_capability_count", ""),
        "prompt_plan_gap_kind": plan_capability.get("prompt_plan_gap_kind", ""),
        "plan_verify_gap_kind": plan_verify_coverage.get("plan_verify_gap_kind", ""),
        "plan_verify_oracle_version": plan_verify_coverage.get("plan_verify_oracle_version", ""),
        "acceptance_confidence_score": confidence["acceptance_confidence_score"],
        "acceptance_confidence_reason": confidence["acceptance_confidence_reason"],
        "prompt_contract_success": prompt_contract_success,
        "capability_acceptance_success": capability_acceptance_success,
        "acceptance_success": acceptance_success,
        "acceptance_failure_kind": failure_kind,
        "acceptance_failure_reasons": failure_reasons,
        "acceptance_false_positive": false_positive,
        "oracle_gap_kind": oracle_gap_kind(false_positive, semantic, plan_output, postcheck),
        "acceptance_oracle_version": ACCEPTANCE_ORACLE_VERSION,
        "acceptance_details": {
            "contract": contract.to_dict(),
            "source_semantic": semantic,
            "plan_capability": plan_capability,
            "plan_verify": plan_verify_coverage,
            "plan_output": plan_output,
            "browser": browser_result,
            "acceptance_confidence": confidence,
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
        "plan_capability_contract_score": "",
        "plan_capability_oracle_version": "",
        "prompt_plan_capability_coverage_score": "",
        "prompt_plan_missing_capability_count": "",
        "plan_required_capability_count": "",
        "plan_verify_declared_coverage_score": "",
        "executed_verify_coverage_score": "",
        "plan_verify_coverage_score": "",
        "plan_verified_capability_count": "",
        "plan_unverified_capability_count": "",
        "prompt_plan_gap_kind": reason,
        "plan_verify_gap_kind": reason,
        "plan_verify_oracle_version": "",
        "acceptance_confidence_score": "",
        "acceptance_confidence_reason": reason,
        "prompt_contract_success": "",
        "capability_acceptance_success": "",
        "acceptance_success": "",
        "acceptance_failure_kind": "",
        "acceptance_failure_reasons": [],
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


def acceptance_failure_reasons(
    *,
    process_success: bool,
    artifact_success: bool,
    build_success: bool | str,
    launch_success: bool | str,
    semantic_success: object,
    plan_output_success: object,
    browser_success: object,
    post_ok: bool,
    failure_kind: str,
) -> list[str]:
    reasons: list[str] = []
    if not process_success:
        reasons.append("process_failure")
    if not artifact_success:
        reasons.append("artifact_failure")
    if not post_ok:
        reasons.append("postcheck_failure")
    if build_success is False:
        reasons.append("build_failure")
    if launch_success is False:
        reasons.append("launch_failure")
    if semantic_success is False:
        reasons.append("source_semantic_failure")
    if plan_output_success is False:
        reasons.append("plan_output_contract_failure")
    if browser_success is False:
        reasons.append("browser_behavior_failure")
    if failure_kind and failure_kind not in reasons:
        reasons.append(failure_kind)
    return reasons


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


def acceptance_confidence(
    *,
    acceptance_success: bool,
    plan_output_score: object,
    plan_verify_score: object,
    verify_adequacy_score: object,
    prompt_plan_score: object,
    build_success: object,
    launch_success: object,
    semantic: dict[str, Any],
    plan_output: dict[str, Any],
    plan_verify: dict[str, Any],
) -> dict[str, Any]:
    output_score = float_or_default(plan_output_score, 100.0 if plan_output_score == "" else 0.0)
    verify_score = float_or_default(plan_verify_score, 100.0 if plan_verify_score == "" else 0.0)
    adequacy = float_or_default(verify_adequacy_score, verify_score)
    prompt_score = float_or_default(prompt_plan_score, 100.0 if prompt_plan_score == "" else 0.0)
    build_score = tri_state_score(build_success)
    launch_score = tri_state_score(launch_success)
    score = round(
        0.25 * output_score
        + 0.25 * verify_score
        + 0.20 * adequacy
        + 0.10 * prompt_score
        + 0.10 * build_score
        + 0.10 * launch_score,
        1,
    )
    reasons: list[str] = []
    if output_score < 70:
        score = min(score, 70.0)
        reasons.append("plan_output_adherence_below_70")
    if verify_score < 40:
        score = min(score, 75.0)
        reasons.append("plan_verify_coverage_below_40")
    if prompt_score < 70:
        score = min(score, 80.0)
        reasons.append("prompt_plan_capability_coverage_below_70")
    if semantic_inconclusive(semantic, plan_output, plan_verify):
        score = min(score, 70.0)
        reasons.append("semantic_inconclusive_needs_behavior_oracle")
    if plan_verify.get("plan_verify_gap_kind") == "browser_required_but_not_declared":
        reasons.append("browser_oracle_unavailable")
    if not acceptance_success:
        score = min(score, 50.0)
        reasons.append("acceptance_success_false")
    return {
        "acceptance_confidence_score": round(max(0.0, min(100.0, score)), 1),
        "acceptance_confidence_reason": ";".join(dict.fromkeys(reasons)),
    }


def tri_state_score(value: object) -> float:
    if value is True:
        return 100.0
    if value is False:
        return 0.0
    return 100.0


def float_or_default(value: object, default: float) -> float:
    if value in {"", None, "not_applicable"}:
        return default
    try:
        return float(value)  # type: ignore[arg-type]
    except (TypeError, ValueError):
        return default


def semantic_inconclusive(
    semantic: dict[str, Any],
    plan_output: dict[str, Any],
    plan_verify: dict[str, Any],
) -> bool:
    if semantic.get("source_semantic_success") is not False:
        return False
    missing = semantic.get("source_semantic_details", {}).get("missing_capabilities", [])
    if not missing or len(missing) > 2:
        return False
    plan_score = float_or_default(plan_output.get("plan_output_adherence_score", ""), 0.0)
    verify_score = float_or_default(plan_verify.get("plan_verify_coverage_score", ""), 0.0)
    return plan_score >= 60.0 or verify_score >= 60.0
