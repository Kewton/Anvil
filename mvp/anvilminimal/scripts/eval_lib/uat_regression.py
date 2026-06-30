from __future__ import annotations

from typing import Any


def summarize_uat_regression_fixture(fixture: dict[str, Any]) -> dict[str, Any]:
    """Summarize UAT regression evidence without depending on one scenario name."""

    events = [event for event in fixture.get("events", []) if isinstance(event, dict)]
    artifacts = fixture.get("artifacts", {}) or {}
    build = fixture.get("build", {}) or {}
    browser = fixture.get("browser", {}) or {}

    phase_scaffold_failures = [
        event
        for event in events
        if event.get("event") == "ultra_phase_failed"
        and event.get("stage") in {"scaffold", "lint"}
    ]
    verify_policy_errors = [
        event
        for event in events
        if event.get("event") == "planner_error"
        and event.get("planner_error_kind") == "verify_command_policy_error"
    ]
    recovery_prompt_events = [
        event for event in events if event.get("event") == "recovery_prompt_saved"
    ]
    path_only_stops = [
        event
        for event in events
        if event.get("event") == "loop_stop"
        and event.get("reason") == "required_artifacts_satisfied_after_tool"
    ]
    recovery_prompts = list(artifacts.get("recovery_prompts") or [])
    recovery_ultra_plans = list(artifacts.get("recovery_ultra_plans") or [])

    build_passed = build.get("ok") is True
    browser_failed = browser.get("ok") is False

    return {
        "fixture_id": fixture.get("id", ""),
        "phase_scaffold_error_detected": bool(phase_scaffold_failures),
        "failed_phase_id": first_non_empty(phase_scaffold_failures, "phase_id"),
        "verify_command_policy_error_detected": bool(verify_policy_errors),
        "verify_policy_error_attempts": [
            event.get("repair_attempt", "") for event in verify_policy_errors
        ],
        "recovery_prompt_saved": bool(recovery_prompt_events or recovery_prompts),
        "recovery_ultra_plan_missing": bool(
            (recovery_prompt_events or recovery_prompts) and not recovery_ultra_plans
        ),
        "build_pass_browser_fail": build_passed and browser_failed,
        "browser_http_status": browser.get("http_status", ""),
        "browser_failure_kind": browser.get("failure_kind", ""),
        "path_only_early_stop_detected": bool(path_only_stops),
        "path_only_stop_count": len(path_only_stops),
        "path_only_stop_required_paths": [
            list(event.get("required_paths") or []) for event in path_only_stops
        ],
    }


def first_non_empty(events: list[dict[str, Any]], key: str) -> Any:
    for event in events:
        value = event.get(key)
        if value not in (None, ""):
            return value
    return ""

