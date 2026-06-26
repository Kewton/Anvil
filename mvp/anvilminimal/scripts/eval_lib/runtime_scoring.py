from __future__ import annotations

from collections import Counter
from pathlib import Path
from typing import Any


RUNTIME_EXECUTION_MODES = {"minimal-loop", "plan-run", "ultra-plan-run", "ultra-step-run"}
WRITE_TOOLS = {"Write", "Edit", "MultiEdit"}
INSPECTION_TOOLS = {"Read", "Glob", "Grep"}


def score_runtime_health(
    events: list[dict[str, Any]],
    *,
    mode: str,
    success: bool,
    scenario: dict[str, Any],
    workdir: Path,
) -> dict[str, float | str]:
    runtime_event_names = {
        "provider_response",
        "tool_call_raw",
        "tool_execute",
        "tool_validation_error",
        "artifact_stagnation_feedback",
        "loop_stop",
        "step_obligation_scope",
        "plan_final_contract",
    }
    has_runtime_events = any(event.get("event") in runtime_event_names for event in events)
    if mode not in RUNTIME_EXECUTION_MODES or not has_runtime_events:
        return {
            "runtime_friction_score": "",
            "artifact_progress_score": "",
            "finalization_score": "",
            "tool_policy_compatibility_score": "",
            "plan_run_runtime_health_score": "",
            "prompt_contract_score": score_prompt_contract(events),
            "step_obligation_scope_score": "",
            "step_obligation_scope_violation_count": "",
        }

    names = Counter(
        str(event.get("name", ""))
        for event in events
        if event.get("event") == "tool_call_raw" and event.get("name")
    )
    event_counts = Counter(str(event.get("event", "")) for event in events)
    stop = next((event for event in reversed(events) if event.get("event") == "loop_stop"), {})
    stop_reason = str(stop.get("reason", ""))
    validation_errors = event_counts.get("tool_validation_error", 0)
    execution_errors = sum(
        1
        for event in events
        if event.get("event") == "tool_execute" and str(event.get("status", "")) == "error"
    )
    no_tool_responses = sum(
        1
        for event in events
        if event.get("event") == "provider_response" and int(event.get("tool_calls") or 0) == 0
    )
    repeated_inspection = max(0, sum(names.get(name, 0) for name in INSPECTION_TOOLS) - 4)
    bash_overuse = max(0, names.get("Bash", 0) - 2)
    stagnation = event_counts.get("artifact_stagnation_feedback", 0)

    runtime_friction = 100.0
    runtime_friction -= 30.0 * validation_errors
    runtime_friction -= 30.0 * execution_errors
    runtime_friction -= 20.0 * no_tool_responses
    runtime_friction -= 8.0 * repeated_inspection
    runtime_friction -= 6.0 * bash_overuse
    runtime_friction -= 10.0 * stagnation
    runtime_friction = clamp(runtime_friction)

    write_calls = sum(names.get(name, 0) for name in WRITE_TOOLS)
    required_paths = [str(path) for path in scenario.get("expected_artifacts", []) or []]
    if required_paths:
        existing = sum(1 for path in required_paths if (workdir / path).exists())
        artifact_ratio = existing / max(1, len(required_paths))
    else:
        artifact_ratio = 1.0 if write_calls else 0.0
    artifact_progress = 20.0 + min(30.0, write_calls * 15.0) + 50.0 * artifact_ratio
    if "required_artifacts_satisfied" in stop_reason:
        artifact_progress += 10.0
    if "max_iterations" in stop_reason:
        artifact_progress -= 30.0
    if validation_errors or execution_errors:
        artifact_progress -= 20.0
    artifact_progress = clamp(artifact_progress)

    finalization = 100.0 if success else 20.0
    if "required_artifacts_satisfied" in stop_reason and not success:
        finalization = max(finalization, 50.0)
    if "max_iterations" in stop_reason:
        finalization = min(finalization, 30.0)
    if no_tool_responses:
        finalization = min(finalization, 50.0)
    finalization = clamp(finalization)

    policy = 100.0
    policy -= 35.0 * validation_errors
    policy -= 35.0 * execution_errors
    policy = clamp(policy)

    runtime_health = round(
        0.35 * runtime_friction
        + 0.25 * artifact_progress
        + 0.20 * policy
        + 0.20 * finalization,
        1,
    )
    obligation_scope_score, obligation_scope_violations = score_step_obligation_scope(events)
    return {
        "runtime_friction_score": round(runtime_friction, 1),
        "artifact_progress_score": round(artifact_progress, 1),
        "finalization_score": round(finalization, 1),
        "tool_policy_compatibility_score": round(policy, 1),
        "plan_run_runtime_health_score": runtime_health,
        "prompt_contract_score": score_prompt_contract(events),
        "step_obligation_scope_score": obligation_scope_score,
        "step_obligation_scope_violation_count": obligation_scope_violations,
    }


def score_prompt_contract(events: list[dict[str, Any]]) -> float | str:
    contract_events = [
        event for event in events if event.get("event") == "step_prompt_contract"
    ]
    if not contract_events:
        return ""
    scores = []
    for event in contract_events:
        checks = [
            bool(event.get("has_overall_goal")),
            bool(event.get("has_required_final_artifacts")),
            bool(event.get("has_expected_paths")),
            bool(event.get("has_verify_commands")),
            bool(event.get("has_expected_result")),
            bool(event.get("has_bounded_repair_policy")),
            event.get("prompt_body_saved") is False,
        ]
        if event.get("prior_artifact_context_applicable"):
            checks.append(bool(event.get("has_prior_artifact_context")))
        scores.append(100.0 * sum(1 for check in checks if check) / len(checks))
    return round(sum(scores) / len(scores), 1)


def score_step_obligation_scope(events: list[dict[str, Any]]) -> tuple[float | str, int | str]:
    scope_events = [
        event
        for event in events
        if event.get("event") == "step_obligation_scope"
        and event.get("session_scope") == "plan-run-step"
    ]
    if not scope_events:
        return "", ""
    scores = []
    violations = 0
    for event in scope_events:
        explicit = [str(path) for path in event.get("explicit_required_paths", []) or []]
        effective = [str(path) for path in event.get("effective_required_paths", []) or []]
        prompt_extracted = [
            str(path) for path in event.get("prompt_extracted_paths", []) or []
        ]
        contract_paths = [
            str(path) for path in event.get("completion_contract_paths", []) or []
        ]
        extraction_enabled = bool(event.get("prompt_extracted_paths_enabled"))
        contract_merge_enabled = bool(event.get("completion_contract_path_merge_enabled"))
        contract_verify_enabled = bool(event.get("completion_contract_verification_enabled"))
        score = 100.0
        if extraction_enabled:
            score -= 30.0 if prompt_extracted else 15.0
        if contract_merge_enabled:
            score -= 30.0 if contract_paths else 15.0
        if contract_verify_enabled:
            score -= 20.0
        if effective != explicit:
            score -= 30.0
        score = clamp(score)
        if score < 100.0:
            violations += 1
        scores.append(score)
    return round(sum(scores) / len(scores), 1), violations


def clamp(value: float) -> float:
    return max(0.0, min(100.0, value))
