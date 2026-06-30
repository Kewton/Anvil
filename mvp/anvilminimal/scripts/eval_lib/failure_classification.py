from __future__ import annotations

import json
import re
from pathlib import Path
from typing import Any

KNOWN_FAILURE_KINDS = {
    "diagnostic_skipped",
    "max_iterations",
    "max_iterations_after_provider_error",
    "missing_tool_call",
    "path_confinement_error",
    "plan_final_contract_failure",
    "final_acceptance_failure",
    "final_acceptance_repair_failed",
    "final_acceptance_repair_exhausted",
    "phase_scaffold_error",
    "planner_lint_error",
    "planner_schema_error",
    "postcheck_failure",
    "profile_contract_failure",
    "provider_http_status",
    "provider_model_unavailable",
    "provider_parse_error",
    "provider_transient_exhausted",
    "required_artifacts_missing",
    "recoverable_tool_error_repeated",
    "timeout",
    "tool_argument_decode_error",
    "tool_execution_error",
    "tool_validation_error",
    "unclassified_process_failure",
    "step_obligation_scope_violation",
    "step_verify_failure",
    "artifact_recovery_exhausted",
    "deferred_verify_requirement_pending",
    "test_discovery_failure",
    "test_framework_mismatch",
    "verify_repair_no_change",
    "verify_repair_exhausted",
    "verify_repair_progress_invalid",
    "verify_repair_progress_regressed",
    "verify_repair_progress_unchanged",
    "verify_command_policy_error",
    "process_failure",
    "artifact_failure",
    "build_failure",
    "launch_failure",
    "plan_output_contract_failure",
    "plan_output_missing_required_capabilities",
    "source_semantic_failure",
    "static_title_only",
    "browser_behavior_failure",
    "missing_required_capabilities",
    "missing_required_evidence",
    "weak_verification_evidence",
    "build_not_verified",
    "dependency_missing",
    "dependency_setup_blocked",
    "dependency_setup_failed",
    "dependency_setup_missing",
    "build_after_setup_failed",
    "build_verify_blocked",
    "build_verify_failed",
    "verifier_missing",
    "verifier_bootstrap_blocked",
    "repair_target_misdirected",
    "repair_stagnation",
    "package_lock_stale",
    "profile_static_build_gap",
}

PROVIDER_FAILURE_KINDS = {
    "provider_http_status",
    "provider_model_unavailable",
    "provider_parse_error",
    "provider_transient_exhausted",
    "max_iterations_after_provider_error",
}
PLANNING_FAILURE_KINDS = {
    "planner_lint_error",
    "planner_schema_error",
    "verify_command_policy_error",
    "phase_scaffold_error",
}
BRIDGE_FAILURE_KINDS = {
    "plan_final_contract_failure",
    "final_acceptance_failure",
    "final_acceptance_repair_failed",
    "final_acceptance_repair_exhausted",
    "profile_contract_failure",
    "step_obligation_scope_violation",
    "step_verify_failure",
    "deferred_verify_requirement_pending",
    "dependency_setup_blocked",
    "dependency_setup_failed",
    "dependency_setup_missing",
    "build_after_setup_failed",
    "build_verify_blocked",
    "verifier_missing",
    "verifier_bootstrap_blocked",
    "profile_static_build_gap",
}
POSTCHECK_FAILURE_KINDS = {"postcheck_failure"}
ENVIRONMENT_FAILURE_KINDS = {"timeout", "diagnostic_skipped"}
ACCEPTANCE_FAILURE_KINDS = {
    "artifact_failure",
    "build_failure",
    "launch_failure",
    "plan_output_contract_failure",
    "plan_output_missing_required_capabilities",
    "source_semantic_failure",
    "static_title_only",
    "browser_behavior_failure",
}


