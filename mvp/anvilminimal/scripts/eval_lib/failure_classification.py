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
    "postcheck_failure",
    "provider_http_status",
    "provider_model_unavailable",
    "provider_parse_error",
    "provider_transient_exhausted",
    "required_artifacts_missing",
    "timeout",
    "tool_argument_decode_error",
    "tool_execution_error",
    "tool_validation_error",
    "unclassified_process_failure",
    "verify_repair_exhausted",
}


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
    if "path does not exist:" in lower or "no such file or directory" in lower:
        return {"failure_kind": "tool_execution_error", "tool_error_kind": "path_missing"}
    if "function_call arguments" in stderr or "provider parse" in lower:
        return {"failure_kind": "provider_parse_error"}
    if "path escapes workspace" in lower:
        return {"failure_kind": "path_confinement_error"}
    if "minimal loop reached max_iterations" in lower:
        return {"failure_kind": "max_iterations"}
    if "completion contract verify failed" in lower:
        return {"failure_kind": "verify_repair_exhausted"}
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
