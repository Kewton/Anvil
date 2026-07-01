from __future__ import annotations

import json
from pathlib import Path
from typing import Any

from .acceptance_contract import contract_from_scenario
from .postcheck import load_postcheck_events


def evaluate_browser_oracle(
    scenario: dict[str, Any],
    workdir: Path,
    *,
    enabled: bool = False,
    run_dir: Path | None = None,
    postcheck: dict[str, Any] | None = None,
) -> dict[str, Any]:
    """Browser acceptance hook.

    The default eval path is deterministic and dependency-light. Browser checks
    are exposed as an explicit adapter point so acceptance-required suites can
    enable them without making smoke eval depend on Playwright availability.
    """

    contract = contract_from_scenario(scenario)
    required = "browser_interaction" in (contract.oracle_contract.get("deterministic_oracles") or [])
    evidence = load_browser_evidence(workdir=workdir, run_dir=run_dir)
    if evidence:
        return evidence

    dev_server_result = browser_result_from_postcheck(postcheck or {}, workdir=workdir)
    if dev_server_result:
        return dev_server_result

    if not enabled:
        return {
            "browser_success": "",
            "browser_failure_kind": "",
            "browser_details": {
                "applicable": required,
                "status": "not_enabled",
                "workdir": str(workdir),
            },
        }
    return {
        "browser_success": "",
        "browser_failure_kind": "",
        "browser_details": {
            "applicable": required,
            "status": "adapter_not_implemented",
            "workdir": str(workdir),
        },
    }


def load_browser_evidence(*, workdir: Path, run_dir: Path | None) -> dict[str, Any] | None:
    for path in browser_evidence_candidates(workdir=workdir, run_dir=run_dir):
        if not path.is_file():
            continue
        try:
            parsed = json.loads(path.read_text(encoding="utf-8"))
        except Exception as exc:
            return {
                "browser_success": False,
                "browser_failure_kind": "browser_evidence_invalid",
                "browser_details": {
                    "status": "failed",
                    "reason": type(exc).__name__,
                    "evidence_path": str(path),
                    "workdir": str(workdir),
                },
            }
        if not isinstance(parsed, dict):
            return {
                "browser_success": False,
                "browser_failure_kind": "browser_evidence_invalid",
                "browser_details": {
                    "status": "failed",
                    "reason": "not_object",
                    "evidence_path": str(path),
                    "workdir": str(workdir),
                },
            }
        return normalize_browser_evidence(parsed, evidence_path=path, workdir=workdir)
    return None


def browser_evidence_candidates(*, workdir: Path, run_dir: Path | None) -> list[Path]:
    names = [
        "browser-readiness.json",
        "browser.json",
        "browser-readiness-evidence.json",
    ]
    dirs: list[Path] = []
    if run_dir is not None:
        dirs.append(run_dir)
    dirs.extend([workdir / ".anvil", workdir])
    candidates: list[Path] = []
    for directory in dirs:
        for name in names:
            candidates.append(directory / name)
    return candidates


def normalize_browser_evidence(
    evidence: dict[str, Any],
    *,
    evidence_path: Path,
    workdir: Path,
) -> dict[str, Any]:
    success = bool_field(evidence, "ok", "success", "browser_success")
    details = evidence.get("browser_details") if isinstance(evidence.get("browser_details"), dict) else {}
    status_value = text_field(evidence, "status") or text_field(details, "status")
    http_status = int_field(evidence, "http_status", "status_code") or int_field(
        details,
        "http_status",
        "status_code",
    )
    if http_status is not None and http_status >= 400:
        return browser_fail(
            failure_kind=browser_failure_kind(evidence, details, http_status=http_status),
            evidence_path=evidence_path,
            workdir=workdir,
            status="failed",
            http_status=http_status,
        )
    if browser_unavailable_status(status_value):
        return browser_unavailable(
            status=browser_unavailable_reason(evidence, details, status_value or "unavailable"),
            evidence_path=evidence_path,
            workdir=workdir,
            http_status=http_status,
        )
    if success is False:
        return browser_fail(
            failure_kind=browser_failure_kind(evidence, details, http_status=http_status),
            evidence_path=evidence_path,
            workdir=workdir,
            status=status_value or "failed",
            http_status=http_status,
        )
    explicit_failure = browser_detail_failure(evidence, details)
    if explicit_failure:
        return browser_fail(
            failure_kind=explicit_failure,
            evidence_path=evidence_path,
            workdir=workdir,
            status="failed",
            http_status=http_status,
        )
    if saved_browser_evidence_has_required_details(evidence, details):
        return browser_pass(evidence_path=evidence_path, workdir=workdir, http_status=http_status)
    if (
        success is True
        or (http_status is not None and 200 <= http_status < 400)
        or status_value in {"ok", "pass", "passed", "ready"}
    ):
        return browser_unavailable(
            status="browser_render_or_interaction_evidence_missing",
            evidence_path=evidence_path,
            workdir=workdir,
            http_status=http_status,
        )
    return browser_unavailable(
        status=status_value or "evidence_inconclusive",
        evidence_path=evidence_path,
        workdir=workdir,
        http_status=http_status,
    )