def failure_layer_for_kind(kind: str | None) -> str:
    normalized = str(kind or "")
    if not normalized:
        return ""
    if normalized in PROVIDER_FAILURE_KINDS:
        return "provider"
    if normalized in PLANNING_FAILURE_KINDS:
        return "planning"
    if normalized in BRIDGE_FAILURE_KINDS:
        return "bridge"
    if normalized in {
        "missing_required_capabilities",
        "missing_required_evidence",
        "weak_verification_evidence",
        "build_not_verified",
        "dependency_missing",
        "dependency_setup_blocked",
        "dependency_setup_failed",
        "dependency_setup_missing",
        "build_after_setup_failed",
        "build_verify_blocked",
        "build_verify_failed",
        "verifier_missing",
        "verifier_bootstrap_blocked",
        "repair_target_misdirected",
        "repair_stagnation",
        "package_lock_stale",
        "profile_static_build_gap",
    }:
        return "runtime"
    if normalized in POSTCHECK_FAILURE_KINDS:
        return "postcheck"
    if normalized in ENVIRONMENT_FAILURE_KINDS:
        return "environment"
    if normalized in ACCEPTANCE_FAILURE_KINDS:
        return "acceptance"
    return "runtime"


def capability_failure_included(kind: str | None) -> bool | str:
    layer = failure_layer_for_kind(kind)
    if not layer:
        return ""
    return layer not in {"provider", "environment"}


def read_jsonl(path: Path) -> list[dict[str, Any]]:
    if not path.exists():
        return []
    events: list[dict[str, Any]] = []
    for line in path.read_text(encoding="utf-8").splitlines():
        if not line.strip():
            continue
        try:
            event = json.loads(line)
        except json.JSONDecodeError:
            events.append({"event": "event_parse_error", "raw": line[:200]})
            continue
        if isinstance(event, dict):
            events.append(event)
    return events


def classify_events(events: list[dict[str, Any]]) -> dict[str, Any]:
    last_provider_error = next(
        (event for event in reversed(events) if event.get("event") == "provider_error"),
        None,
    )
    for event in reversed(events):
        name = event.get("event")
        if name == "planner_error":
            return {
                "failure_kind": event.get("planner_error_kind", "planner_schema_error"),
                "planner_stage": event.get("planner_stage", ""),
                "planner_error_kind": event.get("planner_error_kind", ""),
                "planner_error_message": event.get("planner_error_message", ""),
                "planner_provider": event.get("planner_provider", ""),
                "planner_model": event.get("planner_model", ""),
                "planner_repair_attempts": event.get("repair_attempt", ""),
            }
        if name == "step_verify_failure":
            return {
                "failure_kind": "step_verify_failure",
                "last_loop_stop": "step_verify_failure",
                "repair_target": event.get("repair_target", ""),
            }
        if name == "ultra_phase_failed":
            reason = str(event.get("reason", ""))
            lower_reason = reason.lower()
            if "failed verification after bounded repair" in lower_reason:
                return {
                    "failure_kind": "step_verify_failure",
                    "last_loop_stop": "ultra_phase_failed",
                    "phase_failure_stage": event.get("stage", ""),
                    "phase_id": event.get("phase_id", ""),
                }
            if "profile verification" in lower_reason:
                return {
                    "failure_kind": "profile_contract_failure",
                    "last_loop_stop": "ultra_phase_failed",
                    "phase_failure_stage": event.get("stage", ""),
                    "phase_id": event.get("phase_id", ""),
                }
            if event.get("stage") in {"scaffold", "lint"}:
                return {
                    "failure_kind": "phase_scaffold_error",
                    "last_loop_stop": "ultra_phase_failed",
                    "phase_failure_stage": event.get("stage", ""),
                    "phase_id": event.get("phase_id", ""),
                }
        if name == "final_acceptance_repair_exhausted":
            return {
                "failure_kind": "final_acceptance_repair_exhausted",
                "last_loop_stop": "final_acceptance_repair_exhausted",
                "lifecycle_stage": event.get("lifecycle_stage", ""),
                "repair_target": event.get("repair_target", ""),
                "missing_artifacts": ",".join(
                    str(path) for path in event.get("missing_paths", []) or []
                ),
            }
        if name == "final_acceptance_repair_failed":
            return {
                "failure_kind": "final_acceptance_repair_failed",
                "last_loop_stop": "final_acceptance_repair_failed",
                "lifecycle_stage": event.get("lifecycle_stage", ""),
                "repair_target": event.get("repair_target", ""),
            }
        if name == "ultra_final_acceptance_failed":
            return {
                "failure_kind": "final_acceptance_failure",
                "last_loop_stop": "ultra_final_acceptance_failed",
                "lifecycle_stage": event.get("lifecycle_stage", ""),
                "repair_target": event.get("repair_target", ""),
                "missing_artifacts": ",".join(
                    str(path) for path in event.get("missing_paths", []) or []
                ),
            }
        if name == "plan_final_contract" and event.get("ok") is False:
            return {
                "failure_kind": "plan_final_contract_failure",
                "last_loop_stop": "plan_final_contract_failure",
                "missing_artifacts": ",".join(
                    str(path) for path in event.get("missing_final_artifacts", []) or []
                ),
            }
        if name == "step_obligation_scope":
            if (
                event.get("session_scope") == "plan-run-step"
                and (
                    event.get("prompt_extracted_paths_enabled")
                    or event.get("completion_contract_path_merge_enabled")
                    or event.get("completion_contract_verification_enabled")
                    or list(event.get("effective_required_paths", []) or [])
                    != list(event.get("explicit_required_paths", []) or [])
                )
            ):
                return {
                    "failure_kind": "step_obligation_scope_violation",
                    "last_loop_stop": "step_obligation_scope_violation",
                }
        if name == "loop_stop" and event.get("reason") == "artifact_recovery_exhausted":
            return {
                "failure_kind": "artifact_recovery_exhausted",
                "last_loop_stop": "artifact_recovery_exhausted",
                "artifact_target_path": event.get("last_target_path", ""),
                "artifact_last_model_action": event.get("last_model_action", ""),
                "artifact_recovery_attempts": event.get("attempts", ""),
            }
        if name == "loop_stop" and event.get("reason") == "verify_repair_no_change":
            return {
                "failure_kind": "verify_repair_no_change",
                "last_loop_stop": "verify_repair_no_change",
                "repair_target": event.get("repair_target", ""),
            }
        if name == "loop_stop" and event.get("reason") == "test_discovery_failure":
            return {
                "failure_kind": "test_discovery_failure",
                "last_loop_stop": "test_discovery_failure",
            }
        if name == "loop_stop" and event.get("reason") == "test_framework_mismatch":
            return {
                "failure_kind": "test_framework_mismatch",
                "last_loop_stop": "test_framework_mismatch",
            }
        if name == "loop_stop" and event.get("reason") == "recoverable_tool_error_repeated":
            return {
                "failure_kind": "recoverable_tool_error_repeated",
                "last_loop_stop": "recoverable_tool_error_repeated",
                "tool_error_kind": event.get("error_kind", ""),
                "tool_name": event.get("name", ""),
            }
        if name == "loop_stop" and str(event.get("reason", "")).startswith("verify_repair_progress_"):
            return {
                "failure_kind": event.get("reason", "verify_repair_exhausted"),
                "last_loop_stop": event.get("reason", ""),
                "repair_progress": event.get("repair_progress", ""),
            }
        if name == "loop_stop" and event.get("reason") == "deferred_verify_requirement_pending":
            return {
                "failure_kind": "deferred_verify_requirement_pending",
                "last_loop_stop": "deferred_verify_requirement_pending",
            }
        if name == "loop_stop" and event.get("reason") in {
            "missing_required_capabilities",
            "missing_required_evidence",
            "weak_verification_evidence",
            "build_not_verified",
            "dependency_missing",
            "dependency_setup_blocked",
            "dependency_setup_failed",
            "dependency_setup_missing",
            "build_after_setup_failed",
            "build_verify_blocked",
            "build_verify_failed",
            "verifier_missing",
            "verifier_bootstrap_blocked",
            "repair_target_misdirected",
            "repair_stagnation",
            "package_lock_stale",
            "profile_static_build_gap",
        }:
            kind = event.get("reason")
            if kind == "dependency_setup_missing":
                setup_status = str(event.get("dependency_setup_status", ""))
                verifier_state = str(event.get("verifier_bootstrap_state", ""))
                if setup_status in {"blocked", "timed_out"}:
                    kind = "dependency_setup_blocked"
                elif setup_status == "failed":
                    kind = "dependency_setup_failed"
                elif verifier_state == "verifier_missing":
                    kind = "verifier_missing"
                elif verifier_state in {"dependency_setup_blocked", "dependency_setup_failed"}:
                    kind = "verifier_bootstrap_blocked"
            if kind == "build_verify_failed":
                lifecycle = event.get("build_verifier_lifecycle", []) or []
                statuses = [
                    str((item.get("setup") or {}).get("status", ""))
                    for item in lifecycle
                    if isinstance(item, dict) and isinstance(item.get("setup"), dict)
                ]
                if "passed" in statuses:
                    kind = "build_after_setup_failed"
            return {
                "failure_kind": kind,
                "last_loop_stop": event.get("reason", ""),
                "repair_target": event.get("repair_target", ""),
                "missing_capabilities": ",".join(
                    str(item) for item in event.get("missing_capabilities", []) or []
                ),
                "missing_evidence": ",".join(
                    str(item) for item in event.get("missing_evidence", []) or []
                ),
                "weak_evidence": ",".join(
                    str(item) for item in event.get("weak_evidence", []) or []
                ),
                "build_verifier_statuses": ",".join(
                    str(item) for item in event.get("build_verifier_statuses", []) or []
                ),
            }
        if name == "loop_stop" and event.get("reason") == "profile_contract_failure":
            return {
                "failure_kind": "profile_contract_failure",
                "last_loop_stop": "profile_contract_failure",
            }
        if name == "loop_stop" and event.get("reason") == "verify_repair_exhausted":
            return {
                "failure_kind": "verify_repair_exhausted",
                "last_loop_stop": "verify_repair_exhausted",
            }
        if name == "loop_stop" and event.get("reason") == "required_artifacts_missing":
            return {
                "failure_kind": "required_artifacts_missing",
                "last_loop_stop": "required_artifacts_missing",
            }
        if name == "tool_validation_error":
            return {
                "failure_kind": "tool_validation_error",
                "tool_error_kind": event.get("error_kind", ""),
                "tool_name": event.get("name", ""),
            }
        if name == "tool_execute" and event.get("status") == "error":
            return {
                "failure_kind": "tool_execution_error",
                "tool_error_kind": event.get("error_kind", ""),
                "tool_name": event.get("name", ""),
            }
        if name == "provider_parse_error":
            return {
                "failure_kind": "provider_parse_error",
                "provider_error_kind": event.get("error_kind", "provider_parse_error"),
            }
        if name == "provider_error":
            status = event.get("status")
            status_int = int(status) if str(status).isdigit() else None
            if status_int == 404:
                kind = "provider_model_unavailable"
            elif status_int in {429, 500, 502, 503, 504} or event.get("error_kind") in {"network", "timeout"}:
                kind = "provider_transient_exhausted"
            else:
                kind = "provider_http_status" if status else event.get("error_kind", "provider_http_status")
            return {
                "failure_kind": kind,
                "provider_error_kind": event.get("error_kind", ""),
                "provider_http_status": status or "",
                "provider_attempts": event.get("attempt", ""),
            }
        if name == "postcheck_summary" and event.get("ok") is False:
            return {"failure_kind": "postcheck_failure"}
        if (
            name == "acceptance_summary"
            and event.get("acceptance_success") is False
            and event.get("process_success") is not False
        ):
            return {
                "failure_kind": event.get("acceptance_failure_kind")
                or "final_acceptance_failure",
                "acceptance_failure_reasons": ",".join(
                    str(reason)
                    for reason in event.get("acceptance_failure_reasons", []) or []
                ),
            }
        if name == "diagnostic_skipped":
            return {"failure_kind": "diagnostic_skipped"}
    if last_provider_error:
        return {
            "failure_kind": "max_iterations_after_provider_error",
            "provider_error_kind": last_provider_error.get("error_kind", ""),
            "provider_http_status": last_provider_error.get("status", ""),
        }
    return {}