def browser_result_from_postcheck(postcheck: dict[str, Any], *, workdir: Path) -> dict[str, Any] | None:
    events_path = postcheck.get("events_path")
    if not events_path:
        return None
    events = load_postcheck_events(Path(events_path))
    dev_events = [event for event in events if event.get("event") == "dev_server"]
    if not dev_events:
        dev_events = [
            event
            for event in events
            if event.get("event") == "dev_server_lifecycle" and event.get("stage") in {"wait", "probe"}
        ]
    if not dev_events:
        return None
    event = dev_events[-1]
    status = int_value(event.get("status")) or int_value(event.get("http_status"))
    if status is not None and status >= 400:
        return browser_fail(
            failure_kind=browser_failure_kind(event, {}, http_status=status),
            evidence_path=None,
            workdir=workdir,
            status="failed",
            http_status=status,
            source="postcheck_dev_server",
        )
    if event.get("ready") is False or event.get("ok") is False:
        return browser_unavailable(
            status=text_field(event, "failure_kind") or "dev_server_readiness_unavailable",
            evidence_path=None,
            workdir=workdir,
            http_status=status,
            source="postcheck_dev_server",
        )
    if event.get("ready") is True or event.get("ok") is True:
        return browser_unavailable(
            status="browser_render_or_interaction_evidence_missing",
            evidence_path=None,
            workdir=workdir,
            http_status=status,
            source="postcheck_dev_server",
        )
    return None


def browser_pass(*, evidence_path: Path | None, workdir: Path, http_status: int | None) -> dict[str, Any]:
    return {
        "browser_success": True,
        "browser_failure_kind": "",
        "browser_details": {
            "status": "passed",
            "http_status": http_status,
            "evidence_path": str(evidence_path) if evidence_path else "",
            "workdir": str(workdir),
        },
    }


def browser_fail(
    *,
    failure_kind: str,
    evidence_path: Path | None,
    workdir: Path,
    status: str,
    http_status: int | None,
    source: str = "evidence",
) -> dict[str, Any]:
    return {
        "browser_success": False,
        "browser_failure_kind": failure_kind,
        "browser_details": {
            "status": status,
            "http_status": http_status,
            "evidence_path": str(evidence_path) if evidence_path else "",
            "source": source,
            "workdir": str(workdir),
        },
    }


def browser_unavailable(
    *,
    status: str,
    evidence_path: Path | None,
    workdir: Path,
    http_status: int | None,
    source: str = "evidence",
) -> dict[str, Any]:
    return {
        "browser_success": "",
        "browser_failure_kind": "",
        "browser_details": {
            "status": status,
            "http_status": http_status,
            "evidence_path": str(evidence_path) if evidence_path else "",
            "source": source,
            "workdir": str(workdir),
        },
    }


def browser_failure_kind(
    evidence: dict[str, Any],
    details: dict[str, Any],
    *,
    http_status: int | None,
) -> str:
    kind = text_field(evidence, "browser_failure_kind", "failure_kind", "error_kind") or text_field(
        details,
        "browser_failure_kind",
        "failure_kind",
        "error_kind",
    )
    if kind in {"tailwind_dev_pipeline_failure", "css_dev_pipeline_failure", "nextjs_dev_pipeline_failure"}:
        return kind
    if http_status is not None and http_status >= 400:
        return f"browser_http_{http_status}"
    return kind or "browser_behavior_failure"


def browser_unavailable_status(status: str) -> bool:
    return status in {"not_enabled", "adapter_not_implemented", "unavailable", "skipped"} or status.startswith(
        ("unavailable:", "browser_unavailable:")
    ) or status == "browser_unavailable"


def browser_unavailable_reason(evidence: dict[str, Any], details: dict[str, Any], status: str) -> str:
    return (
        text_field(evidence, "browser_failure_kind", "failure_kind", "error_kind", "reason")
        or text_field(details, "browser_failure_kind", "failure_kind", "error_kind", "reason")
        or status
    )


def saved_browser_evidence_has_required_details(
    evidence: dict[str, Any], details: dict[str, Any]
) -> bool:
    return any_true(evidence, details, "route_rendered", "rendered", "page_loaded", "dom_ready") and any_true(
        evidence,
        details,
        "interaction_performed",
        "basic_interaction",
        "interaction_success",
        "input_event_observed",
        "keyboard_event_observed",
        "pointer_event_observed",
        "state_changed",
        "visible_state_changed",
    )


def browser_detail_failure(evidence: dict[str, Any], details: dict[str, Any]) -> str:
    if any_false(evidence, details, "route_rendered", "rendered", "page_loaded", "dom_ready"):
        return "browser_route_not_rendered"
    if any_false(evidence, details, "canvas_found", "canvas_available"):
        return "canvas_unavailable"
    if any_false(evidence, details, "interactive_surface", "interaction_surface"):
        return "interactive_surface_missing"
    if any_false(
        evidence,
        details,
        "input_event_observed",
        "keyboard_event_observed",
        "pointer_event_observed",
    ):
        return "input_event_missing"
    if any_false(evidence, details, "state_changed", "visible_state_changed"):
        return "interaction_state_change_missing"
    return ""


def any_true(evidence: dict[str, Any], details: dict[str, Any], *keys: str) -> bool:
    return any(bool_field(evidence, key) is True or bool_field(details, key) is True for key in keys)


def any_false(evidence: dict[str, Any], details: dict[str, Any], *keys: str) -> bool:
    return any(bool_field(evidence, key) is False or bool_field(details, key) is False for key in keys)


def bool_field(value: dict[str, Any], *keys: str) -> bool | None:
    for key in keys:
        raw = value.get(key)
        if isinstance(raw, bool):
            return raw
    return None


def int_field(value: dict[str, Any], *keys: str) -> int | None:
    for key in keys:
        parsed = int_value(value.get(key))
        if parsed is not None:
            return parsed
    return None


def int_value(value: object) -> int | None:
    if isinstance(value, bool):
        return None
    if isinstance(value, int):
        return value
    if isinstance(value, str):
        try:
            return int(value)
        except ValueError:
            return None
    return None


def text_field(value: dict[str, Any], *keys: str) -> str:
    for key in keys:
        raw = value.get(key)
        if isinstance(raw, str):
            return raw.strip().lower()
    return ""