def classify_stderr(stderr: str, rc: int | str | None = None, timeout: bool = False) -> dict[str, Any]:
    if timeout or str(rc) == "124":
        return {"failure_kind": "timeout"}
    lower = stderr.lower()
    if "missing string argument `" in stderr or "unknown tool:" in stderr:
        return {"failure_kind": "tool_validation_error"}
    planner = classify_planner_stderr(stderr)
    if planner:
        return planner
    if "recoverable tool error repeated" in lower:
        return {"failure_kind": "recoverable_tool_error_repeated"}
    if "path_not_found_recoverable" in lower:
        return {"failure_kind": "tool_validation_error", "tool_error_kind": "path_not_found_recoverable"}
    if "path does not exist:" in lower or "no such file or directory" in lower:
        return {"failure_kind": "tool_execution_error", "tool_error_kind": "path_missing"}
    if "function_call arguments" in stderr or "provider parse" in lower:
        return {"failure_kind": "provider_parse_error"}
    if "path escapes workspace" in lower:
        return {"failure_kind": "path_confinement_error"}
    if "minimal loop reached max_iterations" in lower:
        return {"failure_kind": "max_iterations"}
    if "artifact recovery exhausted" in lower:
        return {"failure_kind": "artifact_recovery_exhausted"}
    if "verify repair made no file changes" in lower:
        return {"failure_kind": "verify_repair_no_change"}
    if "test_framework_mismatch" in lower or "pytest_style_under_unittest" in lower:
        return {"failure_kind": "test_framework_mismatch"}
    if "test_discovery_failure" in lower or "no tests ran" in lower or "ran 0 tests" in lower:
        return {"failure_kind": "test_discovery_failure"}
    if "completion contract verify failed" in lower:
        if "dependency_setup_missing" in lower or "node_modules/.bin/next missing" in lower:
            return {"failure_kind": "dependency_setup_missing"}
        if "build_verify_failed" in lower:
            return {"failure_kind": "build_verify_failed"}
        if "build_verify_blocked" in lower:
            return {"failure_kind": "build_verify_blocked"}
        if "build_verify_policy_rejected" in lower or "verify command may not" in lower:
            return {"failure_kind": "verify_command_policy_error"}
        if "missing_required_capabilities" in lower:
            return {"failure_kind": "missing_required_capabilities"}
        if "missing_required_evidence" in lower:
            return {"failure_kind": "missing_required_evidence"}
        if "weak_verification_evidence" in lower:
            return {"failure_kind": "weak_verification_evidence"}
        if "deferred verify requirement pending" in lower:
            return {"failure_kind": "deferred_verify_requirement_pending"}
        if "profile contract" in lower or "scripts.build must be next build" in lower:
            return {"failure_kind": "profile_contract_failure"}
        return {"failure_kind": "verify_repair_exhausted"}
    if "plan final contract failed" in lower:
        return {"failure_kind": "plan_final_contract_failure"}
    if "ultra final acceptance failed after bounded repair" in lower:
        return {"failure_kind": "final_acceptance_repair_exhausted"}
    if "ultra final acceptance repair failed" in lower:
        return {"failure_kind": "final_acceptance_repair_failed"}
    if "ultra final acceptance failed" in lower:
        return {"failure_kind": "final_acceptance_failure"}
    if "step obligation scope" in lower:
        return {"failure_kind": "step_obligation_scope_violation"}
    if "failed verification after bounded repair" in lower:
        return {"failure_kind": "step_verify_failure"}
    if "missing tool call for action prompt" in lower:
        return {"failure_kind": "missing_tool_call"}
    http = re.search(r"(OpenAI Responses API|Gemini interactions API) failed: (\d{3})", stderr)
    if http:
        status = int(http.group(2))
        if status in {429, 500, 502, 503, 504}:
            return {
                "failure_kind": "provider_transient_exhausted",
                "provider_error_kind": "http_status",
                "provider_http_status": status,
            }
        return {
            "failure_kind": "provider_model_unavailable" if status == 404 else "provider_http_status",
            "provider_error_kind": "http_status",
            "provider_http_status": status,
        }
    if "404 not found" in lower and "gemini" in lower:
        return {
            "failure_kind": "provider_model_unavailable",
            "provider_error_kind": "http_status",
            "provider_http_status": 404,
        }
    if str(rc) not in {"", "0", "None"}:
        return {"failure_kind": "unclassified_process_failure"}
    return {"failure_kind": "postcheck_failure"}


def classify_planner_stderr(stderr: str) -> dict[str, Any]:
    lower = stderr.lower()
    if (
        "stepplan missing goal" in lower
        or "stepplan has no steps" in lower
        or "stepplan invalid json" in lower
        or "ultraplan missing goal" in lower
        or "ultraplan has no phases" in lower
        or "ultra plan generation must not emit tool calls" in lower
    ):
        return {
            "failure_kind": "planner_schema_error",
            "planner_stage": "schema",
            "planner_error_kind": "planner_schema_error",
        }
    if "invalid generated ultraplan after corrective retries" in lower:
        if (
            "ultra phase" in lower
            or "ultraplan must have" in lower
            or "natural-language goal" in lower
            or "repl command" in lower
        ):
            return {
                "failure_kind": "phase_scaffold_error",
                "planner_stage": "scaffold",
                "planner_error_kind": "phase_scaffold_error",
            }
        return {
            "failure_kind": "planner_schema_error",
            "planner_stage": "schema",
            "planner_error_kind": "planner_schema_error",
        }
    if "stepplan unsafe expected path" in lower:
        return {
            "failure_kind": "path_confinement_error",
            "planner_stage": "schema",
            "planner_error_kind": "path_confinement_error",
        }
    if "ultra phase must have id and prompt" in lower:
        return {
            "failure_kind": "planner_schema_error",
            "planner_stage": "schema",
            "planner_error_kind": "planner_schema_error",
        }
    if "verify command may not use shell control syntax" in lower or "verify command is blocked" in lower:
        return {
            "failure_kind": "verify_command_policy_error",
            "planner_stage": "verify_policy",
            "planner_error_kind": "verify_command_policy_error",
        }
    if (
        "duplicate expected path ownership" in lower
        or "implement step must declare concrete expected paths" in lower
        or "verify command requires dependency setup or package manifest first" in lower
        or "next.js build verify requires an entrypoint expected path first" in lower
    ):
        return {
            "failure_kind": "planner_lint_error",
            "planner_stage": "lint",
            "planner_error_kind": "planner_lint_error",
        }
    if "phase scaffold failed" in lower:
        return {
            "failure_kind": "phase_scaffold_error",
            "planner_stage": "scaffold",
            "planner_error_kind": "phase_scaffold_error",
        }
    return {}


def classify_failure(
    *,
    events: list[dict[str, Any]],
    stderr: str,
    rc: int | str | None,
    timeout: bool,
    post_ok: bool,
) -> dict[str, Any]:
    event_classification = classify_events(events)
    if event_classification:
        return event_classification
    if not post_ok and str(rc) == "0":
        return {"failure_kind": "postcheck_failure"}
    return classify_stderr(stderr, rc=rc, timeout=timeout)


def known_failure_kind(kind: str) -> bool:
    return kind in KNOWN_FAILURE_KINDS


def normalize_failure_kind(row: dict[str, Any]) -> str:
    direct = str(row.get("failure_kind", "") or "").strip()
    if direct:
        return direct
    extras = parse_extras_json(row.get("extras_json", ""))
    extra_kind = str(extras.get("failure_kind", "") or "").strip()
    if extra_kind:
        return extra_kind
    acceptance_kind = str(row.get("acceptance_failure_kind", "") or "").strip()
    if row_value_is_false(row.get("acceptance_success")) and acceptance_kind:
        return acceptance_kind
    plan_output_kind = str(row.get("plan_output_failure_kind", "") or "").strip()
    if row_value_is_false(row.get("plan_output_adherence_success")) and plan_output_kind:
        return plan_output_kind
    return ""


def failure_kind_required_for_row(row: dict[str, Any]) -> bool:
    success = str(row.get("success", "") or "").strip().lower()
    if success in {"true", "diagnostic_skipped"}:
        return row_value_is_false(row.get("acceptance_success"))
    extras = parse_extras_json(row.get("extras_json", ""))
    if extras.get("dry_run") is True:
        return False
    if success in {"dry-run", "dry_run", "skipped"}:
        return False
    if str(row.get("rc", "") or "").strip() not in {"", "0"}:
        return True
    if row_value_is_false(row.get("process_success")):
        return True
    if row_value_is_false(row.get("acceptance_success")):
        return True
    if success == "false":
        return True
    return False


def blank_failure_kind_gate_violations(rows: list[dict[str, Any]]) -> list[dict[str, Any]]:
    violations: list[dict[str, Any]] = []
    for row in rows:
        if not failure_kind_required_for_row(row):
            continue
        if normalize_failure_kind(row):
            continue
        violations.append(
            {
                "run_id": row.get("run_id", ""),
                "scenario": row.get("scenario", ""),
                "mode": row.get("mode", ""),
                "rc": row.get("rc", ""),
                "success": row.get("success", ""),
                "process_success": row.get("process_success", ""),
                "acceptance_success": row.get("acceptance_success", ""),
            }
        )
    return violations


def row_value_is_false(value: Any) -> bool:
    if value is False:
        return True
    return str(value).strip().lower() == "false"


def parse_extras_json(raw: Any) -> dict[str, Any]:
    if isinstance(raw, dict):
        return raw
    if not raw:
        return {}
    try:
        parsed = json.loads(str(raw))
    except (TypeError, json.JSONDecodeError):
        return {}
    return parsed if isinstance(parsed, dict) else {}
